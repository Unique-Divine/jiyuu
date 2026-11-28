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

#[repr(C)]
struct ByteRange {
    start: u32,
    end: u32,
}

// The C bridge marks only the initialized fields before Tree-sitter reads them.
unsafe extern "C" {
    fn cbm_ffi_ts_node_type(node: *const RawNode) -> *const c_char;
    fn cbm_ffi_ts_node_byte_range(node: *const RawNode) -> ByteRange;
    fn cbm_ffi_ts_node_child_count(node: *const RawNode) -> u32;
    fn cbm_ffi_ts_node_child(node: *const RawNode, index: u32) -> RawNode;
    fn cbm_ffi_ts_node_parent(node: *const RawNode) -> RawNode;
    #[cfg(feature = "c-backend")]
    fn cbm_ffi_ts_node_is_named(node: *const RawNode) -> bool;
    fn cbm_ffi_ts_node_child_by_field_name(
        node: *const RawNode,
        name: *const c_char,
        len: u32,
    ) -> RawNode;
    fn cbm_ffi_ts_tree_cursor_new(node: *const RawNode) -> RawCursor;
    fn cbm_ffi_ts_tree_cursor_reset_to(
        cursor: *mut RawCursor,
        source: *const RawCursor,
    );
    fn cbm_ffi_ts_tree_cursor_delete(cursor: *mut RawCursor);
    fn cbm_ffi_ts_tree_cursor_current_node(cursor: *const RawCursor) -> RawNode;
    fn cbm_ffi_ts_tree_cursor_goto_first_child(cursor: *mut RawCursor) -> bool;
    fn cbm_ffi_ts_tree_cursor_goto_next_sibling(cursor: *mut RawCursor) -> bool;
    fn cbm_ffi_ts_tree_cursor_goto_parent(cursor: *mut RawCursor) -> bool;
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
        Self(unsafe { cbm_ffi_ts_tree_cursor_new(&node) })
    }

    /// # Safety
    /// The source tree must remain alive for the destination cursor's lifetime.
    pub(crate) unsafe fn reset_to(&mut self, source: &Self) {
        // SAFETY: Both cursors are live, and the destination is exclusively borrowed.
        unsafe { cbm_ffi_ts_tree_cursor_reset_to(&mut self.0, &source.0) };
    }

    pub(crate) fn node(&self) -> RawNode {
        // SAFETY: A live cursor has a current node in its borrowed tree.
        unsafe { cbm_ffi_ts_tree_cursor_current_node(&self.0) }
    }

    pub(crate) fn first_child(&mut self) -> bool {
        // SAFETY: This method exclusively borrows the live cursor.
        unsafe { cbm_ffi_ts_tree_cursor_goto_first_child(&mut self.0) }
    }

    pub(crate) fn next_sibling(&mut self) -> bool {
        // SAFETY: This method exclusively borrows the live cursor.
        unsafe { cbm_ffi_ts_tree_cursor_goto_next_sibling(&mut self.0) }
    }

    pub(crate) fn parent(&mut self) -> bool {
        // SAFETY: This method exclusively borrows the live cursor.
        unsafe { cbm_ffi_ts_tree_cursor_goto_parent(&mut self.0) }
    }
}

impl Drop for TreeCursor {
    fn drop(&mut self) {
        // SAFETY: The guard uniquely owns the initialized cursor.
        unsafe { cbm_ffi_ts_tree_cursor_delete(&mut self.0) };
    }
}

impl RawNode {
    pub(crate) const NULL: Self = Self {
        context: [0; 4],
        id: std::ptr::null(),
        tree: std::ptr::null(),
    };

    pub(crate) fn is_null(self) -> bool {
        self.id.is_null()
    }

    /// # Safety
    /// The node and its ancestors must borrow a live tree.
    pub(crate) unsafe fn parent(self) -> Option<Self> {
        // SAFETY: The caller provides the live tree.
        let parent = unsafe { cbm_ffi_ts_node_parent(&self) };
        (!parent.is_null()).then_some(parent)
    }

    /// Visit direct children by index, including anonymous tokens.
    ///
    /// # Safety
    /// Keep the borrowed tree alive through the last use of the iterator and its nodes.
    pub(crate) unsafe fn indexed_children(self) -> impl Iterator<Item = Self> {
        // SAFETY: The caller provides the live tree for the count and each lookup.
        let count = unsafe { cbm_ffi_ts_node_child_count(&self) };
        (0..count)
            .map(move |index| unsafe { cbm_ffi_ts_node_child(&self, index) })
    }

    /// These accessors require a node borrowed from a still-live native tree.
    pub(crate) unsafe fn field(self, name: &CStr) -> Option<Self> {
        // SAFETY: The caller guarantees the live tree, and name is a C string.
        let child = unsafe {
            cbm_ffi_ts_node_child_by_field_name(
                &self,
                name.as_ptr(),
                name.count_bytes() as u32,
            )
        };
        (!child.is_null()).then_some(child)
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
        unsafe {
            self.children()
                .filter(|child| cbm_ffi_ts_node_is_named(child))
        }
    }

    pub(crate) unsafe fn child_of_kind(self, name: &CStr) -> Option<Self> {
        // SAFETY: The caller keeps the borrowed tree live.
        unsafe { self.children().find(|child| child.is(name)) }
    }

    pub(crate) unsafe fn is(self, kind: &CStr) -> bool {
        // SAFETY: The caller guarantees the live tree.
        unsafe { self.kind() == kind }
    }

    pub(crate) unsafe fn kind(self) -> &'static CStr {
        // SAFETY: A live node supplies a static, NUL-terminated grammar string.
        unsafe { CStr::from_ptr(cbm_ffi_ts_node_type(&self)) }
    }

    pub(crate) unsafe fn range(self) -> std::ops::Range<usize> {
        // SAFETY: The caller guarantees the live tree.
        let range = unsafe { cbm_ffi_ts_node_byte_range(&self) };
        range.start as usize..range.end as usize
    }

    /// # Safety
    /// The node must borrow the live tree parsed from `source`.
    #[cfg(feature = "c-backend")]
    pub(crate) unsafe fn text(self, source: &[u8]) -> Option<&[u8]> {
        // SAFETY: The caller provides the live tree.
        source.get(unsafe { self.range() })
    }
}
