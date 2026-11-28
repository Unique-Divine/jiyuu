/*
 * Compare the Rust node helpers with the C reference in test-seam builds.
 */
#include "test_framework.h"
#include "cbm.h"
#include "arena.h"
#include "helpers.h"
#include "lang_specs.h"
#include "tree_sitter/api.h"

#include <stdio.h>
#include <string.h>

#ifdef CBM_ENABLE_TEST_SEAMS

/* Directories and basenames whose product covers every rule of
 * cbm_is_test_file, including the 255-byte stem truncation. */
static const char *const k_dirs[] = {
    "", "a/", "src/a/", "tests/", "test/", "spec/", "__tests__/", "src/tests/", "x/test/y/",
    "x/spec/", "a__tests__/", "testsx/", "spec", "/", "a/tests", "Tests/",
};
static const char *const k_bases[] = {
    "", "a", "main.go", "main_test.go", "test_a.py", "a_test.py", "a.py", "a.test.ts",
    "a.spec.js", "a_test.tsx", "a_spec.ets", "test_a.js", "a.ts", ".test", ".spec.ts",
    "a.test", "aTest.java", "aTests.java", "aSpec.java", "aTest.kt", "aSpec.kt",
    "aTest.scala", "aSpec.scala", "a_test.rs", "test_a.rs", "a.rs", "a_test.rb",
    "a_spec.rb", "test_a.rb", "aTest.php", "aTests.cs", "aTest.cs", "a_test.c",
    "a_test.cc", "a_test.cpp", "test_a.c", "TestA.m", "test_a.m", "a.m", "Test", "test_",
    "a.b.spec.ts", "a.spec", "x.test.d.ts", "\xff_test.go", "foo\\tests\\a.ts",
};

static int g_mismatches;

static void name_mismatch(const char *fn, const char *name, int lang, int rust, int c) {
    if (g_mismatches++ < 20) {
        fprintf(stderr, "  %s(\"%s\", %d): rust %d, c %d\n", fn, name ? name : "(null)", lang,
                rust, c);
    }
}


TEST(node_helpers_kind_predicates_match_c_reference) {
    static const char *const kinds[] = {
        NULL, "", "namespace_definition", "internal_module",
        "for_statement", "while_statement", "do_statement", "do_while_statement",
        "for_in_statement", "for_of_statement", "for_each_statement", "foreach_statement",
        "enhanced_for_statement", "for_range_loop", "c_style_for_statement", "for_expression",
        "while_expression", "loop_expression", "while_let_expression", "repeat_statement",
        "repeat_while_statement", "forGenerator", "until", "while_modifier", "until_modifier",
        "for", "while", "For", "while_statement_x", "internal_module_x",
    };
    g_mismatches = 0;
    for (size_t i = 0; i < sizeof(kinds) / sizeof(kinds[0]); i++) {
        const char *kind = kinds[i];
        for (int lang = -2; lang < CBM_LANG_COUNT + 2; lang++) {
            CBMLanguage language = (CBMLanguage)lang;
            int rust = cbm_is_namespace_scope_kind(language, kind);
            int c = cbm_c_reference_is_namespace_scope_kind(language, kind);
            if (rust != c) {
                name_mismatch("cbm_is_namespace_scope_kind", kind, lang, rust, c);
            }
        }
        if (kind) {
            int rust = cbm_is_loop_node_type(kind);
            int c = cbm_c_reference_is_loop_node_type(kind);
            if (rust != c) {
                name_mismatch("cbm_is_loop_node_type", kind, -1, rust, c);
            }
        }
    }
    ASSERT_EQ(g_mismatches, 0);
    PASS();
}

TEST(node_helpers_test_file_matches_c_reference) {
    g_mismatches = 0;
    for (int lang = -2; lang < CBM_LANG_COUNT + 2; lang++) {
        ASSERT_EQ(cbm_is_test_file(NULL, (CBMLanguage)lang),
                  cbm_c_reference_is_test_file(NULL, (CBMLanguage)lang));
    }
    char path[1024];
    char long_base[400];
    int checked = 0;
    for (size_t d = 0; d < sizeof(k_dirs) / sizeof(k_dirs[0]); d++) {
        for (size_t b = 0; b < sizeof(k_bases) / sizeof(k_bases[0]) + 4; b++) {
            const char *base;
            if (b < sizeof(k_bases) / sizeof(k_bases[0])) {
                base = k_bases[b];
            } else {
                /* Stems around the C helper's 255-byte buffer. */
                static const size_t stems[] = {254, 255, 256, 300};
                size_t n = stems[b - sizeof(k_bases) / sizeof(k_bases[0])];
                memset(long_base, 'x', n);
                memcpy(long_base + n - 5, "_test", 5);
                memcpy(long_base + n, ".ts", 4);
                base = long_base;
            }
            snprintf(path, sizeof(path), "%s%s", k_dirs[d], base);
            for (int lang = -2; lang < CBM_LANG_COUNT + 2; lang++) {
                int r = cbm_is_test_file(path, (CBMLanguage)lang);
                int c = cbm_c_reference_is_test_file(path, (CBMLanguage)lang);
                if (r != c) {
                    name_mismatch("cbm_is_test_file", path, lang, r, c);
                }
                checked++;
            }
        }
    }
    ASSERT_GT(checked, 10000);
    ASSERT_EQ(g_mismatches, 0);
    PASS();
}

typedef struct {
    CBMLanguage lang;
    const char *source;
} HelperSource;

static const HelperSource k_sources[] = {
    {CBM_LANG_PYTHON, ""},
    {CBM_LANG_PYTHON, "[a, \xce\xb2, c]\n[]\n"},
    {CBM_LANG_GO, "package p\n\nimport \"fmt\"\n\ntype T struct{ a int }\n\n"
                  "func (t *T) M(x int) int {\n\tfor i := 0; i < x; i++ {\n"
                  "\t\tif i > 2 { fmt.Println(i) }\n\t}\n\treturn x\n}\n"},
    {CBM_LANG_PYTHON, "import os\nfrom a import b\n\n@dec\nclass C(B):\n"
                      "    def m(self, x):\n        while x:\n            x -= 1\n"
                      "        return [y for y in range(3) if y]\n"},
    {CBM_LANG_JAVASCRIPT, "import x from 'y';\nclass A extends B {\n  m() {\n"
                          "    for (const k of o) { if (k) throw new Error(k); }\n"
                          "    return () => `t${a}`;\n  }\n}\nexport default A;\n"},
    {CBM_LANG_TYPESCRIPT, "namespace N { export interface I { a: number } }\n"
                          "export function f<T>(x: T): T { do { x = x; } while (0); return x; }\n"},
    {CBM_LANG_RUST, "use std::fmt;\nmod m { pub struct S { a: u8 } }\n"
                    "impl S { fn f(&self) -> u8 { loop { match 1 { _ => break } } 0 } }\n"},
    {CBM_LANG_C, "#include <stdio.h>\n#ifdef X\nint a;\n#else\nint b;\n#endif\n"
                 "static int *f(int x) { for (;;) { if (x) return 0; } }\n"},
    {CBM_LANG_CPP, "namespace n { class C { public: void m(); }; }\n"
                   "void n::C::m() { for (auto x : v) { while (x) {} } }\n"},
    {CBM_LANG_JAVA, "package p;\nimport a.B;\n@A class C extends D implements E {\n"
                    "  int f;\n  void m() throws X { for (int i : a) { if (i>0) m(); } }\n}\n"},
    {CBM_LANG_NIX, "{ a.\"b-c\".d = x: x; e = { f = 1; }; }\n"},
    {CBM_LANG_ELIXIR, "defmodule M do\n  def f(x) when is_integer(x), do: x\nend\n"},
    {CBM_LANG_RUBY, "class A < B\n  def m\n    until x do; end\n  end\nend\n"},
    {CBM_LANG_CSHARP, "namespace N { [A][B] class C { void M() { foreach (var x in y) {} } } }\n"},
};

static int same_node(TSNode a, TSNode b) {
    if (ts_node_is_null(a) || ts_node_is_null(b)) {
        return ts_node_is_null(a) && ts_node_is_null(b);
    }
    return ts_node_eq(a, b) && ts_node_start_byte(a) == ts_node_start_byte(b) &&
           ts_node_end_byte(a) == ts_node_end_byte(b);
}

/* All node-taking helpers at one node. */
static int compare_at(CBMArena *arena, TSNode node, const char *source) {
    int ok = 1;
    const char *rt = cbm_node_text(arena, node, source);
    const char *ct = cbm_c_reference_node_text(arena, node, source);
    uint32_t start = ts_node_start_byte(node);
    uint32_t end = ts_node_end_byte(node);
    size_t len = end > start ? end - start : 0;
    if (!rt || !ct || memcmp(rt, ct, len + 1) != 0) {
        fprintf(stderr, "  cbm_node_text differs at %s\n", ts_node_type(node));
        ok = 0;
    }
    /* Probe every child kind, the node's own kind and a kind no grammar has. */
    uint32_t count = ts_node_child_count(node);
    for (uint32_t i = 0; i <= count + 1; i++) {
        const char *kind = i < count    ? ts_node_type(ts_node_child(node, i))
                           : i == count ? ts_node_type(node)
                                        : "no_such_kind";
        if (!same_node(cbm_find_child_by_kind(node, kind),
                       cbm_c_reference_find_child_by_kind(node, kind))) {
            fprintf(stderr, "  cbm_find_child_by_kind(%s, %s) differs\n", ts_node_type(node),
                    kind);
            ok = 0;
        }
        static const int maxes[] = {-1, 0, 1, 2, 64};
        for (size_t m = 0; m < sizeof(maxes) / sizeof(maxes[0]); m++) {
            TSNode rout[64], cout[64];
            int rn = cbm_find_children_by_kind(node, kind, rout, maxes[m]);
            int cn = cbm_c_reference_find_children_by_kind(node, kind, cout, maxes[m]);
            int same = rn == cn;
            for (int k = 0; same && k < rn; k++) {
                same = same_node(rout[k], cout[k]);
            }
            if (!same) {
                fprintf(stderr, "  cbm_find_children_by_kind(%s, %s, %d) differs\n",
                        ts_node_type(node), kind, maxes[m]);
                ok = 0;
            }
        }
    }
    /* Every ancestor kind, at every depth up to past the root. */
    int depth = 0;
    for (TSNode p = ts_node_parent(node); !ts_node_is_null(p); p = ts_node_parent(p)) {
        depth++;
    }
    for (TSNode p = node; !ts_node_is_null(p); p = ts_node_parent(p)) {
        const char *kind = ts_node_type(p);
        for (int d = -1; d <= depth + 1; d++) {
            if (cbm_has_ancestor_kind(node, kind, d) !=
                cbm_c_reference_has_ancestor_kind(node, kind, d)) {
                fprintf(stderr, "  cbm_has_ancestor_kind(%s, %s, %d) differs\n",
                        ts_node_type(node), kind, d);
                ok = 0;
            }
        }
    }
    return ok;
}

TEST(node_helpers_tree_walk_matches_c_reference) {
    int nodes = 0;
    for (size_t s = 0; s < sizeof(k_sources) / sizeof(k_sources[0]); s++) {
        const HelperSource *src = &k_sources[s];
        TSParser *parser = ts_parser_new();
        ASSERT_NOT_NULL(parser);
        ASSERT_TRUE(ts_parser_set_language(parser, cbm_ts_language(src->lang)));
        TSTree *tree =
            ts_parser_parse_string(parser, NULL, src->source, (uint32_t)strlen(src->source));
        ASSERT_NOT_NULL(tree);
        CBMArena arena;
        cbm_arena_init(&arena);
        TSTreeCursor cursor = ts_tree_cursor_new(ts_tree_root_node(tree));
        CBMArena closed = {0};
        TSNode root = ts_tree_root_node(tree);
        ASSERT_NULL(cbm_node_text(&closed, root, src->source));
        ASSERT_NULL(cbm_c_reference_node_text(&closed, root, src->source));
        int ok = 1;
        for (;;) {
            ok &= compare_at(&arena, ts_tree_cursor_current_node(&cursor), src->source);
            nodes++;
            if (ts_tree_cursor_goto_first_child(&cursor)) {
                continue;
            }
            while (!ts_tree_cursor_goto_next_sibling(&cursor)) {
                if (!ts_tree_cursor_goto_parent(&cursor)) {
                    goto done;
                }
            }
        }
    done:
        ts_tree_cursor_delete(&cursor);
        cbm_arena_destroy(&arena);
        ts_tree_delete(tree);
        ts_parser_delete(parser);
        if (!ok) {
            fprintf(stderr, "  in source %zu (lang %d)\n", s, (int)src->lang);
        }
        ASSERT_TRUE(ok);
    }
    ASSERT_GT(nodes, 500);
    PASS();
}

#endif /* CBM_ENABLE_TEST_SEAMS */

SUITE(node_helpers) {
#ifdef CBM_ENABLE_TEST_SEAMS
    RUN_TEST(node_helpers_kind_predicates_match_c_reference);
    RUN_TEST(node_helpers_test_file_matches_c_reference);
    RUN_TEST(node_helpers_tree_walk_matches_c_reference);
#endif
}
