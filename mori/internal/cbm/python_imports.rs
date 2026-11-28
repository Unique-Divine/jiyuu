//! Extract Python import rows from Mori's pinned tree-sitter AST.

use std::ffi::c_void;

use crate::native_tree::RawNode;

#[repr(C)]
struct Bytes {
    data: *const u8,
    len: usize,
}

impl From<&[u8]> for Bytes {
    fn from(bytes: &[u8]) -> Self {
        Self {
            data: bytes.as_ptr(),
            len: bytes.len(),
        }
    }
}

#[repr(C)]
struct ImportRow {
    local: Bytes,
    prefix: Bytes,
    member: Bytes,
    needs_dot: bool,
}

type Emit = unsafe extern "C" fn(*mut c_void, *const ImportRow);
type Extract =
    unsafe extern "C" fn(RawNode, *const u8, usize, Emit, *mut c_void);

unsafe extern "C" {
    fn cbm_python_set_import_extractor(extract: Extract);
}

struct ImportSink {
    emit: Emit,
    context: *mut c_void,
}

impl ImportSink {
    fn row(&self, local: &[u8], prefix: Option<&[u8]>, member: &[u8]) {
        let row = ImportRow {
            local: local.into(),
            prefix: prefix.unwrap_or_default().into(),
            member: member.into(),
            needs_dot: prefix.is_some_and(|prefix| !prefix.ends_with(b".")),
        };
        // SAFETY: C lends the sink and copies these bytes before returning.
        unsafe { (self.emit)(self.context, &row) };
    }
}

unsafe fn text(node: RawNode, source: &[u8]) -> Option<&[u8]> {
    // SAFETY: The node borrows the live tree for this source.
    let bytes = unsafe { node.text(source) }?;
    // Native import names are C strings. Keep raw bytes up to the first NUL.
    bytes.split(|&byte| byte == 0).next()
}

fn last_part(path: &[u8]) -> &[u8] {
    path.rsplit(|&byte| matches!(byte, b'/' | b'.' | b':' | b'\\'))
        .next()
        .unwrap_or_default()
}

unsafe fn aliased(
    node: RawNode,
    source: &[u8],
    prefix: Option<&[u8]>,
    sink: &ImportSink,
) {
    // SAFETY: All descendants borrow this callback's live tree.
    unsafe {
        let Some(name) = node
            .field(c"name")
            .and_then(|name| text(name, source))
            .filter(|name| !name.is_empty())
        else {
            return;
        };
        let local = node
            .field(c"alias")
            .and_then(|alias| text(alias, source))
            .unwrap_or_else(|| last_part(name));
        sink.row(local, prefix, name);
    }
}

unsafe fn direct_name(node: RawNode, source: &[u8], sink: &ImportSink) {
    // SAFETY: The node borrows this callback's live tree.
    unsafe {
        if node.is(c"aliased_import") {
            aliased(node, source, None, sink);
        } else if let Some(module) =
            text(node, source).filter(|name| !name.is_empty())
        {
            let root = module
                .split(|&byte| byte == b'.')
                .next()
                .unwrap_or_default();
            sink.row(root, None, module);
        }
    }
}

unsafe fn direct_import(node: RawNode, source: &[u8], sink: &ImportSink) {
    // SAFETY: All descendants borrow this callback's live tree.
    unsafe {
        // Prefer the grammar's name field as the native extractor does.
        if let Some(name) = node.field(c"name") {
            direct_name(name, source, sink);
            return;
        }
        for child in node.children() {
            if child.is(c"dotted_name")
                || child.is(c"identifier")
                || child.is(c"aliased_import")
            {
                direct_name(child, source, sink);
            }
        }
    }
}

unsafe fn from_import(node: RawNode, source: &[u8], sink: &ImportSink) {
    // SAFETY: All descendants borrow this callback's live tree.
    unsafe {
        if node.is(c"future_import_statement") {
            sink.row(b"__future__", None, b"__future__");
            return;
        }
        let module_node = node.field(c"module_name").or_else(|| {
            node.children().find(|child| {
                child.is(c"dotted_name") || child.is(c"relative_import")
            })
        });
        let module_start = module_node.map(|module| module.range().start);
        let prefix = module_node.and_then(|module| text(module, source));

        let mut saw_item = false;
        for child in node.children() {
            if child.is(c"identifier") || child.is(c"dotted_name") {
                if module_start == Some(child.range().start) {
                    continue;
                }
                if let Some(name) =
                    text(child, source).filter(|name| !name.is_empty())
                {
                    sink.row(name, prefix, name);
                }
                saw_item = true;
            } else if child.is(c"aliased_import") {
                aliased(child, source, prefix, sink);
                saw_item = true;
            } else if child.is(c"wildcard_import")
                && let Some(module) = prefix.filter(|module| !module.is_empty())
            {
                sink.row(last_part(module), None, module);
                saw_item = true;
            }
        }
        if !saw_item
            && let Some(module) = prefix.filter(|module| !module.is_empty())
        {
            sink.row(last_part(module), None, module);
        }
    }
}

unsafe extern "C" fn extract(
    node: RawNode,
    source: *const u8,
    source_len: usize,
    emit: Emit,
    context: *mut c_void,
) {
    // SAFETY: C lends the live tree, initialized source and sink for this call.
    unsafe {
        let source = std::slice::from_raw_parts(source, source_len);
        let sink = ImportSink { emit, context };
        if node.is(c"import_statement") {
            direct_import(node, source, &sink);
        } else {
            from_import(node, source, &sink);
        }
    }
}

/// Install the native Python import callback.
///
/// # Safety
///
/// Call during process startup, before other threads extract files or
/// install callbacks.
pub unsafe fn register() {
    // SAFETY: The caller guarantees exclusive startup registration.
    unsafe { cbm_python_set_import_extractor(extract) };
}
