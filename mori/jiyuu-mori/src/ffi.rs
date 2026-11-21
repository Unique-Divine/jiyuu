//! Rust startup for the linked C program.

use std::ffi::{CStr, CString, OsStr, c_char, c_int, c_void};
use std::ops::ControlFlow;
use std::os::unix::ffi::OsStrExt;
use std::{panic, ptr};

use crate::project_page::{ProjectPage, ProjectRecord, build_project_page};
use crate::stdio::c_transport;
use crate::store::{FileOutlineQuery, Store};
use crate::tools;
use rusqlite::{Connection, ffi::sqlite3};

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

type ProjectIdentityEmit = unsafe extern "C" fn(
    context: *mut c_void,
    name: *const c_char,
    root_path: *const c_char,
) -> c_int;
type ProjectIdentityReader = unsafe extern "C" fn(
    store: *mut c_void,
    name_capacity: usize,
    emit: ProjectIdentityEmit,
    context: *mut c_void,
) -> c_int;
type ProjectStatsReader = unsafe extern "C" fn(
    store: *mut c_void,
    project: *const c_char,
    nodes: *mut c_int,
    edges: *mut c_int,
);
type ProjectMetadataEmit = unsafe extern "C" fn(
    context: *mut c_void,
    root_path: *const c_char,
    indexed_at: *const c_char,
);
type ProjectMetadataReader = unsafe extern "C" fn(
    store: *mut c_void,
    project: *const c_char,
    emit: ProjectMetadataEmit,
    context: *mut c_void,
) -> bool;

const STORE_OK: c_int = 0;
const STORE_ERR: c_int = -1;
const STORE_CANCELLED: c_int = -3;

unsafe extern "C" {
    /// The C program's `main`, renamed when `src/main.c` is compiled with
    /// `-DCBM_AS_LIBRARY` (target `libmori-c` in `Makefile.cbm`). Its first
    /// statement is `cbm_alloc_init()`, which binds SQLite and Tree-sitter to
    /// mimalloc.
    fn cbm_c_main(argc: c_int, argv: *mut *mut c_char) -> c_int;

    fn cbm_alloc_init();
    fn cbm_mcp_set_read_override(handle: ReadOverride, release: ReleasePayload);
    fn cbm_mcp_set_project_readers(
        identity: ProjectIdentityReader,
        stats: ProjectStatsReader,
        metadata: ProjectMetadataReader,
    );
    fn cbm_mcp_set_project_page_builder(
        builder: unsafe extern "C" fn(
            *mut ProjectRecord,
            c_int,
            *mut ProjectPage,
        ),
    );
    fn cbm_store_get_db(store: *mut c_void) -> *mut sqlite3;
    fn cbm_store_set_file_outline_reader(reader: OutlineReader);
    fn cbm_mcp_project_adr_presence(
        store: *mut c_void,
        project: *const c_char,
    ) -> c_int;
}

// The returned wrapper must be dropped before C releases its store.
unsafe fn borrow_c_store(store: *mut c_void) -> Option<Store> {
    // SAFETY: callers pass the live C store and use the wrapper within the
    // callback. from_handle does not close the C connection on drop.
    let db = unsafe { cbm_store_get_db(store) };
    if db.is_null() {
        return None;
    }
    Some(Store::from_connection(
        unsafe { Connection::from_handle(db) }.ok()?,
    ))
}

unsafe extern "C" fn read_project_identity(
    store: *mut c_void,
    name_capacity: usize,
    emit: ProjectIdentityEmit,
    context: *mut c_void,
) -> c_int {
    const SKIP: c_int = 0;
    const MISSING_ROOT: c_int = -1;
    panic::catch_unwind(|| {
        // SAFETY: C passes an open query-only store and keeps it live until
        // the callback returns.
        let Some(store) = (unsafe { borrow_c_store(store) }) else {
            return SKIP;
        };
        let Ok(Some(identity)) = store.project_identity(name_capacity) else {
            return SKIP;
        };
        let Some(root_path) = identity.root_path else {
            return MISSING_ROOT;
        };
        let (Ok(name), Ok(root_path)) =
            (CString::new(identity.name), CString::new(root_path))
        else {
            return SKIP;
        };
        // SAFETY: C copies both strings synchronously into its project record.
        unsafe { emit(context, name.as_ptr(), root_path.as_ptr()) }
    })
    .unwrap_or(SKIP)
}

unsafe extern "C" fn read_project_stats(
    store: *mut c_void,
    project: *const c_char,
    nodes: *mut c_int,
    edges: *mut c_int,
) {
    let result = panic::catch_unwind(|| {
        // SAFETY: C keeps the store and project string live until return.
        let store = unsafe { borrow_c_store(store) }?;
        let project = unsafe { CStr::from_ptr(project) }.to_bytes();
        Some(store.project_stats(project))
    })
    .ok()
    .flatten()
    .unwrap_or((STORE_ERR, STORE_ERR));
    // SAFETY: C supplies distinct writable result fields in its record.
    unsafe { (*nodes, *edges) = result };
}

unsafe extern "C" fn read_project_metadata(
    store: *mut c_void,
    project: *const c_char,
    emit: ProjectMetadataEmit,
    context: *mut c_void,
) -> bool {
    panic::catch_unwind(|| {
        // SAFETY: C keeps the store and project string live until return.
        let store = unsafe { borrow_c_store(store) }?;
        let project = unsafe { CStr::from_ptr(project) }.to_bytes();
        let metadata = store.project_metadata(project).ok()??;
        let root_path = metadata
            .root_path
            .and_then(|value| CString::new(value).ok());
        let indexed_at = metadata
            .indexed_at
            .and_then(|value| CString::new(value).ok());
        let root_path = root_path
            .as_ref()
            .map_or(ptr::null(), |value| value.as_ptr());
        let indexed_at = indexed_at
            .as_ref()
            .map_or(ptr::null(), |value| value.as_ptr());
        // SAFETY: the sink copies both nullable strings synchronously.
        unsafe { emit(context, root_path, indexed_at) };
        Some(())
    })
    .ok()
    .flatten()
    .is_some()
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
        let store = unsafe { borrow_c_store(store) }?;
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
        jiyuu_mori::mori_register_graphql_callbacks();
        c_transport::register();
        crate::query_cli::c_args::register();
        crate::cli_output::register();
        crate::initialize::c_initialize::register();
        crate::tool_list::c_tool_list::register();
    }

    // SAFETY: argc and argv come straight from the C runtime and meet the
    // contract of C main, which cbm_c_main is.
    unsafe {
        cbm_mcp_set_read_override(read_override, release_payload);
        cbm_store_set_file_outline_reader(read_file_outline);
        cbm_mcp_set_project_readers(
            read_project_identity,
            read_project_stats,
            read_project_metadata,
        );
        cbm_mcp_set_project_page_builder(build_project_page);
        cbm_c_main(argc, argv)
    }
}
