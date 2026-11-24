//! Rust directory iteration and admission over native discovery filters.

mod budget;

use budget::{Budget, Limits, Violation};
use std::ffi::{CStr, CString, OsStr, c_char, c_long, c_void};
use std::fs;
use std::mem::MaybeUninit;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::panic::catch_unwind;
use std::path::Path;
use std::ptr::NonNull;

#[repr(C)]
pub(super) struct Walk {
    _opaque: [u8; 0],
}

#[repr(C)]
pub(super) struct Frame {
    _opaque: [u8; 0],
}

pub(super) type Walker = unsafe extern "C" fn(*mut Walk, *mut Frame) -> bool;
type Admit = unsafe extern "C" fn(*mut c_void, i64, *mut Violation) -> bool;

unsafe extern "C" {
    #[cfg(not(test))]
    fn cbm_pipeline_set_discover_walk(walker: Walker);
    #[cfg(not(test))]
    fn cbm_discover_set_admission_walk(walker: Walker);
    fn cbm_discover_walk_limits(walk: *const Walk, limits: *mut Limits);
    fn cbm_discover_walk_prepare(walk: *mut Walk, frame: *mut Frame) -> bool;
    fn cbm_discover_frame_path(frame: *const Frame) -> *const c_char;
    fn cbm_discover_walk_entry(
        walk: *mut Walk,
        frame: *const Frame,
        name: *const c_char,
        admit: Admit,
        state: *mut c_void,
    ) -> *mut Frame;
    fn cbm_discover_frame_free(frame: *mut Frame);
    fn cbm_discover_walk_stopped(walk: *mut Walk) -> bool;
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

/// Register the Rust walker.
///
/// # Safety
///
/// Call before the C engine starts any indexing threads.
#[cfg(not(test))]
pub unsafe fn register() {
    // SAFETY: the binary calls this during process startup, and the callback
    // lives for the process lifetime.
    unsafe {
        cbm_pipeline_set_discover_walk(walk);
        if std::env::var_os("MORI_IMPL_ADMISSION").as_deref()
            != Some(OsStr::new("c"))
        {
            cbm_discover_set_admission_walk(walk_bounded);
        }
    }
}

pub(super) unsafe extern "C" fn walk(walk: *mut Walk, root: *mut Frame) -> bool {
    run_walk(walk, root, false)
}

pub(super) unsafe extern "C" fn walk_bounded(
    walk: *mut Walk,
    root: *mut Frame,
) -> bool {
    run_walk(walk, root, true)
}

fn run_walk(walk: *mut Walk, root: *mut Frame, count_only: bool) -> bool {
    // SAFETY: C lends the walk state and root until the callback returns.
    // Keep the root borrowed and release all owned children before returning.
    catch_unwind(|| unsafe {
        let mut limits = MaybeUninit::uninit();
        cbm_discover_walk_limits(walk, limits.as_mut_ptr());
        let mut budget = Budget::new(limits.assume_init());
        if budget.deadline_expired(|| cbm_now_ms()) {
            return false;
        }
        let mut pending: Vec<Child> = Vec::new();
        if !scan_directory(walk, root, &mut pending, count_only, &mut budget) {
            return false;
        }
        // C scans the parent completely and visits children in reverse
        // discovery order. The order determines which ignores fit in the cap.
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
        let path = Path::new(OsStr::from_bytes(path.to_bytes()));
        // Bounded admission fails if a directory cannot be opened. Both
        // native walkers stop reading the current directory on a readdir error.
        let Ok(mut entries) = fs::read_dir(path) else {
            return !count_only;
        };
        loop {
            if budget.deadline_expired(|| cbm_now_ms()) {
                return false;
            }
            if cbm_discover_walk_stopped(walk) {
                break;
            }
            let Some(Ok(entry)) = entries.next() else {
                break;
            };
            let name = CString::new(entry.file_name().into_vec())
                .expect("directory entry names contain no NUL");
            if let Some(child) = NonNull::new(cbm_discover_walk_entry(
                walk,
                frame,
                name.as_ptr(),
                admit,
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
