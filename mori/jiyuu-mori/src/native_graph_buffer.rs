//! Shared read ABI for the native graph buffer.

use std::ffi::{c_char, c_int, c_void};

// Keep this layout aligned with cbm_gbuf_node_t in graph_buffer.h.
#[repr(C)]
pub(super) struct Node {
    pub(super) id: i64,
    pub(super) label: *const c_char,
    pub(super) name: *const c_char,
    pub(super) qualified_name: *const c_char,
    pub(super) file_path: *const c_char,
    pub(super) start_line: c_int,
    pub(super) end_line: c_int,
    pub(super) properties_json: *const c_char,
}

unsafe extern "C" {
    pub(super) fn cbm_gbuf_find_by_id(
        graph: *const c_void,
        id: i64,
    ) -> *const Node;

    pub(super) fn cbm_gbuf_find_by_label(
        graph: *const c_void,
        label: *const c_char,
        nodes: *mut *const *const Node,
        count: *mut c_int,
    ) -> c_int;

    pub(super) fn cbm_gbuf_find_by_name(
        graph: *const c_void,
        name: *const c_char,
        nodes: *mut *const *const Node,
        count: *mut c_int,
    ) -> c_int;
}
