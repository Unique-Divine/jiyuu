//! GraphQL callbacks used by the linked executable and native fixture runner.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr;

use rusqlite::{
    Connection,
    ffi::{sqlite3, sqlite3_stmt},
};

use crate::gqlgen::{
    ROOT_COUNT, ResolverIndex, ResolverScope, SchemaField, SchemaIndex,
};
use crate::graphql_link::{
    Action, delete_stale_link, find_handler, plan_link, upsert_link,
};

#[cfg(feature = "native-graphql-fixtures")]
mod fixture_witness {
    use std::sync::atomic::{AtomicU32, Ordering};

    pub(super) const GLOB: u32 = 1 << 0;
    pub(super) const ROUTE_PLAN: u32 = 1 << 1;
    pub(super) const ROUTE_WRITE: u32 = 1 << 2;
    pub(super) const LINK_FIND: u32 = 1 << 3;
    pub(super) const LINK_UPSERT: u32 = 1 << 4;
    pub(super) const LINK_DELETE: u32 = 1 << 5;
    pub(super) const DOCUMENT: u32 = 1 << 6;
    pub(super) const ROOT_MATCH: u32 = 1 << 7;

    static VISITED: AtomicU32 = AtomicU32::new(0);

    pub(super) fn mark(callback: u32) {
        VISITED.fetch_or(callback, Ordering::Relaxed);
    }

    #[unsafe(no_mangle)]
    extern "C" fn mori_graphql_fixture_callbacks_seen() -> u32 {
        VISITED.load(Ordering::Relaxed)
    }
}

#[repr(C)]
struct GraphqlRoutePlan {
    field_index: usize,
    resolver_id: i64,
    ambiguous: bool,
}

type RoutePlanner = unsafe extern "C" fn(
    *const c_void,
    *const c_void,
    *const c_void,
    usize,
) -> GraphqlRoutePlan;
type RouteEvidenceBuild = unsafe extern "C" fn(*const c_void) -> *mut c_void;
type RouteEvidenceFree = unsafe extern "C" fn(*mut c_void);
type RootMatch = unsafe extern "C" fn(
    *const c_void,
    *const c_char,
    c_int,
    *const c_char,
) -> c_int;
type SchemaVisitor = unsafe extern "C" fn(
    *mut c_void,
    *const c_char,
    c_int,
    *const *const c_char,
    bool,
);
type PackageVisitor = unsafe extern "C" fn(*mut c_void, *const c_char);
type MethodVisitor = unsafe extern "C" fn(
    *mut c_void,
    *const c_char,
    bool,
    *const c_char,
    *const c_char,
    i64,
);

#[repr(C)]
struct GraphqlRouteField {
    owner: *const c_char,
    name: *const c_char,
    file_path: *const c_char,
    resolver_pkg: *const c_char,
    configured: bool,
}

#[derive(Default)]
struct RouteEvidence {
    schemas: SchemaIndex,
    resolvers: ResolverIndex,
}

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

type SignatureSink = unsafe extern "C" fn(*mut c_void, *const u8, usize) -> bool;
type DocumentEncoder = unsafe extern "C" fn(
    *const c_char,
    bool,
    usize,
    *mut bool,
    SignatureSink,
    *mut c_void,
) -> bool;

/// ABI mirror of `cbm_graphql_handler_t` in `pass_cross_repo.h`.
#[repr(C)]
struct GraphqlHandler {
    handler_id: i64,
    ambiguous: bool,
    failed: bool,
}

unsafe extern "C" {
    fn cbm_graphql_set_document_encoder(encode: DocumentEncoder);

    fn cbm_gqlgen_set_rust_backend(
        matcher: extern "C" fn(glob: *const c_char, path: *const c_char) -> bool,
        planner: RoutePlanner,
        writer: RouteWriter,
        build: RouteEvidenceBuild,
        release: RouteEvidenceFree,
        match_root: RootMatch,
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
        find: extern "C" fn(
            *mut sqlite3,
            *const c_char,
            *mut c_char,
            usize,
            *mut c_char,
            usize,
        ) -> GraphqlHandler,
    );

    fn cbm_gqlgen_read_schema_evidence(
        ctx: *const c_void,
        schema: SchemaVisitor,
        context: *mut c_void,
    );
    fn cbm_gqlgen_read_resolver_evidence(
        ctx: *const c_void,
        package: PackageVisitor,
        method: MethodVisitor,
        context: *mut c_void,
    );
    fn cbm_gqlgen_route_field_at(
        ctx: *const c_void,
        first: *const c_void,
        index: usize,
    ) -> GraphqlRouteField;

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
}

unsafe extern "C" fn encode_document(
    source: *const c_char,
    require_keyword: bool,
    capacity: usize,
    shorthand: *mut bool,
    sink: SignatureSink,
    context: *mut c_void,
) -> bool {
    #[cfg(feature = "native-graphql-fixtures")]
    fixture_witness::mark(fixture_witness::DOCUMENT);

    // SAFETY: The C entry points check the input before lending it to Rust.
    let source = unsafe { CStr::from_ptr(source) }.to_bytes();
    let mut short = false;
    let signature = crate::graphql_document::signature(
        source,
        require_keyword,
        capacity,
        &mut short,
    );
    // SAFETY: The optional flag is writable and the sink borrows the bytes
    // and its context synchronously, without retaining either pointer.
    unsafe {
        if let Some(shorthand) = shorthand.as_mut() {
            *shorthand = short;
        }
        signature.is_some_and(|bytes| sink(context, bytes.as_ptr(), bytes.len()))
    }
}

extern "C" fn gqlgen_glob_match(
    glob: *const c_char,
    path: *const c_char,
) -> bool {
    #[cfg(feature = "native-graphql-fixtures")]
    fixture_witness::mark(fixture_witness::GLOB);
    // SAFETY: C passes NUL-terminated strings valid for the whole call.
    let (glob, path) = unsafe { (CStr::from_ptr(glob), CStr::from_ptr(path)) };
    crate::gqlgen::glob_matches(glob.to_bytes(), path.to_bytes())
}

unsafe extern "C" fn plan_graphql_route(
    index: *const c_void,
    ctx: *const c_void,
    first: *const c_void,
    count: usize,
) -> GraphqlRoutePlan {
    #[cfg(feature = "native-graphql-fixtures")]
    fixture_witness::mark(fixture_witness::ROUTE_PLAN);
    // SAFETY: C lends the live Rust index and the candidate array for this
    // call. The iterator only borrows strings from that array and its context.
    let index = unsafe { &*index.cast::<RouteEvidence>() };
    let candidates = (0..count).map(|offset| unsafe {
        let field = cbm_gqlgen_route_field_at(ctx, first, offset);
        SchemaField {
            owner: CStr::from_ptr(field.owner).to_bytes(),
            name: CStr::from_ptr(field.name).to_bytes(),
            file: CStr::from_ptr(field.file_path).to_bytes(),
            scope: if field.configured {
                ResolverScope::Configured(optional_bytes(field.resolver_pkg))
            } else {
                ResolverScope::Heuristic
            },
        }
    });
    let plan = index.resolvers.plan(count, candidates);
    GraphqlRoutePlan {
        field_index: plan.field_index,
        resolver_id: plan.resolver.unwrap_or(0),
        ambiguous: plan.ambiguous,
    }
}

// Non-null pointers must reference NUL-terminated bytes for the borrow.
unsafe fn optional_bytes<'a>(value: *const c_char) -> Option<&'a [u8]> {
    (!value.is_null()).then(|| unsafe { CStr::from_ptr(value).to_bytes() })
}

unsafe extern "C" fn collect_schema(
    context: *mut c_void,
    file: *const c_char,
    group: c_int,
    roots: *const *const c_char,
    defined: bool,
) {
    // SAFETY: The schema visitor receives a live file string and a three-entry
    // array of optional root names. The index owns copies before returning.
    unsafe {
        let schemas = &mut *context.cast::<SchemaIndex>();
        schemas.add(
            CStr::from_ptr(file).to_bytes(),
            usize::try_from(group).ok(),
            (*roots.cast::<[*const c_char; ROOT_COUNT]>())
                .map(|root| optional_bytes(root)),
            defined,
        );
    }
}

unsafe extern "C" fn collect_package(
    context: *mut c_void,
    directory: *const c_char,
) {
    // SAFETY: C lends the package string and calls each visitor synchronously.
    unsafe {
        let index = &mut *context.cast::<ResolverIndex>();
        index.add_package(CStr::from_ptr(directory).to_bytes());
    }
}

unsafe extern "C" fn collect_method(
    context: *mut c_void,
    file: *const c_char,
    is_go: bool,
    name: *const c_char,
    qn: *const c_char,
    id: i64,
) {
    // SAFETY: The file and any non-null name or QN live through this call.
    unsafe {
        let index = &mut *context.cast::<ResolverIndex>();
        index.add_method(
            CStr::from_ptr(file).to_bytes(),
            is_go,
            optional_bytes(name),
            optional_bytes(qn),
            id,
        );
    }
}

unsafe extern "C" fn build_route_evidence(ctx: *const c_void) -> *mut c_void {
    let mut evidence = RouteEvidence::default();
    // SAFETY: C lends a live route context for this call and retains no
    // pointer to the index passed to the synchronous visitors.
    unsafe {
        cbm_gqlgen_read_schema_evidence(
            ctx,
            collect_schema,
            ptr::from_mut(&mut evidence.schemas).cast(),
        );
        cbm_gqlgen_read_resolver_evidence(
            ctx,
            collect_package,
            collect_method,
            ptr::from_mut(&mut evidence.resolvers).cast(),
        );
    }
    Box::into_raw(Box::new(evidence)).cast()
}

unsafe extern "C" fn free_route_evidence(index: *mut c_void) {
    // SAFETY: C returns the unique handle from build_route_evidence once.
    drop(unsafe { Box::from_raw(index.cast::<RouteEvidence>()) });
}

unsafe extern "C" fn match_schema_root(
    index: *const c_void,
    file: *const c_char,
    group: c_int,
    name: *const c_char,
) -> c_int {
    #[cfg(feature = "native-graphql-fixtures")]
    fixture_witness::mark(fixture_witness::ROOT_MATCH);
    // SAFETY: C lends the index and the type's non-null strings for this call.
    let (index, file, name) = unsafe {
        (
            &*index.cast::<RouteEvidence>(),
            CStr::from_ptr(file),
            CStr::from_ptr(name),
        )
    };
    index
        .schemas
        .root_operation(
            file.to_bytes(),
            usize::try_from(group).ok(),
            name.to_bytes(),
        )
        .map_or(-1, |operation| operation as c_int)
}

unsafe extern "C" fn write_graphql_route(
    gb: *mut c_void,
    row: *const GraphqlRouteRow,
) -> bool {
    #[cfg(feature = "native-graphql-fixtures")]
    fixture_witness::mark(fixture_witness::ROUTE_WRITE);
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

extern "C" fn graphql_delete_stale_link(
    db: *mut sqlite3,
    caller: i64,
    route: i64,
) -> c_int {
    #[cfg(feature = "native-graphql-fixtures")]
    fixture_witness::mark(fixture_witness::LINK_DELETE);
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
    #[cfg(feature = "native-graphql-fixtures")]
    fixture_witness::mark(fixture_witness::LINK_UPSERT);
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

extern "C" fn graphql_find_handler(
    db: *mut sqlite3,
    route: *const c_char,
    name: *mut c_char,
    name_size: usize,
    file: *mut c_char,
    file_size: usize,
) -> GraphqlHandler {
    #[cfg(feature = "native-graphql-fixtures")]
    fixture_witness::mark(fixture_witness::LINK_FIND);
    let mut matched = GraphqlHandler {
        handler_id: 0,
        ambiguous: false,
        failed: true,
    };
    if db.is_null() {
        return matched;
    }
    // SAFETY: C lends a live connection, a NUL-terminated route and separate
    // writable buffers for this call. Rust does not close the C connection.
    let result = unsafe {
        Connection::from_handle(db).and_then(|db| {
            find_handler(
                &db,
                CStr::from_ptr(route),
                std::slice::from_raw_parts_mut(name.cast(), name_size),
                std::slice::from_raw_parts_mut(file.cast(), file_size),
            )
        })
    };
    if let Ok((handler_id, ambiguous)) = result {
        matched = GraphqlHandler {
            handler_id,
            ambiguous,
            failed: false,
        };
    }
    matched
}

/// Register Rust GraphQL callbacks with the C graph pipeline.
///
/// # Safety
/// Call once during process startup, before any C indexing or linking work.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mori_register_graphql_callbacks() {
    // SAFETY: Callers register at startup before starting any graph work.
    unsafe {
        cbm_graphql_set_document_encoder(encode_document);
        cbm_gqlgen_set_rust_backend(
            gqlgen_glob_match,
            plan_graphql_route,
            write_graphql_route,
            build_route_evidence,
            free_route_evidence,
            match_schema_root,
        );
        cbm_cross_repo_set_graphql_backend(
            plan_link,
            graphql_delete_stale_link,
            graphql_upsert_link,
            graphql_find_handler,
        );
    }
}
