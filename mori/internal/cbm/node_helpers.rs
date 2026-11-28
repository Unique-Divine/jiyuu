//! Node text, kind, and test-path helpers behind the extractor's C ABI.

use std::ffi::{CStr, c_char, c_int, c_void};

use crate::{lang_specs::lang, native_tree::RawNode};

unsafe extern "C" {
    fn cbm_arena_strndup(
        arena: *mut c_void,
        source: *const c_char,
        len: usize,
    ) -> *mut c_char;
    fn cbm_ffi_mark_written(address: *const c_void, size: usize);
}

/// # Safety
/// The node borrows a live tree, `source` covers its byte range, and `arena`
/// follows the C arena API's requirements.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_node_text(
    arena: *mut c_void,
    node: RawNode,
    source: *const c_char,
) -> *mut c_char {
    // SAFETY: The caller supplies the corresponding live node and source.
    let range = unsafe { node.range() };
    // SAFETY: The arena is caller-owned and the source covers the copied range.
    unsafe {
        let (source, len) = if range.is_empty() {
            (c"".as_ptr(), 0)
        } else {
            (source.add(range.start), range.len())
        };
        cbm_arena_strndup(arena, source, len)
    }
}

/// # Safety
/// `parent` points to a live node, `kind` is a C string, and `out` points
/// to writable node storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_rust_find_child_by_kind(
    parent: *const RawNode,
    kind: *const c_char,
    out: *mut RawNode,
) {
    // SAFETY: The caller supplies the live node, the kind, and output storage.
    unsafe {
        let child = (*parent)
            .indexed_children()
            .find(|child| child.is(CStr::from_ptr(kind)))
            .unwrap_or(RawNode::NULL);
        out.write(child);
    }
}

/// # Safety
/// `parent` borrows a live tree, `kind` is a C string, and `out` must be
/// writable for up to `max` nodes when `max` is positive.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_find_children_by_kind(
    parent: RawNode,
    kind: *const c_char,
    out: *mut RawNode,
    max: c_int,
) -> c_int {
    let mut count = 0;
    // SAFETY: The caller supplies the live node, kind, and output capacity.
    unsafe {
        for child in parent
            .indexed_children()
            .filter(|child| child.is(CStr::from_ptr(kind)))
            .take(max.max(0) as usize)
        {
            out.add(count).write(child);
            count += 1;
        }
        if count > 0 {
            cbm_ffi_mark_written(out.cast(), count * size_of::<RawNode>());
        }
    }
    count as c_int
}

/// # Safety
/// The node must borrow a live tree and `kind` must be a C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_has_ancestor_kind(
    mut node: RawNode,
    kind: *const c_char,
    max_depth: c_int,
) -> bool {
    // SAFETY: All ancestors belong to the caller's live tree.
    unsafe {
        for _ in 0..max_depth {
            let Some(parent) = node.parent() else {
                return false;
            };
            if parent.is(CStr::from_ptr(kind)) {
                return true;
            }
            node = parent;
        }
    }
    false
}

/// # Safety
/// `kind` must be null or point to a C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_is_namespace_scope_kind(
    language: c_int,
    kind: *const c_char,
) -> bool {
    if kind.is_null() {
        return false;
    }
    let expected = match language {
        lang::CPP | lang::CUDA => c"namespace_definition",
        lang::TYPESCRIPT | lang::TSX | lang::ARKTS => c"internal_module",
        _ => return false,
    };
    // SAFETY: The caller provides a valid C string when the pointer is non-null.
    unsafe { CStr::from_ptr(kind) == expected }
}

/// # Safety
/// `kind` must point to a C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_is_loop_node_type(kind: *const c_char) -> bool {
    // SAFETY: The caller provides a valid C string.
    let kind = unsafe { CStr::from_ptr(kind) }.to_bytes();
    matches!(
        kind,
        b"for_statement"
            | b"while_statement"
            | b"do_statement"
            | b"do_while_statement"
            | b"for_in_statement"
            | b"for_of_statement"
            | b"for_each_statement"
            | b"foreach_statement"
            | b"enhanced_for_statement"
            | b"for_range_loop"
            | b"c_style_for_statement"
            | b"for_expression"
            | b"while_expression"
            | b"loop_expression"
            | b"while_let_expression"
            | b"repeat_statement"
            | b"repeat_while_statement"
            | b"forGenerator"
            | b"until"
            | b"while_modifier"
            | b"until_modifier"
            | b"for"
            | b"while"
    )
}

fn is_test_file(path: &[u8], language: c_int) -> bool {
    let base = path.rsplit(|&b| b == b'/').next().unwrap_or_default();
    // The legacy match for __tests__/ also accepts it as a directory suffix.
    if path.split_inclusive(|&b| b == b'/').any(|part| {
        matches!(part, b"tests/" | b"test/" | b"spec/")
            || part.ends_with(b"__tests__/")
    }) {
        return true;
    }

    let test_prefix = base.starts_with(b"test_");
    match language {
        lang::GO => base.ends_with(b"_test.go"),
        lang::PYTHON => test_prefix || base.ends_with(b"_test.py"),
        lang::JAVASCRIPT | lang::TYPESCRIPT | lang::TSX | lang::ARKTS => {
            let stem_end = base
                .iter()
                .rposition(|&b| b == b'.')
                .filter(|&dot| dot != 0)
                .unwrap_or(base.len());
            // The C helper strips the extension into a 256-byte buffer.
            let stem = &base[..stem_end.min(255)];
            test_prefix
                || [b".test", b".spec", b"_test", b"_spec"]
                    .iter()
                    .any(|s| stem.ends_with(*s))
        }
        lang::JAVA | lang::KOTLIN | lang::SCALA => [
            b"Test.java".as_slice(),
            b"Tests.java",
            b"Spec.java",
            b"Test.kt",
            b"Spec.kt",
            b"Test.scala",
            b"Spec.scala",
        ]
        .iter()
        .any(|s| base.ends_with(s)),
        lang::RUST => test_prefix || base.ends_with(b"_test.rs"),
        lang::RUBY => {
            test_prefix
                || base.ends_with(b"_test.rb")
                || base.ends_with(b"_spec.rb")
        }
        lang::PHP => base.ends_with(b"Test.php"),
        lang::CSHARP => {
            base.ends_with(b"Tests.cs") || base.ends_with(b"Test.cs")
        }
        lang::CPP | lang::C => {
            test_prefix
                || [b"_test.c".as_slice(), b"_test.cc", b"_test.cpp"]
                    .iter()
                    .any(|s| base.ends_with(s))
        }
        lang::MATLAB => test_prefix || base.starts_with(b"Test"),
        _ => false,
    }
}

/// # Safety
/// `path` must be null or point to a C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_is_test_file(
    path: *const c_char,
    language: c_int,
) -> bool {
    if path.is_null() {
        return false;
    }
    // SAFETY: The caller provides a valid C string when the pointer is non-null.
    is_test_file(unsafe { CStr::from_ptr(path) }.to_bytes(), language)
}
