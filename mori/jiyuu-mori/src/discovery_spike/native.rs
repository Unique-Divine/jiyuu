//! Rust directory iteration over native discovery policy and output buffers.

use std::ffi::{CStr, CString, OsStr, c_char};
use std::fs;
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

unsafe extern "C" {
    #[cfg(not(test))]
    fn cbm_pipeline_set_discover_walk(walker: Walker);
    fn cbm_discover_walk_prepare(walk: *mut Walk, frame: *mut Frame) -> bool;
    fn cbm_discover_frame_path(frame: *const Frame) -> *const c_char;
    fn cbm_discover_walk_entry(
        walk: *mut Walk,
        frame: *const Frame,
        name: *const c_char,
    ) -> *mut Frame;
    fn cbm_discover_frame_free(frame: *mut Frame);
    fn cbm_discover_walk_stopped(walk: *mut Walk) -> bool;
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
    unsafe { cbm_pipeline_set_discover_walk(walk) };
}

pub(super) unsafe extern "C" fn walk(walk: *mut Walk, root: *mut Frame) -> bool {
    // SAFETY: C lends the walk state and root until the callback returns.
    // Keep the root borrowed and release all owned children before returning.
    catch_unwind(|| unsafe {
        let mut pending: Vec<Child> = Vec::new();
        if !scan_directory(walk, root, &mut pending) {
            return false;
        }
        // C scans the parent completely and visits children in reverse
        // discovery order. The order determines which ignores fit in the cap.
        while let Some(child) = pending.pop() {
            if cbm_discover_walk_stopped(walk) {
                break;
            }
            if !scan_directory(walk, child.0.as_ptr(), &mut pending) {
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
) -> bool {
    // SAFETY: walk and frame are valid for this scan. The path borrows the
    // frame. Native policy copies the name before it returns.
    unsafe {
        if !cbm_discover_walk_prepare(walk, frame) {
            return false;
        }
        let path = CStr::from_ptr(cbm_discover_frame_path(frame));
        let path = Path::new(OsStr::from_bytes(path.to_bytes()));
        // Ordinary C discovery treats an unopenable directory as empty and
        // stops reading the current directory if readdir fails.
        let Ok(mut entries) = fs::read_dir(path) else {
            return true;
        };
        while !cbm_discover_walk_stopped(walk) {
            let Some(Ok(entry)) = entries.next() else {
                break;
            };
            let name = CString::new(entry.file_name().into_vec())
                .expect("directory entry names contain no NUL");
            if let Some(child) =
                NonNull::new(cbm_discover_walk_entry(walk, frame, name.as_ptr()))
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
