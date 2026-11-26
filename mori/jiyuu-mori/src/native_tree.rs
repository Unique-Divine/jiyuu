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

/// Matches `TSTreeCursor` in the pinned Tree-sitter header.
#[repr(C)]
struct RawCursor {
    tree: *const c_void,
    id: *const c_void,
    context: [u32; 3],
}

unsafe extern "C" {
    fn ts_node_is_null(node: RawNode) -> bool;
    fn ts_node_type(node: RawNode) -> *const c_char;
    fn ts_node_start_byte(node: RawNode) -> u32;
    fn ts_node_end_byte(node: RawNode) -> u32;
    #[cfg(feature = "c-backend")]
    fn ts_node_is_named(node: RawNode) -> bool;
    fn ts_node_child_by_field_name(
        node: RawNode,
        name: *const c_char,
        len: u32,
    ) -> RawNode;
    fn ts_tree_cursor_new(node: RawNode) -> RawCursor;
    fn ts_tree_cursor_delete(cursor: *mut RawCursor);
    fn ts_tree_cursor_current_node(cursor: *const RawCursor) -> RawNode;
    fn ts_tree_cursor_goto_first_child(cursor: *mut RawCursor) -> bool;
    fn ts_tree_cursor_goto_next_sibling(cursor: *mut RawCursor) -> bool;
    fn ts_tree_cursor_goto_parent(cursor: *mut RawCursor) -> bool;
}

pub(crate) struct TreeCursor(RawCursor);

impl TreeCursor {
    /// Start a cursor at a node in a borrowed tree.
    ///
    /// # Safety
    ///
    /// The tree must stay live until the cursor is dropped. Nodes returned
    /// from the cursor may be used only while the tree stays live.
    pub(crate) unsafe fn new(node: RawNode) -> Self {
        // SAFETY: The caller provides the live node and tree.
        Self(unsafe { ts_tree_cursor_new(node) })
    }

    pub(crate) fn node(&self) -> RawNode {
        // SAFETY: A live cursor has a current node in its borrowed tree.
        unsafe { ts_tree_cursor_current_node(&self.0) }
    }

    pub(crate) fn first_child(&mut self) -> bool {
        // SAFETY: This method exclusively borrows the live cursor.
        unsafe { ts_tree_cursor_goto_first_child(&mut self.0) }
    }

    pub(crate) fn next_sibling(&mut self) -> bool {
        // SAFETY: This method exclusively borrows the live cursor.
        unsafe { ts_tree_cursor_goto_next_sibling(&mut self.0) }
    }

    pub(crate) fn parent(&mut self) -> bool {
        // SAFETY: This method exclusively borrows the live cursor.
        unsafe { ts_tree_cursor_goto_parent(&mut self.0) }
    }
}

impl Drop for TreeCursor {
    fn drop(&mut self) {
        // SAFETY: The guard uniquely owns the initialized cursor.
        unsafe { ts_tree_cursor_delete(&mut self.0) };
    }
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

    /// Walk visible children in source order, including anonymous tokens.
    ///
    /// # Safety
    ///
    /// Keep the borrowed tree live until the iterator is dropped. Use each
    /// yielded node only while its tree stays live.
    pub(crate) unsafe fn children(self) -> impl Iterator<Item = Self> {
        // SAFETY: The caller keeps the borrowed tree live through iterator drop.
        let mut cursor = unsafe { TreeCursor::new(self) };
        let mut has_child = cursor.first_child();
        std::iter::from_fn(move || {
            if !has_child {
                return None;
            }
            let child = cursor.node();
            has_child = cursor.next_sibling();
            Some(child)
        })
    }

    #[cfg(feature = "c-backend")]
    pub(crate) unsafe fn named_children(self) -> impl Iterator<Item = Self> {
        // SAFETY: Namedness is checked while each yielded node's tree is live.
        unsafe { self.children().filter(|child| ts_node_is_named(*child)) }
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
