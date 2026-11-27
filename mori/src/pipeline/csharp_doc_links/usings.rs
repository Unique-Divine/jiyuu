//! Select and visit namespace and static using directives in declaration order.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr::NonNull;
use std::slice;

#[repr(C)]
struct NameScope {
    name: *const c_char,
    scope: c_int,
    top: bool,
}

#[repr(C)]
struct UsingId {
    scope: c_int,
    id: c_int,
}

#[repr(C)]
struct Postings {
    names: *const NameScope,
    name_count: usize,
    namespaces: *const UsingId,
    namespace_count: usize,
    entities: *const UsingId,
    entity_count: usize,
}

#[repr(C)]
struct Operations {
    visit: unsafe extern "C" fn(*mut c_void, c_int) -> bool,
    allocate: unsafe extern "C" fn(usize) -> *mut c_int,
    release: unsafe extern "C" fn(*mut c_int),
    work: Option<unsafe extern "C" fn(u64)>,
}

impl Operations {
    fn count(&self, work: usize) {
        if let Some(count) = self.work {
            // SAFETY: the C test callback accepts any work count.
            unsafe { count(work as u64) };
        }
    }
}

const STACK_CAPACITY: usize = 32;

// Only spill on larger candidate sets. An incomplete set must be discarded
// before any visit, since the caller must retry the entire ordered scan.
struct Candidates<'a> {
    stack: [c_int; STACK_CAPACITY],
    heap: Option<NonNull<c_int>>,
    len: usize,
    capacity: usize,
    ops: &'a Operations,
}

impl<'a> Candidates<'a> {
    fn new(ops: &'a Operations) -> Self {
        Self {
            stack: [0; STACK_CAPACITY],
            heap: None,
            len: 0,
            capacity: STACK_CAPACITY,
            ops,
        }
    }

    fn as_mut_ptr(&mut self) -> *mut c_int {
        self.heap
            .map_or_else(|| self.stack.as_mut_ptr(), NonNull::as_ptr)
    }

    fn push(&mut self, id: c_int) -> bool {
        if self.len == self.capacity {
            let capacity = self.capacity * 2;
            if capacity > isize::MAX as usize / size_of::<c_int>() {
                return false;
            }
            // SAFETY: the callback provides space for capacity integers.
            let Some(grown) =
                NonNull::new(unsafe { (self.ops.allocate)(capacity) })
            else {
                return false;
            };
            // SAFETY: the current buffer has len initialized integers, and
            // the new allocation is disjoint and larger.
            unsafe {
                self.as_mut_ptr()
                    .copy_to_nonoverlapping(grown.as_ptr(), self.len)
            };
            self.ops.count(self.len);
            if let Some(old) = self.heap.replace(grown) {
                unsafe { (self.ops.release)(old.as_ptr()) };
            }
            self.capacity = capacity;
        }
        // SAFETY: there is room for at least one more integer.
        unsafe { self.as_mut_ptr().add(self.len).write(id) };
        self.len += 1;
        true
    }

    fn as_mut_slice(&mut self) -> &mut [c_int] {
        // SAFETY: the first len integers have been initialized by push.
        unsafe { slice::from_raw_parts_mut(self.as_mut_ptr(), self.len) }
    }
}

impl Drop for Candidates<'_> {
    fn drop(&mut self) {
        if let Some(heap) = self.heap {
            unsafe { (self.ops.release)(heap.as_ptr()) };
        }
    }
}

fn first_index(
    mut lo: usize,
    mut hi: usize,
    mut before: impl FnMut(usize) -> bool,
) -> usize {
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if before(mid) {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo
}

fn collect(rows: &[UsingId], scope: c_int, ids: &mut Candidates<'_>) -> bool {
    let start = first_index(0, rows.len(), |index| {
        ids.ops.count(1);
        rows[index].scope < scope
    });
    for row in &rows[start..] {
        ids.ops.count(1);
        if row.scope != scope {
            break;
        }
        if !ids.push(row.id) {
            return false;
        }
    }
    true
}

// C publishes a null pointer for an empty table.
unsafe fn entries<'a, T>(data: *const T, len: usize) -> &'a [T] {
    if len == 0 {
        &[]
    } else {
        unsafe { slice::from_raw_parts(data, len) }
    }
}

fn visit_indexed(
    postings: &Postings,
    name: &[u8],
    count: c_int,
    ops: &Operations,
    visit: &mut impl FnMut(c_int) -> bool,
) -> bool {
    // SAFETY: these are the immutable, sorted tables of a completed index.
    let names = unsafe { entries(postings.names, postings.name_count) };
    let namespaces =
        unsafe { entries(postings.namespaces, postings.namespace_count) };
    let entities = unsafe { entries(postings.entities, postings.entity_count) };
    let compare = |index: usize| {
        ops.count(1);
        unsafe { CStr::from_ptr(names[index].name) }
            .to_bytes()
            .cmp(name)
    };
    let start = first_index(0, names.len(), |index| compare(index).is_lt());
    let end = first_index(start, names.len(), |index| !compare(index).is_gt());
    if end - start >= count as usize {
        // A common name must not make a small scope walk more postings than
        // it has directives.
        return false;
    }

    let mut candidates = Candidates::new(ops);
    for row in &names[start..end] {
        ops.count(1);
        let rows = if row.top { namespaces } else { entities };
        if !collect(rows, row.scope, &mut candidates) {
            return false;
        }
    }

    let sorted = candidates.as_mut_slice();
    sorted.sort_unstable_by(|left, right| {
        ops.count(1);
        left.cmp(right)
    });
    let mut previous = None;
    for &id in sorted.iter() {
        ops.count(1);
        // Own and twin postings can select the same directive. Distinct
        // directives remain distinct even when they name the same target.
        if previous != Some(id) && !visit(id) {
            break;
        }
        previous = Some(id);
    }
    true
}

// The optional postings are supplied only when both C indexes are ready.
// Each ID belongs to the caller's directive array. Returning false from
// visit stops traversal once resolution has a decisive result.
#[unsafe(no_mangle)]
unsafe extern "C" fn mori_cs_doclink_visit_usings(
    postings: *const Postings,
    name: *const c_char,
    count: c_int,
    ops: *const Operations,
    data: *mut c_void,
) {
    let ops = unsafe { &*ops };
    let mut visit = |id| unsafe { (ops.visit)(data, id) };
    if let Some(postings) = unsafe { postings.as_ref() } {
        let name = unsafe { CStr::from_ptr(name) }.to_bytes();
        if visit_indexed(postings, name, count, ops, &mut visit) {
            return;
        }
    }
    for id in 0..count {
        if !visit(id) {
            break;
        }
    }
}
