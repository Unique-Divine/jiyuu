//! GraphQL SDL extraction using the parser and grammar already linked by Mori.

use std::ffi::{CStr, c_char, c_void};

/// Matches the pinned Tree-sitter header. A node borrows its source tree.
#[repr(C)]
#[derive(Clone, Copy)]
struct RawNode {
    context: [u32; 4],
    id: *const c_void,
    tree: *const c_void,
}

#[repr(C)]
#[derive(Default)]
struct Span {
    start: u32,
    end: u32,
    present: bool,
}

#[repr(C)]
struct FieldSpans {
    name: Span,
    field_type: Span,
}

unsafe extern "C" {
    fn ts_node_is_null(node: RawNode) -> bool;
    fn ts_node_type(node: RawNode) -> *const c_char;
    fn ts_node_start_byte(node: RawNode) -> u32;
    fn ts_node_end_byte(node: RawNode) -> u32;
    fn ts_node_child_count(node: RawNode) -> u32;
    fn ts_node_child(node: RawNode, index: u32) -> RawNode;
    fn ts_node_child_by_field_name(
        node: RawNode,
        name: *const c_char,
        len: u32,
    ) -> RawNode;

    fn cbm_graphql_set_field_extractor(
        extract: unsafe extern "C" fn(RawNode) -> FieldSpans,
    );
    fn cbm_graphql_set_parse_masker(mask: unsafe extern "C" fn(*mut u8, usize));
}

unsafe fn child_span(node: RawNode, name: &CStr) -> Span {
    // SAFETY: All nodes returned by Tree-sitter borrow the callback's live tree.
    unsafe {
        let child = ts_node_child_by_field_name(
            node,
            name.as_ptr(),
            name.count_bytes() as u32,
        );
        if !ts_node_is_null(child) {
            return Span {
                start: ts_node_start_byte(child),
                end: ts_node_end_byte(child),
                present: true,
            };
        }
        // The pinned GraphQL grammar has no field annotations.
        for index in 0..ts_node_child_count(node) {
            let child = ts_node_child(node, index);
            if CStr::from_ptr(ts_node_type(child)) == name {
                return Span {
                    start: ts_node_start_byte(child),
                    end: ts_node_end_byte(child),
                    present: true,
                };
            }
        }
    }
    Span::default()
}

unsafe extern "C" fn field_spans(node: RawNode) -> FieldSpans {
    // SAFETY: The C extractor lends a valid field node for this call.
    unsafe {
        FieldSpans {
            name: child_span(node, c"name"),
            field_type: child_span(node, c"type"),
        }
    }
}

#[derive(Clone, Copy)]
enum State {
    Code,
    Comment,
    String,
    Block,
}

fn mask_string_escapes(source: &mut [u8]) {
    let mut state = State::Code;
    let mut index = 0;
    while index < source.len() {
        let byte = source[index];
        match state {
            State::Code if byte == b'#' => state = State::Comment,
            State::Code if source[index..].starts_with(b"\"\"\"") => {
                index += 2;
                state = State::Block;
            }
            State::Code if byte == b'"' => state = State::String,
            State::Comment if byte == b'\n' => state = State::Code,
            State::String if byte == b'\\' => {
                if let Some(next) = source.get_mut(index + 1)
                    && *next != b'\n'
                {
                    *next = b' ';
                    source[index] = b' ';
                    index += 1;
                }
            }
            State::String if byte == b'"' || byte == b'\n' => {
                state = State::Code
            }
            State::Block if byte == b'\\' => {
                source[index] = b' ';
                if source[index + 1..].starts_with(b"\"\"\"") {
                    source[index + 1..index + 4].fill(b' ');
                    index += 3;
                }
            }
            State::Block if source[index..].starts_with(b"\"\"\"") => {
                index += 2;
                state = State::Code;
            }
            _ => {}
        }
        index += 1;
    }
}

unsafe extern "C" fn mask_parse_input(source: *mut u8, len: usize) {
    // SAFETY: C lends the writable arena copy of exactly len bytes.
    mask_string_escapes(unsafe { std::slice::from_raw_parts_mut(source, len) });
}

pub fn register() {
    // SAFETY: The executable registers the callbacks before extractor threads start.
    unsafe {
        cbm_graphql_set_field_extractor(field_spans);
        cbm_graphql_set_parse_masker(mask_parse_input);
    }
}

#[cfg(test)]
mod tests {
    use super::mask_string_escapes;

    #[test]
    fn masks_escapes_without_shifting_source_spans() {
        let mut source = br##"# quote "\" stays
"One \"two\"" """Block \""" stays""" "\u00e9""##
            .to_vec();
        mask_string_escapes(&mut source);
        assert_eq!(
            source,
            br##"# quote "\" stays
"One   two  " """Block      stays""" "  00e9""##
        );
    }
}
