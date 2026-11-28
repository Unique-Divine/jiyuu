use super::*;
use crate::native_parser::{
    RawTree, ts_parser_delete, ts_parser_new, ts_parser_parse_string,
    ts_parser_set_language, ts_tree_delete,
};
use std::ptr::{NonNull, null, null_mut};

unsafe extern "C" {
    fn ts_tree_root_node(tree: *const RawTree) -> RawNode;
}

struct Tree(NonNull<RawTree>);

impl Tree {
    fn python(source: &[u8]) -> Self {
        // SAFETY: Tree-sitter borrows the slice only through the parse call.
        let tree = unsafe {
            let parser = ts_parser_new();
            assert!(!parser.is_null());
            let grammar = crate::lang_specs::cbm_ts_language(lang::PYTHON);
            let tree = if ts_parser_set_language(parser, grammar) {
                ts_parser_parse_string(
                    parser,
                    null(),
                    source.as_ptr().cast(),
                    source.len() as u32,
                )
            } else {
                null_mut()
            };
            ts_parser_delete(parser);
            tree
        };
        Self(NonNull::new(tree).expect("parse Python fixture"))
    }

    fn root(&self) -> RawNode {
        // SAFETY: This struct owns the tree.
        unsafe { ts_tree_root_node(self.0.as_ptr()) }
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        // SAFETY: This struct owns the tree.
        unsafe { ts_tree_delete(self.0.as_ptr()) }
    }
}

#[test]
fn test_paths_keep_the_legacy_byte_and_stem_rules() {
    let cases = [
        (lang::GO, b"a_test.go".as_slice(), true),
        (lang::GO, b"test_a.go", false),
        (lang::GO, b"\xff_test.go", true),
        (lang::PYTHON, b"pkg/test_script", true),
        (lang::PYTHON, b"pkg/a_test.py", true),
        (lang::JAVASCRIPT, b"a.test.js", true),
        (lang::TSX, b"a_test.tsx", true),
        (lang::ARKTS, b"a_spec.ets", true),
        (lang::TYPESCRIPT, b".test", true),
        (lang::TYPESCRIPT, b"a.test", false),
        (lang::TYPESCRIPT, b"a.test.d.ts", false),
        (lang::TYPESCRIPT, b"test_a.ts", true),
        (lang::JAVA, b"aTests.java", true),
        (lang::KOTLIN, b"aTest.kt", true),
        (lang::SCALA, b"aSpec.scala", true),
        (lang::RUST, b"a_test.rs", true),
        (lang::RUBY, b"a_spec.rb", true),
        (lang::PHP, b"aTest.php", true),
        (lang::CSHARP, b"aTests.cs", true),
        (lang::C, b"a_test.cc", true),
        (lang::CPP, b"a_test.cpp", true),
        (lang::MATLAB, b"TestA.m", true),
        (-1, b"tests/plain", true),
        (-1, b"root/test/nested/plain", true),
        (-1, b"root/spec/plain", true),
        (-1, b"root/prefix__tests__/plain", true),
        (-1, b"root/Tests/plain", false),
        (-1, b"root/tests", false),
        (-1, b"root/tests_extra/plain", false),
        (-1, b"root\\tests\\plain", false),
        (-1, b"", false),
    ];
    for (language, path, expected) in cases {
        assert_eq!(
            is_test_file(path, language),
            expected,
            "lang {language} path {path:?}"
        );
    }
    for (stem_length, expected) in
        [(254, true), (255, true), (256, false), (300, false)]
    {
        let mut path = vec![b'x'; stem_length - 5];
        path.extend_from_slice(b"_test.ts");
        assert_eq!(is_test_file(&path, lang::TYPESCRIPT), expected);
    }
    // SAFETY: The C accessor accepts a null path.
    assert!(!unsafe { cbm_is_test_file(null(), lang::GO) });
}

#[test]
fn node_kinds_keep_order_and_ancestor_depth() {
    let source = "[a, β, c]\n[]\n".as_bytes();
    let tree = Tree::python(source);
    let root = tree.root();
    // SAFETY: The nodes and output slots remain live for each FFI call.
    unsafe {
        let expression = root.child_of_kind(c"expression_statement").unwrap();
        let list = expression.child_of_kind(c"list").unwrap();
        let mut first = RawNode::NULL;
        cbm_rust_find_child_by_kind(&list, c"identifier".as_ptr(), &mut first);
        assert_eq!(first.text(source), Some(b"a".as_slice()));
        cbm_rust_find_child_by_kind(&list, c"[".as_ptr(), &mut first);
        assert_eq!(first.text(source), Some(b"[".as_slice()));
        cbm_rust_find_child_by_kind(&list, c"absent".as_ptr(), &mut first);
        assert!(first.is_null());

        let mut output = [RawNode::NULL; 4];
        let kind = c"identifier".as_ptr();
        assert_eq!(cbm_find_children_by_kind(list, kind, null_mut(), -1), 0);
        assert_eq!(cbm_find_children_by_kind(list, kind, null_mut(), 0), 0);
        assert_eq!(
            cbm_find_children_by_kind(list, kind, output.as_mut_ptr(), 2),
            2
        );
        assert!(output[2].is_null());
        assert_eq!(output[0].text(source), Some(b"a".as_slice()));
        assert_eq!(output[1].text(source), Some("β".as_bytes()));
        assert_eq!(
            cbm_find_children_by_kind(list, kind, output.as_mut_ptr(), 4),
            3
        );
        assert_eq!(output[2].text(source), Some(b"c".as_slice()));

        assert!(!cbm_has_ancestor_kind(output[1], c"list".as_ptr(), -1));
        assert!(!cbm_has_ancestor_kind(output[1], c"list".as_ptr(), 0));
        assert!(cbm_has_ancestor_kind(output[1], c"list".as_ptr(), 1));
        assert!(!cbm_has_ancestor_kind(output[1], c"module".as_ptr(), 2));
        assert!(cbm_has_ancestor_kind(output[1], c"module".as_ptr(), 3));
        assert!(!cbm_has_ancestor_kind(root, c"module".as_ptr(), 1));
    }
}

#[test]
fn node_text_handles_empty_spans_and_closed_arenas() {
    let empty = Tree::python(b"");
    let source = c"[a, b]";
    let nonempty = Tree::python(source.to_bytes());
    // SAFETY: Both nodes keep live trees, the empty span reads no source
    // bytes, and the C arena API accepts a null arena.
    unsafe {
        assert_eq!(empty.root().text(b""), Some(b"".as_slice()));
        assert!(cbm_node_text(null_mut(), empty.root(), null()).is_null());
        assert!(
            cbm_node_text(null_mut(), nonempty.root(), source.as_ptr())
                .is_null()
        );
    }
}

#[test]
fn loop_and_namespace_kinds_match_exact_spellings() {
    // SAFETY: Each non-null pointer refers to a static, terminated string.
    unsafe {
        for language in [lang::CPP, lang::CUDA] {
            assert!(cbm_is_namespace_scope_kind(
                language,
                c"namespace_definition".as_ptr()
            ));
            assert!(!cbm_is_namespace_scope_kind(
                language,
                c"internal_module".as_ptr()
            ));
        }
        for language in [lang::TYPESCRIPT, lang::TSX, lang::ARKTS] {
            assert!(cbm_is_namespace_scope_kind(
                language,
                c"internal_module".as_ptr()
            ));
        }
        assert!(!cbm_is_namespace_scope_kind(
            -1,
            c"namespace_definition".as_ptr()
        ));
        assert!(!cbm_is_namespace_scope_kind(lang::CPP, null()));
        for kind in [
            c"for",
            c"forGenerator",
            c"foreach_statement",
            c"until_modifier",
            c"repeat_while_statement",
        ] {
            assert!(cbm_is_loop_node_type(kind.as_ptr()));
        }
        for kind in [c"", c"For", c"for_statements", c"if_statement"] {
            assert!(!cbm_is_loop_node_type(kind.as_ptr()));
        }
    }
}
