//! Field lookup for the native C and C++ resolver.

use std::ffi::{CStr, c_char, c_void};
use std::marker::PhantomData;
use std::ptr::null;

#[repr(C)]
struct RegisteredType {
    _opaque: [u8; 0],
}

#[repr(C)]
struct FieldType {
    _opaque: [u8; 0],
}

#[derive(Clone, Copy)]
#[repr(C)]
struct RecordRow {
    identity: *const RegisteredType,
    qualified_name: *const c_char,
    alias_of: *const c_char,
    bases: *const *const c_char,
    field_names: *const *const c_char,
    field_types: *const *const FieldType,
}

type VisitRecord = unsafe extern "C" fn(*mut c_void, RecordRow) -> bool;

#[repr(C)]
struct Registry {
    data: *const c_void,
    exact: unsafe extern "C" fn(*const c_void, *const c_char) -> RecordRow,
    exact_long_in_module:
        unsafe extern "C" fn(*const c_void, *const c_char) -> RecordRow,
    visit_short: unsafe extern "C" fn(
        *const c_void,
        *const c_char,
        VisitRecord,
        *mut c_void,
    ),
    module: *const c_char,
    includes: *const *const c_char,
    include_count: usize,
}

#[derive(Clone, Copy)]
struct Record<'a>(RecordRow, PhantomData<&'a Registry>);

impl<'a> Record<'a> {
    fn name(self) -> Option<&'a CStr> {
        // SAFETY: The row and its strings are borrowed from the registry.
        unsafe { c_str(self.0.qualified_name) }
    }

    fn alias(self) -> Option<&'a CStr> {
        // SAFETY: The row and its strings are borrowed from the registry.
        unsafe { c_str(self.0.alias_of) }
    }

    fn bases(self) -> impl Iterator<Item = &'a CStr> {
        self.names(self.0.bases)
    }

    fn fields(self) -> impl Iterator<Item = &'a CStr> {
        self.names(self.0.field_names)
    }

    fn names(
        self,
        mut next: *const *const c_char,
    ) -> impl Iterator<Item = &'a CStr> {
        std::iter::from_fn(move || {
            if next.is_null() {
                return None;
            }
            // SAFETY: Each array is null-terminated and owned by the registry.
            let name = unsafe { c_str(*next) };
            // SAFETY: Only a non-sentinel entry has a subsequent entry.
            next = if name.is_some() {
                unsafe { next.add(1) }
            } else {
                null()
            };
            name
        })
    }

    fn has_members(self) -> bool {
        self.alias().is_some()
            || self.fields().next().is_some()
            || self.bases().next().is_some()
    }
}

// SAFETY: A non-null pointer must be a C string valid for the inferred lifetime.
unsafe fn c_str<'a>(pointer: *const c_char) -> Option<&'a CStr> {
    if pointer.is_null() {
        None
    } else {
        Some(unsafe { CStr::from_ptr(pointer) })
    }
}

fn module_contains(module: &[u8], name: &[u8]) -> bool {
    !module.is_empty()
        && name
            .strip_prefix(module)
            .is_some_and(|rest| rest.starts_with(b"."))
}

#[derive(Default)]
enum Candidate<'a> {
    #[default]
    None,
    Unique(Record<'a>),
    Ambiguous,
}

impl<'a> Candidate<'a> {
    fn note(&mut self, candidate: Record<'a>) {
        match self {
            Self::None => *self = Self::Unique(candidate),
            Self::Unique(previous) if previous.name() == candidate.name() => {
                *previous = candidate
            }
            _ => *self = Self::Ambiguous,
        }
    }

    fn or(self, fallback: Self) -> Self {
        match self {
            Self::None => fallback,
            _ => self,
        }
    }
}

impl Registry {
    fn exact(&self, name: &CStr) -> Option<Record<'_>> {
        // SAFETY: The host callback returns a borrowed row. A null identity marks a miss.
        let row = unsafe { (self.exact)(self.data, name.as_ptr()) };
        self.borrow_row(row)
    }

    fn borrow_row(&self, row: RecordRow) -> Option<Record<'_>> {
        (!row.identity.is_null()).then_some(Record(row, PhantomData))
    }

    fn module(&self) -> Option<&CStr> {
        // SAFETY: The host borrows the module name for the duration of lookup.
        unsafe { c_str(self.module) }
    }

    fn includes_module(&self, name: &[u8]) -> bool {
        (0..self.include_count).any(|i| {
            // SAFETY: The host provides include_count readable entries.
            unsafe { c_str(*self.includes.add(i)) }
                .is_some_and(|module| module_contains(module.to_bytes(), name))
        })
    }

    fn short_candidates<'a, F: FnMut(Record<'a>) -> bool>(
        &'a self,
        name: &CStr,
        mut visit: F,
    ) {
        unsafe extern "C" fn accept<'a, F: FnMut(Record<'a>) -> bool>(
            state: *mut c_void,
            row: RecordRow,
        ) -> bool {
            // SAFETY: The host visits borrowed rows synchronously and passes back
            // the address of the live closure for each visit.
            unsafe { (&mut *state.cast::<F>())(Record(row, PhantomData)) }
        }

        // SAFETY: The callback and state are used only during this call.
        unsafe {
            (self.visit_short)(
                self.data,
                name.as_ptr(),
                accept::<F>,
                (&mut visit as *mut F).cast(),
            );
        }
    }

    fn exact_in_module(&self, module: &CStr, name: &CStr) -> Option<Record<'_>> {
        let len = module.to_bytes().len() + 1 + name.to_bytes_with_nul().len();
        let mut stack = [0; 1024];
        if len > stack.len() {
            // SAFETY: The host builds the key in its arena and returns a borrowed
            // row. An allocation failure is a miss, so the short-name scan runs.
            let row =
                unsafe { (self.exact_long_in_module)(self.data, name.as_ptr()) };
            return self.borrow_row(row);
        }
        let full = &mut stack[..len];
        let (prefix, suffix) = full.split_at_mut(module.to_bytes().len());
        prefix.copy_from_slice(module.to_bytes());
        suffix[0] = b'.';
        suffix[1..].copy_from_slice(name.to_bytes_with_nul());
        // SAFETY: Both inputs are C strings, and only the final byte is NUL.
        self.exact(unsafe { CStr::from_bytes_with_nul_unchecked(full) })
    }

    fn by_short_name(&self, short: &CStr, members_only: bool) -> Candidate<'_> {
        let mut local = Candidate::None;
        let mut included = Candidate::None;
        let mut any = Candidate::None;
        let module = self.module();
        self.short_candidates(short, |record| {
            let Some(name) = record.name().map(CStr::to_bytes) else {
                return true;
            };
            let parent = name
                .strip_suffix(short.to_bytes())
                .and_then(|prefix| prefix.strip_suffix(b"."));
            if parent.is_none_or(<[u8]>::is_empty)
                || (members_only && !record.has_members())
            {
                return true;
            }
            // Preserve the local registry's order when resolving a short name.
            if module
                .is_some_and(|module| module_contains(module.to_bytes(), name))
            {
                local.note(record);
                return false;
            }
            any.note(record);
            if self.includes_module(name) {
                included.note(record);
            }
            true
        });
        local.or(included).or(any)
    }

    fn resolve_record(&self, name: &CStr) -> Option<Record<'_>> {
        let exact = self.exact(name).or_else(|| {
            self.module()
                .and_then(|module| self.exact_in_module(module, name))
        });
        if exact.is_some_and(Record::has_members) {
            return exact;
        }

        let short = name.to_bytes().iter().rposition(|&ch| ch == b'.').map_or(
            name,
            |dot| {
                // SAFETY: This is a suffix of the original NUL-terminated name.
                unsafe {
                    CStr::from_bytes_with_nul_unchecked(
                        &name.to_bytes_with_nul()[dot + 1..],
                    )
                }
            },
        );
        // Member-bearing records beat forward declarations or parameter stubs.
        // An ambiguous member-bearing name must never fall back to a stub.
        match self.by_short_name(short, true) {
            Candidate::Unique(record) => Some(record),
            Candidate::Ambiguous => None,
            Candidate::None if exact.is_some() => exact,
            Candidate::None => match self.by_short_name(short, false) {
                Candidate::Unique(record) => Some(record),
                _ => None,
            },
        }
    }

    fn lookup_field(
        &self,
        name: &CStr,
        field: &CStr,
        require_type: bool,
        depth: usize,
    ) -> Option<(Record<'_>, *const FieldType)> {
        if depth > 5 {
            return None;
        }
        let record = self.resolve_record(name)?;
        if let Some(alias) = record.alias() {
            return self.lookup_field(alias, field, require_type, depth + 1);
        }
        for (index, candidate) in record.fields().enumerate() {
            if candidate != field {
                continue;
            }
            if !require_type {
                return Some((record, null()));
            }
            if !record.0.field_types.is_null() {
                // SAFETY: The host's type array is parallel to the field names.
                // An untyped field may hold a null entry before later typed fields.
                let ty = unsafe { *record.0.field_types.add(index) };
                if !ty.is_null() {
                    return Some((record, ty));
                }
            }
        }
        record.bases().find_map(|base| {
            self.lookup_field(base, field, require_type, depth + 1)
        })
    }
}

/// Resolve a record if field is null, a declaring record if ty is null,
/// or a typed field when both are supplied. Return the registry's original
/// record identity and field type. Neither pointer is retained.
///
/// # Safety
/// Registry and string pointers must be readable for the duration of this call.
/// The host supplies valid callbacks and borrowed rows, with name arrays
/// terminated by NULL and any field type array parallel to its name array.
/// The type output, when supplied, points to writable storage.
#[unsafe(no_mangle)]
unsafe extern "C" fn mori_c_field_lookup(
    registry: &Registry,
    name: *const c_char,
    field: *const c_char,
    ty: *mut *const FieldType,
) -> *const RegisteredType {
    let Some(name) = (unsafe { c_str(name) }) else {
        return null();
    };
    let record = match unsafe { c_str(field) } {
        None => registry.resolve_record(name),
        Some(field) => registry.lookup_field(name, field, !ty.is_null(), 0).map(
            |(record, field_type)| {
                if !ty.is_null() {
                    unsafe { *ty = field_type };
                }
                record
            },
        ),
    };
    record.map_or(null(), |record| record.0.identity)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
