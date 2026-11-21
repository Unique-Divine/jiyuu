//! The only module that knows about C. Deleting the C program later means
//! deleting this module and its callers.

use std::ffi::{CStr, OsStr, c_char, c_int};
use std::os::unix::ffi::OsStrExt;

unsafe extern "C" {
    /// The C program's `main`, renamed when `src/main.c` is compiled with
    /// `-DCBM_AS_LIBRARY` (target `libmori-c` in `Makefile.cbm`). Its first
    /// statement is `cbm_alloc_init()`, which binds SQLite and Tree-sitter to
    /// mimalloc.
    fn cbm_c_main(argc: c_int, argv: *mut *mut c_char) -> c_int;

    fn cbm_alloc_init();
}

/// The process entry point. Dispatches the developer export in Rust.
/// Otherwise forwards the kernel's argc and argv to C unchanged, including
/// worker and daemon re-execs
/// (`cli --index-worker ...`, the hidden daemon argument), which C
/// classifies itself. Returning hands the code to the C runtime's `exit`.
///
/// # Safety
///
/// Called only by the C runtime, with argc strings in argv followed by
/// NULL, as for any C `main`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn main(argc: c_int, argv: *mut *mut c_char) -> c_int {
    // SAFETY: the C runtime provides argc initialized, non-null argv slots.
    let args = unsafe { std::slice::from_raw_parts(argv, argc as usize) };
    if let Some(first) = args.get(1) {
        // SAFETY: each argv slot points to a NUL-terminated string.
        if unsafe { CStr::from_ptr(*first) }.to_bytes() == b"dev" {
            // SAFETY: SQLite has not been used yet. Initialize the same
            // allocator as C main before the exporter opens the database.
            unsafe { cbm_alloc_init() };
            return crate::export::run(args[2..].iter().map(|&arg| {
                // SAFETY: the same argv contract applies to remaining slots.
                OsStr::from_bytes(unsafe { CStr::from_ptr(arg) }.to_bytes())
            }));
        }
    }
    // SAFETY: argc and argv come straight from the C runtime and meet the
    // contract of C main, which cbm_c_main is.
    unsafe { cbm_c_main(argc, argv) }
}
