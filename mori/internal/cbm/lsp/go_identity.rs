//! Go caller identities shared by definition extraction and call resolution.

use std::ffi::{CStr, c_char, c_void};
use std::ptr;

use crate::go_eval::native::cbm_go_eval_copy_name;
use crate::native_tree::{BorrowedNode, RawNode};

pub(crate) fn receiver_name<'source>(
    source: &'source [u8],
    receiver: BorrowedNode<'_>,
) -> Option<&'source [u8]> {
    for parameter in receiver
        .children()
        .filter(|child| child.kind() == c"parameter_declaration")
    {
        let mut current = parameter.field(c"type");
        for _ in 0..4 {
            let Some(ty) = current else { break };
            match ty.kind().to_bytes() {
                b"type_identifier" => {
                    return source
                        .get(ty.range())
                        .filter(|name| !name.is_empty());
                }
                b"pointer_type" | b"generic_type" => {
                    current =
                        ty.field(c"type").or_else(|| ty.named_children().next());
                }
                _ => break,
            }
        }
    }
    None
}

fn init_ordinal(source: &[u8], declaration: BorrowedNode<'_>) -> usize {
    let mut root = declaration;
    while let Some(parent) = root.parent() {
        root = parent;
    }
    let mut ordinal = 0;
    for child in root
        .children()
        .filter(|node| node.kind() == c"function_declaration")
    {
        let Some(name) = child
            .field(c"name")
            .and_then(|node| source.get(node.range()))
        else {
            continue;
        };
        if name == b"init" {
            ordinal += 1;
            if child.same_as(declaration) {
                break;
            }
        }
    }
    ordinal
}

/// Use the per-file ordinal so inserting lines earlier in the file does
/// not change the callable identity.
pub(crate) fn init_qn(
    source: &[u8],
    declaration: BorrowedNode<'_>,
    base: &CStr,
    path: &CStr,
) -> Option<Vec<u8>> {
    let basename = path
        .to_bytes()
        .rsplit(|&byte| byte == b'/')
        .next()
        .unwrap_or_default();
    let ordinal = init_ordinal(source, declaration).to_string();
    let mut qn = Vec::new();
    qn.try_reserve_exact(
        base.count_bytes()
            .checked_add(basename.len())?
            .checked_add(ordinal.len())?
            .checked_add(2)?,
    )
    .ok()?;
    qn.extend_from_slice(base.to_bytes());
    qn.push(b'#');
    qn.extend_from_slice(basename);
    qn.push(b':');
    qn.extend_from_slice(ordinal.as_bytes());
    Some(qn)
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_go_receiver_type_name(
    arena: *mut c_void,
    receiver: *const RawNode,
    source: *const u8,
    len: usize,
) -> *const c_char {
    // SAFETY: C lends the live tree and source through this call.
    let receiver = unsafe { BorrowedNode::from_root(&*receiver) };
    let source = unsafe { std::slice::from_raw_parts(source, len) };
    let Some(name) = receiver_name(source, receiver) else {
        return ptr::null();
    };
    // SAFETY: The native arena copies the source span synchronously.
    unsafe { cbm_go_eval_copy_name(arena, name.as_ptr(), name.len()) }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_go_init_qn(
    arena: *mut c_void,
    base: *const c_char,
    path: *const c_char,
    declaration: *const RawNode,
    source: *const u8,
    len: usize,
) -> *const c_char {
    // SAFETY: C supplies live strings, source, and tree.
    let declaration = unsafe { BorrowedNode::from_root(&*declaration) };
    let source = unsafe { std::slice::from_raw_parts(source, len) };
    let base_name = unsafe { CStr::from_ptr(base) };
    let path = unsafe { CStr::from_ptr(path) };
    let Some(qn) = init_qn(source, declaration, base_name, path) else {
        return base;
    };
    let qn = unsafe { cbm_go_eval_copy_name(arena, qn.as_ptr(), qn.len()) };
    if qn.is_null() { base } else { qn }
}
