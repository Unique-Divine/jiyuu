/*
 * test_enrichment.c -- the decorator tags pass (src/pipeline/decorator_tags):
 * words that two or more decorated definitions share become tags.
 */
#include "test_framework.h"
#include "test_gbuf_dump.h"

#include "graph_buffer/graph_buffer.h"
#include "pipeline/pipeline_internal.h"

#include <stdio.h>
#include <string.h>

/* ── Rust parity ─────────────────────────────────────────────────── */

typedef struct {
    const char *label;
    const char *name;
    const char *props;
} en_node_t;

static cbm_gbuf_t *en_graph(const en_node_t *nodes, int count) {
    cbm_gbuf_t *gb = cbm_gbuf_new("p", "/tmp/p");
    for (int i = 0; i < count; i++) {
        char qn[128];
        char file[64];
        snprintf(qn, sizeof(qn), "p.mod.%s", nodes[i].name);
        snprintf(file, sizeof(file), "mod%d.py", i % 3);
        cbm_gbuf_upsert_node(gb, nodes[i].label, nodes[i].name, qn, file, i + 1, i + 2,
                             nodes[i].props);
    }
    return gb;
}

/* Runs the C and the Rust pass on two copies of one fixture: the same
 * return value and the same graph. */
static int en_compare(const en_node_t *nodes, int count, const char *what, int *tagged) {
    cbm_gbuf_t *c_gb = en_graph(nodes, count);
    cbm_gbuf_t *rust_gb = en_graph(nodes, count);
    int c_tagged = cbm_pipeline_pass_decorator_tags(c_gb, "p");
    int rust_tagged = mori_pipeline_pass_decorator_tags(rust_gb, "p");
    int rc = tg_dumps_differ(c_gb, rust_gb, what);
    if (c_tagged != rust_tagged) {
        printf("  %s: C tagged %d, Rust %d\n", what, c_tagged, rust_tagged);
        rc = 1;
    }
    *tagged = rust_tagged;
    cbm_gbuf_free(c_gb);
    cbm_gbuf_free(rust_gb);
    return rc;
}

/* The properties of `name` after the Rust pass on the whole fixture. */
static int en_props_are(const cbm_gbuf_t *gb, const char *name, const char *want) {
    char qn[128];
    snprintf(qn, sizeof(qn), "p.mod.%s", name);
    const cbm_gbuf_node_t *n = cbm_gbuf_find_by_qn(gb, qn);
    const char *got = n ? n->properties_json : "(no node)";
    if (strcmp(got, want) != 0) {
        printf("  %s: got  %s\n  %*s  want %s\n", name, got, (int)strlen(name), "", want);
        return 1;
    }
    return 0;
}

/* A decorator of `len` bytes: "@shared" and then `fill` repeated, so its
 * 255-byte cut falls where the caller chose. */
static void en_long_decorator(char *out, size_t cap, const char *fill, size_t len) {
    size_t pos = (size_t)snprintf(out, cap, "{\"decorators\":[\"@shared");
    size_t start = pos;
    size_t fl = strlen(fill);
    while (pos - start + 7 < len && pos + fl + 8 < cap) {
        memcpy(out + pos, fill, fl);
        pos += fl;
    }
    snprintf(out + pos, cap - pos, "\"]}");
}

TEST(decorator_tags_rust_matches_c) {
    /* Past the 255-byte cut: one ASCII word, and one that ends inside a
     * two-byte UTF-8 sequence (a tag yyjson cannot write). */
    static char long_ascii_a[1024];
    static char long_ascii_b[1024];
    static char long_utf8_a[1024];
    static char long_utf8_b[1024];
    en_long_decorator(long_ascii_a, sizeof(long_ascii_a), "ab", 400);
    en_long_decorator(long_ascii_b, sizeof(long_ascii_b), "ab", 400);
    en_long_decorator(long_utf8_a, sizeof(long_utf8_a), "x\xc3\xa9", 400);
    en_long_decorator(long_utf8_b, sizeof(long_utf8_b), "x\xc3\xa9", 400);
    static const char many_words[] =
        "{\"decorators\":[\"@aa.bb.cc.dd.ee.ff.gg.hh.ii.jj.kk.ll.mm.nn.oo.pp.qq.rr.ss.tt.uu.vv"
        ".ww.xx.yy.zz.ab.ac.ad.ae.af.ag.ah.ai.aj\"]}";
    static const char many_camel[] = "{\"decorators\":[\"@aaBbCcDdEeFfGgHhIiJjKkLlMmNnOoPpQqRr\"]}";
    const en_node_t nodes[] = {
        /* the Python fixture: login and required on two, cache on two */
        {"Function", "list_orders", "{\"decorators\":[\"@login_required\"]}"},
        {"Function", "get_order", "{\"decorators\":[\"@login_required\"],\"x\":1}"},
        {"Function", "compute_total", "{\"decorators\":[\"@cache\"]}"},
        {"Function", "compute_tax", "{\"decorators\":[\"@cache\",\"@cache\"]}"},
        {"Function", "special", "{\"decorators\":[\"@unique_helper\"]}"},
        /* the Java fixture */
        {"Method", "listOwners", "{\"decorators\":[\"@GetMapping(\\\"/owners\\\")\"]}"},
        {"Method", "createOwner", "{\"decorators\":[\"@PostMapping(\\\"/owners\\\")\"]}"},
        {"Method", "updateOwner",
         "{\"decorators\":[\"@Transactional\",\"@PostMapping(\\\"/owners/{id}\\\")\"]}"},
        /* attributes, stopwords, short words */
        {"Struct", "Point", "{\"decorators\":[\"#[derive(Debug, Clone)]\",\"#[test]\"]}"},
        {"Class", "Pair", "{\"decorators\":[\"#[derive(Debug)]\"]}"},
        {"Function", "route",
         "{\"decorators\":[\"@app.get(\\\"/api\\\")\",\"@x\",\"@router.post\"]}"},
        {"Function", "route2", "{\"decorators\":[\"@router.post\",\"@a:b-c/d\"]}"},
        /* existing tags (twice), other members kept in order, a nested object */
        {"Function", "retagged",
         "{\"decorator_tags\":[\"old\"],\"a\":{\"k\":[1,2]},\"decorators\":[\"@cache\"],"
         "\"decorator_tags\":7,\"z\":null}"},
        /* numbers and strings as yyjson reads and writes them */
        {"Function", "numbers",
         "{ \"decorators\" : [ \"@cache\" ] , \"n0\":-0, \"n1\":-0.0, \"n2\":1E2, \"n3\":1e-400,"
         "\"n4\":18446744073709551616, \"n5\":-9223372036854775808, \"n6\":0.1, \"n7\":1e21,"
         "\"n8\":123456.789, \"n9\":2.5e+3, \"s\":\"\\u00e9\\/\\u001f\\b\\f\\u2028\\ud83d\\ude00\","
         "\"t\":true, \"f\":false }"},
        {"Function", "nul_cut", "{\"decorators\":[\"@cache\\u0000Ignored\",\"@nul_cut\"]}"},
        /* the first decorators member counts; non-strings are skipped */
        {"Function", "dup_key", "{\"decorators\":[1,\"@cache\"],\"decorators\":[\"@login\"]}"},
        {"Function", "first_wrong", "{\"decorators\":\"@cache\",\"decorators\":[\"@cache\"]}"},
        {"Function", "only_numbers", "{\"decorators\":[1,2]}"},
        {"Function", "empty_list", "{\"decorators\":[]}"},
        /* invalid or non-object properties: never tagged */
        {"Function", "invalid", "{\"decorators\":[\"@cache\"]"},
        {"Function", "trailing", "{\"decorators\":[\"@cache\"]} x"},
        {"Function", "lone_surrogate", "{\"decorators\":[\"@cache\"],\"s\":\"\\ud800\"}"},
        {"Function", "root_array", "[{\"decorators\":[\"@cache\"]}]"},
        {"Function", "no_props", NULL},
        /* labels that are not tagged */
        {"Variable", "var", "{\"decorators\":[\"@cache\",\"@login_required\"]}"},
        {"Module", "mod", "{\"decorators\":[\"@cache\"]}"},
        /* long decorators, many words, many camel parts */
        {"Function", "long_ascii_a", long_ascii_a},
        {"Method", "long_ascii_b", long_ascii_b},
        {"Function", "long_utf8_a", long_utf8_a},
        {"Class", "long_utf8_b", long_utf8_b},
        {"Function", "many_words_a", many_words},
        {"Function", "many_words_b", many_words},
        {"Function", "many_camel_a", many_camel},
        {"Function", "many_camel_b", many_camel},
        /* raw UTF-8 and bytes past ASCII in words */
        {"Function", "utf8_a", "{\"decorators\":[\"@caf\xc3\xa9_r\xc3\xa9sum\xc3\xa9\"]}"},
        {"Function", "utf8_b", "{\"decorators\":[\"@Caf\xc3\xa9\"]}"},
    };
    int count = (int)(sizeof(nodes) / sizeof(nodes[0]));
    int tagged = 0;
    ASSERT_EQ(en_compare(nodes, count, "full fixture", &tagged), 0);
    ASSERT_EQ(tagged, 23);
    cbm_gbuf_t *full = en_graph(nodes, count);
    ASSERT_EQ(mori_pipeline_pass_decorator_tags(full, "p"), 23);
    int wrong = 0;
    wrong += en_props_are(full, "get_order",
                          "{\"decorators\":[\"@login_required\"],\"x\":1,\"decorator_tags\":["
                          "\"login\",\"required\"]}");
    wrong += en_props_are(full, "special", "{\"decorators\":[\"@unique_helper\"]}");
    wrong +=
        en_props_are(full, "updateOwner",
                     "{\"decorators\":[\"@Transactional\",\"@PostMapping(\\\"/owners/{id}\\\")\"],"
                     "\"decorator_tags\":[\"mapping\",\"post\"]}");
    wrong += en_props_are(full, "Point",
                          "{\"decorators\":[\"#[derive(Debug, Clone)]\",\"#[test]\"],"
                          "\"decorator_tags\":[\"derive\"]}");
    wrong += en_props_are(full, "route",
                          "{\"decorators\":[\"@app.get(\\\"/api\\\")\",\"@x\",\"@router.post\"],"
                          "\"decorator_tags\":[\"post\"]}");
    wrong += en_props_are(full, "retagged",
                          "{\"a\":{\"k\":[1,2]},\"decorators\":[\"@cache\"],\"z\":null,"
                          "\"decorator_tags\":[\"cache\"]}");
    wrong += en_props_are(
        full, "numbers",
        "{\"decorators\":[\"@cache\"],\"n0\":0,\"n1\":-0.0,\"n2\":100.0,\"n3\":0.0,"
        "\"n4\":18446744073709552000.0,\"n5\":-9223372036854775808,\"n6\":0.1,\"n7\":1e21,"
        "\"n8\":123456.789,\"n9\":2500.0,"
        "\"s\":\"\xc3\xa9/\\u001F\\b\\f\xe2\x80\xa8\xf0\x9f\x98\x80\",\"t\":true,\"f\":false,"
        "\"decorator_tags\":[\"cache\"]}");
    wrong += en_props_are(full, "nul_cut",
                          "{\"decorators\":[\"@cache\\u0000Ignored\",\"@nul_cut\"],"
                          "\"decorator_tags\":[\"cache\"]}");
    wrong += en_props_are(full, "dup_key",
                          "{\"decorators\":[1,\"@cache\"],\"decorators\":[\"@login\"],"
                          "\"decorator_tags\":[\"cache\"]}");
    wrong += en_props_are(full, "first_wrong",
                          "{\"decorators\":\"@cache\",\"decorators\":[\"@cache\"]}");
    wrong +=
        en_props_are(full, "lone_surrogate", "{\"decorators\":[\"@cache\"],\"s\":\"\\ud800\"}");
    wrong += en_props_are(full, "var", "{\"decorators\":[\"@cache\",\"@login_required\"]}");
    wrong += en_props_are(full, "long_utf8_a", long_utf8_a);
    wrong += en_props_are(
        full, "utf8_b", "{\"decorators\":[\"@Caf\xc3\xa9\"],\"decorator_tags\":[\"caf\xc3\xa9\"]}");
    wrong += en_props_are(
        full, "many_camel_a",
        "{\"decorators\":[\"@aaBbCcDdEeFfGgHhIiJjKkLlMmNnOoPpQqRr\"],\"decorator_tags\":[\"aa\","
        "\"bb\",\"cc\",\"dd\",\"ee\",\"ff\",\"gg\",\"hh\",\"ii\",\"jj\",\"kk\",\"ll\",\"mm\","
        "\"nn\","
        "\"oo\",\"pp\"]}");
    /* the shared word of the long ASCII decorators: its first 255 bytes */
    char want_long[1024];
    char word[300] = "shared";
    for (size_t i = strlen(word); i < 254; i += 2) {
        memcpy(word + i, "ab", 2);
    }
    word[254] = '\0';
    snprintf(want_long, sizeof(want_long), "%.*s,\"decorator_tags\":[\"%s\"]}",
             (int)strlen(long_ascii_a) - 1, long_ascii_a, word);
    wrong += en_props_are(full, "long_ascii_a", want_long);
    ASSERT_EQ(wrong, 0);
    cbm_gbuf_free(full);
    /* each prefix: the tag vocabulary changes as nodes join */
    for (int k = 0; k < count; k++) {
        char what[64];
        snprintf(what, sizeof(what), "first %d nodes", k);
        ASSERT_EQ(en_compare(nodes, k, what, &tagged), 0);
    }
    /* decorated, but no word shared: nothing tagged */
    const en_node_t lonely[] = {
        {"Function", "a", "{\"decorators\":[\"@alpha\"]}"},
        {"Function", "b", "{\"decorators\":[\"@beta\"]}"},
    };
    ASSERT_EQ(en_compare(lonely, 2, "no shared words", &tagged), 0);
    ASSERT_EQ(tagged, 0);
    /* NULL graph or project */
    cbm_gbuf_t *gb = en_graph(nodes, 4);
    ASSERT_EQ(cbm_pipeline_pass_decorator_tags(NULL, "p"), 0);
    ASSERT_EQ(mori_pipeline_pass_decorator_tags(NULL, "p"), 0);
    ASSERT_EQ(cbm_pipeline_pass_decorator_tags(gb, NULL), 0);
    ASSERT_EQ(mori_pipeline_pass_decorator_tags(gb, NULL), 0);
    cbm_gbuf_free(gb);
    PASS();
}

/* Running the pass twice changes nothing the second time. */
TEST(decorator_tags_rerun_rust_matches_c) {
    const en_node_t nodes[] = {
        {"Function", "a", "{\"decorators\":[\"@login_required\"]}"},
        {"Function", "b", "{\"decorators\":[\"@login_required\",\"@cache\"]}"},
        {"Method", "c", "{\"decorators\":[\"@cache\"]}"},
    };
    cbm_gbuf_t *c_gb = en_graph(nodes, 3);
    cbm_gbuf_t *rust_gb = en_graph(nodes, 3);
    ASSERT_EQ(cbm_pipeline_pass_decorator_tags(c_gb, "p"), 3);
    ASSERT_EQ(mori_pipeline_pass_decorator_tags(rust_gb, "p"), 3);
    ASSERT_EQ(cbm_pipeline_pass_decorator_tags(c_gb, "p"), 3);
    ASSERT_EQ(mori_pipeline_pass_decorator_tags(rust_gb, "p"), 3);
    ASSERT_EQ(tg_dumps_differ(c_gb, rust_gb, "second run"), 0);
    const cbm_gbuf_node_t *b = cbm_gbuf_find_by_qn(rust_gb, "p.mod.b");
    ASSERT_NOT_NULL(b);
    ASSERT_STR_EQ(b->properties_json, "{\"decorators\":[\"@login_required\",\"@cache\"],"
                                      "\"decorator_tags\":[\"cache\",\"login\",\"required\"]}");
    cbm_gbuf_free(c_gb);
    cbm_gbuf_free(rust_gb);
    PASS();
}

SUITE(enrichment) {
    RUN_TEST(decorator_tags_rust_matches_c);
    RUN_TEST(decorator_tags_rerun_rust_matches_c);
}
