//! The only module that knows about C. Deleting the C program later means
//! deleting this module and its callers.

use std::ffi::{CStr, OsStr, c_char, c_int, c_void};
use std::os::unix::ffi::OsStrExt;
use std::ptr;

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
        )
    };

    // SAFETY: argc and argv come straight from the C runtime and meet the
    // contract of C main, which cbm_c_main is.
    unsafe { cbm_c_main(argc, argv) }
}
