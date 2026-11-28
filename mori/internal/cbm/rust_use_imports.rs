//! Rust use trees from Mori's pinned Tree-sitter parser.

use std::ffi::c_void;

use crate::native_tree::RawNode;

type Emit =
    unsafe extern "C" fn(*mut c_void, *const u8, usize, *const u8, usize);

enum Work {
    Visit(RawNode),
    RestorePrefix(usize),
}

fn append_path(path: &mut Vec<u8>, text: &[u8]) {
    // C compacts token text using isspace and then joins C strings.
    let mut compact = text
        .iter()
        .copied()
        .take_while(|&byte| byte != 0)
        .filter(|&byte| !matches!(byte, b' ' | b'\t'..=b'\r'));
    if let Some(first) = compact.next() {
        if !path.is_empty() {
            path.extend_from_slice(b"::");
        }
        path.push(first);
        path.extend(compact);
    }
}

unsafe fn text(node: Option<RawNode>, source: &[u8]) -> &[u8] {
    // SAFETY: Every node and byte span belongs to the caller's live tree.
    unsafe { node.and_then(|node| node.text(source)).unwrap_or_default() }
}

unsafe fn walk(
    argument: RawNode,
    source: &[u8],
    context: *mut c_void,
    emit: Emit,
) -> bool {
    let mut pending = vec![Work::Visit(argument)];
    let mut path = Vec::new();
    let mut alias = Vec::new();
    let mut emitted = false;
    // SAFETY: All nodes and output callbacks borrow the caller's tree/context
    // only for the duration of this synchronous walk.
    unsafe {
        while let Some(work) = pending.pop() {
            let node = match work {
                Work::Visit(node) => node,
                Work::RestorePrefix(length) => {
                    path.truncate(length);
                    continue;
                }
            };
            if node.is(c"line_comment") || node.is(c"block_comment") {
                continue;
            }
            if node.is(c"use_list") {
                let first_child = pending.len();
                pending.extend(node.named_children().map(Work::Visit));
                pending[first_child..].reverse();
                continue;
            }

            let prefix_length = path.len();
            if node.is(c"scoped_use_list") {
                if let Some(list) = node.field(c"list") {
                    append_path(&mut path, text(node.field(c"path"), source));
                    pending.push(Work::RestorePrefix(prefix_length));
                    pending.push(Work::Visit(list));
                }
                continue;
            }

            alias.clear();
            if node.is(c"use_as_clause") {
                append_path(&mut path, text(node.field(c"path"), source));
                append_path(&mut alias, text(node.field(c"alias"), source));
            } else if node.is(c"use_wildcard") {
                append_path(
                    &mut path,
                    text(node.named_children().next(), source),
                );
                append_path(&mut path, b"*");
            } else {
                append_path(&mut path, text(Some(node), source));
            }
            if !path.is_empty() {
                emit(
                    context,
                    path.as_ptr(),
                    path.len(),
                    alias.as_ptr(),
                    alias.len(),
                );
                emitted = true;
            }
            path.truncate(prefix_length);
        }
    }
    emitted
}

/// Read top-level Rust use declarations and lend each row to the C sink.
///
/// # Safety
/// The caller lends a live tree, its initialized source, and a sink that
/// copies the output bytes before returning.
#[unsafe(no_mangle)]
unsafe extern "C" fn mori_rust_extract_use_imports(
    root: RawNode,
    source: *const u8,
    source_len: usize,
    context: *mut c_void,
    emit: Emit,
) -> bool {
    // SAFETY: C lends the live source, tree, and sink. The sink copies bytes
    // during the callback, before the scratch path and alias are reused.
    unsafe {
        let source = std::slice::from_raw_parts(source, source_len);
        let mut emitted = false;
        for declaration in
            root.children().filter(|node| node.is(c"use_declaration"))
        {
            if let Some(argument) = declaration.field(c"argument") {
                emitted |= walk(argument, source, context, emit);
            }
        }
        emitted
    }
}

#[cfg(test)]
mod tests {
    use super::append_path;

    #[test]
    fn paths_compact_separators_and_respect_native_nul_termination() {
        let mut path = Vec::new();
        append_path(&mut path, b" :: crate ");
        append_path(&mut path, b" module \t :: \x0b nested\0ignored");
        append_path(&mut path, b"\r \n");
        append_path(&mut path, b"*");
        assert_eq!(&path, b"::crate::module::nested::*");
    }
}
