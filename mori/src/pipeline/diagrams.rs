//! The graph side of diagrams (internal/cbm/diagram.c).
//!
//! A Diagram node is a definition of the file that holds it. A diagram block
//! of a document also hangs under the Section it is written in, and each
//! DiagramElement under its Diagram (DEFINES, from the definition's
//! parent_class). The arrows of a diagram are DIAGRAM_ARROW edges between the
//! file's DiagramElement nodes, with the arrow's text, its style and its
//! line. Both paths of the definitions pass (pass_definitions.c,
//! pass_parallel.c) call in here, so they cannot drift apart.

use super::native_graph_buffer::cbm_gbuf_find_by_qn;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::slice;

// Byte limits of the C writer: each escaped text, then the whole object.
const TEXT_MAX: usize = 512;
const PROPS_MAX: usize = 1536;

// Keep this layout aligned with CBMDiagramArrow in internal/cbm/cbm.h.
#[repr(C)]
pub(super) struct DiagramArrow {
    from_qn: *const c_char,
    to_qn: *const c_char,
    label: *const c_char,
    style: *const c_char,
    line: u32,
}

unsafe extern "C" {
    fn cbm_gbuf_insert_edge(
        graph: *mut c_void,
        source: i64,
        target: i64,
        kind: *const c_char,
        props: *const c_char,
    ) -> i64;
    fn cbm_json_escape(
        out: *mut c_char,
        size: c_int,
        text: *const c_char,
    ) -> c_int;
}

fn is_diagram_label(label: &CStr) -> bool {
    matches!(label.to_bytes(), b"Diagram" | b"DiagramElement")
}

fn escape(text: *const c_char) -> Vec<u8> {
    let text = if text.is_null() { c"".as_ptr() } else { text };
    let mut buf = [0u8; TEXT_MAX];
    // SAFETY: buf holds TEXT_MAX bytes and text is NUL-terminated.
    let len = unsafe {
        cbm_json_escape(buf.as_mut_ptr().cast(), TEXT_MAX as c_int, text)
    };
    buf[..len as usize].to_vec()
}

/// The properties of one DIAGRAM_ARROW edge, NUL-terminated, or `None` when
/// they would not fit the C writer's buffer.
fn arrow_props(arrow: &DiagramArrow) -> Option<Vec<u8>> {
    // SAFETY: a non-null label is a NUL-terminated string of the result.
    let labeled =
        !arrow.label.is_null() && unsafe { *arrow.label } != 0 as c_char;
    let mut props = Vec::with_capacity(64);
    if labeled {
        props.extend_from_slice(br#"{"label":""#);
        props.extend_from_slice(&escape(arrow.label));
        props.extend_from_slice(br#"","style":""#);
    } else {
        props.extend_from_slice(br#"{"style":""#);
    }
    props.extend_from_slice(&escape(arrow.style));
    props.extend_from_slice(br#"","line":"#);
    props.extend_from_slice(arrow.line.to_string().as_bytes());
    props.push(b'}');
    if props.len() >= PROPS_MAX {
        return None;
    }
    props.push(0);
    Some(props)
}

/// Writes the DEFINES edge from a Diagram's or a DiagramElement's container
/// (the Section a diagram block sits in, the Diagram of an element) to the
/// definition's node; nothing for other definitions.
///
/// # Safety
///
/// `graph` is a live graph buffer or null; `label` and `parent_class` are
/// NUL-terminated strings or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mori_pipeline_diagram_parent_edge(
    graph: *mut c_void,
    label: *const c_char,
    parent_class: *const c_char,
    node_id: i64,
) {
    if graph.is_null() || parent_class.is_null() || node_id <= 0 {
        return;
    }
    // SAFETY: label is NUL-terminated when non-null.
    if label.is_null() || !is_diagram_label(unsafe { CStr::from_ptr(label) }) {
        return;
    }
    // SAFETY: the buffer owns the returned node for the duration of this call.
    let parent = unsafe { cbm_gbuf_find_by_qn(graph, parent_class) };
    if parent.is_null() {
        return;
    }
    let parent_id = unsafe { (*parent).id };
    if parent_id != node_id {
        unsafe {
            cbm_gbuf_insert_edge(
                graph,
                parent_id,
                node_id,
                c"DEFINES".as_ptr(),
                c"{}".as_ptr(),
            );
        }
    }
}

/// Writes the DIAGRAM_ARROW edges of one file's diagrams.
///
/// # Safety
///
/// `graph` is a live graph buffer or null; `arrows` points at `count`
/// arrows whose strings are NUL-terminated or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mori_pipeline_diagram_arrows(
    graph: *mut c_void,
    arrows: *const DiagramArrow,
    count: c_int,
) {
    if graph.is_null() || arrows.is_null() || count <= 0 {
        return;
    }
    // SAFETY: the caller lends count arrows.
    let arrows = unsafe { slice::from_raw_parts(arrows, count as usize) };
    for arrow in arrows {
        if arrow.from_qn.is_null() || arrow.to_qn.is_null() {
            continue;
        }
        let (from, to) = unsafe {
            (
                cbm_gbuf_find_by_qn(graph, arrow.from_qn),
                cbm_gbuf_find_by_qn(graph, arrow.to_qn),
            )
        };
        if from.is_null() || to.is_null() {
            continue;
        }
        let Some(props) = arrow_props(arrow) else {
            continue;
        };
        // SAFETY: both nodes belong to graph; props is NUL-terminated.
        unsafe {
            cbm_gbuf_insert_edge(
                graph,
                (*from).id,
                (*to).id,
                c"DIAGRAM_ARROW".as_ptr(),
                props.as_ptr().cast(),
            );
        }
    }
}
