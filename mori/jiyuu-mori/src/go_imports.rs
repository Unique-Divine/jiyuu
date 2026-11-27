//! Extract Go import records from Mori's pinned Tree-sitter tree.

use std::ffi::{c_char, c_void};

use crate::native_tree::{BorrowedNode, RawNode};

#[derive(Debug, PartialEq, Eq)]
struct GoImport<'a> {
    module_path: &'a [u8],
    local_name: &'a [u8],
}

impl<'a> GoImport<'a> {
    fn from_spelling(path: &'a [u8], alias: Option<&'a [u8]>) -> Option<Self> {
        // Native stores source spelling up to the first NUL. Matching quotes
        // enclose the interpreted path without decoding escapes. Raw imports
        // retain their backticks.
        let spelling = path.split(|byte| *byte == 0).next().unwrap_or_default();
        let module_path = match spelling {
            [quote @ (b'\'' | b'"'), inner @ .., end] if quote == end => inner,
            _ => spelling,
        };
        if module_path.is_empty() || module_path == b"C" {
            return None;
        }

        // Preserve the default alias used by the native import resolver,
        // including its handling of dotted paths and escape bytes.
        let local_name = alias.unwrap_or_else(|| {
            module_path
                .rsplit(|byte| matches!(*byte, b'/' | b'.' | b':' | b'\\'))
                .next()
                .unwrap_or_default()
        });
        Some(Self {
            module_path,
            local_name,
        })
    }
}

type ImportSink = unsafe extern "C" fn(
    context: *mut c_void,
    path: *const c_char,
    path_len: usize,
    name: *const c_char,
    name_len: usize,
);

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_go_extract_imports(
    root: RawNode,
    source: *const c_char,
    source_len: usize,
    context: *mut c_void,
    sink: ImportSink,
) {
    // SAFETY: C lends the source bytes and the corresponding live tree.
    let source =
        unsafe { std::slice::from_raw_parts(source.cast::<u8>(), source_len) };
    // SAFETY: The input node and source are valid for the callback.
    let root = unsafe { BorrowedNode::from_root(&root) };
    let emit = |spec: BorrowedNode<'_>| {
        let path = spec
            .field(c"path")
            .and_then(|node| source.get(node.range()));
        let alias = spec
            .field(c"name")
            .and_then(|node| source.get(node.range()));
        let import = path.and_then(|path| GoImport::from_spelling(path, alias));
        if let Some(GoImport {
            module_path,
            local_name,
        }) = import
        {
            // SAFETY: Both slices remain live until the C sink copies them.
            unsafe {
                sink(
                    context,
                    module_path.as_ptr().cast(),
                    module_path.len(),
                    local_name.as_ptr().cast(),
                    local_name.len(),
                );
            }
        }
    };

    for declaration in root
        .children()
        .filter(|node| node.kind() == c"import_declaration")
    {
        for child in declaration.children() {
            if child.kind() == c"import_spec" {
                emit(child);
            } else if child.kind() == c"import_spec_list" {
                for spec in child
                    .children()
                    .filter(|node| node.kind() == c"import_spec")
                {
                    emit(spec);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_native_import_spelling_and_aliases() {
        type Case<'a> = (&'a [u8], Option<&'a [u8]>, GoImport<'a>);
        let cases: &[Case<'_>] = &[
            (
                br#""example.net/team/lib.v2""#,
                None,
                GoImport {
                    module_path: b"example.net/team/lib.v2",
                    local_name: b"v2",
                },
            ),
            (
                b"`example.net/team/raw`",
                None,
                GoImport {
                    module_path: b"`example.net/team/raw`",
                    local_name: b"raw`",
                },
            ),
            (
                br#""example.net/team\x2fname""#,
                Some(b"renamed"),
                GoImport {
                    module_path: br"example.net/team\x2fname",
                    local_name: b"renamed",
                },
            ),
            (
                br#""example.net/team/dot""#,
                Some(b"."),
                GoImport {
                    module_path: b"example.net/team/dot",
                    local_name: b".",
                },
            ),
            (
                br#""example.net/team/blank""#,
                Some(b"_"),
                GoImport {
                    module_path: b"example.net/team/blank",
                    local_name: b"_",
                },
            ),
        ];
        for &(path, alias, ref import) in cases {
            assert_eq!(
                GoImport::from_spelling(path, alias).as_ref(),
                Some(import)
            );
        }
    }

    #[test]
    fn skips_empty_and_cgo_spelling_and_preserves_recovered_bytes() {
        for path in [b"".as_slice(), br#""""#, br#""C""#, b"\0unfinished"] {
            assert_eq!(GoImport::from_spelling(path, None), None);
        }
        assert_eq!(
            GoImport::from_spelling(b"\"pkg/unfinished\0extra", None),
            Some(GoImport {
                module_path: b"\"pkg/unfinished",
                local_name: b"unfinished",
            }),
        );
    }
}
