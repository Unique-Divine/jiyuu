//! The only module that knows about C. Deleting the C program later means
//! deleting this module and its callers.

use std::ffi::{CString, c_char, c_int};

unsafe extern "C" {
    /// The C program's `main`, renamed when `src/main.c` is compiled with
    /// `-DCBM_AS_LIBRARY` (target `libmori-c` in `Makefile.cbm`). Its first
    /// statement is `cbm_alloc_init()`, which binds SQLite and Tree-sitter to
    /// mimalloc, so nothing here may touch SQLite before the call.
    fn cbm_c_main(argc: c_int, argv: *mut *mut c_char) -> c_int;
}

/// Runs the C program with `args` as argv and returns its exit code.
pub fn run_c_main(args: Vec<CString>) -> i32 {
    restore_c_signal_defaults();
    let argc = c_int::try_from(args.len()).unwrap_or(c_int::MAX);
    // argv lives for the rest of the process, as a real argv does: C keeps
    // pointers into it (daemon and worker roles), so it is never freed.
    let mut argv: Vec<*mut c_char> =
        args.into_iter().map(CString::into_raw).collect();
    argv.push(std::ptr::null_mut());
    let argv = argv.leak();
    // SAFETY: argv holds argc valid NUL-terminated strings followed by NULL,
    // all leaked so they outlive the call, which is the contract of C main.
    unsafe { cbm_c_main(argc, argv.as_mut_ptr()) }
}

/// Undoes the one piece of Rust runtime set-up that changes what the C
/// program sees: Rust starts `main` with SIGPIPE ignored, while C `main`
/// starts with the default action (a write to a closed pipe ends the
/// process).
fn restore_c_signal_defaults() {
    #[cfg(unix)]
    // SAFETY: setting a signal disposition to SIG_DFL before any other
    // thread exists has no memory-safety preconditions.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}
