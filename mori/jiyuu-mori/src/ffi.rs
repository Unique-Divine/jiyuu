//! The only module that knows about C. Deleting the C program later means
//! deleting this module and its callers.

use std::ffi::{c_char, c_int};

use crate::stdio::c_transport;

unsafe extern "C" {
    /// The C program's `main`, renamed when `src/main.c` is compiled with
    /// `-DCBM_AS_LIBRARY` (target `libmori-c` in `Makefile.cbm`). Its first
    /// statement is `cbm_alloc_init()`, which binds SQLite and Tree-sitter to
    /// mimalloc.
    fn cbm_c_main(argc: c_int, argv: *mut *mut c_char) -> c_int;
}

/// The process entry point. Forwards the kernel's argc and argv to the C
/// program unchanged, including the worker and daemon re-execs
/// (`cli --index-worker ...`, the hidden daemon argument), which C
/// classifies itself. Returning hands the code to the C runtime's `exit`.
///
/// # Safety
///
/// Called only by the C runtime, with argc strings in argv followed by
/// NULL, as for any C `main`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn main(argc: c_int, argv: *mut *mut c_char) -> c_int {
    // SAFETY: registration happens once before C startup. The process
    // arguments come from the C runtime and obey the contract of C main.
    unsafe {
        c_transport::register();
        cbm_c_main(argc, argv)
    }
}
