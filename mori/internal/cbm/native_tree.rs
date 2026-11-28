//! Node access to Mori's pinned Tree-sitter runtime.

use std::ffi::{CStr, c_char, c_void};
use std::marker::PhantomData;
use std::ops::Range;

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
    fn cbm_ffi_ts_node_is_named(node: *const RawNode) -> bool;
    fn cbm_ffi_ts_node_named_child_count(node: *const RawNode) -> u32;
    fn cbm_ffi_ts_node_parent(node: *const RawNode) -> RawNode;
    fn cbm_ffi_ts_node_eq(left: *const RawNode, right: *const RawNode) -> bool;
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
    fn cbm_ffi_ts_tree_cursor_current_field_name(
        cursor: *const RawCursor,
    ) -> *const c_char;
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

    fn field_name(&self) -> Option<&'static CStr> {
        // SAFETY: The cursor points into its live grammar tree.
        let name = unsafe { cbm_ffi_ts_tree_cursor_current_field_name(&self.0) };
        (!name.is_null()).then(|| unsafe { CStr::from_ptr(name) })
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

    pub(crate) unsafe fn named_children(self) -> impl Iterator<Item = Self> {
        // SAFETY: The count and yielded children belong to the same live tree.
        // Avoid allocating a cursor stack for leaf nodes.
        unsafe {
            (cbm_ffi_ts_node_named_child_count(&self) != 0)
                .then(|| self.children())
                .into_iter()
                .flatten()
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
    /// The node must borrow a live tree.
    pub(crate) unsafe fn text(self, source: &[u8]) -> Option<&[u8]> {
        // SAFETY: The caller provides the live tree.
        source.get(unsafe { self.range() })
    }
}

/// A node that cannot outlive the borrowed tree handle supplied by the
/// parser bridge. Child nodes and child iterators carry the same lifetime.
#[derive(Clone, Copy)]
pub(crate) struct BorrowedNode<'tree> {
    raw: RawNode,
    tree: PhantomData<&'tree RawNode>,
}

impl<'tree> BorrowedNode<'tree> {
    /// # Safety
    ///
    /// The native tree behind `raw` must stay live during the borrow.
    pub(crate) unsafe fn from_root(raw: &'tree RawNode) -> Self {
        Self {
            raw: *raw,
            tree: PhantomData,
        }
    }

    fn with_raw(self, raw: RawNode) -> Self {
        Self { raw, ..self }
    }

    pub(crate) fn kind(self) -> &'static CStr {
        // SAFETY: The borrow guarantees the tree remains live.
        unsafe { self.raw.kind() }
    }

    pub(crate) fn range(self) -> Range<usize> {
        // SAFETY: The borrow guarantees the tree remains live.
        unsafe { self.raw.range() }
    }

    pub(crate) fn text(self, source: &[u8]) -> Option<&[u8]> {
        // SAFETY: The borrow guarantees the tree remains live.
        unsafe { self.raw.text(source) }
    }

    pub(crate) fn field(self, name: &CStr) -> Option<Self> {
        // SAFETY: The child field belongs to the same borrowed tree.
        unsafe { self.raw.field(name) }.map(|child| self.with_raw(child))
    }

    pub(crate) fn children(self) -> impl Iterator<Item = Self> {
        // SAFETY: The iterator owns a copy of this borrow.
        unsafe { self.raw.children() }.map(move |child| self.with_raw(child))
    }

    pub(crate) fn parent(self) -> Option<Self> {
        // SAFETY: The parent belongs to the same live tree.
        unsafe { self.raw.parent() }.map(|parent| self.with_raw(parent))
    }

    pub(crate) fn same_as(self, other: Self) -> bool {
        // SAFETY: Both arguments carry live tree borrows.
        unsafe { cbm_ffi_ts_node_eq(self.as_raw(), other.as_raw()) }
    }

    pub(crate) fn child_fields(
        self,
    ) -> impl Iterator<Item = (Self, Option<&'static CStr>)> {
        // SAFETY: The iterator retains this borrow as long as its cursor lives.
        let mut cursor = unsafe { TreeCursor::new(self.raw) };
        let mut has_child = cursor.first_child();
        std::iter::from_fn(move || {
            if !has_child {
                return None;
            }
            let field = (self.with_raw(cursor.node()), cursor.field_name());
            has_child = cursor.next_sibling();
            Some(field)
        })
    }

    pub(crate) fn as_raw(&self) -> &RawNode {
        &self.raw
    }

    pub(crate) fn is_named(self) -> bool {
        // SAFETY: This node belongs to the live tree borrow.
        unsafe { cbm_ffi_ts_node_is_named(self.as_raw()) }
    }

    pub(crate) fn unnamed_children(self) -> impl Iterator<Item = Self> {
        self.children().filter(|child| !child.is_named())
    }

    pub(crate) fn named_children(self) -> impl Iterator<Item = Self> {
        // SAFETY: The iterator owns a copy of this borrow.
        unsafe { self.raw.named_children() }
            .map(move |child| self.with_raw(child))
    }
}
