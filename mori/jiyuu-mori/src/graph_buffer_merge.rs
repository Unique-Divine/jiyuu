//! Ordered worker-buffer merge. The native buffer still owns graph storage.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_int, c_void};

use crate::native_graph_buffer::Node;

// Keep this layout aligned with cbm_gbuf_edge_t in graph_buffer.h.
#[repr(C)]
struct Edge {
    id: i64,
    source_id: i64,
    target_id: i64,
    kind: *const c_char,
    properties_json: *const c_char,
}

type MergeNode =
    unsafe extern "C" fn(*mut c_void, *const c_void, *const Node) -> i64;
type InsertEdge = unsafe extern "C" fn(
    *mut c_void,
    i64,
    i64,
    *const c_char,
    *const c_char,
) -> i64;

/// Choose the same canonical definition as native upsert. Full ties refresh
/// the existing definition with the incoming one.
///
/// The caller has already handled the structural container guard.
///
/// # Safety
/// Both pointers and their non-null string fields stay readable for the call.
#[unsafe(no_mangle)]
unsafe extern "C" fn mori_gbuf_source_wins(
    existing: *const Node,
    existing_rank: c_int,
    source: *const Node,
    source_rank: c_int,
) -> bool {
    unsafe fn text(field: &*const c_char) -> &[u8] {
        if field.is_null() {
            return b"";
        }
        // SAFETY: the caller lends the initialized NUL-terminated field.
        unsafe { CStr::from_ptr(*field).to_bytes() }
    }

    // SAFETY: the caller lends both nodes and their fields for comparison.
    unsafe {
        let existing = &*existing;
        let source = &*source;
        (
            source_rank,
            Reverse(text(&source.file_path)),
            source.start_line,
            text(&source.name),
            text(&source.label),
        ) >= (
            existing_rank,
            Reverse(text(&existing.file_path)),
            existing.start_line,
            text(&existing.name),
            text(&existing.label),
        )
    }
}

/// Merge nodes before edges so both endpoints can use the canonical IDs.
/// The node callback returns zero for a skipped node.
///
/// # Safety
/// The two buffers are distinct. The source arrays and their rows stay live
/// throughout the call. The callbacks borrow both buffers and mutate only
/// the destination. The caller discards staging data on failure.
#[unsafe(no_mangle)]
unsafe extern "C" fn mori_gbuf_merge_rows(
    dst: *mut c_void,
    src: *const c_void,
    nodes: *const *const Node,
    node_count: c_int,
    edges: *const *const Edge,
    edge_count: c_int,
    merge_node: MergeNode,
    insert_edge: InsertEdge,
) -> c_int {
    let mut remap = HashMap::new();
    for index in 0..node_count {
        // SAFETY: the caller lends this row from the source buffer.
        let (node, source_id) = unsafe {
            let node = *nodes.add(index as usize);
            (node, (*node).id)
        };
        // SAFETY: the callback mutates only the separate destination buffer.
        let canonical_id = unsafe { merge_node(dst, src, node) };
        if canonical_id != 0 && canonical_id != source_id {
            if remap.try_reserve(1).is_err() {
                return -1;
            }
            remap.insert(source_id, canonical_id);
        }
    }

    let canonical_id = |id| remap.get(&id).copied().unwrap_or(id);
    for index in 0..edge_count {
        // SAFETY: inserting into the destination cannot invalidate this source row.
        unsafe {
            let edge = &**edges.add(index as usize);
            insert_edge(
                dst,
                canonical_id(edge.source_id),
                canonical_id(edge.target_id),
                edge.kind,
                edge.properties_json,
            );
        }
    }
    0
}
