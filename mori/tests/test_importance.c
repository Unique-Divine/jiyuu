/*
 * test_importance.c — index-time per-symbol importance score (importance.rs).
 *
 * Formula under test:
 *   importance = sqrt(num_refs) * priv * generic * distinct * test_penalty
 *
 * Three of these tests exist to catch failure modes that are SILENT — they
 * produce a wrong index with a fully green build:
 *
 *   - importance_pass_actually_runs_in_full_pipeline
 *       binds the predump registration. A hand-written PREDUMP_PASS_COUNT that
 *       lags the pass table skips the LAST-registered pass and nothing else
 *       notices. This test fails loudly if the score never lands.
 *   - importance_rehydrated_node_keeps_exactly_one_key
 *   - importance_second_index_run_keeps_exactly_one_key
 *       bind the idempotent write-back. The incremental path rehydrates nodes
 *       that already carry "importance"; a plain append yields
 *       {"importance":1.0,...,"importance":2.0} — property corruption, not a
 *       build failure.
 *   - importance_distinct_file_count_is_linear_in_group_size
 *       binds the cost shape: the distinct-file count must be computed once
 *       per distinct NAME, not once per node. The per-node shape is O(k^3) in
 *       a same-name group and is measurably disqualifying on real corpora.
 */
#include "test_framework.h"
#include "test_helpers.h"

#include "pipeline/pipeline.h"
#include "pipeline/pipeline_internal.h"
#include "graph_buffer/graph_buffer.h"
#include "store/store.h"
#include "foundation/compat.h"
#include "foundation/log.h"

#include <ctype.h>
#include <float.h>
#include <limits.h>
#include <math.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* ── Shared helpers ───────────────────────────────────────────────────── */

/* Parse the numeric value of "importance" out of a properties_json blob.
 * Returns a sentinel (-999.0) when the key is absent — callers that care about
 * presence assert on imp_key_count() instead. */
static double imp_value(const char *json) {
    if (!json) {
        return -999.0;
    }
    const char *p = strstr(json, "\"importance\":");
    if (!p) {
        return -999.0;
    }
    return strtod(p + strlen("\"importance\":"), NULL);
}

/* How many times the key appears. The duplicate-key corruption this suite
 * guards against shows up here as 2. */
static int imp_key_count(const char *json) {
    if (!json) {
        return 0;
    }
    int n = 0;
    const char *p = json;
    while ((p = strstr(p, "\"importance\":")) != NULL) {
        n++;
        p += strlen("\"importance\":");
    }
    return n;
}

static void imp_run_pass(cbm_gbuf_t *gb) {
    mori_pipeline_pass_importance(gb);
}

/* Every stored node of the given label must carry exactly `expect` copies of
 * the key. Returns the number of nodes checked, or -1 on a store error. */
static int imp_check_label(cbm_store_t *s, const char *project, const char *label, int expect) {
    cbm_node_t *nodes = NULL;
    int count = 0;
    if (cbm_store_find_nodes_by_label(s, project, label, &nodes, &count) != CBM_STORE_OK) {
        return -1;
    }
    int bad = 0;
    for (int i = 0; i < count; i++) {
        if (imp_key_count(nodes[i].properties_json) != expect) {
            bad++;
            printf("    %s node '%s' props=%s\n", label, nodes[i].name ? nodes[i].name : "?",
                   nodes[i].properties_json ? nodes[i].properties_json : "(null)");
        }
    }
    cbm_store_free_nodes(nodes, count);
    return bad == 0 ? count : -1;
}

/* ── Store-level score readout (for the closure-delta tests) ──────────── */

typedef struct {
    char **qns;
    double *scores;
    int count;
} ImpScoreSet;

static void imp_scores_free(ImpScoreSet *set) {
    for (int i = 0; i < set->count; i++) {
        free(set->qns[i]);
    }
    free(set->qns);
    free(set->scores);
    set->qns = NULL;
    set->scores = NULL;
    set->count = 0;
}

/* Collect (qualified_name -> importance) for every scored label in a store. */
static int imp_collect_scores(const char *db_path, const char *project, ImpScoreSet *out) {
    static const char *const kLabels[] = {"Function", "Method", "Class"};
    memset(out, 0, sizeof(*out));
    cbm_store_t *s = cbm_store_open_path(db_path);
    if (!s) {
        return -1;
    }
    int rc = 0;
    for (int li = 0; li < 3; li++) {
        cbm_node_t *nodes = NULL;
        int n = 0;
        if (cbm_store_find_nodes_by_label(s, project, kLabels[li], &nodes, &n) != CBM_STORE_OK) {
            rc = -1;
            break;
        }
        char **qns = realloc(out->qns, (size_t)(out->count + n) * sizeof(*qns));
        double *sc = realloc(out->scores, (size_t)(out->count + n) * sizeof(*sc));
        if (n > 0 && (!qns || !sc)) {
            cbm_store_free_nodes(nodes, n);
            rc = -1;
            break;
        }
        if (qns) {
            out->qns = qns;
        }
        if (sc) {
            out->scores = sc;
        }
        for (int i = 0; i < n; i++) {
            out->qns[out->count] = strdup(nodes[i].qualified_name ? nodes[i].qualified_name : "");
            out->scores[out->count] = imp_value(nodes[i].properties_json);
            out->count++;
        }
        cbm_store_free_nodes(nodes, n);
    }
    cbm_store_close(s);
    return rc;
}

static double imp_score_of(const ImpScoreSet *set, const char *qn_suffix) {
    for (int i = 0; i < set->count; i++) {
        size_t l = strlen(set->qns[i]);
        size_t k = strlen(qn_suffix);
        if (l >= k && strcmp(set->qns[i] + (l - k), qn_suffix) == 0) {
            return set->scores[i];
        }
    }
    return -999.0;
}

/* Write a fixture whose `helper` in hot.py is called from `callers` other
 * files — the inbound edges that only exist project-wide, never in a delta
 * buffer. `tag` lets the caller perturb hot.py to force a re-parse. */
static int imp_write_hot_fixture(const char *root, int callers, const char *tag) {
    char body[256];
    snprintf(body, sizeof(body), "%sdef helper():\n    return 1\n", tag);
    if (th_write_file(TH_PATH(root, "hot.py"), body) != 0) {
        return -1;
    }
    for (int i = 0; i < callers; i++) {
        char rel[64];
        char src[192];
        snprintf(rel, sizeof(rel), "c%d.py", i);
        snprintf(src, sizeof(src), "import hot\n\ndef use%d():\n    return hot.helper()\n", i);
        if (th_write_file(TH_PATH(root, rel), src) != 0) {
            return -1;
        }
    }
    return 0;
}

/* ── Registration: the pass must actually execute ─────────────────────── */

/* A miscounted PREDUMP_PASS_COUNT silently drops the last-registered pass.
 * The score is the only observable, so assert on the score. */
TEST(importance_pass_actually_runs_in_full_pipeline) {
    char *tmp = th_mktempdir("cbm_imp_run");
    if (!tmp) {
        FAIL("tmpdir");
    }
    char root[512];
    snprintf(root, sizeof(root), "%s", tmp); /* th_mktempdir returns a static buffer */

    ASSERT_EQ(th_write_file(TH_PATH(root, "main.py"),
                            "def helper():\n    return 1\n\n"
                            "def caller():\n    return helper() + helper()\n\n"
                            "class Widget:\n    def render(self):\n        return helper()\n"),
              0);

    char db_path[600];
    snprintf(db_path, sizeof(db_path), "%s/graph.db", root);
    cbm_pipeline_t *p = cbm_pipeline_new(root, db_path, CBM_MODE_FULL);
    ASSERT_NOT_NULL(p);
    ASSERT_EQ(cbm_pipeline_run(p), 0);
    char project[256];
    snprintf(project, sizeof(project), "%s", cbm_pipeline_project_name(p));
    cbm_pipeline_free(p);

    cbm_store_t *s = cbm_store_open_path(db_path);
    ASSERT_NOT_NULL(s);

    /* Non-vacuous: the corpus really produced symbols of each kind. */
    int fc = imp_check_label(s, project, "Function", 1);
    int mc = imp_check_label(s, project, "Method", 1);
    int cc = imp_check_label(s, project, "Class", 1);
    printf("    scored Function=%d Method=%d Class=%d\n", fc, mc, cc);
    ASSERT_GT(fc, 0);
    ASSERT_GT(mc, 0);
    ASSERT_GT(cc, 0);

    /* File nodes are deliberately excluded from scoring. */
    cbm_node_t *files = NULL;
    int filec = 0;
    ASSERT_EQ(cbm_store_find_nodes_by_label(s, project, "File", &files, &filec), CBM_STORE_OK);
    for (int i = 0; i < filec; i++) {
        ASSERT_EQ(imp_key_count(files[i].properties_json), 0);
    }
    cbm_store_free_nodes(files, filec);

    cbm_store_close(s);
    th_rmtree(root);
    PASS();
}

/* ── Formula ──────────────────────────────────────────────────────────── */

TEST(importance_base_is_sqrt_of_incoming_refs) {
    cbm_gbuf_t *gb = cbm_gbuf_new("test-proj", "/tmp/test");
    ASSERT_NOT_NULL(gb);

    int64_t target =
        cbm_gbuf_upsert_node(gb, "Function", "target", "pkg.target", "pkg/main.go", 1, 1, "{}");
    int64_t c1 = cbm_gbuf_upsert_node(gb, "Function", "c1", "pkg.c1", "pkg/main.go", 2, 2, "{}");
    int64_t c2 = cbm_gbuf_upsert_node(gb, "Function", "c2", "pkg.c2", "pkg/main.go", 3, 3, "{}");
    int64_t c3 = cbm_gbuf_upsert_node(gb, "Function", "c3", "pkg.c3", "pkg/main.go", 4, 4, "{}");
    int64_t lonely =
        cbm_gbuf_upsert_node(gb, "Function", "lonely", "pkg.lonely", "pkg/main.go", 5, 5, "{}");
    ASSERT_GT(target, 0);
    ASSERT_GT(lonely, 0);

    cbm_gbuf_insert_edge(gb, c1, target, "CALLS", "{}");
    cbm_gbuf_insert_edge(gb, c2, target, "CALLS", "{}");
    cbm_gbuf_insert_edge(gb, c3, target, "USAGE", "{}"); /* spans BOTH edge types */

    imp_run_pass(gb);

    const cbm_gbuf_node_t *tn = cbm_gbuf_find_by_id(gb, target);
    ASSERT_NOT_NULL(tn);
    ASSERT_EQ(imp_key_count(tn->properties_json), 1);
    ASSERT_FLOAT_EQ(imp_value(tn->properties_json), sqrt(3.0), 1e-6);

    /* num_refs == 0 -> sqrt(0) == 0, still written. */
    const cbm_gbuf_node_t *ln = cbm_gbuf_find_by_id(gb, lonely);
    ASSERT_NOT_NULL(ln);
    ASSERT_EQ(imp_key_count(ln->properties_json), 1);
    ASSERT_FLOAT_EQ(imp_value(ln->properties_json), 0.0, 1e-9);

    cbm_gbuf_free(gb);
    PASS();
}

TEST(importance_private_names_are_demoted) {
    cbm_gbuf_t *gb = cbm_gbuf_new("test-proj", "/tmp/test");
    ASSERT_NOT_NULL(gb);

    int64_t priv =
        cbm_gbuf_upsert_node(gb, "Function", "_run", "pkg._run", "pkg/main.go", 1, 1, "{}");
    int64_t pub = cbm_gbuf_upsert_node(gb, "Function", "run", "pkg.run", "pkg/main.go", 2, 2, "{}");
    for (int i = 0; i < 4; i++) {
        char name[CBM_SZ_32];
        char qn[CBM_SZ_64];
        snprintf(name, sizeof(name), "p%d", i);
        snprintf(qn, sizeof(qn), "pkg.p%d", i);
        int64_t c =
            cbm_gbuf_upsert_node(gb, "Function", name, qn, "pkg/main.go", 10 + i, 10 + i, "{}");
        cbm_gbuf_insert_edge(gb, c, priv, "CALLS", "{}");
        snprintf(name, sizeof(name), "q%d", i);
        snprintf(qn, sizeof(qn), "pkg.q%d", i);
        c = cbm_gbuf_upsert_node(gb, "Function", name, qn, "pkg/main.go", 20 + i, 20 + i, "{}");
        cbm_gbuf_insert_edge(gb, c, pub, "CALLS", "{}");
    }

    imp_run_pass(gb);

    ASSERT_FLOAT_EQ(imp_value(cbm_gbuf_find_by_id(gb, priv)->properties_json), sqrt(4.0) * 0.1,
                    1e-6);
    ASSERT_FLOAT_EQ(imp_value(cbm_gbuf_find_by_id(gb, pub)->properties_json), sqrt(4.0), 1e-6);

    cbm_gbuf_free(gb);
    PASS();
}

/* Generic = defined in >= 5 DISTINCT FILES. Two defs of one name in the same
 * file count once, so the boundary is tested on files, not on node count. */
TEST(importance_generic_names_are_demoted_on_distinct_files) {
    cbm_gbuf_t *gb = cbm_gbuf_new("test-proj", "/tmp/test");
    ASSERT_NOT_NULL(gb);

    /* "generic": 5 distinct files -> demoted. */
    int64_t generic = 0;
    for (int i = 0; i < 5; i++) {
        char qn[CBM_SZ_64];
        char file[CBM_SZ_64];
        snprintf(qn, sizeof(qn), "pkg%d.generic", i);
        snprintf(file, sizeof(file), "pkg%d/main.go", i);
        int64_t id = cbm_gbuf_upsert_node(gb, "Function", "generic", qn, file, 1, 1, "{}");
        if (i == 0) {
            generic = id;
        }
    }
    /* "narrow": 4 distinct files, but 6 nodes — two duplicates share a file,
     * so the DISTINCT count is 4 and it stays undemoted. */
    int64_t narrow = 0;
    for (int i = 0; i < 4; i++) {
        char qn[CBM_SZ_64];
        char file[CBM_SZ_64];
        snprintf(qn, sizeof(qn), "n%d.narrow", i);
        snprintf(file, sizeof(file), "n%d/main.go", i);
        int64_t id = cbm_gbuf_upsert_node(gb, "Function", "narrow", qn, file, 1, 1, "{}");
        if (i == 0) {
            narrow = id;
        }
    }
    cbm_gbuf_upsert_node(gb, "Function", "narrow", "n0.narrow_b", "n0/main.go", 9, 9, "{}");
    cbm_gbuf_upsert_node(gb, "Function", "narrow", "n1.narrow_b", "n1/main.go", 9, 9, "{}");

    for (int i = 0; i < 9; i++) {
        char name[CBM_SZ_32];
        char qn[CBM_SZ_64];
        snprintf(name, sizeof(name), "z%d", i);
        snprintf(qn, sizeof(qn), "z.z%d", i);
        int64_t c = cbm_gbuf_upsert_node(gb, "Function", name, qn, "z/main.go", i, i, "{}");
        cbm_gbuf_insert_edge(gb, c, i < 4 ? generic : narrow, "CALLS", "{}");
    }

    imp_run_pass(gb);

    /* generic: sqrt(4) * 0.1;  narrow: sqrt(5), undemoted. */
    ASSERT_FLOAT_EQ(imp_value(cbm_gbuf_find_by_id(gb, generic)->properties_json), sqrt(4.0) * 0.1,
                    1e-6);
    ASSERT_FLOAT_EQ(imp_value(cbm_gbuf_find_by_id(gb, narrow)->properties_json), sqrt(5.0), 1e-6);

    cbm_gbuf_free(gb);
    PASS();
}

TEST(importance_distinctive_names_are_promoted) {
    cbm_gbuf_t *gb = cbm_gbuf_new("test-proj", "/tmp/test");
    ASSERT_NOT_NULL(gb);

    /* snake_case >= 8, camelCase >= 8, and a plain long lowercase word that is
     * NOT distinctive (no '_', no hump). */
    int64_t snake = cbm_gbuf_upsert_node(gb, "Function", "load_user_profile",
                                         "pkg.load_user_profile", "pkg/a.go", 1, 1, "{}");
    int64_t camel = cbm_gbuf_upsert_node(gb, "Function", "loadUserProfile", "pkg.loadUserProfile",
                                         "pkg/b.go", 1, 1, "{}");
    int64_t plain =
        cbm_gbuf_upsert_node(gb, "Function", "aggregate", "pkg.aggregate", "pkg/c.go", 1, 1, "{}");
    int64_t shortnm =
        cbm_gbuf_upsert_node(gb, "Function", "get_x", "pkg.get_x", "pkg/d.go", 1, 1, "{}");

    const int64_t targets[] = {snake, camel, plain, shortnm};
    for (int t = 0; t < 4; t++) {
        char name[CBM_SZ_32];
        char qn[CBM_SZ_64];
        snprintf(name, sizeof(name), "cc%d", t);
        snprintf(qn, sizeof(qn), "z.cc%d", t);
        int64_t c = cbm_gbuf_upsert_node(gb, "Function", name, qn, "z/main.go", t, t, "{}");
        cbm_gbuf_insert_edge(gb, c, targets[t], "CALLS", "{}");
    }

    imp_run_pass(gb);

    ASSERT_FLOAT_EQ(imp_value(cbm_gbuf_find_by_id(gb, snake)->properties_json), 1.0 * 10.0, 1e-6);
    ASSERT_FLOAT_EQ(imp_value(cbm_gbuf_find_by_id(gb, camel)->properties_json), 1.0 * 10.0, 1e-6);
    ASSERT_FLOAT_EQ(imp_value(cbm_gbuf_find_by_id(gb, plain)->properties_json), 1.0, 1e-6);
    ASSERT_FLOAT_EQ(imp_value(cbm_gbuf_find_by_id(gb, shortnm)->properties_json), 1.0, 1e-6);

    cbm_gbuf_free(gb);
    PASS();
}

/* Test scaffolding is demoted two ways: living in a test file (the graph's own
 * cbm_is_test_path classifier) or being the target of a TESTS edge. */
TEST(importance_test_scaffolding_is_demoted) {
    cbm_gbuf_t *gb = cbm_gbuf_new("test-proj", "/tmp/test");
    ASSERT_NOT_NULL(gb);

    int64_t in_test_file = cbm_gbuf_upsert_node(gb, "Function", "fixture", "t.fixture",
                                                "pkg/thing_test.go", 1, 1, "{}");
    int64_t tests_target =
        cbm_gbuf_upsert_node(gb, "Function", "helper", "pkg.helper", "pkg/util.go", 1, 1, "{}");
    int64_t plain =
        cbm_gbuf_upsert_node(gb, "Function", "worker", "pkg.worker", "pkg/util.go", 2, 2, "{}");
    /* A production file whose NAME merely contains "test" must NOT be demoted. */
    int64_t testutil = cbm_gbuf_upsert_node(gb, "Function", "spawn", "pkg.spawn",
                                            "pkg/testutil_helpers.go", 1, 1, "{}");

    const int64_t targets[] = {in_test_file, tests_target, plain, testutil};
    for (int t = 0; t < 4; t++) {
        char name[CBM_SZ_32];
        char qn[CBM_SZ_64];
        snprintf(name, sizeof(name), "d%d", t);
        snprintf(qn, sizeof(qn), "z.d%d", t);
        int64_t c = cbm_gbuf_upsert_node(gb, "Function", name, qn, "z/main.go", t, t, "{}");
        cbm_gbuf_insert_edge(gb, c, targets[t], "CALLS", "{}");
    }
    int64_t tfn = cbm_gbuf_upsert_node(gb, "Function", "TestHelper", "t.TestHelper",
                                       "pkg/util_test.go", 5, 5, "{}");
    cbm_gbuf_insert_edge(gb, tfn, tests_target, "TESTS", "{}");

    imp_run_pass(gb);

    ASSERT_FLOAT_EQ(imp_value(cbm_gbuf_find_by_id(gb, in_test_file)->properties_json), 1.0 * 0.1,
                    1e-6);
    ASSERT_FLOAT_EQ(imp_value(cbm_gbuf_find_by_id(gb, tests_target)->properties_json), 1.0 * 0.1,
                    1e-6);
    ASSERT_FLOAT_EQ(imp_value(cbm_gbuf_find_by_id(gb, plain)->properties_json), 1.0, 1e-6);
    ASSERT_FLOAT_EQ(imp_value(cbm_gbuf_find_by_id(gb, testutil)->properties_json), 1.0, 1e-6);

    cbm_gbuf_free(gb);
    PASS();
}

/* ── Idempotent write-back ────────────────────────────────────────────── */

TEST(importance_append_prop_overwrites_instead_of_duplicating) {
    cbm_gbuf_t *gb = cbm_gbuf_new("test-proj", "/tmp/test");
    ASSERT_NOT_NULL(gb);
    int64_t id = cbm_gbuf_upsert_node(gb, "Function", "f", "p.f", "p/a.go", 1, 1,
                                      "{\"loop_depth\":2,\"recursive\":false}");
    cbm_gbuf_node_t *n = (cbm_gbuf_node_t *)cbm_gbuf_find_by_id(gb, id);
    ASSERT_NOT_NULL(n);

    cbm_pipeline_importance_append_prop(n, 1.5);
    ASSERT_EQ(imp_key_count(n->properties_json), 1);
    ASSERT_FLOAT_EQ(imp_value(n->properties_json), 1.5, 1e-6);

    cbm_pipeline_importance_append_prop(n, 42.25);
    printf("    after second write: %s\n", n->properties_json);
    ASSERT_EQ(imp_key_count(n->properties_json), 1);
    ASSERT_FLOAT_EQ(imp_value(n->properties_json), 42.25, 1e-6);

    /* Sibling keys survive the in-place overwrite, on both sides of the key. */
    ASSERT_TRUE(strstr(n->properties_json, "\"loop_depth\":2") != NULL);
    ASSERT_TRUE(strstr(n->properties_json, "\"recursive\":false") != NULL);

    /* A key that is NOT last: overwrite must not truncate the tail. */
    free(n->properties_json);
    n->properties_json = strdup("{\"importance\":0.000000,\"tail\":7}");
    ASSERT_NOT_NULL(n->properties_json);
    cbm_pipeline_importance_append_prop(n, 3.0);
    ASSERT_EQ(imp_key_count(n->properties_json), 1);
    ASSERT_FLOAT_EQ(imp_value(n->properties_json), 3.0, 1e-6);
    ASSERT_TRUE(strstr(n->properties_json, "\"tail\":7") != NULL);

    /* A string VALUE that merely contains the key text is not a key. */
    free(n->properties_json);
    n->properties_json = strdup("{\"doc\":\"see \\\"importance\\\": below\"}");
    ASSERT_NOT_NULL(n->properties_json);
    cbm_pipeline_importance_append_prop(n, 2.0);
    ASSERT_FLOAT_EQ(imp_value(n->properties_json), 2.0, 1e-6);
    ASSERT_TRUE(strstr(n->properties_json, "\"doc\":") != NULL);

    /* Empty object and a non-object blob. */
    free(n->properties_json);
    n->properties_json = strdup("{}");
    ASSERT_NOT_NULL(n->properties_json);
    cbm_pipeline_importance_append_prop(n, 0.5);
    ASSERT_STR_EQ(n->properties_json, "{\"importance\":0.500000}");

    free(n->properties_json);
    n->properties_json = strdup("not json");
    ASSERT_NOT_NULL(n->properties_json);
    cbm_pipeline_importance_append_prop(n, 9.0);
    ASSERT_STR_EQ(n->properties_json, "not json");

    cbm_gbuf_free(gb);
    PASS();
}

/* Running the pass twice over the same graph — the shape the incremental path
 * produces when it re-scores an already-scored rehydrated buffer. */
TEST(importance_pass_is_idempotent_across_reruns) {
    cbm_gbuf_t *gb = cbm_gbuf_new("test-proj", "/tmp/test");
    ASSERT_NOT_NULL(gb);
    int64_t t =
        cbm_gbuf_upsert_node(gb, "Function", "target", "pkg.target", "pkg/main.go", 1, 1, "{}");
    int64_t c =
        cbm_gbuf_upsert_node(gb, "Function", "caller", "pkg.caller", "pkg/main.go", 2, 2, "{}");
    cbm_gbuf_insert_edge(gb, c, t, "CALLS", "{}");

    imp_run_pass(gb);
    char first[CBM_SZ_512];
    snprintf(first, sizeof(first), "%s", cbm_gbuf_find_by_id(gb, t)->properties_json);
    imp_run_pass(gb);
    const char *second = cbm_gbuf_find_by_id(gb, t)->properties_json;

    printf("    run1=%s run2=%s\n", first, second);
    ASSERT_EQ(imp_key_count(second), 1);
    ASSERT_STR_EQ(first, second); /* re-scoring is a fixed point */

    cbm_gbuf_free(gb);
    PASS();
}

/* The literal incremental shape: index, rehydrate the stored graph exactly as
 * pipeline_incremental.c does, re-run the pass. Nodes arrive already carrying
 * the key — an append-only write-back yields two. */
TEST(importance_rehydrated_node_keeps_exactly_one_key) {
    char *tmp = th_mktempdir("cbm_imp_rehy");
    if (!tmp) {
        FAIL("tmpdir");
    }
    char root[512];
    snprintf(root, sizeof(root), "%s", tmp); /* th_mktempdir returns a static buffer */

    ASSERT_EQ(th_write_file(TH_PATH(root, "main.py"),
                            "def helper():\n    return 1\n\n"
                            "def caller():\n    return helper() + helper()\n"),
              0);
    char db_path[600];
    snprintf(db_path, sizeof(db_path), "%s/graph.db", root);
    cbm_pipeline_t *p = cbm_pipeline_new(root, db_path, CBM_MODE_FULL);
    ASSERT_NOT_NULL(p);
    ASSERT_EQ(cbm_pipeline_run(p), 0);
    char project[256];
    snprintf(project, sizeof(project), "%s", cbm_pipeline_project_name(p));
    cbm_pipeline_free(p);

    cbm_gbuf_t *gb = cbm_gbuf_new(project, root);
    ASSERT_NOT_NULL(gb);
    ASSERT_EQ(cbm_gbuf_load_from_db(gb, db_path, project), 0);

    /* Precondition: the rehydrated nodes already carry the key. Without this
     * the test could pass while proving nothing. */
    const cbm_gbuf_node_t **fns = NULL;
    int fc = 0;
    ASSERT_EQ(cbm_gbuf_find_by_label(gb, "Function", &fns, &fc), 0);
    ASSERT_GT(fc, 0);
    for (int i = 0; i < fc; i++) {
        ASSERT_EQ(imp_key_count(fns[i]->properties_json), 1);
    }

    imp_run_pass(gb);

    ASSERT_EQ(cbm_gbuf_find_by_label(gb, "Function", &fns, &fc), 0);
    ASSERT_GT(fc, 0);
    for (int i = 0; i < fc; i++) {
        if (imp_key_count(fns[i]->properties_json) != 1) {
            printf("    duplicate key on '%s': %s\n", fns[i]->name, fns[i]->properties_json);
        }
        ASSERT_EQ(imp_key_count(fns[i]->properties_json), 1);
    }

    cbm_gbuf_free(gb);
    th_rmtree(root);
    PASS();
}

/* Pass-timing observer: which importance-pass call sites fired. */
static atomic_int g_saw_incr_importance = 0;
static atomic_int g_saw_full_importance = 0;
static void imp_pass_sink(const char *line) {
    if (!line || !strstr(line, "pass.timing")) {
        return;
    }
    if (strstr(line, "pass=incr_importance")) {
        atomic_fetch_add_explicit(&g_saw_incr_importance, 1, memory_order_relaxed);
    } else if (strstr(line, "pass=importance")) {
        atomic_fetch_add_explicit(&g_saw_full_importance, 1, memory_order_relaxed);
    }
}

/* End-to-end, on the incremental route that actually rehydrates the stored
 * graph (LEGACY_PARTIAL — production's other incremental route, closure
 * repair, works on a delta buffer and never sees pre-scored nodes). Index,
 * edit a file, index again: the run must be observed on that route, the pass
 * must have run on it, and every stored symbol must carry exactly one key. An
 * append-only write-back produces two here. */
TEST(importance_second_index_run_keeps_exactly_one_key) {
    char *tmp = th_mktempdir("cbm_imp_incr");
    if (!tmp) {
        FAIL("tmpdir");
    }
    char root[512];
    snprintf(root, sizeof(root), "%s", tmp); /* th_mktempdir returns a static buffer */

    ASSERT_EQ(th_write_file(TH_PATH(root, "a.py"), "def helper():\n    return 1\n"), 0);
    ASSERT_EQ(
        th_write_file(TH_PATH(root, "b.py"), "import a\n\ndef caller():\n    return a.helper()\n"),
        0);

    char db_path[600];
    snprintf(db_path, sizeof(db_path), "%s/graph.db", root);

    atomic_store(&g_saw_incr_importance, 0);
    atomic_store(&g_saw_full_importance, 0);
    cbm_log_set_sink_ex(imp_pass_sink, CBM_LOG_SINK_TEE);

    cbm_pipeline_t *p1 = cbm_pipeline_new(root, db_path, CBM_MODE_FULL);
    ASSERT_NOT_NULL(p1);
    ASSERT_EQ(cbm_pipeline_run(p1), 0);
    char project[256];
    snprintf(project, sizeof(project), "%s", cbm_pipeline_project_name(p1));
    cbm_pipeline_free(p1);

    /* Edit one file so the second run has real work but stays incremental. */
    ASSERT_EQ(th_write_file(TH_PATH(root, "b.py"),
                            "import a\n\ndef caller():\n    return a.helper() + a.helper()\n"),
              0);

    cbm_pipeline_incremental_test_force_legacy_partial_once();
    cbm_pipeline_t *p2 = cbm_pipeline_new(root, db_path, CBM_MODE_FULL);
    ASSERT_NOT_NULL(p2);
    ASSERT_EQ(cbm_pipeline_run(p2), 0);
    cbm_incremental_route_t route = cbm_pipeline_incremental_test_last_route();
    cbm_pipeline_free(p2);
    cbm_log_set_sink(NULL);

    /* Route proof: without it a silent fall-back to a full reindex would make
     * the duplicate-key assertion below vacuous. */
    ASSERT_EQ((int)route, (int)CBM_INCREMENTAL_ROUTE_LEGACY_PARTIAL);

    int incr = atomic_load(&g_saw_incr_importance);
    int full = atomic_load(&g_saw_full_importance);
    printf("    importance pass runs: full=%d incremental=%d\n", full, incr);
    ASSERT_GT(full, 0); /* first run scored via the predump table */
    /* The second run must have re-scored too — through whichever path it took.
     * If it was NOT incremental the duplicate-key trap is untested, so say so
     * rather than passing quietly. */
    if (incr == 0) {
        th_rmtree(root);
        FAIL("second run did not re-score on the incremental path — trap untested");
    }

    cbm_store_t *s = cbm_store_open_path(db_path);
    ASSERT_NOT_NULL(s);
    int fc = imp_check_label(s, project, "Function", 1);
    printf("    functions with exactly one importance key: %d\n", fc);
    ASSERT_GT(fc, 0);
    cbm_store_close(s);

    th_rmtree(root);
    PASS();
}

/* ── Closure-delta route: the graph is only complete in the store ─────── */

/* REGRESSION. `helper` lives in hot.py and is called from 25 other files. Edit
 * hot.py and re-index: the closure-delta route purges hot.py's rows, rebuilds
 * them from a PROXY buffer that holds no project-wide edges, and persists what
 * it computed. Scoring off that buffer sees in-degree 0 and stores 0.0 for a
 * symbol with 25 real callers. The fix rescores in SQL after cbm_delta_patch
 * has re-linked the snapshotted inbound edges. */
TEST(importance_closure_delta_keeps_referenced_symbol_sane) {
    enum { CALLERS = 25 };
    char *tmp = th_mktempdir("cbm_imp_delta");
    if (!tmp) {
        FAIL("tmpdir");
    }
    char root[512];
    snprintf(root, sizeof(root), "%s", tmp);
    char *tmp2 = th_mktempdir("cbm_imp_deltadb");
    if (!tmp2) {
        th_rmtree(root);
        FAIL("tmpdir2");
    }
    char dbdir[512];
    snprintf(dbdir, sizeof(dbdir), "%s", tmp2);

    ASSERT_EQ(imp_write_hot_fixture(root, CALLERS, ""), 0);

    char db_path[600];
    snprintf(db_path, sizeof(db_path), "%s/graph.db", dbdir);
    cbm_pipeline_t *p1 = cbm_pipeline_new(root, db_path, CBM_MODE_FULL);
    ASSERT_NOT_NULL(p1);
    ASSERT_EQ(cbm_pipeline_run(p1), 0);
    char project[256];
    snprintf(project, sizeof(project), "%s", cbm_pipeline_project_name(p1));
    cbm_pipeline_free(p1);

    ImpScoreSet before = {0};
    ASSERT_EQ(imp_collect_scores(db_path, project, &before), 0);
    double full_score = imp_score_of(&before, "helper");
    printf("    full-index helper score %.6f (expect sqrt(%d) = %.6f)\n", full_score, CALLERS,
           sqrt((double)CALLERS));
    /* Non-vacuous: the callers really produced edges. If this is 0 the fixture
     * stopped exercising the bug and the test below would prove nothing. */
    ASSERT_TRUE(full_score > 1.0);
    imp_scores_free(&before);

    /* Touch hot.py so the delta route has exactly one file to repair. */
    ASSERT_EQ(imp_write_hot_fixture(root, CALLERS, "# touched\n"), 0);

    uint64_t sql0 = atomic_load_explicit(&g_importance_store_rows, memory_order_relaxed);
    cbm_pipeline_t *p2 = cbm_pipeline_new(root, db_path, CBM_MODE_FULL);
    ASSERT_NOT_NULL(p2);
    ASSERT_EQ(cbm_pipeline_run(p2), 0);
    cbm_incremental_route_t route = cbm_pipeline_incremental_test_last_route();
    cbm_pipeline_free(p2);
    uint64_t sql_rows = atomic_load_explicit(&g_importance_store_rows, memory_order_relaxed) - sql0;

    /* Route proof — without it a fall-back to a full reindex would repair the
     * score for the wrong reason and the test would pass vacuously. */
    printf("    route=%d (closure_repair=%d)  sql_rescored_rows=%llu\n", (int)route,
           (int)CBM_INCREMENTAL_ROUTE_CLOSURE_REPAIR, (unsigned long long)sql_rows);
    ASSERT_EQ((int)route, (int)CBM_INCREMENTAL_ROUTE_CLOSURE_REPAIR);

    ImpScoreSet after = {0};
    ASSERT_EQ(imp_collect_scores(db_path, project, &after), 0);
    double delta_score = imp_score_of(&after, "helper");
    printf("    after closure-delta helper score %.6f\n", delta_score);
    imp_scores_free(&after);

    th_rmtree(root);
    th_rmtree(dbdir);
    /* THE SYMPTOM, asserted before any mechanism guard: the graph is unchanged,
     * so the score must be unchanged. Scoring off the proxy buffer yields 0.0. */
    ASSERT_FLOAT_EQ(delta_score, full_score, 1e-6);
    /* Mechanism guard second, so a regression fails on the wrong score rather
     * than on bookkeeping. */
    ASSERT_GT((long long)sql_rows, 0);
    PASS();
}

/* EQUIVALENCE. A full index and an incremental index that converge on the same
 * tree must produce the same scores — for every symbol, not just the hot one.
 * Same directory and therefore the same project name and qualified names, so
 * the two score maps are directly comparable. */
TEST(importance_full_and_incremental_converge_on_same_scores) {
    enum { CALLERS = 12 };
    char *tmp = th_mktempdir("cbm_imp_equiv");
    if (!tmp) {
        FAIL("tmpdir");
    }
    char root[512];
    snprintf(root, sizeof(root), "%s", tmp);
    char *tmp2 = th_mktempdir("cbm_imp_equivdb");
    if (!tmp2) {
        th_rmtree(root);
        FAIL("tmpdir2");
    }
    char dbdir[512];
    snprintf(dbdir, sizeof(dbdir), "%s", tmp2);

    /* v1: every caller but c0 calls helper. c0 imports hot but does not. */
    ASSERT_EQ(imp_write_hot_fixture(root, CALLERS, ""), 0);
    ASSERT_EQ(th_write_file(TH_PATH(root, "c0.py"), "import hot\n\ndef use0():\n    return 1\n"),
              0);
    ASSERT_EQ(th_write_file(TH_PATH(root, "extra.py"),
                            "import hot\n\ndef load_user_profile():\n    return hot.helper()\n"),
              0);

    char db_incr[600];
    snprintf(db_incr, sizeof(db_incr), "%s/incr.db", dbdir);
    cbm_pipeline_t *p1 = cbm_pipeline_new(root, db_incr, CBM_MODE_FULL);
    ASSERT_NOT_NULL(p1);
    ASSERT_EQ(cbm_pipeline_run(p1), 0);
    char project[256];
    snprintf(project, sizeof(project), "%s", cbm_pipeline_project_name(p1));
    cbm_pipeline_free(p1);

    /* v2: c0's BODY starts calling helper. Body-only, so c0's surface is
     * unchanged and the closure planner accepts the delta — but the in-degree
     * of `helper`, which lives in the UNCHANGED hot.py, goes up by one. That
     * is the case a changed-files-only rescore gets wrong: the symbol whose
     * score moved is not in the changed set at all. */
    ASSERT_EQ(th_write_file(TH_PATH(root, "c0.py"),
                            "import hot\n\ndef use0():\n    return hot.helper()\n"),
              0);

    uint64_t sql0 = atomic_load_explicit(&g_importance_store_rows, memory_order_relaxed);
    cbm_pipeline_t *p2 = cbm_pipeline_new(root, db_incr, CBM_MODE_FULL);
    ASSERT_NOT_NULL(p2);
    ASSERT_EQ(cbm_pipeline_run(p2), 0);
    cbm_incremental_route_t route = cbm_pipeline_incremental_test_last_route();
    cbm_pipeline_free(p2);
    uint64_t sql_rows = atomic_load_explicit(&g_importance_store_rows, memory_order_relaxed) - sql0;
    printf("    route=%d (closure_repair=%d)  sql_rescored_rows=%llu\n", (int)route,
           (int)CBM_INCREMENTAL_ROUTE_CLOSURE_REPAIR, (unsigned long long)sql_rows);
    ASSERT_EQ((int)route, (int)CBM_INCREMENTAL_ROUTE_CLOSURE_REPAIR);

    /* Same tree, indexed cold into a separate database. */
    char db_full[600];
    snprintf(db_full, sizeof(db_full), "%s/full.db", dbdir);
    cbm_pipeline_t *p3 = cbm_pipeline_new(root, db_full, CBM_MODE_FULL);
    ASSERT_NOT_NULL(p3);
    ASSERT_EQ(cbm_pipeline_run(p3), 0);
    cbm_pipeline_free(p3);

    ImpScoreSet incr = {0};
    ImpScoreSet full = {0};
    ASSERT_EQ(imp_collect_scores(db_incr, project, &incr), 0);
    ASSERT_EQ(imp_collect_scores(db_full, project, &full), 0);
    printf("    scored symbols: incremental=%d full=%d\n", incr.count, full.count);

    int mismatches = 0;
    for (int i = 0; i < full.count; i++) {
        double a = full.scores[i];
        double b = imp_score_of(&incr, full.qns[i]);
        if (!(fabs(a - b) < 1e-6)) {
            mismatches++;
            if (mismatches <= 5) {
                printf("    MISMATCH %s: full=%.6f incremental=%.6f\n", full.qns[i], a, b);
            }
        }
    }
    int count_full = full.count;
    int count_incr = incr.count;
    imp_scores_free(&incr);
    imp_scores_free(&full);
    th_rmtree(root);
    th_rmtree(dbdir);

    ASSERT_GT(count_full, 5); /* non-vacuous: there really are symbols to compare */
    ASSERT_EQ(count_incr, count_full);
    ASSERT_EQ(mismatches, 0);          /* the symptom */
    ASSERT_GT((long long)sql_rows, 0); /* mechanism guard, asserted second */
    PASS();
}

/* ── Cost shape ───────────────────────────────────────────────────────── */

/* The distinct-file count must be computed once per distinct NAME. Computing
 * it per NODE makes a same-name group of size k cost O(k^3) — measurably
 * disqualifying on real corpora. Assert on the pass's own work counter, not
 * on wall time: doubling a same-name group must roughly double the work. */
TEST(importance_distinct_file_count_is_linear_in_group_size) {
    enum { K = 200 };

    uint64_t visits[2];
    for (int leg = 0; leg < 2; leg++) {
        int k = (leg == 0) ? K : 2 * K;
        cbm_gbuf_t *gb = cbm_gbuf_new("test-proj", "/tmp/test");
        ASSERT_NOT_NULL(gb);
        for (int i = 0; i < k; i++) {
            char qn[CBM_SZ_64];
            char file[CBM_SZ_64];
            snprintf(qn, sizeof(qn), "pkg%d.toString", i);
            snprintf(file, sizeof(file), "pkg%d/a.java", i);
            ASSERT_GT(cbm_gbuf_upsert_node(gb, "Method", "toString", qn, file, 1, 1, "{}"), 0);
        }
        uint64_t v0 = atomic_load_explicit(&g_importance_name_visits, memory_order_relaxed);
        imp_run_pass(gb);
        visits[leg] = atomic_load_explicit(&g_importance_name_visits, memory_order_relaxed) - v0;
        cbm_gbuf_free(gb);
    }

    double ratio = (visits[0] > 0) ? (double)visits[1] / (double)visits[0] : 0.0;
    printf("    name visits k=%d -> %llu, k=%d -> %llu (ratio %.2f; linear ~2, per-node ~4)\n", K,
           (unsigned long long)visits[0], 2 * K, (unsigned long long)visits[1], ratio);

    /* Non-vacuous: the group really was scanned. */
    ASSERT_GT((long long)visits[0], 0);
    /* Memoized: one scan of the whole group, so visits == k exactly. */
    ASSERT_EQ((long long)visits[0], (long long)K);
    ASSERT_EQ((long long)visits[1], (long long)(2 * K));
    ASSERT_TRUE(ratio <= 2.75);
    PASS();
}

/* ── Regression cases from the C comparison ────────────────────────── */

#define IMP_FNV_SEED UINT64_C(14695981039346656037)

static uint64_t imp_fnv(uint64_t hash, const void *data, size_t size) {
    const unsigned char *bytes = data;
    for (size_t i = 0; i < size; i++) {
        hash = (hash ^ bytes[i]) * UINT64_C(1099511628211);
    }
    return hash;
}

static uint32_t imp_next(uint32_t *state) {
    *state = *state * 1664525U + 1013904223U;
    return *state >> 8;
}

/* Property blobs the writer must append to, overwrite in, or leave alone. */
static const char *const kImpProps[] = {
    "{}",
    "{\"a\":1}",
    "{\"importance\":1.0}",
    "{\"importance\":1.0,\"b\":2}",
    "{ \"importance\" :  {\"x\":[1,\"}\"]}, \"b\":2}",
    "{\"doc\":\"\\\"importance\\\":1\"}",
    "{\"importance\":\"open}",
    "{\"importance\":}",
    "{\"importance\"}",
    "{  }",
    "[]",
    "{",
    "",
    "not json",
};
enum { IMP_PROP_COUNT = (int)(sizeof(kImpProps) / sizeof(kImpProps[0])) };

/* Names across every multiplier, files across the test-path rule, and
 * labels inside and outside the scored set. */
static void imp_build(cbm_gbuf_t *gb, uint32_t seed) {
    static const char *const names[] = {
        "_run",     "run",       "load_user_profile", "loadUserProfile", "aggregate", "get_x",
        "toString", "_long_key", "getURLId",          "TestThing",       "x",         "Example",
    };
    static const char *const files[] = {
        "pkg/a.go",       "pkg/b.go", "pkg/c_test.go", "src/x.test.ts",           "tests/util.py",
        "lib/d.py",       "lib/e.py", "lib/f.py",      "pkg/testutil_helpers.go", "",
        "OrderTest.java",
    };
    static const char *const labels[] = {"Function", "Method", "Class", "File", "Variable"};
    static const char *const kinds[] = {"CALLS", "USAGE", "TESTS", "IMPORTS"};
    enum { IMP_NODES = 240, IMP_EDGES = 1600 };
    uint32_t state = seed;
    int64_t ids[IMP_NODES];
    for (int i = 0; i < IMP_NODES; i++) {
        char qn[CBM_SZ_64];
        const char *name = names[imp_next(&state) % (sizeof(names) / sizeof(names[0]))];
        snprintf(qn, sizeof(qn), "imp.%s.%d", name, i);
        const char *label = labels[imp_next(&state) % 5];
        const char *file = files[imp_next(&state) % (sizeof(files) / sizeof(files[0]))];
        const char *props = kImpProps[imp_next(&state) % IMP_PROP_COUNT];
        ids[i] = cbm_gbuf_upsert_node(gb, label, name, qn, file, 1, 1, props);
    }
    for (int e = 0; e < IMP_EDGES; e++) {
        int64_t a = ids[imp_next(&state) % IMP_NODES];
        int64_t b = ids[imp_next(&state) % IMP_NODES];
        cbm_gbuf_insert_edge(gb, a, b, kinds[imp_next(&state) % 4], "{}");
    }
}

typedef struct {
    char **lines;
    int count;
    int cap;
} imp_lines_t;

static void imp_add_line(imp_lines_t *lines, const char *line) {
    if (lines->count == lines->cap) {
        lines->cap = lines->cap ? lines->cap * 2 : 256;
        lines->lines = realloc(lines->lines, (size_t)lines->cap * sizeof(char *));
    }
    lines->lines[lines->count++] = strdup(line);
}

static void imp_collect_node(const cbm_gbuf_node_t *n, void *data) {
    char line[CBM_SZ_1K];
    snprintf(line, sizeof(line), "node %s %s %s %s %d %d %s", n->qualified_name, n->label,
             n->name, n->file_path, n->start_line, n->end_line, n->properties_json);
    imp_add_line(data, line);
}

static void imp_collect_edge(const cbm_gbuf_edge_t *edge, void *data) {
    char line[CBM_SZ_256];
    snprintf(line, sizeof(line), "edge %s %lld %lld %s", edge->type, (long long)edge->source_id,
             (long long)edge->target_id, edge->properties_json);
    imp_add_line(data, line);
}

static int imp_cmp_line(const void *a, const void *b) {
    return strcmp(*(char *const *)a, *(char *const *)b);
}

static void imp_lines_free(imp_lines_t *lines) {
    for (int i = 0; i < lines->count; i++) {
        free(lines->lines[i]);
    }
    free(lines->lines);
    memset(lines, 0, sizeof(*lines));
}

static void imp_gbuf_lines(const cbm_gbuf_t *gb, imp_lines_t *out) {
    memset(out, 0, sizeof(*out));
    cbm_gbuf_foreach_node(gb, imp_collect_node, out);
    cbm_gbuf_foreach_edge(gb, imp_collect_edge, out);
    qsort(out->lines, (size_t)out->count, sizeof(char *), imp_cmp_line);
}

static uint64_t imp_lines_digest(const imp_lines_t *lines) {
    uint64_t digest = IMP_FNV_SEED;
    for (int i = 0; i < lines->count; i++) {
        digest = imp_fnv(digest, lines->lines[i], strlen(lines->lines[i]) + 1);
    }
    return digest;
}

static double imp_generated_score(uint32_t *state, int iteration) {
    if (iteration % 2) {
        double base = sqrt((double)(imp_next(state) % 100000));
        double penalty = imp_next(state) % 2 ? 0.1 : 1.0;
        double bonus = imp_next(state) % 2 ? 10.0 : 1.0;
        return base * penalty * bonus;
    }
    uint32_t value = imp_next(state);
    unsigned shift = imp_next(state) % 24;
    return (double)value / (double)(1U << shift);
}

/* The score formula from the C reference. */
static double imp_reference_score(const cbm_importance_inputs_t *in) {
    const char *name = in->name;
    bool private_name = name && name[0] == '_';
    bool distinctive = false;
    if (name && strlen(name) >= 8) {
        distinctive = strchr(name, '_') != NULL;
        for (size_t i = 1; name[i] && !distinctive; i++) {
            distinctive = islower((unsigned char)name[i - 1]) && isupper((unsigned char)name[i]);
        }
    }
    double score = sqrt((double)(in->num_refs > 0 ? in->num_refs : 0));
    if (private_name) {
        score *= 0.1;
    }
    if (in->name_distinct_files >= 5) {
        score *= 0.1;
    }
    if (distinctive) {
        score *= 10.0;
    }
    if (cbm_is_test_path(in->file_path) || in->tests_target) {
        score *= 0.1;
    }
    return score;
}

/* Replay the C writer's output on the fixed property cases. Check generated
 * values against printf and score multipliers against the reference formula. */
TEST(importance_writer_and_rule_regression) {
    static const double scores[] = {
        0.0,     -0.0,      1.5,                  42.25,           0.0078125, 0.0000005,
        2.5e-7,  1.0 / 3.0, 0x1.6a09e667f3bcdp+0, 1234567.8912345, 1e20,      1e24,
        1e25,    1e40,      -3.75,                INFINITY,        -INFINITY, NAN,
        DBL_MAX, DBL_MIN,   4.9e-324,
    };
    uint64_t digest = IMP_FNV_SEED;
    for (int p = 0; p < IMP_PROP_COUNT; p++) {
        for (size_t k = 0; k < sizeof(scores) / sizeof(scores[0]); k++) {
            char *out = cbm_pipeline_importance_set_prop(kImpProps[p], scores[k]);
            const char *text = out ? out : "(null)";
            digest = imp_fnv(digest, text, strlen(text) + 1);
            free(out);
        }
    }
    ASSERT_EQ(digest, UINT64_C(0x759febcb2386a01b));
    char *from_null = cbm_pipeline_importance_set_prop(NULL, 2.0);
    ASSERT_STR_EQ(from_null, "{\"importance\":2.000000}");
    free(from_null);
    char *tie = cbm_pipeline_importance_set_prop("{}", 0.0078125);
    ASSERT_STR_EQ(tie, "{\"importance\":0.007812}");
    free(tie);
    ASSERT_NULL(cbm_pipeline_importance_set_prop("{}", 1e25));

    /* Generated scores: sqrt of a count times the multipliers, as the pass
     * writes them, plus arbitrary doubles. */
    uint32_t state = 0x1a2b3c4dU;
    for (int t = 0; t < 20000; t++) {
        double score = imp_generated_score(&state, t);
        char want[CBM_SZ_64];
        snprintf(want, sizeof(want), "{\"importance\":%.6f}", score);
        char *got = cbm_pipeline_importance_set_prop("{}", score);
        ASSERT_NOT_NULL(got);
        if (strcmp(got, want) != 0) {
            printf("    score=%.17g want=%s got=%s\n", score, want, got);
        }
        ASSERT_STR_EQ(got, want);
        free(got);
    }

    static const char *const names[] = {NULL,       "",         "_plain",   "_long_key",
                                        "getUser",  "getURLId", "longword", "get_name",
                                        "aBcDeFgH", "abcdefgH"};
    static const char *const paths[] = {NULL, "pkg/testutil_helpers.go", "pkg/thing_test.go",
                                        "tests/x.py", "x.go"};
    static const int refs[] = {INT_MIN, -1, 0, 1, 2, 37, INT_MAX};
    static const int files[] = {INT_MIN, 0, 4, 5, 6, INT_MAX};
    for (size_t a = 0; a < sizeof(names) / sizeof(names[0]); a++) {
        for (size_t b = 0; b < sizeof(paths) / sizeof(paths[0]); b++) {
            for (size_t c = 0; c < sizeof(refs) / sizeof(refs[0]); c++) {
                for (size_t d = 0; d < sizeof(files) / sizeof(files[0]); d++) {
                    for (int tests = 0; tests < 2; tests++) {
                        cbm_importance_inputs_t in = {
                            .name = names[a],
                            .file_path = paths[b],
                            .num_refs = refs[c],
                            .name_distinct_files = files[d],
                            .tests_target = tests != 0,
                        };
                        double want = imp_reference_score(&in);
                        double got = cbm_pipeline_importance_score(&in);
                        ASSERT_EQ(memcmp(&want, &got, sizeof(double)), 0);
                    }
                }
            }
        }
    }
    ASSERT_EQ(cbm_pipeline_importance_score(NULL), 0);
    PASS();
}

/* Sorted graph rows and work counts captured from the C pass. */
TEST(importance_pass_regression) {
    static const struct {
        uint64_t digest;
        uint64_t nodes;
        uint64_t visits;
    } expected[] = {
        {UINT64_C(0xbd5e09bd86786a27), 159, 240},
        {UINT64_C(0x67c518ff4e818813), 153, 240},
        {UINT64_C(0x930853fe5eae2d36), 150, 240},
        {UINT64_C(0x545d15ff43eaf4df), 150, 240},
        {UINT64_C(0x6989cabc16ebd535), 142, 240},
        {UINT64_C(0xdac544c588b59c7b), 145, 240},
        {UINT64_C(0x306fb10d1d29bc89), 149, 240},
        {UINT64_C(0x3f13a401c113dc10), 147, 240},
        {UINT64_C(0x46ccfffa4e28e382), 151, 240},
        {UINT64_C(0x0c19f95a7b60093c), 152, 240},
        {UINT64_C(0x9a283571d45b9359), 143, 240},
        {UINT64_C(0x8b50c5fe40aaaa27), 135, 240},
    };
    for (size_t i = 0; i < sizeof(expected) / sizeof(expected[0]); i++) {
        uint32_t seed = (uint32_t)i + 1;
        cbm_gbuf_t *graph = cbm_gbuf_new("imp-proj", "/tmp/imp");
        ASSERT_NOT_NULL(graph);
        imp_build(graph, seed * 2246822519U);
        uint64_t nodes = atomic_load_explicit(&g_importance_nodes, memory_order_relaxed);
        uint64_t visits = atomic_load_explicit(&g_importance_name_visits, memory_order_relaxed);
        mori_pipeline_pass_importance(graph);
        ASSERT_EQ(atomic_load_explicit(&g_importance_nodes, memory_order_relaxed) - nodes,
                  expected[i].nodes);
        ASSERT_EQ(atomic_load_explicit(&g_importance_name_visits, memory_order_relaxed) - visits,
                  expected[i].visits);

        imp_lines_t lines;
        imp_gbuf_lines(graph, &lines);
        ASSERT_EQ(imp_lines_digest(&lines), expected[i].digest);
        imp_lines_free(&lines);

        mori_pipeline_pass_importance(graph);
        imp_gbuf_lines(graph, &lines);
        ASSERT_EQ(imp_lines_digest(&lines), expected[i].digest);
        imp_lines_free(&lines);
        cbm_gbuf_free(graph);
    }
    mori_pipeline_pass_importance(NULL);
    PASS();
}

/* Graph content digests and row counts captured from the C SQL recompute. */
TEST(importance_store_recompute_regression) {
    static const struct {
        const char *digest;
        uint64_t rows;
    } expected[] = {
        {"8f1ba659a16cf6bf4842d28b85150ea15cd046203513753c32fb759883e2994b", 154},
        {"b6b962503cc0ef3f251dc78da82595edf3291957becfe4886b0824d311e4dcba", 151},
        {"ba6c71c6f73fcb3136e60875c6e419a9e9f047c58cc11073e4687cb754d32424", 144},
        {"b4b9930e423853e74616a4f1ab304b9a0f93812680358bc4a8ba9ba7c95320f1", 158},
        {"3c668dca33202b1bdec96f2f664088e1cc25f5587c8b2ed761ab153b8ef42e8e", 141},
        {"1e17f8d74bb628b9926080ba13682385b3e67f8de01cacfff673bb4687212167", 138},
    };
    for (size_t i = 0; i < sizeof(expected) / sizeof(expected[0]); i++) {
        uint32_t seed = (uint32_t)i + 1;
        cbm_gbuf_t *graph = cbm_gbuf_new("imp-proj", "/tmp/imp");
        ASSERT_NOT_NULL(graph);
        imp_build(graph, seed * 3266489917U);
        cbm_store_t *store = cbm_store_open_memory();
        ASSERT_NOT_NULL(store);
        ASSERT_EQ(cbm_gbuf_flush_to_store(graph, store), 0);
        cbm_gbuf_free(graph);

        uint64_t rows = atomic_load_explicit(&g_importance_store_rows, memory_order_relaxed);
        ASSERT_EQ(cbm_pipeline_importance_recompute_store(store, "imp-proj"), 0);
        ASSERT_EQ(atomic_load_explicit(&g_importance_store_rows, memory_order_relaxed) - rows,
                  expected[i].rows);

        unsigned char digest[32];
        char hex[2 * sizeof(digest) + 1];
        ASSERT_EQ(cbm_store_graph_content_digest(store, "imp-proj", digest), CBM_STORE_OK);
        for (size_t j = 0; j < sizeof(digest); j++) {
            snprintf(hex + 2 * j, 3, "%02x", (unsigned)digest[j]);
        }
        ASSERT_STR_EQ(hex, expected[i].digest);
        cbm_store_close(store);
    }
    ASSERT_EQ(cbm_pipeline_importance_recompute_store(NULL, "imp-proj"), -1);
    PASS();
}

/* ── Suite ────────────────────────────────────────────────────────────── */

SUITE(importance) {
    /* Registration — catches a miscounted PREDUMP_PASS_COUNT. */
    RUN_TEST(importance_pass_actually_runs_in_full_pipeline);

    /* Formula. */
    RUN_TEST(importance_base_is_sqrt_of_incoming_refs);
    RUN_TEST(importance_private_names_are_demoted);
    RUN_TEST(importance_generic_names_are_demoted_on_distinct_files);
    RUN_TEST(importance_distinctive_names_are_promoted);
    RUN_TEST(importance_test_scaffolding_is_demoted);

    /* Idempotent write-back — the incremental duplicate-key trap. */
    RUN_TEST(importance_append_prop_overwrites_instead_of_duplicating);
    RUN_TEST(importance_pass_is_idempotent_across_reruns);
    RUN_TEST(importance_rehydrated_node_keeps_exactly_one_key);
    RUN_TEST(importance_second_index_run_keeps_exactly_one_key);

    /* Closure-delta route — the store-level recompute. */
    RUN_TEST(importance_closure_delta_keeps_referenced_symbol_sane);
    RUN_TEST(importance_full_and_incremental_converge_on_same_scores);

    /* Cost shape. */
    RUN_TEST(importance_distinct_file_count_is_linear_in_group_size);

    /* Parity — the Rust pass against the C reference. */
    RUN_TEST(importance_writer_and_rule_regression);
    RUN_TEST(importance_pass_regression);
    RUN_TEST(importance_store_recompute_regression);
}
