//! Rust directory iteration, selection, and admission over native inputs.

mod budget;
mod selection;

use budget::{Budget, Limits, Violation};
use selection::{Check, Decision};
use std::borrow::Cow;
use std::ffi::{CStr, c_char, c_int, c_long, c_void};
use std::mem::MaybeUninit;
use std::panic::catch_unwind;
use std::ptr::NonNull;

#[repr(C)]
pub(super) struct Walk {
    _opaque: [u8; 0],
}

#[repr(C)]
pub(super) struct Frame {
    _opaque: [u8; 0],
}

#[repr(C)]
struct Selection {
    _opaque: [u8; 0],
}

#[repr(C)]
struct EntryOps {
    admit: unsafe extern "C" fn(*mut c_void, i64, *mut Violation) -> bool,
    select: unsafe extern "C" fn(*const Selection, bool) -> Decision,
}

static ENTRY_OPS: EntryOps = EntryOps { admit, select };

unsafe extern "C" {
    fn cbm_discover_walk_limits(walk: *const Walk, limits: *mut Limits);
    fn cbm_discover_walk_prepare(walk: *mut Walk, frame: *mut Frame) -> bool;
    fn cbm_discover_frame_path(frame: *const Frame) -> *const c_char;
    fn cbm_discover_walk_entry(
        walk: *mut Walk,
        frame: *const Frame,
        name: *const c_char,
        ops: *const EntryOps,
        state: *mut c_void,
    ) -> *mut Frame;
    fn cbm_discover_frame_free(frame: *mut Frame);
    fn cbm_discover_walk_stopped(walk: *mut Walk) -> bool;
    fn cbm_discover_selection_check(
        entry: *const Selection,
        check: Check,
    ) -> c_int;
    fn cbm_now_ms() -> u64;
    fn cbm_max_file_bytes() -> c_long;
}

struct Child(NonNull<Frame>);

impl Drop for Child {
    fn drop(&mut self) {
        // SAFETY: walk_entry transfers ownership of each returned child.
        unsafe { cbm_discover_frame_free(self.0.as_ptr()) };
    }
}

#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn mori_discover_walk(
    walk: *mut Walk,
    root: *mut Frame,
) -> bool {
    // SAFETY: C lends the walk state and root until the callback returns.
    // Keep the root borrowed and release all owned children before returning.
    catch_unwind(|| unsafe {
        let mut limits = MaybeUninit::uninit();
        cbm_discover_walk_limits(walk, limits.as_mut_ptr());
        let mut budget = Budget::new(limits.assume_init());
        let count_only = budget.limits.max_files >= 0;
        if budget.deadline_expired(|| cbm_now_ms()) {
            return false;
        }
        let mut pending: Vec<Child> = Vec::new();
        if !scan_directory(walk, root, &mut pending, count_only, &mut budget) {
            return false;
        }
        // Match the reference order. Scan each parent before visiting its
        // children in reverse discovery order. This fixes the ignore sample.
        while let Some(child) = pending.pop() {
            if budget.deadline_expired(|| cbm_now_ms()) {
                return false;
            }
            if cbm_discover_walk_stopped(walk) {
                break;
            }
            if !scan_directory(
                walk,
                child.0.as_ptr(),
                &mut pending,
                count_only,
                &mut budget,
            ) {
                return false;
            }
        }
        true
    })
    .unwrap_or(false)
}

unsafe fn scan_directory(
    walk: *mut Walk,
    frame: *mut Frame,
    pending: &mut Vec<Child>,
    count_only: bool,
    budget: &mut Budget,
) -> bool {
    // SAFETY: walk and frame are valid for this scan. The path borrows the
    // frame. Native policy copies the name before it returns.
    unsafe {
        if !cbm_discover_walk_prepare(walk, frame) {
            return false;
        }
        let path = CStr::from_ptr(cbm_discover_frame_path(frame));
        // Bounded admission fails if a directory cannot be opened.
        let Some(mut entries) = Directory::open(path) else {
            return !count_only;
        };
        loop {
            if budget.deadline_expired(|| cbm_now_ms()) {
                return false;
            }
            if cbm_discover_walk_stopped(walk) {
                break;
            }
            let Some(name) = entries.next_name() else {
                break;
            };
            if let Some(child) = NonNull::new(cbm_discover_walk_entry(
                walk,
                frame,
                name.as_ptr(),
                &ENTRY_OPS,
                std::ptr::from_mut(budget).cast(),
            ))
            .map(Child)
            {
                if pending.try_reserve(1).is_err() {
                    return false;
                }
                pending.push(child);
            }
        }
        true
    }
}

#[cfg(unix)]
struct Directory(std::fs::ReadDir);

#[cfg(unix)]
impl Directory {
    fn open(path: &CStr) -> Option<Self> {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        use std::path::Path;

        std::fs::read_dir(Path::new(OsStr::from_bytes(path.to_bytes())))
            .ok()
            .map(Self)
    }

    // Like readdir in the reference, an iteration error ends this directory.
    fn next_name(&mut self) -> Option<Cow<'_, CStr>> {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStringExt;

        let entry = self.0.next()?.ok()?;
        Some(Cow::Owned(
            CString::new(entry.file_name().into_vec())
                .expect("directory entry names contain no NUL"),
        ))
    }
}

#[cfg(windows)]
struct Directory(NonNull<c_void>);

#[cfg(windows)]
unsafe extern "C" {
    fn cbm_opendir(path: *const c_char) -> *mut c_void;
    fn cbm_discover_dir_next(directory: *mut c_void) -> *const c_char;
    fn cbm_closedir(directory: *mut c_void);
}

#[cfg(windows)]
impl Directory {
    fn open(path: &CStr) -> Option<Self> {
        // SAFETY: cbm_opendir takes a UTF-8 C string and returns an owned
        // enumeration handle. Keep its Win32 path and encoding behavior.
        NonNull::new(unsafe { cbm_opendir(path.as_ptr()) }).map(Self)
    }

    fn next_name(&mut self) -> Option<Cow<'_, CStr>> {
        // SAFETY: The name is valid until the next read or close. The borrow
        // prevents either operation while the caller uses it.
        let name = unsafe { cbm_discover_dir_next(self.0.as_ptr()) };
        (!name.is_null()).then(|| Cow::Borrowed(unsafe { CStr::from_ptr(name) }))
    }
}

#[cfg(windows)]
impl Drop for Directory {
    fn drop(&mut self) {
        // SAFETY: Only this wrapper owns the enumeration handle.
        unsafe { cbm_closedir(self.0.as_ptr()) };
    }
}

unsafe extern "C" fn select(entry: *const Selection, is_dir: bool) -> Decision {
    // SAFETY: C lends the selection and its referenced paths and metadata
    // until this callback returns.
    let check = |check| unsafe { cbm_discover_selection_check(entry, check) };
    if is_dir {
        selection::directory(check)
    } else {
        selection::file(check)
    }
}

unsafe extern "C" fn admit(
    state: *mut c_void,
    size: i64,
    violation: *mut Violation,
) -> bool {
    // SAFETY: walk_entry lends a live Budget and a writable diagnostic for
    // this call. cbm_max_file_bytes returns a positive extraction ceiling.
    let budget = unsafe { &mut *state.cast::<Budget>() };
    match budget.admit(size, || unsafe { cbm_max_file_bytes() as u64 }) {
        Ok(()) => true,
        Err(reason) => {
            unsafe { violation.write(reason) };
            false
        }
    }
}
