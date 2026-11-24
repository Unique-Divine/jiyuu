//! Node access to Mori's pinned Tree-sitter runtime.

use std::ffi::{CStr, c_char, c_void};

/// Matches the pinned Tree-sitter header. A node borrows its source tree.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct RawNode {
    context: [u32; 4],
    id: *const c_void,
    tree: *const c_void,
}

unsafe extern "C" {
    fn ts_node_is_null(node: RawNode) -> bool;
    fn ts_node_type(node: RawNode) -> *const c_char;
    fn ts_node_start_byte(node: RawNode) -> u32;
    fn ts_node_end_byte(node: RawNode) -> u32;
    fn ts_node_child_count(node: RawNode) -> u32;
    fn ts_node_named_child_count(node: RawNode) -> u32;
    fn ts_node_named_child(node: RawNode, index: u32) -> RawNode;
    fn ts_node_child(node: RawNode, index: u32) -> RawNode;
    fn ts_node_child_by_field_name(
        node: RawNode,
        name: *const c_char,
        len: u32,
    ) -> RawNode;
}

impl RawNode {
    /// These accessors require a node borrowed from a still-live native tree.
    pub(crate) unsafe fn field(self, name: &CStr) -> Option<Self> {
        // SAFETY: The caller guarantees the live tree, and name is a C string.
        let child = unsafe {
            ts_node_child_by_field_name(
                self,
                name.as_ptr(),
                name.count_bytes() as u32,
            )
        };
        (!unsafe { ts_node_is_null(child) }).then_some(child)
    }

    pub(crate) unsafe fn children(self) -> impl Iterator<Item = Self> {
        // SAFETY: The caller keeps the borrowed tree live throughout iteration.
        let count = unsafe { ts_node_child_count(self) };
        (0..count).map(move |index| unsafe { ts_node_child(self, index) })
    }

    pub(crate) unsafe fn named_children(self) -> impl Iterator<Item = Self> {
        // SAFETY: The caller keeps the borrowed tree live throughout iteration.
        let count = unsafe { ts_node_named_child_count(self) };
        (0..count).map(move |index| unsafe { ts_node_named_child(self, index) })
    }

    pub(crate) unsafe fn child_of_kind(self, name: &CStr) -> Option<Self> {
        // SAFETY: The caller keeps the borrowed tree live.
        unsafe { self.children().find(|child| child.is(name)) }
    }

    pub(crate) unsafe fn is(self, kind: &CStr) -> bool {
        // SAFETY: Native node kinds are NUL-terminated for a valid live node.
        unsafe { CStr::from_ptr(ts_node_type(self)) == kind }
    }

    pub(crate) unsafe fn range(self) -> std::ops::Range<usize> {
        // SAFETY: The caller guarantees the live tree.
        unsafe {
            ts_node_start_byte(self) as usize..ts_node_end_byte(self) as usize
        }
    }
}
