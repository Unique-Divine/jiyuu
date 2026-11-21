//! Rust startup for the linked C program.

use std::ffi::{CStr, CString, OsStr, c_char, c_int, c_void};
use std::ops::ControlFlow;
use std::os::unix::ffi::OsStrExt;
use std::{panic, ptr};

use rusqlite::{Connection, ffi::sqlite3};

use crate::store::{FileOutlineQuery, Store};
use crate::tools;

type ReadOverride = unsafe extern "C" fn(
    tool: *const c_char,
    options: *const c_void,
    store: *mut c_void,
    project: *const c_char,
) -> *mut c_char;
type ReleasePayload = unsafe extern "C" fn(payload: *mut c_char);

#[repr(C)]
struct GraphSchemaOptions {
    limit: c_int,
    offset: c_int,
    include_properties: bool,
}

#[repr(C)]
struct OutlineRow {
    name: *const c_char,
    label: *const c_char,
    qualified_name: *const c_char,
    start_line: c_int,
    end_line: c_int,
}

type OutlineEmit = unsafe extern "C" fn(
    context: *mut c_void,
    row: *const OutlineRow,
    lengths: *const usize,
) -> c_int;
type OutlineReader = unsafe extern "C" fn(
    db: *mut sqlite3,
    project: *const c_char,
    file_path: *const c_char,
    labels: *const *const c_char,
    label_count: c_int,
    limit: c_int,
    offset: c_int,
    total: *mut c_int,
    emit: OutlineEmit,
    context: *mut c_void,
) -> c_int;

const STORE_OK: c_int = 0;
const STORE_ERR: c_int = -1;
const STORE_CANCELLED: c_int = -3;

use crate::stdio::c_transport;

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
    fn cbm_mcp_set_read_override(handle: ReadOverride, release: ReleasePayload);
    fn cbm_store_get_db(store: *mut c_void) -> *mut sqlite3;
    fn cbm_store_set_file_outline_reader(reader: OutlineReader);
    fn cbm_mcp_project_adr_presence(
        store: *mut c_void,
        project: *const c_char,
    ) -> c_int;
}

unsafe extern "C" fn read_file_outline(
    db: *mut sqlite3,
    project: *const c_char,
    file_path: *const c_char,
    labels: *const *const c_char,
    label_count: c_int,
    limit: c_int,
    offset: c_int,
    total: *mut c_int,
    emit: OutlineEmit,
    context: *mut c_void,
) -> c_int {
    let result = panic::catch_unwind(|| {
        // SAFETY: the C store validates the NUL-terminated arguments and
        // label array before the call. The SQLite handle remains owned by C.
        let (project, path, labels, connection) = unsafe {
            let labels = if label_count == 0 {
                &[]
            } else {
                std::slice::from_raw_parts(labels, label_count as usize)
            };
            let labels = labels
                .iter()
                .map(|&label| CStr::from_ptr(label).to_bytes())
                .collect::<Vec<_>>();
            (
                CStr::from_ptr(project).to_bytes(),
                CStr::from_ptr(file_path).to_bytes(),
                labels,
                Connection::from_handle(db)?,
            )
        };
        let query = FileOutlineQuery {
            project,
            path,
            labels: &labels,
            limit,
            offset,
        };
        Store::from_connection(connection).file_outline(query, |row| {
            let fields = [row.name, row.label, row.qualified_name];
            let pointers = fields.map(|bytes| {
                if bytes.is_empty() {
                    c"".as_ptr()
                } else {
                    bytes.as_ptr().cast()
                }
            });
            let lengths = fields.map(<[u8]>::len);
            let raw = OutlineRow {
                name: pointers[0],
                label: pointers[1],
                qualified_name: pointers[2],
                start_line: row.start_line,
                end_line: row.end_line,
            };
            // SAFETY: the C sink copies the row synchronously before SQLite
            // advances, using the exact byte lengths of the borrowed slices.
            let status = unsafe { emit(context, &raw, lengths.as_ptr()) };
            if status == STORE_OK {
                ControlFlow::Continue(())
            } else {
                ControlFlow::Break(status)
            }
        })
    });
    match result {
        Ok(Ok(ControlFlow::Continue(count))) => {
            // SAFETY: the C store validates the output pointer before calling.
            unsafe { *total = count };
            STORE_OK
        }
        Ok(Ok(ControlFlow::Break(status))) => status,
        Ok(Err(rusqlite::Error::SqliteFailure(error, _)))
            if error.code == rusqlite::ErrorCode::OperationInterrupted =>
        {
            STORE_CANCELLED
        }
        _ => STORE_ERR,
    }
}

unsafe extern "C" fn read_override(
    tool: *const c_char,
    options: *const c_void,
    store: *mut c_void,
    project: *const c_char,
) -> *mut c_char {
    if tool.is_null()
        || options.is_null()
        || store.is_null()
        || project.is_null()
    {
        return ptr::null_mut();
    }
    let result = panic::catch_unwind(|| {
        // SAFETY: the C dispatch borrows valid NUL-terminated strings and a
        // request-owned C store for the duration of this synchronous call.
        let tool = unsafe { CStr::from_ptr(tool) };
        if tool.to_bytes() != b"get_graph_schema" {
            return None;
        }
        // SAFETY: the C dispatcher attaches normalized schema settings to
        // this tool name and keeps them live until the callback returns.
        let options = unsafe { &*options.cast::<GraphSchemaOptions>() };
        let project_name = unsafe { CStr::from_ptr(project) }.to_str().ok()?;
        let adr_present =
            match unsafe { cbm_mcp_project_adr_presence(store, project) } {
                0 => Some(false),
                1 => Some(true),
                _ => None,
            };
        let db = unsafe { cbm_store_get_db(store) };
        if db.is_null() {
            return None;
        }
        // SAFETY: C retains ownership; from_handle leaves the existing
        // connection open, and Rust finalizes its own statements on return.
        let store =
            Store::from_connection(unsafe { Connection::from_handle(db) }.ok()?);
        let payload = tools::graph_schema(
            &store,
            project_name,
            options.limit as usize,
            options.offset as usize,
            options.include_properties,
            adr_present,
        )?;
        CString::new(payload).ok()
    });
    result
        .ok()
        .flatten()
        .map_or(ptr::null_mut(), CString::into_raw)
}

unsafe extern "C" fn release_payload(payload: *mut c_char) {
    // SAFETY: C calls this once for each non-null pointer read_override
    // returns via CString::into_raw, without modifying the allocation.
    drop(unsafe { CString::from_raw(payload) });
}

extern "C" fn gqlgen_glob_match(
    glob: *const c_char,
    path: *const c_char,
) -> bool {
    // SAFETY: C passes NUL-terminated strings valid for the whole call.
    let (glob, path) = unsafe { (CStr::from_ptr(glob), CStr::from_ptr(path)) };
    crate::gqlgen::glob_matches(glob.to_bytes(), path.to_bytes())
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
        c_transport::register();
    }

    // SAFETY: argc and argv come straight from the C runtime and meet the
    // contract of C main, which cbm_c_main is.
    unsafe {
        cbm_mcp_set_read_override(read_override, release_payload);
        cbm_store_set_file_outline_reader(read_file_outline);
        cbm_c_main(argc, argv)
    }
}
