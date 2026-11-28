//! GraphQL SDL extraction using the parser and grammar already linked by Mori.

use std::ffi::CStr;
#[cfg(feature = "c-backend")]
use std::ffi::c_void;

use crate::native_tree::RawNode;

#[cfg(feature = "c-backend")]
type SchemaEmit = unsafe extern "C" fn(*mut c_void, bool, FieldSpans);

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

#[cfg(feature = "c-backend")]
unsafe extern "C" {
    fn cbm_graphql_set_parse_masker(mask: unsafe extern "C" fn(*mut u8, usize));
    fn cbm_graphql_set_schema_extractor(
        extract: unsafe extern "C" fn(RawNode, SchemaEmit, *mut c_void),
    );
}

unsafe fn child_span(node: RawNode, name: &CStr) -> Span {
    // SAFETY: All nodes returned by Tree-sitter borrow the callback's live tree.
    let span = unsafe {
        node.field(name)
            .or_else(|| node.child_of_kind(name))
            .map(|child| child.range())
    };
    match span {
        Some(range) => Span {
            start: range.start as u32,
            end: range.end as u32,
            present: true,
        },
        None => Span::default(),
    }
}

/// Read the name and type spans of a GraphQL field definition.
///
/// # Safety
/// The node must borrow a live GraphQL field definition.
#[unsafe(no_mangle)]
unsafe extern "C" fn mori_graphql_extract_field_spans(
    node: RawNode,
) -> FieldSpans {
    // SAFETY: The C extractor lends a valid field node for this call.
    unsafe {
        FieldSpans {
            name: child_span(node, c"name"),
            field_type: child_span(node, c"type"),
        }
    }
}

#[cfg(feature = "c-backend")]
unsafe fn schema_root_rows(
    node: RawNode,
    emit: SchemaEmit,
    context: *mut c_void,
) {
    // SAFETY: The extractor lends the tree and emitter for a synchronous walk.
    unsafe {
        for child in node.named_children() {
            if child.is(c"root_operation_type_definition") {
                let name = child_span(child, c"operation_type");
                let field_type = child_span(child, c"named_type");
                if name.present && field_type.present {
                    emit(context, false, FieldSpans { name, field_type });
                }
            }
        }
    }
}

#[cfg(feature = "c-backend")]
unsafe fn walk_schema(
    node: RawNode,
    depth: u8,
    emit: SchemaEmit,
    context: *mut c_void,
) {
    // SAFETY: All children borrow the same live tree and emission is synchronous.
    unsafe {
        for child in node.named_children() {
            if child.is(c"schema_definition") {
                emit(
                    context,
                    true,
                    FieldSpans {
                        name: Span::default(),
                        field_type: Span::default(),
                    },
                );
                schema_root_rows(child, emit, context);
            } else if child.is(c"schema_extension") {
                schema_root_rows(child, emit, context);
            } else if depth > 1 {
                walk_schema(child, depth - 1, emit, context);
            }
        }
    }
}

#[cfg(feature = "c-backend")]
unsafe extern "C" fn schema_roots(
    node: RawNode,
    emit: SchemaEmit,
    context: *mut c_void,
) {
    // SAFETY: C lends the root, emitter and context for the duration of this call.
    // Walk source_file > document > definition > type_system_definition.
    unsafe { walk_schema(node, 4, emit, context) };
}

#[cfg(feature = "c-backend")]
#[derive(Clone, Copy)]
enum State {
    Code,
    Comment,
    String,
    Block,
}

#[cfg(feature = "c-backend")]
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

#[cfg(feature = "c-backend")]
unsafe extern "C" fn mask_parse_input(source: *mut u8, len: usize) {
    // SAFETY: C lends the writable arena copy of exactly len bytes.
    mask_string_escapes(unsafe { std::slice::from_raw_parts_mut(source, len) });
}

/// Install the GraphQL input masker and operation-root extractor.
///
/// # Safety
///
/// Call during process startup, before other threads can extract files or
/// install callbacks.
#[cfg(feature = "c-backend")]
pub unsafe fn register() {
    // SAFETY: The executable registers the callbacks before extractor threads start.
    unsafe {
        cbm_graphql_set_parse_masker(mask_parse_input);
        cbm_graphql_set_schema_extractor(schema_roots);
    }
}

#[cfg(all(test, feature = "c-backend"))]
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
