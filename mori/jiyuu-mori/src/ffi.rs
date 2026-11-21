//! Rust startup for the linked C program.

use std::ffi::{CStr, CString, OsStr, c_char, c_int, c_void};
use std::ops::ControlFlow;
use std::os::unix::ffi::OsStrExt;
use std::{panic, ptr};

use rusqlite::{Connection, ffi::sqlite3};

use crate::graphql_link::{Action, plan_link};
use crate::link::RoutePlan;
use crate::stdio::c_transport;
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

const STORE_OK: c_int = 0;
const STORE_ERR: c_int = -1;
const STORE_CANCELLED: c_int = -3;

#[repr(C)]
struct GraphqlRoutePlan {
    field_index: usize,
    resolver: *const c_void,
    ambiguous: bool,
}

type RoutePlanner =
    unsafe extern "C" fn(*mut c_void, *const c_void, usize) -> GraphqlRoutePlan;
type ResolverVisitor =
    unsafe extern "C" fn(usize, *const c_void, *mut c_void) -> bool;

#[repr(C)]
struct GraphqlRouteRow {
    path: *const c_char,
    qn: *const c_char,
    file_path: *const c_char,
    return_type: *const c_char,
    reason: *const c_char,
    field_id: i64,
    resolver_id: i64,
    start_line: c_int,
    end_line: c_int,
    deprecated: bool,
    ambiguous: bool,
}

type RouteWriter =
    unsafe extern "C" fn(*mut c_void, *const GraphqlRouteRow) -> bool;

unsafe extern "C" {
    /// The C program's `main`, renamed when `src/main.c` is compiled with
    /// `-DCBM_AS_LIBRARY` (target `libmori-c` in `Makefile.cbm`). Its first
    /// statement is `cbm_alloc_init()`, which binds SQLite and Tree-sitter to
    /// mimalloc.
    fn cbm_c_main(argc: c_int, argv: *mut *mut c_char) -> c_int;

    fn cbm_alloc_init();
    fn cbm_gqlgen_set_rust_backend(
        matcher: extern "C" fn(glob: *const c_char, path: *const c_char) -> bool,
        planner: RoutePlanner,
        writer: RouteWriter,
    );
    fn cbm_cross_repo_set_graphql_link_policy(
        policy: extern "C" fn(bool, bool, bool, bool, bool) -> Action,
    );

    fn cbm_gqlgen_visit_route_resolvers(
        ctx: *mut c_void,
        first: *const c_void,
        count: usize,
        visit: ResolverVisitor,
        user: *mut c_void,
    );

    fn cbm_gbuf_upsert_node(
        gb: *mut c_void,
        label: *const c_char,
        name: *const c_char,
        qualified_name: *const c_char,
        file_path: *const c_char,
        start_line: c_int,
        end_line: c_int,
        properties: *const c_char,
    ) -> i64;
    fn cbm_gbuf_insert_edge(
        gb: *mut c_void,
        source: i64,
        target: i64,
        kind: *const c_char,
        properties: *const c_char,
    ) -> i64;
    fn cbm_mcp_set_read_override(handle: ReadOverride, release: ReleasePayload);
    fn cbm_mcp_set_project_readers(
        identity: ProjectIdentityReader,
        stats: ProjectStatsReader,
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

extern "C" fn gqlgen_glob_match(
    glob: *const c_char,
    path: *const c_char,
) -> bool {
    // SAFETY: C passes NUL-terminated strings valid for the whole call.
    let (glob, path) = unsafe { (CStr::from_ptr(glob), CStr::from_ptr(path)) };
    crate::gqlgen::glob_matches(glob.to_bytes(), path.to_bytes())
}

unsafe extern "C" fn plan_graphql_route(
    ctx: *mut c_void,
    first: *const c_void,
    count: usize,
) -> GraphqlRoutePlan {
    let mut plan = RoutePlan::<*const c_void>::new(count);
    // SAFETY: C passes its initialized context and a nonempty candidate
    // array. It visits candidates synchronously and never retains user data.
    unsafe {
        cbm_gqlgen_visit_route_resolvers(
            ctx,
            first,
            count,
            consider_graphql_resolver,
            ptr::from_mut(&mut plan).cast(),
        );
    }
    GraphqlRoutePlan {
        field_index: plan.field_index,
        resolver: plan.resolver.unwrap_or(ptr::null()),
        ambiguous: plan.ambiguous,
    }
}

unsafe extern "C" fn consider_graphql_resolver(
    field_index: usize,
    resolver: *const c_void,
    user: *mut c_void,
) -> bool {
    // SAFETY: plan_graphql_route lends this stack value to the visitor for
    // the duration of the visit, and C calls it synchronously.
    let plan = unsafe { &mut *user.cast::<RoutePlan<*const c_void>>() };
    plan.consider(field_index, resolver)
}

unsafe extern "C" fn write_graphql_route(
    gb: *mut c_void,
    row: *const GraphqlRouteRow,
) -> bool {
    // SAFETY: The route pass lends its live graph and this row for the call.
    // The schema type and reason always point to NUL-terminated strings.
    let row = unsafe { &*row };
    let (return_type, reason) =
        unsafe { (CStr::from_ptr(row.return_type), CStr::from_ptr(row.reason)) };
    let props = crate::route_writer::route_properties(
        return_type.to_bytes(),
        reason.to_bytes(),
        row.deprecated,
        row.ambiguous,
    );
    // SAFETY: All strings are valid for this call and the buffer copies them.
    let route_id = unsafe {
        cbm_gbuf_upsert_node(
            gb,
            c"Route".as_ptr(),
            row.path,
            row.qn,
            row.file_path,
            row.start_line,
            row.end_line,
            props.as_ptr().cast(),
        )
    };
    if route_id == 0 {
        return false;
    }
    let (handler, via) = if row.resolver_id == 0 {
        (row.field_id, c"{\"via\":\"graphql_schema_field\"}")
    } else {
        (row.resolver_id, c"{\"via\":\"gqlgen_resolver\"}")
    };
    // SAFETY: Both IDs come from this graph. The buffer copies the strings.
    unsafe {
        cbm_gbuf_insert_edge(
            gb,
            handler,
            route_id,
            c"HANDLES".as_ptr(),
            via.as_ptr(),
        );
    }
    true
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
        cbm_gqlgen_set_rust_backend(
            gqlgen_glob_match,
            plan_graphql_route,
            write_graphql_route,
        );
        c_transport::register();
        cbm_cross_repo_set_graphql_link_policy(plan_link);
        crate::query_cli::c_args::register();
    }

    // SAFETY: argc and argv come straight from the C runtime and meet the
    // contract of C main, which cbm_c_main is.
    unsafe {
        cbm_mcp_set_read_override(read_override, release_payload);
        cbm_store_set_file_outline_reader(read_file_outline);
        cbm_mcp_set_project_readers(read_project_identity, read_project_stats);
        cbm_c_main(argc, argv)
    }
}
