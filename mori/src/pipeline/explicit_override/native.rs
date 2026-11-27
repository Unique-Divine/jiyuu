//! Read the hierarchy and apply its edge delta through the C graph buffer.

use super::{Hierarchy, Method};
use std::{
    collections::HashSet,
    ffi::{CStr, c_char, c_int, c_void},
};

type ReadBase = unsafe extern "C" fn(*mut c_void, i64, i64);
type ReadMethod = unsafe extern "C" fn(*mut c_void, i64, i64, *const c_char);

unsafe extern "C" {
    fn cbm_override_read_bases(
        graph: *const c_void,
        data: *mut c_void,
        read: ReadBase,
    );
    fn cbm_override_read_methods(
        graph: *const c_void,
        class: i64,
        data: *mut c_void,
        read: ReadMethod,
    ) -> bool;
    fn cbm_gbuf_insert_edge(
        graph: *mut c_void,
        source_id: i64,
        target_id: i64,
        kind: *const c_char,
        properties: *const c_char,
    ) -> i64;
}

unsafe extern "C" fn read_base(data: *mut c_void, source: i64, target: i64) {
    // SAFETY: the C reader lends exclusive access to the snapshot for this call.
    let hierarchy = unsafe { &mut *data.cast::<Hierarchy>() };
    hierarchy.bases.entry(source).or_default().push(target);
}

unsafe extern "C" fn read_method(
    data: *mut c_void,
    class: i64,
    id: i64,
    name: *const c_char,
) {
    // SAFETY: the C reader lends the snapshot and a non-null C string.
    let hierarchy = unsafe { &mut *data.cast::<Hierarchy>() };
    let name = unsafe { CStr::from_ptr(name) };
    hierarchy.methods.entry(class).or_default().push(Method {
        id,
        name: name.to_bytes().to_vec(),
    });
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_explicit_override(
    graph: *mut c_void,
    max_levels: c_int,
) -> c_int {
    let mut hierarchy = Hierarchy::default();
    let data = (&raw mut hierarchy).cast();
    // SAFETY: the bridge owns the graph for the duration of this call.
    // The readers are synchronous and do not retain the Rust context.
    unsafe { cbm_override_read_bases(graph, data, read_base) };
    let participants: HashSet<_> = hierarchy
        .bases
        .iter()
        .flat_map(|(&class, bases)| {
            std::iter::once(class).chain(bases.iter().copied())
        })
        .collect();
    let eligible: HashSet<_> = participants
        .into_iter()
        .filter(|&class| {
            // SAFETY: the callback receives only method names valid for the call.
            unsafe { cbm_override_read_methods(graph, class, data, read_method) }
        })
        .collect();
    let edges = hierarchy.edges(
        |class| eligible.contains(&class),
        max_levels.max(0) as usize,
    );
    for edge in &edges {
        // SAFETY: source and target are IDs in this graph. The buffer copies
        // the constant C strings and deduplicates edges on repeated passes.
        unsafe {
            cbm_gbuf_insert_edge(
                graph,
                edge.source,
                edge.target,
                c"OVERRIDE".as_ptr(),
                c"{}".as_ptr(),
            );
        }
    }
    edges.len() as c_int
}
