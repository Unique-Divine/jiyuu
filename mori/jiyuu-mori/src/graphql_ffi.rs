//! GraphQL callbacks used by the linked executable and native fixture runner.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr;

use rusqlite::{
    Connection,
    ffi::{sqlite3, sqlite3_stmt},
};

use crate::graphql_link::{
    Action, delete_stale_link, find_handler, plan_link, upsert_link,
};
use crate::link::RoutePlan;

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

/// ABI mirror of `cbm_graphql_handler_t` in `pass_cross_repo.h`.
#[repr(C)]
struct GraphqlHandler {
    handler_id: i64,
    ambiguous: bool,
    failed: bool,
}

unsafe extern "C" {
    fn cbm_gqlgen_set_rust_backend(
        matcher: extern "C" fn(glob: *const c_char, path: *const c_char) -> bool,
        planner: RoutePlanner,
        writer: RouteWriter,
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

extern "C" fn graphql_find_handler(
    db: *mut sqlite3,
    route: *const c_char,
    name: *mut c_char,
    name_size: usize,
    file: *mut c_char,
    file_size: usize,
) -> GraphqlHandler {
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
        cbm_gqlgen_set_rust_backend(
            gqlgen_glob_match,
            plan_graphql_route,
            write_graphql_route,
        );
        cbm_cross_repo_set_graphql_backend(
            plan_link,
            graphql_delete_stale_link,
            graphql_upsert_link,
            graphql_find_handler,
        );
    }
}
