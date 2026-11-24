//! Markdown heading text from the same Tree-sitter tree as native extraction.

use std::ffi::c_void;
use std::mem::MaybeUninit;

use crate::native_tree::RawNode;

#[repr(C)]
struct RawCursor {
    tree: *const c_void,
    id: *const c_void,
    context: [u32; 3],
}

unsafe extern "C" {
    fn ts_tree_cursor_new(node: RawNode) -> RawCursor;
    fn ts_tree_cursor_delete(cursor: *mut RawCursor);
    fn ts_tree_cursor_current_node(cursor: *const RawCursor) -> RawNode;
    fn ts_tree_cursor_goto_first_child(cursor: *mut RawCursor) -> bool;
    fn ts_tree_cursor_goto_next_sibling(cursor: *mut RawCursor) -> bool;
    fn ts_tree_cursor_goto_parent(cursor: *mut RawCursor) -> bool;

    fn cbm_markdown_set_heading_extractor(
        extract: unsafe extern "C" fn(
            RawNode,
            *const u8,
            usize,
            *mut u8,
            usize,
        ) -> usize,
    );
}

struct Cursor(RawCursor);

impl Drop for Cursor {
    fn drop(&mut self) {
        // SAFETY: This guard uniquely owns an initialized cursor.
        unsafe { ts_tree_cursor_delete(&mut self.0) };
    }
}

fn append(
    written: &mut usize,
    output: &mut [MaybeUninit<u8>],
    bytes: &[u8],
) -> Option<()> {
    let end = written.checked_add(bytes.len())?;
    let destination = output.get_mut(*written..end)?;
    // SAFETY: The bounded output region is disjoint from the initialized
    // source bytes. Copy initializes exactly the written prefix.
    unsafe {
        std::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            destination.as_mut_ptr().cast::<u8>(),
            bytes.len(),
        );
    }
    *written = end;
    Some(())
}

unsafe fn setext(
    node: RawNode,
    source: &[u8],
    output: &mut [MaybeUninit<u8>],
) -> Option<usize> {
    // SAFETY: The caller lends the live tree through this synchronous call.
    unsafe {
        let paragraph = node.field(c"heading_content")?;
        let content = paragraph.child_of_kind(c"inline").unwrap_or(paragraph);
        let mut cursor = Cursor(ts_tree_cursor_new(content));
        let span = content.range();
        let mut previous = span.start;
        let mut written = 0;
        loop {
            let current = ts_tree_cursor_current_node(&cursor.0);
            if current.is(c"block_continuation") {
                let marker = current.range();
                append(
                    &mut written,
                    output,
                    source.get(previous..marker.start)?,
                )?;
                previous = marker.end;
            } else if ts_tree_cursor_goto_first_child(&mut cursor.0) {
                continue;
            }
            while !ts_tree_cursor_goto_next_sibling(&mut cursor.0) {
                if !ts_tree_cursor_goto_parent(&mut cursor.0) {
                    append(
                        &mut written,
                        output,
                        source.get(previous..span.end)?,
                    )?;
                    return Some(written);
                }
            }
        }
    }
}

unsafe fn atx(node: RawNode, source: &[u8]) -> Option<&[u8]> {
    // SAFETY: The caller lends the live tree, and the return borrows source only.
    unsafe {
        if let Some(child) = node
            .children()
            .find(|child| child.is(c"heading_content") || child.is(c"inline"))
        {
            return source.get(child.range());
        }
        let mut full = source.get(node.range())?;
        while let Some(rest) = full.strip_prefix(b"#") {
            full = rest;
        }
        while let Some(rest) = full.strip_prefix(b" ") {
            full = rest;
        }
        Some(full)
    }
}

unsafe extern "C" fn extract_heading(
    node: RawNode,
    source: *const u8,
    source_len: usize,
    output: *mut u8,
    output_len: usize,
) -> usize {
    // SAFETY: C lends the live tree, immutable source, and disjoint writable
    // arena allocation. The buffers outlive this synchronous callback.
    unsafe {
        let source = std::slice::from_raw_parts(source, source_len);
        let output = std::slice::from_raw_parts_mut(
            output.cast::<MaybeUninit<u8>>(),
            output_len,
        );
        if node.is(c"atx_heading") {
            let mut written = 0;
            if let Some(bytes) = atx(node, source) {
                let _ = append(&mut written, output, bytes);
            }
            written
        } else {
            setext(node, source, output).unwrap_or_default()
        }
    }
}

/// Install the native callback used by Markdown heading extraction.
///
/// # Safety
///
/// Call during process startup, before other threads can extract files or
/// install callbacks.
pub unsafe fn register() {
    // SAFETY: Registration runs before extractor threads start.
    unsafe { cbm_markdown_set_heading_extractor(extract_heading) };
}
