//! The only module that knows about C. Deleting the C program later means
//! deleting this module and its callers.

use std::ffi::{CStr, OsStr, c_char, c_int};
use std::os::unix::ffi::OsStrExt;

use rusqlite::{
    Connection,
    ffi::{sqlite3, sqlite3_stmt},
};

use crate::graphql_link::{Action, delete_stale_link, plan_link, upsert_link};

unsafe extern "C" {
    /// The C program's `main`, renamed when `src/main.c` is compiled with
    /// `-DCBM_AS_LIBRARY` (target `libmori-c` in `Makefile.cbm`). Its first
    /// statement is `cbm_alloc_init()`, which binds SQLite and Tree-sitter to
    /// mimalloc.
    fn cbm_c_main(argc: c_int, argv: *mut *mut c_char) -> c_int;

    fn cbm_alloc_init();
    fn cbm_gqlgen_set_glob_matcher(
        matcher: extern "C" fn(glob: *const c_char, path: *const c_char) -> bool,
    );
    fn cbm_cross_repo_set_graphql_backend(
        policy: extern "C" fn(bool, bool, bool, bool, bool) -> Action,
        delete: extern "C" fn(*mut sqlite3, i64, i64) -> c_int,
        upsert: extern "C" fn(
            *mut sqlite3_stmt,
            *const c_char,
            i64,
            i64,
            *const c_char,
        ) -> bool,
    );
}

extern "C" fn gqlgen_glob_match(
    glob: *const c_char,
    path: *const c_char,
) -> bool {
    // SAFETY: C passes NUL-terminated strings valid for the whole call.
    let (glob, path) = unsafe { (CStr::from_ptr(glob), CStr::from_ptr(path)) };
    crate::gqlgen::glob_matches(glob.to_bytes(), path.to_bytes())
}

extern "C" fn graphql_delete_stale_link(
    db: *mut sqlite3,
    caller: i64,
    route: i64,
) -> c_int {
    // SAFETY: C passes a live connection owned by this run. The non-owning
    // wrapper and its statements are dropped before C resumes using it.
    let result = unsafe { Connection::from_handle(db) }
        .and_then(|db| delete_stale_link(&db, caller, route));
    result.map(c_int::from).unwrap_or(-1)
}

extern "C" fn graphql_upsert_link(
    stmt: *mut sqlite3_stmt,
    project: *const c_char,
    source: i64,
    target: i64,
    properties: *const c_char,
) -> bool {
    // SAFETY: The graph store lends a reset edge-upsert statement and the
    // C strings are NUL-terminated and valid until the callback returns.
    unsafe {
        upsert_link(
            stmt,
            CStr::from_ptr(project),
            source,
            target,
            CStr::from_ptr(properties),
        )
    }
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
    // SAFETY: C has not started any indexing threads and the function pointer
    // stays valid for the process lifetime, including each worker's own main.
    unsafe {
        cbm_gqlgen_set_glob_matcher(gqlgen_glob_match);
        cbm_cross_repo_set_graphql_backend(
            plan_link,
            graphql_delete_stale_link,
            graphql_upsert_link,
        );
    }

    // SAFETY: argc and argv come straight from the C runtime and meet the
    // contract of C main, which cbm_c_main is.
    unsafe { cbm_c_main(argc, argv) }
}
