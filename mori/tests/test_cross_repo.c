/*
 * test_cross_repo.c — Input, work-bound, and write-failure guards for the
 * cross-repository matching pass.
 */
#include "test_framework.h"
#include "test_helpers.h"

#include "foundation/compat.h"
#include "foundation/mem_core.h"
#include "pipeline/pass_cross_repo.h"
#include "pipeline/pipeline.h"
#include "pipeline/pipeline_internal.h"
#include "watcher/watcher.h"

#include <sqlite3/sqlite3.h>
#include <yyjson/yyjson.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#if !defined(_WIN32)
#include <sys/wait.h>
#endif

typedef struct {
    char cache[256];
    char *saved_cache;
} cross_repo_fixture_t;

static bool cross_repo_fixture_begin(cross_repo_fixture_t *fixture) {
    memset(fixture, 0, sizeof(*fixture));
    const char *saved = getenv("CBM_CACHE_DIR");
    if (saved) {
        fixture->saved_cache = strdup(saved);
        if (!fixture->saved_cache) {
            return false;
        }
    }
    snprintf(fixture->cache, sizeof(fixture->cache), "/tmp/cbm-cross-hardening-XXXXXX");
    return cbm_mkdtemp(fixture->cache) != NULL &&
           cbm_setenv("CBM_CACHE_DIR", fixture->cache, 1) == 0;
}

static void cross_repo_fixture_end(cross_repo_fixture_t *fixture) {
    if (fixture->saved_cache) {
        (void)cbm_setenv("CBM_CACHE_DIR", fixture->saved_cache, 1);
    } else {
        (void)cbm_unsetenv("CBM_CACHE_DIR");
    }
    if (fixture->cache[0]) {
        th_rmtree(fixture->cache);
    }
    free(fixture->saved_cache);
    memset(fixture, 0, sizeof(*fixture));
}

static bool cross_repo_project_path(const cross_repo_fixture_t *fixture, const char *project,
                                    char *out, size_t out_size) {
    int written = snprintf(out, out_size, "%s/%s.db", fixture->cache, project);
    return written > 0 && (size_t)written < out_size;
}

static bool cross_repo_create_project(const cross_repo_fixture_t *fixture, const char *project) {
    char path[512];
    if (!cross_repo_project_path(fixture, project, path, sizeof(path))) {
        return false;
    }
    cbm_store_t *store = cbm_store_open_path(path);
    if (!store) {
        return false;
    }
    bool ok = cbm_store_upsert_project(store, project, fixture->cache) == CBM_STORE_OK;
    cbm_store_close(store);
    return ok;
}

/* Seed one HTTP_CALLS/HANDLES pair into two exact project stores. The suffix
 * keeps node QNs unique when a source is linked to more than one target. */
static bool cross_repo_seed_http_pair(const cross_repo_fixture_t *fixture,
                                      const char *source_project, const char *target_project,
                                      const char *route_path, const char *suffix) {
    char source_path[512];
    char target_path[512];
    if (!cross_repo_project_path(fixture, source_project, source_path, sizeof(source_path)) ||
        !cross_repo_project_path(fixture, target_project, target_path, sizeof(target_path))) {
        return false;
    }
    cbm_store_t *source = cbm_store_open_path(source_path);
    cbm_store_t *target = cbm_store_open_path(target_path);
    if (!source || !target) {
        cbm_store_close(source);
        cbm_store_close(target);
        return false;
    }

    bool ok = cbm_store_upsert_project(source, source_project, fixture->cache) == CBM_STORE_OK &&
              cbm_store_upsert_project(target, target_project, fixture->cache) == CBM_STORE_OK;
    char caller_qn[256];
    char local_route_qn[256];
    char target_route_qn[256];
    char handler_qn[256];
    char route_name[128];
    char edge_props[256];
    snprintf(caller_qn, sizeof(caller_qn), "%s.call.%s", source_project, suffix);
    snprintf(local_route_qn, sizeof(local_route_qn), "%s.local-route.%s", source_project, suffix);
    snprintf(target_route_qn, sizeof(target_route_qn), "__route__GET__%s", route_path);
    snprintf(handler_qn, sizeof(handler_qn), "%s.handle.%s", target_project, suffix);
    snprintf(route_name, sizeof(route_name), "GET %s", route_path);
    snprintf(edge_props, sizeof(edge_props), "{\"url_path\":\"%s\",\"method\":\"GET\"}",
             route_path);

    cbm_node_t caller = {.project = source_project,
                         .label = "Function",
                         .name = "call_remote",
                         .qualified_name = caller_qn,
                         .file_path = "client.c"};
    cbm_node_t local_route = {.project = source_project,
                              .label = "Route",
                              .name = route_name,
                              .qualified_name = local_route_qn,
                              .file_path = "client.c"};
    int64_t caller_id = ok ? cbm_store_upsert_node(source, &caller) : 0;
    int64_t local_route_id = ok ? cbm_store_upsert_node(source, &local_route) : 0;
    cbm_edge_t http_call = {.project = source_project,
                            .source_id = caller_id,
                            .target_id = local_route_id,
                            .type = "HTTP_CALLS",
                            .properties_json = edge_props};
    ok = ok && caller_id > 0 && local_route_id > 0 && cbm_store_insert_edge(source, &http_call) > 0;

    cbm_node_t target_route = {.project = target_project,
                               .label = "Route",
                               .name = route_name,
                               .qualified_name = target_route_qn,
                               .file_path = "server.c"};
    cbm_node_t handler = {.project = target_project,
                          .label = "Function",
                          .name = "handle_remote",
                          .qualified_name = handler_qn,
                          .file_path = "server.c"};
    int64_t target_route_id = ok ? cbm_store_upsert_node(target, &target_route) : 0;
    int64_t handler_id = ok ? cbm_store_upsert_node(target, &handler) : 0;
    cbm_edge_t handles = {.project = target_project,
                          .source_id = handler_id,
                          .target_id = target_route_id,
                          .type = "HANDLES"};
    ok = ok && target_route_id > 0 && handler_id > 0 && cbm_store_insert_edge(target, &handles) > 0;

    cbm_store_close(source);
    cbm_store_close(target);
    return ok;
}

/* Give the source project its own handler for the local Route the seeded
 * HTTP_CALLS edge points at (caller-local HANDLES), as when a monorepo's
 * client code calls the API the same repo serves. (#1459) */
static bool cross_repo_seed_local_handler(const cross_repo_fixture_t *fixture,
                                          const char *source_project, const char *suffix) {
    char source_path[512];
    if (!cross_repo_project_path(fixture, source_project, source_path, sizeof(source_path))) {
        return false;
    }
    cbm_store_t *source = cbm_store_open_path(source_path);
    if (!source) {
        return false;
    }
    char local_route_qn[256];
    char handler_qn[256];
    snprintf(local_route_qn, sizeof(local_route_qn), "%s.local-route.%s", source_project, suffix);
    snprintf(handler_qn, sizeof(handler_qn), "%s.local-handle.%s", source_project, suffix);
    cbm_node_t route = {0};
    bool ok =
        cbm_store_find_node_by_qn(source, source_project, local_route_qn, &route) == CBM_STORE_OK;
    int64_t route_id = ok ? route.id : 0;
    if (ok) {
        cbm_node_free_fields(&route);
    }
    cbm_node_t handler = {.project = source_project,
                          .label = "Function",
                          .name = "handle_local",
                          .qualified_name = handler_qn,
                          .file_path = "server.c"};
    int64_t handler_id = ok ? cbm_store_upsert_node(source, &handler) : 0;
    cbm_edge_t handles = {.project = source_project,
                          .source_id = handler_id,
                          .target_id = route_id,
                          .type = "HANDLES"};
    ok = ok && route_id > 0 && handler_id > 0 && cbm_store_insert_edge(source, &handles) > 0;
    cbm_store_close(source);
    return ok;
}

/* Give the source project a canonical "__route__GET__<path>" Route with its
 * own handler, distinct from the node the seeded HTTP_CALLS points at — the
 * shape extraction produces for a method-less fetch() against a local
 * app.get() route. (#1459) */
static bool cross_repo_seed_local_get_route(const cross_repo_fixture_t *fixture,
                                            const char *source_project, const char *route_path) {
    char source_path[512];
    if (!cross_repo_project_path(fixture, source_project, source_path, sizeof(source_path))) {
        return false;
    }
    cbm_store_t *source = cbm_store_open_path(source_path);
    if (!source) {
        return false;
    }
    char route_qn[256];
    char handler_qn[256];
    snprintf(route_qn, sizeof(route_qn), "__route__GET__%s", route_path);
    snprintf(handler_qn, sizeof(handler_qn), "%s.local-get-handler", source_project);
    cbm_node_t route = {.project = source_project,
                        .label = "Route",
                        .name = route_path,
                        .qualified_name = route_qn,
                        .file_path = "server.c"};
    cbm_node_t handler = {.project = source_project,
                          .label = "Function",
                          .name = "handle_local_get",
                          .qualified_name = handler_qn,
                          .file_path = "server.c"};
    int64_t route_id = cbm_store_upsert_node(source, &route);
    int64_t handler_id = cbm_store_upsert_node(source, &handler);
    cbm_edge_t handles = {.project = source_project,
                          .source_id = handler_id,
                          .target_id = route_id,
                          .type = "HANDLES"};
    bool ok = route_id > 0 && handler_id > 0 && cbm_store_insert_edge(source, &handles) > 0;
    cbm_store_close(source);
    return ok;
}

static bool cross_repo_exec(const cross_repo_fixture_t *fixture, const char *project,
                            const char *sql) {
    char path[512];
    if (!cross_repo_project_path(fixture, project, path, sizeof(path))) {
        return false;
    }
    cbm_store_t *store = cbm_store_open_path_existing(path);
    if (!store) {
        return false;
    }
    char *error = NULL;
    int rc = sqlite3_exec(cbm_store_get_db(store), sql, NULL, NULL, &error);
    sqlite3_free(error);
    cbm_store_close(store);
    return rc == SQLITE_OK;
}

static int cross_repo_count_edges(const cross_repo_fixture_t *fixture, const char *project,
                                  const char *edge_type) {
    char path[512];
    if (!cross_repo_project_path(fixture, project, path, sizeof(path))) {
        return -1;
    }
    cbm_store_t *store = cbm_store_open_path_query(path);
    if (!store) {
        return -1;
    }
    int count = cbm_store_count_edges_by_type(store, project, edge_type);
    cbm_store_close(store);
    return count;
}

TEST(cross_repo_null_target_fails_without_dereference) {
    cross_repo_fixture_t fixture;
    if (!cross_repo_fixture_begin(&fixture) ||
        !cross_repo_create_project(&fixture, "null-target-source")) {
        cross_repo_fixture_end(&fixture);
        FAIL("failed to create isolated source project");
    }

    bool rejected = false;
#if defined(_WIN32)
    const char *targets[] = {NULL};
    cbm_cross_repo_result_t result = cbm_cross_repo_match("null-target-source", targets, 1);
    rejected = result.failed;
    cbm_cross_repo_result_free(&result);
#else
    fflush(NULL);
    pid_t child = fork();
    if (child == 0) {
        const char *targets[] = {NULL};
        cbm_cross_repo_result_t result = cbm_cross_repo_match("null-target-source", targets, 1);
        cbm_cross_repo_result_free(&result);
        _exit(result.failed ? 0 : 2);
    }
    int status = 0;
    rejected = child > 0 && waitpid(child, &status, 0) == child && WIFEXITED(status) &&
               WEXITSTATUS(status) == 0;
#endif

    cross_repo_fixture_end(&fixture);
    ASSERT_TRUE(rejected);
    PASS();
}

TEST(cross_repo_wildcard_keeps_projects_containing_internal_tokens) {
    cross_repo_fixture_t fixture;
    bool setup = cross_repo_fixture_begin(&fixture) &&
                 cross_repo_seed_http_pair(&fixture, "wildcard-source", "orders_config_service",
                                           "/config-orders", "a") &&
                 cross_repo_seed_http_pair(&fixture, "wildcard-source", "orders_cross_repo_service",
                                           "/cross-orders", "b") &&
                 cross_repo_seed_http_pair(&fixture, "wildcard-source", "orders-wal-service",
                                           "/wal-orders", "c") &&
                 cross_repo_seed_http_pair(&fixture, "wildcard-source", "orders-shm-service",
                                           "/shm-orders", "d");
    if (!setup) {
        cross_repo_fixture_end(&fixture);
        FAIL("failed to seed wildcard fixture");
    }

    const char *targets[] = {"*"};
    cbm_cross_repo_result_t result = cbm_cross_repo_match("wildcard-source", targets, 1);
    cross_repo_fixture_end(&fixture);
    cbm_cross_repo_result_free(&result);

    ASSERT_FALSE(result.failed);
    ASSERT_EQ(result.projects_scanned, 4);
    ASSERT_EQ(result.http_edges, 4);
    PASS();
}

/* A project store written before the #768 edges.local_name_gen column: it
 * still answers read-only queries (list_projects shows it), but the
 * read-write open every cross-repo target needs refuses it until a reindex. */
static bool cross_repo_create_pre768_project(const cross_repo_fixture_t *fixture,
                                             const char *project) {
    char path[512];
    if (!cross_repo_project_path(fixture, project, path, sizeof(path))) {
        return false;
    }
    sqlite3 *db = NULL;
    if (sqlite3_open(path, &db) != SQLITE_OK) {
        sqlite3_close(db);
        return false;
    }
    char sql[1024];
    snprintf(sql, sizeof(sql),
             "CREATE TABLE projects(name TEXT PRIMARY KEY, indexed_at TEXT NOT NULL,"
             " root_path TEXT NOT NULL);"
             "CREATE TABLE nodes(id INTEGER PRIMARY KEY AUTOINCREMENT, project TEXT NOT NULL,"
             " label TEXT NOT NULL, name TEXT NOT NULL, qualified_name TEXT NOT NULL,"
             " file_path TEXT DEFAULT '', start_line INTEGER DEFAULT 0,"
             " end_line INTEGER DEFAULT 0, properties TEXT DEFAULT '{}',"
             " UNIQUE(project, qualified_name));"
             "CREATE TABLE edges(id INTEGER PRIMARY KEY AUTOINCREMENT, project TEXT NOT NULL,"
             " source_id INTEGER NOT NULL, target_id INTEGER NOT NULL, type TEXT NOT NULL,"
             " properties TEXT DEFAULT '{}', UNIQUE(source_id, target_id, type));"
             "INSERT INTO projects VALUES('%s', '2026-06-01T00:00:00Z', '/pre768');",
             project);
    bool ok = sqlite3_exec(db, sql, NULL, NULL, NULL) == SQLITE_OK;
    sqlite3_close(db);
    return ok;
}

/* #2133: ["*"] enumerated every store a read-only open accepts, then aborted
 * the whole run on the first one the matcher's read-write open refused — a
 * single pre-#768 index anywhere in the cache made the wildcard fail with
 * "missing, invalid, or not indexed" while naming the same live targets
 * worked. A store the matcher cannot use is not a wildcard target. */
TEST(cross_repo_wildcard_skips_pre768_store_issue2133) {
    cross_repo_fixture_t fixture;
    bool setup = cross_repo_fixture_begin(&fixture) &&
                 cross_repo_seed_http_pair(&fixture, "wild-src", "wild-api", "/orders", "w") &&
                 cross_repo_create_pre768_project(&fixture, "aa-pre768-store");
    if (!setup) {
        cross_repo_fixture_end(&fixture);
        FAIL("failed to seed pre-#768 wildcard fixture");
    }

    const char *targets[] = {"*"};
    cbm_cross_repo_result_t result = cbm_cross_repo_match("wild-src", targets, 1);
    int edges = cross_repo_count_edges(&fixture, "wild-src", "CROSS_HTTP_CALLS");
    cross_repo_fixture_end(&fixture);
    cbm_cross_repo_result_free(&result);

    ASSERT_FALSE(result.failed);
    ASSERT_EQ(result.projects_scanned, 1);
    ASSERT_EQ(result.http_edges, 1);
    ASSERT_EQ(edges, 1);
    PASS();
}

/* Naming an unusable store stays an error, but it must be refused during
 * validation, before the source's previous CROSS_* generation is deleted. */
TEST(cross_repo_named_pre768_target_fails_before_cleanup_issue2133) {
    cross_repo_fixture_t fixture;
    bool setup = cross_repo_fixture_begin(&fixture) &&
                 cross_repo_seed_http_pair(&fixture, "named-src", "named-api", "/orders", "n") &&
                 cross_repo_create_pre768_project(&fixture, "aa-pre768-store");
    if (!setup) {
        cross_repo_fixture_end(&fixture);
        FAIL("failed to seed pre-#768 named fixture");
    }

    const char *live[] = {"named-api"};
    cbm_cross_repo_result_t initial = cbm_cross_repo_match("named-src", live, 1);
    int before = cross_repo_count_edges(&fixture, "named-src", "CROSS_HTTP_CALLS");
    const char *with_pre768[] = {"named-api", "aa-pre768-store"};
    cbm_cross_repo_result_t result = cbm_cross_repo_match("named-src", with_pre768, 2);
    int after = cross_repo_count_edges(&fixture, "named-src", "CROSS_HTTP_CALLS");
    cross_repo_fixture_end(&fixture);
    cbm_cross_repo_result_free(&initial);
    cbm_cross_repo_result_free(&result);

    ASSERT_FALSE(initial.failed);
    ASSERT_EQ(before, 1);
    ASSERT_TRUE(result.failed);
    ASSERT_EQ(after, before);
    PASS();
}

/* Render the "cross_repo" status object the MCP/UI surfaces attach for
 * `project`, read from its store the way index_status does (query open). */
static char *cross_repo_status_json(const cross_repo_fixture_t *fixture, const char *project) {
    char path[512];
    if (!cross_repo_project_path(fixture, project, path, sizeof(path))) {
        return NULL;
    }
    cbm_store_t *store = cbm_store_open_path_query(path);
    if (!store) {
        return NULL;
    }
    yyjson_mut_doc *doc = yyjson_mut_doc_new(NULL);
    yyjson_mut_val *root = yyjson_mut_obj(doc);
    yyjson_mut_doc_set_root(doc, root);
    char *json = NULL;
    if (cbm_cross_repo_add_status_json(doc, root, store, project)) {
        json = yyjson_mut_write(doc, 0, NULL);
    }
    yyjson_mut_doc_free(doc);
    cbm_store_close(store);
    return json;
}

/* #2133 follow-up: an empty CROSS_* set could mean "never linked" or "linked,
 * nothing matched" and no surface told them apart. The source store records
 * each run; before any run the status says so explicitly. */
TEST(cross_repo_status_distinguishes_never_run_from_ran_without_links) {
    cross_repo_fixture_t fixture;
    bool setup = cross_repo_fixture_begin(&fixture) &&
                 cross_repo_create_project(&fixture, "status-src") &&
                 cross_repo_create_project(&fixture, "status-api");
    if (!setup) {
        cross_repo_fixture_end(&fixture);
        FAIL("failed to seed status fixture");
    }

    char *before = cross_repo_status_json(&fixture, "status-src");
    const char *targets[] = {"status-api"};
    cbm_cross_repo_result_t result = cbm_cross_repo_match("status-src", targets, 1);
    char *after = cross_repo_status_json(&fixture, "status-src");
    cross_repo_fixture_end(&fixture);

    bool before_never = before && strstr(before, "\"cross_repo\":{\"status\":\"never_run\"}");
    bool after_ran =
        after && strstr(after, "\"status\":\"ran\"") && strstr(after, "\"outcome\":\"complete\"") &&
        strstr(after, "\"targets\":[\"status-api\"]") && strstr(after, "\"projects_scanned\":1") &&
        strstr(after, "\"total_cross_edges\":0") && strstr(after, "\"last_run_at\":\"20");
    free(before);
    free(after);
    cbm_cross_repo_result_free(&result);

    ASSERT_FALSE(result.failed);
    ASSERT_TRUE(before_never);
    ASSERT_TRUE(after_ran);
    PASS();
}

/* #2133 follow-up: ["*"] skips a pre-#768 store (it cannot be linked until a
 * reindex); the result must name it rather than silently covering fewer
 * projects, and the recorded status counts it. */
TEST(cross_repo_wildcard_reports_skipped_pre768_store) {
    cross_repo_fixture_t fixture;
    bool setup = cross_repo_fixture_begin(&fixture) &&
                 cross_repo_seed_http_pair(&fixture, "skip-src", "skip-api", "/orders", "s") &&
                 cross_repo_create_pre768_project(&fixture, "aa-pre768-store");
    if (!setup) {
        cross_repo_fixture_end(&fixture);
        FAIL("failed to seed skipped-store fixture");
    }

    const char *targets[] = {"*"};
    cbm_cross_repo_result_t result = cbm_cross_repo_match("skip-src", targets, 1);
    char *status = cross_repo_status_json(&fixture, "skip-src");
    cross_repo_fixture_end(&fixture);

    bool named = result.skipped_count == 1 && result.skipped_projects &&
                 strcmp(result.skipped_projects[0].project, "aa-pre768-store") == 0 &&
                 strcmp(result.skipped_projects[0].reason, "pre_768_schema") == 0;
    bool status_counts =
        status && strstr(status, "\"skipped_projects\":1") && strstr(status, "\"targets\":[\"*\"]");
    free(status);
    cbm_cross_repo_result_free(&result);

    ASSERT_FALSE(result.failed);
    ASSERT_EQ(result.projects_scanned, 1);
    ASSERT_TRUE(named);
    ASSERT_TRUE(status_counts);
    PASS();
}

static bool cross_repo_seed_bounded_scan(const cross_repo_fixture_t *fixture,
                                         const char *source_project, const char *target_project) {
    enum { TEST_SCAN_ROWS = 4097 };
    char source_path[512];
    char target_path[512];
    if (!cross_repo_project_path(fixture, source_project, source_path, sizeof(source_path)) ||
        !cross_repo_project_path(fixture, target_project, target_path, sizeof(target_path))) {
        return false;
    }
    cbm_store_t *source = cbm_store_open_path(source_path);
    cbm_store_t *target = cbm_store_open_path(target_path);
    if (!source || !target) {
        cbm_store_close(source);
        cbm_store_close(target);
        return false;
    }
    bool ok = cbm_store_upsert_project(source, source_project, fixture->cache) == CBM_STORE_OK &&
              cbm_store_upsert_project(target, target_project, fixture->cache) == CBM_STORE_OK;
    cbm_node_t caller = {.project = source_project,
                         .label = "Function",
                         .name = "bounded_caller",
                         .qualified_name = "bounded.source.caller",
                         .file_path = "client.c"};
    int64_t caller_id = ok ? cbm_store_upsert_node(source, &caller) : 0;
    ok = ok && caller_id > 0 &&
         sqlite3_exec(cbm_store_get_db(source), "BEGIN IMMEDIATE", NULL, NULL, NULL) == SQLITE_OK;
    for (int i = 0; ok && i < TEST_SCAN_ROWS; i++) {
        char name[64];
        char qn[96];
        snprintf(name, sizeof(name), "local_route_%d", i);
        snprintf(qn, sizeof(qn), "bounded.source.route.%d", i);
        cbm_node_t local_route = {.project = source_project,
                                  .label = "Route",
                                  .name = name,
                                  .qualified_name = qn,
                                  .file_path = "client.c"};
        int64_t route_id = cbm_store_upsert_node(source, &local_route);
        cbm_edge_t edge = {
            .project = source_project,
            .source_id = caller_id,
            .target_id = route_id,
            .type = "HTTP_CALLS",
            .properties_json = i == TEST_SCAN_ROWS - 1
                                   ? "{\"url_path\":\"/after-bound\",\"method\":\"GET\"}"
                                   : "{}",
        };
        ok = route_id > 0 && cbm_store_insert_edge(source, &edge) > 0;
    }
    if (ok) {
        ok = sqlite3_exec(cbm_store_get_db(source), "COMMIT", NULL, NULL, NULL) == SQLITE_OK;
    } else {
        (void)sqlite3_exec(cbm_store_get_db(source), "ROLLBACK", NULL, NULL, NULL);
    }

    cbm_node_t target_route = {.project = target_project,
                               .label = "Route",
                               .name = "GET /after-bound",
                               .qualified_name = "__route__GET__/after-bound",
                               .file_path = "server.c"};
    cbm_node_t handler = {.project = target_project,
                          .label = "Function",
                          .name = "bounded_handler",
                          .qualified_name = "bounded.target.handler",
                          .file_path = "server.c"};
    int64_t target_route_id = ok ? cbm_store_upsert_node(target, &target_route) : 0;
    int64_t handler_id = ok ? cbm_store_upsert_node(target, &handler) : 0;
    cbm_edge_t handles = {.project = target_project,
                          .source_id = handler_id,
                          .target_id = target_route_id,
                          .type = "HANDLES"};
    ok = ok && target_route_id > 0 && handler_id > 0 && cbm_store_insert_edge(target, &handles) > 0;
    cbm_store_close(source);
    cbm_store_close(target);
    return ok;
}

/* No row count decides which calls are matched: the scans stopped after 4,096
 * rows, so a call examined after 4,096 others never got its CROSS_HTTP_CALLS
 * edge even though the target serves its route. */
TEST(cross_repo_matches_calls_past_4096_rows) {
    cross_repo_fixture_t fixture;
    bool setup = cross_repo_fixture_begin(&fixture) &&
                 cross_repo_seed_bounded_scan(&fixture, "bounded-source", "bounded-target");
    if (!setup) {
        cross_repo_fixture_end(&fixture);
        FAIL("failed to seed bounded scan fixture");
    }
    const char *target = "bounded-target";
    cbm_cross_repo_result_t result = cbm_cross_repo_match("bounded-source", &target, 1);
    cross_repo_fixture_end(&fixture);
    cbm_cross_repo_result_free(&result);

    ASSERT_FALSE(result.failed);
    ASSERT_EQ(result.projects_scanned, 1);
    ASSERT_EQ(result.http_edges, 1);
    PASS();
}

TEST(cross_repo_propagates_delete_failure) {
    cross_repo_fixture_t fixture;
    bool setup = cross_repo_fixture_begin(&fixture) &&
                 cross_repo_seed_http_pair(&fixture, "delete-source", "delete-target",
                                           "/delete-failure", "delete");
    if (!setup) {
        cross_repo_fixture_end(&fixture);
        FAIL("failed to seed delete failure fixture");
    }
    const char *target = "delete-target";
    cbm_cross_repo_result_t initial = cbm_cross_repo_match("delete-source", &target, 1);
    bool trigger_created =
        !initial.failed && initial.http_edges == 1 &&
        cross_repo_exec(&fixture, "delete-source",
                        "CREATE TRIGGER fail_cross_delete BEFORE DELETE ON edges "
                        "WHEN OLD.type = 'CROSS_HTTP_CALLS' BEGIN "
                        "SELECT RAISE(ABORT, 'forced cross delete failure'); END;");
    cbm_cross_repo_result_t failed = {0};
    if (trigger_created) {
        failed = cbm_cross_repo_match("delete-source", &target, 1);
    }
    cross_repo_fixture_end(&fixture);
    cbm_cross_repo_result_free(&initial);
    cbm_cross_repo_result_free(&failed);

    ASSERT_TRUE(trigger_created);
    ASSERT_TRUE(failed.failed);
    ASSERT_EQ(failed.http_edges, 0);
    PASS();
}

TEST(cross_repo_failed_bidirectional_insert_is_not_counted) {
    cross_repo_fixture_t fixture;
    bool setup = cross_repo_fixture_begin(&fixture) &&
                 cross_repo_seed_http_pair(&fixture, "insert-source", "insert-target",
                                           "/insert-failure", "insert") &&
                 cross_repo_exec(&fixture, "insert-target",
                                 "CREATE TRIGGER fail_cross_insert BEFORE INSERT ON edges "
                                 "WHEN NEW.type = 'CROSS_HTTP_CALLS' BEGIN "
                                 "SELECT RAISE(ABORT, 'forced cross insert failure'); END;");
    if (!setup) {
        cross_repo_fixture_end(&fixture);
        FAIL("failed to seed insert failure fixture");
    }
    const char *target = "insert-target";
    cbm_cross_repo_result_t result = cbm_cross_repo_match("insert-source", &target, 1);
    cross_repo_fixture_end(&fixture);
    cbm_cross_repo_result_free(&result);

    ASSERT_TRUE(result.failed);
    ASSERT_EQ(result.http_edges, 0);
    ASSERT_EQ(result.projects_scanned, 0);
    PASS();
}

typedef struct {
    atomic_int *cancelled;
    const char *project;
    int fired;
} cross_repo_cancel_hook_t;

static void cross_repo_cancel_after_write(const char *project, const char *edge_type,
                                          void *opaque) {
    cross_repo_cancel_hook_t *hook = opaque;
    if (strcmp(project, hook->project) == 0 && strcmp(edge_type, "CROSS_HTTP_CALLS") == 0) {
        hook->fired++;
        atomic_store_explicit(hook->cancelled, 1, memory_order_release);
    }
}

TEST(cross_repo_cancel_mid_run_keeps_completed_target_and_stops_before_later_target) {
    cross_repo_fixture_t fixture;
    bool setup =
        cross_repo_fixture_begin(&fixture) &&
        cross_repo_seed_http_pair(&fixture, "cancel-source", "cancel-target-a", "/cancel-a", "a") &&
        cross_repo_seed_http_pair(&fixture, "cancel-source", "cancel-target-b", "/cancel-b", "b") &&
        cross_repo_seed_http_pair(&fixture, "cancel-source", "cancel-target-c", "/cancel-c", "c");
    if (!setup) {
        cross_repo_fixture_end(&fixture);
        FAIL("failed to seed cancellation fixture");
    }

    atomic_int cancelled;
    atomic_init(&cancelled, 0);
    cross_repo_cancel_hook_t hook = {
        .cancelled = &cancelled,
        .project = "cancel-target-b",
    };

    const char *targets[] = {"cancel-target-c", "cancel-target-a", "cancel-target-b"};
    cbm_cross_repo_set_after_insert_hook_for_tests(cross_repo_cancel_after_write, &hook);
    cbm_cross_repo_result_t result =
        cbm_cross_repo_match_cancellable("cancel-source", targets, 3, &cancelled);
    cbm_cross_repo_set_after_insert_hook_for_tests(NULL, NULL);

    int completed_target_edges =
        cross_repo_count_edges(&fixture, "cancel-target-a", "CROSS_HTTP_CALLS");
    int interrupted_target_edges =
        cross_repo_count_edges(&fixture, "cancel-target-b", "CROSS_HTTP_CALLS");
    int later_target_edges =
        cross_repo_count_edges(&fixture, "cancel-target-c", "CROSS_HTTP_CALLS");
    cross_repo_fixture_end(&fixture);
    cbm_cross_repo_result_free(&result);

    ASSERT_EQ(hook.fired, 1);
    ASSERT_TRUE(result.cancelled);
    ASSERT_TRUE(result.partial_results);
    ASSERT_FALSE(result.failed);
    ASSERT_EQ(result.projects_scanned, 1);
    ASSERT_TRUE(completed_target_edges > 0);
    ASSERT_TRUE(interrupted_target_edges > 0);
    ASSERT_EQ(later_target_edges, 0);
    PASS();
}

TEST(cross_repo_pre_cancel_preserves_existing_cross_edges) {
    cross_repo_fixture_t fixture;
    bool setup = cross_repo_fixture_begin(&fixture) &&
                 cross_repo_seed_http_pair(&fixture, "pre-cancel-source", "pre-cancel-target",
                                           "/pre-cancel", "pre");
    if (!setup) {
        cross_repo_fixture_end(&fixture);
        FAIL("failed to seed pre-cancel fixture");
    }

    const char *target = "pre-cancel-target";
    cbm_cross_repo_result_t initial = cbm_cross_repo_match("pre-cancel-source", &target, 1);
    int before = cross_repo_count_edges(&fixture, "pre-cancel-source", "CROSS_HTTP_CALLS");
    atomic_int cancelled;
    atomic_init(&cancelled, 1);
    cbm_cross_repo_result_t result =
        cbm_cross_repo_match_cancellable("pre-cancel-source", &target, 1, &cancelled);
    int after = cross_repo_count_edges(&fixture, "pre-cancel-source", "CROSS_HTTP_CALLS");
    cross_repo_fixture_end(&fixture);
    cbm_cross_repo_result_free(&initial);
    cbm_cross_repo_result_free(&result);

    ASSERT_FALSE(initial.failed);
    ASSERT_TRUE(before > 0);
    ASSERT_TRUE(result.cancelled);
    ASSERT_FALSE(result.partial_results);
    ASSERT_FALSE(result.failed);
    ASSERT_EQ(result.projects_scanned, 0);
    ASSERT_EQ(after, before);
    PASS();
}

/* #1133: a run whose targets resolve to nothing but the source project must
 * fail explicitly. It used to report "success" with projects_scanned:0 --
 * indistinguishable from "these services share no routes" -- and, worse, it
 * had already wiped the source's existing CROSS_* edges before noticing there
 * was nothing to match against. */
TEST(cross_repo_self_only_target_fails_and_keeps_edges_issue1133) {
    cross_repo_fixture_t fixture;
    bool setup =
        cross_repo_fixture_begin(&fixture) &&
        cross_repo_seed_http_pair(&fixture, "self-source", "self-target", "/self-only", "self");
    if (!setup) {
        cross_repo_fixture_end(&fixture);
        FAIL("failed to seed self-only fixture");
    }

    const char *target = "self-target";
    cbm_cross_repo_result_t initial = cbm_cross_repo_match("self-source", &target, 1);
    int before = cross_repo_count_edges(&fixture, "self-source", "CROSS_HTTP_CALLS");
    const char *self = "self-source";
    cbm_cross_repo_result_t result = cbm_cross_repo_match("self-source", &self, 1);
    int after = cross_repo_count_edges(&fixture, "self-source", "CROSS_HTTP_CALLS");
    cross_repo_fixture_end(&fixture);
    cbm_cross_repo_result_free(&initial);
    cbm_cross_repo_result_free(&result);

    ASSERT_FALSE(initial.failed);
    ASSERT_TRUE(before > 0);
    ASSERT_TRUE(result.failed);
    ASSERT_TRUE(result.no_targets);
    ASSERT_EQ(result.projects_scanned, 0);
    ASSERT_EQ(after, before);
    PASS();
}

/* #1133: ["*"] in a store that holds only the source project resolves to zero
 * targets -- same contract as an explicit self-only list. */
TEST(cross_repo_wildcard_with_no_other_project_fails_issue1133) {
    cross_repo_fixture_t fixture;
    if (!cross_repo_fixture_begin(&fixture) ||
        !cross_repo_create_project(&fixture, "lonely-source")) {
        cross_repo_fixture_end(&fixture);
        FAIL("failed to create isolated source project");
    }

    const char *targets[] = {"*"};
    cbm_cross_repo_result_t result = cbm_cross_repo_match("lonely-source", targets, 1);
    cross_repo_fixture_end(&fixture);
    cbm_cross_repo_result_free(&result);

    ASSERT_TRUE(result.failed);
    ASSERT_TRUE(result.no_targets);
    ASSERT_EQ(result.projects_scanned, 0);
    PASS();
}

/* Add the internal "<name>::missed" miss-graph row that indexing writes into
 * the SAME db whenever a file parses partially. */
static bool cross_repo_add_missed_shadow(const cross_repo_fixture_t *fixture, const char *project) {
    char path[512];
    char shadow[256];
    if (!cross_repo_project_path(fixture, project, path, sizeof(path))) {
        return false;
    }
    snprintf(shadow, sizeof(shadow), "%s::missed", project);
    cbm_store_t *store = cbm_store_open_path(path);
    if (!store) {
        return false;
    }
    bool ok = cbm_store_upsert_project(store, shadow, fixture->cache) == CBM_STORE_OK;
    cbm_store_close(store);
    return ok;
}

/* #1609: any project that has ever recorded a parse miss carries a
 * "<name>::missed" shadow row in its own db. cr_store_has_exact_project
 * demanded count == 1 over ALL rows, so that second row made the project
 * unresolvable — as source AND as target — and the whole feature failed with
 * "not indexed" for a project that plainly was. mcp.c already solved exactly
 * this shape for list_projects in #1044; this site never learned it.
 *
 * The control is the pair without shadow rows: the tests above already prove
 * that path returns edges, so a regression here cannot hide behind a fixture
 * that never matched in the first place. */
TEST(cross_repo_accepts_project_with_missed_shadow_row_issue1609) {
    cross_repo_fixture_t fixture;
    bool setup = cross_repo_fixture_begin(&fixture) &&
                 cross_repo_seed_http_pair(&fixture, "shadow-source", "shadow-target", "/orders",
                                           "s") &&
                 cross_repo_add_missed_shadow(&fixture, "shadow-source") &&
                 cross_repo_add_missed_shadow(&fixture, "shadow-target");
    if (!setup) {
        cross_repo_fixture_end(&fixture);
        FAIL("failed to seed shadow-row fixture");
    }

    const char *target = "shadow-target";
    cbm_cross_repo_result_t result = cbm_cross_repo_match("shadow-source", &target, 1);
    cross_repo_fixture_end(&fixture);
    cbm_cross_repo_result_free(&result);

    ASSERT_FALSE(result.failed);
    ASSERT_EQ(result.projects_scanned, 1);
    ASSERT_EQ(result.http_edges, 1);
    PASS();
}

/* #1459 floor: a caller whose own project handles the route it calls is
 * calling itself — another project exposing the same path is not evidence of
 * a cross-service call. Neither direction of the pass may link it. */
TEST(cross_repo_caller_local_handler_blocks_cross_http_issue1459) {
    cross_repo_fixture_t fixture;
    bool setup =
        cross_repo_fixture_begin(&fixture) &&
        cross_repo_seed_http_pair(&fixture, "mono-1459", "api-1459", "/api/users", "mono") &&
        cross_repo_seed_local_handler(&fixture, "mono-1459", "mono");
    if (!setup) {
        cross_repo_fixture_end(&fixture);
        FAIL("failed to seed caller-local route fixture");
    }
    const char *target = "api-1459";
    cbm_cross_repo_result_t fwd = cbm_cross_repo_match("mono-1459", &target, 1);
    const char *consumer = "mono-1459";
    cbm_cross_repo_result_t rev = cbm_cross_repo_match("api-1459", &consumer, 1);
    int mono_edges = cross_repo_count_edges(&fixture, "mono-1459", "CROSS_HTTP_CALLS");
    int api_edges = cross_repo_count_edges(&fixture, "api-1459", "CROSS_HTTP_CALLS");
    cross_repo_fixture_end(&fixture);
    cbm_cross_repo_result_free(&fwd);
    cbm_cross_repo_result_free(&rev);

    ASSERT_FALSE(fwd.failed);
    ASSERT_FALSE(rev.failed);
    ASSERT_EQ(fwd.http_edges, 0);
    ASSERT_EQ(rev.http_edges, 0);
    ASSERT_EQ(mono_edges, 0);
    ASSERT_EQ(api_edges, 0);
    PASS();
}

/* #1459 floor, real-world shape: a method-less client call points at the ANY
 * Route node, while the caller's own handler sits on the GET node for the same
 * path. The call is still served in-project and must not cross. */
TEST(cross_repo_methodless_call_to_local_get_route_blocks_cross_http_issue1459) {
    cross_repo_fixture_t fixture;
    bool setup =
        cross_repo_fixture_begin(&fixture) &&
        cross_repo_seed_http_pair(&fixture, "mono2-1459", "api3-1459", "/api/users", "mono2") &&
        cross_repo_exec(&fixture, "mono2-1459",
                        "UPDATE edges SET properties = '{\"url_path\":\"/api/users\"}' "
                        "WHERE type = 'HTTP_CALLS';") &&
        cross_repo_seed_local_get_route(&fixture, "mono2-1459", "/api/users");
    if (!setup) {
        cross_repo_fixture_end(&fixture);
        FAIL("failed to seed method-less caller-local fixture");
    }
    const char *target = "api3-1459";
    cbm_cross_repo_result_t fwd = cbm_cross_repo_match("mono2-1459", &target, 1);
    const char *consumer = "mono2-1459";
    cbm_cross_repo_result_t rev = cbm_cross_repo_match("api3-1459", &consumer, 1);
    int mono_edges = cross_repo_count_edges(&fixture, "mono2-1459", "CROSS_HTTP_CALLS");
    int api_edges = cross_repo_count_edges(&fixture, "api3-1459", "CROSS_HTTP_CALLS");
    cross_repo_fixture_end(&fixture);
    cbm_cross_repo_result_free(&fwd);
    cbm_cross_repo_result_free(&rev);

    ASSERT_FALSE(fwd.failed);
    ASSERT_FALSE(rev.failed);
    ASSERT_EQ(fwd.http_edges, 0);
    ASSERT_EQ(rev.http_edges, 0);
    ASSERT_EQ(mono_edges, 0);
    ASSERT_EQ(api_edges, 0);
    PASS();
}

/* Control for the #1459 floor: without a caller-local handler the path match
 * still links the caller to the remote handler, in both run directions. */
TEST(cross_repo_caller_without_local_handler_keeps_cross_http_issue1459) {
    cross_repo_fixture_t fixture;
    bool setup =
        cross_repo_fixture_begin(&fixture) &&
        cross_repo_seed_http_pair(&fixture, "client-1459", "api2-1459", "/api/users", "client");
    if (!setup) {
        cross_repo_fixture_end(&fixture);
        FAIL("failed to seed remote-only route fixture");
    }
    const char *target = "api2-1459";
    cbm_cross_repo_result_t fwd = cbm_cross_repo_match("client-1459", &target, 1);
    int client_edges = cross_repo_count_edges(&fixture, "client-1459", "CROSS_HTTP_CALLS");
    int api_edges = cross_repo_count_edges(&fixture, "api2-1459", "CROSS_HTTP_CALLS");
    const char *consumer = "client-1459";
    cbm_cross_repo_result_t rev = cbm_cross_repo_match("api2-1459", &consumer, 1);
    cross_repo_fixture_end(&fixture);
    cbm_cross_repo_result_free(&fwd);
    cbm_cross_repo_result_free(&rev);

    ASSERT_FALSE(fwd.failed);
    ASSERT_FALSE(rev.failed);
    ASSERT_EQ(fwd.http_edges, 1);
    ASSERT_EQ(rev.http_edges, 1);
    ASSERT_EQ(client_edges, 1);
    ASSERT_EQ(api_edges, 1);
    PASS();
}

static bool cross_repo_seed_server_route(const cross_repo_fixture_t *fixture,
                                         const char *target_project, const char *method,
                                         const char *route_path, const char *handler_name) {
    char target_path[512];
    if (!cross_repo_project_path(fixture, target_project, target_path, sizeof(target_path))) {
        return false;
    }
    cbm_store_t *target = cbm_store_open_path(target_path);
    if (!target) {
        return false;
    }
    bool ok = cbm_store_upsert_project(target, target_project, fixture->cache) == CBM_STORE_OK;
    char target_route_qn[256];
    char handler_qn[256];
    char route_name[128];
    snprintf(target_route_qn, sizeof(target_route_qn), "__route__%s__%s", method, route_path);
    snprintf(handler_qn, sizeof(handler_qn), "%s.%s", target_project, handler_name);
    snprintf(route_name, sizeof(route_name), "%s %s", method, route_path);

    cbm_node_t target_route = {.project = target_project,
                               .label = "Route",
                               .name = route_name,
                               .qualified_name = target_route_qn,
                               .file_path = "server.java"};
    cbm_node_t handler = {.project = target_project,
                          .label = "Function",
                          .name = handler_name,
                          .qualified_name = handler_qn,
                          .file_path = "server.java"};
    int64_t target_route_id = ok ? cbm_store_upsert_node(target, &target_route) : 0;
    int64_t handler_id = ok ? cbm_store_upsert_node(target, &handler) : 0;
    cbm_edge_t handles = {.project = target_project,
                          .source_id = handler_id,
                          .target_id = target_route_id,
                          .type = "HANDLES"};
    ok = ok && target_route_id > 0 && handler_id > 0 && cbm_store_insert_edge(target, &handles) > 0;
    cbm_store_close(target);
    return ok;
}

static bool cross_repo_seed_client_call(const cross_repo_fixture_t *fixture,
                                        const char *source_project, const char *caller_name,
                                        const char *callee_name, const char *url_path,
                                        const char *method) {
    char source_path[512];
    if (!cross_repo_project_path(fixture, source_project, source_path, sizeof(source_path))) {
        return false;
    }
    cbm_store_t *source = cbm_store_open_path(source_path);
    if (!source) {
        return false;
    }
    bool ok = cbm_store_upsert_project(source, source_project, fixture->cache) == CBM_STORE_OK;
    char caller_qn[256];
    char local_route_qn[256];
    char edge_props[512];
    snprintf(caller_qn, sizeof(caller_qn), "%s.%s", source_project, caller_name);
    snprintf(local_route_qn, sizeof(local_route_qn), "__route__%s__%s", method ? method : "ANY",
             url_path);
    if (method) {
        snprintf(edge_props, sizeof(edge_props),
                 "{\"callee\":\"%s\",\"url_path\":\"%s\",\"method\":\"%s\"}", callee_name,
                 url_path, method);
    } else {
        snprintf(edge_props, sizeof(edge_props),
                 "{\"callee\":\"%s\",\"url_path\":\"%s\"}", callee_name, url_path);
    }
    cbm_node_t caller = {.project = source_project,
                         .label = "Function",
                         .name = caller_name,
                         .qualified_name = caller_qn,
                         .file_path = "item.service.ts"};
    cbm_node_t local_route = {.project = source_project,
                              .label = "Route",
                              .name = url_path,
                              .qualified_name = local_route_qn,
                              .file_path = "item.service.ts"};
    int64_t caller_id = ok ? cbm_store_upsert_node(source, &caller) : 0;
    int64_t local_route_id = ok ? cbm_store_upsert_node(source, &local_route) : 0;
    cbm_edge_t http_call = {.project = source_project,
                            .source_id = caller_id,
                            .target_id = local_route_id,
                            .type = "HTTP_CALLS",
                            .properties_json = edge_props};
    ok = ok && caller_id > 0 && local_route_id > 0 && cbm_store_insert_edge(source, &http_call) > 0;
    cbm_store_close(source);
    return ok;
}

static int cross_repo_count_edges_like(const cross_repo_fixture_t *fixture, const char *project,
                                       const char *edge_type, const char *pattern) {
    char path[512];
    if (!cross_repo_project_path(fixture, project, path, sizeof(path))) {
        return -1;
    }
    cbm_store_t *store = cbm_store_open_path_query(path);
    if (!store) {
        return -1;
    }
    sqlite3_stmt *s = NULL;
    int count = -1;
    if (sqlite3_prepare_v2(
            cbm_store_get_db(store),
            "SELECT COUNT(*) FROM edges WHERE project = ?1 AND type = ?2 AND properties LIKE ?3",
            -1, &s, NULL) == SQLITE_OK) {
        sqlite3_bind_text(s, 1, project, -1, SQLITE_STATIC);
        sqlite3_bind_text(s, 2, edge_type, -1, SQLITE_STATIC);
        sqlite3_bind_text(s, 3, pattern, -1, SQLITE_STATIC);
        if (sqlite3_step(s) == SQLITE_ROW) {
            count = sqlite3_column_int(s, 0);
        }
        sqlite3_finalize(s);
    }
    cbm_store_close(store);
    return count;
}

/* #2768: Angular HttpClient calls must match cross-repo routes with exact HTTP verbs.
 * When client makes GET requests to /items/list, /items/42, and /items/job/10,
 * and the server defines overlapping routes:
 *   DELETE /items/{id} (seeded first in DB)
 *   GET    /items/{id} (seeded second in DB)
 *   POST   /items/{parentId}/{id} (seeded first in DB)
 *   GET    /items/job/{jobId} (seeded second in DB)
 * the fuzzy route matcher must enforce method compatibility, linking the GET calls
 * to their respective GET endpoints and never to DELETE or POST endpoints.
 * getItem and getJob are seeded WITHOUT a method property (an edge written by an
 * index that did not record the verb): the matcher must infer GET from the
 * callee's verb suffix, or the method-less fuzzy match lands on the DELETE / POST
 * route seeded first. getItems keeps its explicit "GET". */
TEST(cross_repo_angular_http_client_exact_verb_matching_issue2768) {
    cross_repo_fixture_t fixture;
    bool setup = cross_repo_fixture_begin(&fixture) &&
                 /* Target server with overlapping endpoints */
                 cross_repo_seed_server_route(&fixture, "api-2768", "GET", "/items/list",
                                              "handle_list_items") &&
                 cross_repo_seed_server_route(&fixture, "api-2768", "DELETE", "/items/{}",
                                              "handle_delete_item") &&
                 cross_repo_seed_server_route(&fixture, "api-2768", "GET", "/items/{}",
                                              "handle_get_item") &&
                 cross_repo_seed_server_route(&fixture, "api-2768", "POST", "/items/{}/{}",
                                              "handle_post_nested_item") &&
                 cross_repo_seed_server_route(&fixture, "api-2768", "GET", "/items/job/{}",
                                              "handle_get_job") &&
                 /* Angular client making GET requests */
                 cross_repo_seed_client_call(&fixture, "angular-2768", "getItems",
                                             "this.http.get", "/items/list", "GET") &&
                 cross_repo_seed_client_call(&fixture, "angular-2768", "getItem",
                                             "this.http.get", "/items/42", NULL) &&
                 cross_repo_seed_client_call(&fixture, "angular-2768", "getJob",
                                             "this.http.get", "/items/job/10", NULL);
    if (!setup) {
        cross_repo_fixture_end(&fixture);
        FAIL("failed to seed issue 2768 fixture");
    }

    const char *target = "api-2768";
    cbm_cross_repo_result_t fwd = cbm_cross_repo_match("angular-2768", &target, 1);
    ASSERT_FALSE(fwd.failed);
    ASSERT_EQ(fwd.http_edges, 3);

    /* Assert that client CROSS_HTTP_CALLS edges are linked to GET handlers, not DELETE or POST */
    ASSERT_EQ(cross_repo_count_edges_like(&fixture, "angular-2768", "CROSS_HTTP_CALLS",
                                          "%handle_delete_item%"),
              0);
    ASSERT_EQ(cross_repo_count_edges_like(&fixture, "angular-2768", "CROSS_HTTP_CALLS",
                                          "%handle_post_nested_item%"),
              0);
    ASSERT_EQ(cross_repo_count_edges_like(&fixture, "angular-2768", "CROSS_HTTP_CALLS",
                                          "%handle_list_items%"),
              1);
    ASSERT_EQ(cross_repo_count_edges_like(&fixture, "angular-2768", "CROSS_HTTP_CALLS",
                                          "%handle_get_item%"),
              1);
    ASSERT_EQ(cross_repo_count_edges_like(&fixture, "angular-2768", "CROSS_HTTP_CALLS",
                                          "%handle_get_job%"),
              1);

    /* Reverse pass */
    const char *consumer = "angular-2768";
    cbm_cross_repo_result_t rev = cbm_cross_repo_match("api-2768", &consumer, 1);
    ASSERT_FALSE(rev.failed);
    ASSERT_EQ(rev.http_edges, 3);

    cross_repo_fixture_end(&fixture);
    cbm_cross_repo_result_free(&fwd);
    cbm_cross_repo_result_free(&rev);
    PASS();
}

/* A GraphQL field Route can carry more than one HANDLES edge (the gqlgen
 * resolver method and a schema Field). find_route_handler took LIMIT 1 in row
 * order, so the link landed on whichever edge was written first; it must
 * prefer the resolver Method. The Field's HANDLES is seeded first here, so
 * row order alone would pick it. */
TEST(cross_repo_graphql_prefers_resolver_method_handler) {
    cross_repo_fixture_t fixture;
    char client_path[512];
    char server_path[512];
    bool ok = cross_repo_fixture_begin(&fixture) &&
              cross_repo_project_path(&fixture, "gql-client", client_path, sizeof(client_path)) &&
              cross_repo_project_path(&fixture, "gql-server", server_path, sizeof(server_path));
    cbm_store_t *client = ok ? cbm_store_open_path(client_path) : NULL;
    cbm_store_t *server = ok ? cbm_store_open_path(server_path) : NULL;
    ok = client && server &&
         cbm_store_upsert_project(client, "gql-client", fixture.cache) == CBM_STORE_OK &&
         cbm_store_upsert_project(server, "gql-server", fixture.cache) == CBM_STORE_OK;

    const char *route_qn = "__graphql__Query.perp.trades";
    cbm_node_t caller = {.project = "gql-client",
                         .label = "Function",
                         .name = "loadTrades",
                         .qualified_name = "gql-client.perp.loadTrades",
                         .file_path = "perp.ts"};
    cbm_node_t client_route = {.project = "gql-client",
                               .label = "Route",
                               .name = "Query.perp.trades",
                               .qualified_name = route_qn,
                               .file_path = "perp.ts"};
    int64_t caller_id = ok ? cbm_store_upsert_node(client, &caller) : 0;
    int64_t client_route_id = ok ? cbm_store_upsert_node(client, &client_route) : 0;
    cbm_edge_t call = {.project = "gql-client",
                       .source_id = caller_id,
                       .target_id = client_route_id,
                       .type = "GRAPHQL_CALLS",
                       .properties_json = "{\"callee\":\"gql\",\"operation\":\"PerpTrades\","
                                          "\"operation_type\":\"query\","
                                          "\"field_path\":\"perp.trades\"}"};
    ok = ok && caller_id > 0 && client_route_id > 0 && cbm_store_insert_edge(client, &call) > 0;

    cbm_node_t server_route = {.project = "gql-server",
                               .label = "Route",
                               .name = "Query.perp.trades",
                               .qualified_name = route_qn,
                               .file_path = "graph/perp.graphqls"};
    cbm_node_t field = {.project = "gql-server",
                        .label = "Field",
                        .name = "trades",
                        .qualified_name = "gql-server.graph.perp.Perp.trades",
                        .file_path = "graph/perp.graphqls"};
    cbm_node_t method = {.project = "gql-server",
                         .label = "Method",
                         .name = "Trades",
                         .qualified_name = "gql-server.graph.resolver.perpResolver.Trades",
                         .file_path = "graph/resolver/perp.resolvers.go"};
    int64_t server_route_id = ok ? cbm_store_upsert_node(server, &server_route) : 0;
    int64_t field_id = ok ? cbm_store_upsert_node(server, &field) : 0;
    int64_t method_id = ok ? cbm_store_upsert_node(server, &method) : 0;
    cbm_edge_t by_field = {.project = "gql-server",
                           .source_id = field_id,
                           .target_id = server_route_id,
                           .type = "HANDLES",
                           .properties_json = "{\"via\":\"graphql_schema_field\"}"};
    cbm_edge_t by_method = {.project = "gql-server",
                            .source_id = method_id,
                            .target_id = server_route_id,
                            .type = "HANDLES",
                            .properties_json = "{\"via\":\"gqlgen_resolver\"}"};
    ok = ok && server_route_id > 0 && field_id > 0 && method_id > 0 &&
         cbm_store_insert_edge(server, &by_field) > 0 &&
         cbm_store_insert_edge(server, &by_method) > 0;
    cbm_store_close(client);
    cbm_store_close(server);
    if (!ok) {
        cross_repo_fixture_end(&fixture);
        FAIL("failed to seed GraphQL handler fixture");
    }

    const char *target = "gql-server";
    cbm_cross_repo_result_t fwd = cbm_cross_repo_match("gql-client", &target, 1);
    bool failed = fwd.failed;
    int graphql_edges = fwd.graphql_edges;
    int to_resolver = cross_repo_count_edges_like(&fixture, "gql-client", "CROSS_GRAPHQL_CALLS",
                                                  "%perp.resolvers.go%");
    int to_schema = cross_repo_count_edges_like(&fixture, "gql-client", "CROSS_GRAPHQL_CALLS",
                                                "%perp.graphqls%");
    /* Restore CBM_CACHE_DIR before asserting: a failed ASSERT returns early,
     * and a leaked cache dir would break every later suite. */
    cbm_cross_repo_result_free(&fwd);
    cross_repo_fixture_end(&fixture);
    ASSERT_FALSE(failed);
    ASSERT_EQ(graphql_edges, 1);
    ASSERT_EQ(to_resolver, 1);
    ASSERT_EQ(to_schema, 0);
    PASS();
}

/* Seed a GraphQL client and server: `ncallers` client functions (file
 * caller<i>.ts, or with `same_name` all methods `load` of classes C<i> in
 * client.ts) each selecting `route_qn`, and a server Route `route_qn` with
 * props `route_props`, handled by one resolver method. */
static bool cross_repo_seed_graphql_ex(const cross_repo_fixture_t *fixture, const char *route_qn,
                                       int ncallers, const char *route_props, bool same_name) {
    char client_path[512];
    char server_path[512];
    if (!cross_repo_project_path(fixture, "gql-client", client_path, sizeof(client_path)) ||
        !cross_repo_project_path(fixture, "gql-server", server_path, sizeof(server_path))) {
        return false;
    }
    cbm_store_t *client = cbm_store_open_path(client_path);
    cbm_store_t *server = cbm_store_open_path(server_path);
    bool ok = client && server &&
              cbm_store_upsert_project(client, "gql-client", fixture->cache) == CBM_STORE_OK &&
              cbm_store_upsert_project(server, "gql-server", fixture->cache) == CBM_STORE_OK;
    const char *name = route_qn + strlen("__graphql__");
    cbm_node_t client_route = {.project = "gql-client",
                               .label = "Route",
                               .name = name,
                               .qualified_name = route_qn,
                               .file_path = ""};
    int64_t client_route_id = ok ? cbm_store_upsert_node(client, &client_route) : 0;
    ok = ok && client_route_id > 0;
    for (int i = 0; ok && i < ncallers; i++) {
        char fn[64];
        char qn[128];
        char file[64];
        if (same_name) {
            snprintf(fn, sizeof(fn), "load");
            snprintf(qn, sizeof(qn), "gql-client.client.C%d.load", i);
            snprintf(file, sizeof(file), "client.ts");
        } else {
            snprintf(fn, sizeof(fn), "load%d", i);
            snprintf(qn, sizeof(qn), "gql-client.caller%d.load%d", i, i);
            snprintf(file, sizeof(file), "caller%d.ts", i);
        }
        cbm_node_t caller = {.project = "gql-client",
                             .label = "Function",
                             .name = fn,
                             .qualified_name = qn,
                             .file_path = file};
        int64_t caller_id = cbm_store_upsert_node(client, &caller);
        cbm_edge_t call = {.project = "gql-client",
                           .source_id = caller_id,
                           .target_id = client_route_id,
                           .type = "GRAPHQL_CALLS",
                           .properties_json = "{\"callee\":\"gql\",\"operation\":\"Op\","
                                              "\"operation_type\":\"query\"}"};
        ok = caller_id > 0 && cbm_store_insert_edge(client, &call) > 0;
    }
    cbm_node_t server_route = {.project = "gql-server",
                               .label = "Route",
                               .name = name,
                               .qualified_name = route_qn,
                               .file_path = "graph/schema.graphqls",
                               .properties_json = route_props};
    cbm_node_t method = {.project = "gql-server",
                         .label = "Method",
                         .name = "Ping",
                         .qualified_name = "gql-server.graph.resolver.queryResolver.Ping",
                         .file_path = "graph/resolver/schema.resolvers.go"};
    int64_t server_route_id = ok ? cbm_store_upsert_node(server, &server_route) : 0;
    int64_t method_id = ok ? cbm_store_upsert_node(server, &method) : 0;
    cbm_edge_t handles = {.project = "gql-server",
                          .source_id = method_id,
                          .target_id = server_route_id,
                          .type = "HANDLES",
                          .properties_json = "{\"via\":\"gqlgen_resolver\"}"};
    ok = ok && server_route_id > 0 && method_id > 0 && cbm_store_insert_edge(server, &handles) > 0;
    cbm_store_close(client);
    cbm_store_close(server);
    return ok;
}

static bool cross_repo_seed_graphql(const cross_repo_fixture_t *fixture, const char *route_qn,
                                    int ncallers, const char *route_props) {
    return cross_repo_seed_graphql_ex(fixture, route_qn, ncallers, route_props, false);
}

/* One integer from an SQL query over a project's store, -1 on error. */
static int cross_repo_query_int(const cross_repo_fixture_t *fixture, const char *project,
                                const char *sql) {
    char path[512];
    sqlite3 *db = NULL;
    sqlite3_stmt *st = NULL;
    int n = -1;
    if (cross_repo_project_path(fixture, project, path, sizeof(path)) &&
        sqlite3_open_v2(path, &db, SQLITE_OPEN_READONLY, NULL) == SQLITE_OK &&
        sqlite3_prepare_v2(db, sql, -1, &st, NULL) == SQLITE_OK && sqlite3_step(st) == SQLITE_ROW) {
        n = sqlite3_column_int(st, 0);
    }
    sqlite3_finalize(st);
    sqlite3_close(db);
    return n;
}

/* Three client functions select one field: the server's reverse edge (one
 * row per handler, Route and type) names every caller under "callers", not
 * only the last one written. */
TEST(cross_repo_graphql_reverse_edge_keeps_every_caller) {
    cross_repo_fixture_t fixture;
    bool ok = cross_repo_fixture_begin(&fixture) &&
              cross_repo_seed_graphql(&fixture, "__graphql__Query.ping", 3,
                                      "{\"source\":\"graphql_schema\"}");
    if (!ok) {
        cross_repo_fixture_end(&fixture);
        FAIL("failed to seed GraphQL callers fixture");
    }
    const char *target = "gql-server";
    cbm_cross_repo_result_t fwd = cbm_cross_repo_match("gql-client", &target, 1);
    bool failed = fwd.failed;
    int graphql_edges = fwd.graphql_edges;
    cbm_cross_repo_result_free(&fwd);
    int forward = cross_repo_query_int(
        &fixture, "gql-client", "SELECT COUNT(*) FROM edges WHERE type = 'CROSS_GRAPHQL_CALLS'");
    int reverse = cross_repo_query_int(
        &fixture, "gql-server", "SELECT COUNT(*) FROM edges WHERE type = 'CROSS_GRAPHQL_CALLS'");
    int callers = cross_repo_query_int(
        &fixture, "gql-server",
        "SELECT COUNT(DISTINCT json_extract(c.value, '$.function') || ' ' || "
        "json_extract(c.value, '$.file')) FROM edges e, json_each(e.properties, '$.callers') c "
        "WHERE e.type = 'CROSS_GRAPHQL_CALLS' AND json_extract(c.value, '$.project') = "
        "'gql-client'");
    cross_repo_fixture_end(&fixture);
    ASSERT_FALSE(failed);
    ASSERT_EQ(graphql_edges, 3);
    ASSERT_EQ(forward, 3);
    ASSERT_EQ(reverse, 1);
    ASSERT_EQ(callers, 3);
    PASS();
}

static const char CROSS_REPO_GQL_CALLERS[] =
    "SELECT COUNT(*) FROM edges e, json_each(e.properties, '$.callers') c "
    "WHERE e.type = 'CROSS_GRAPHQL_CALLS' AND json_extract(c.value, '$.project') = 'gql-client'";

/* Methods A.load and B.load in one client file are two callers: the
 * reverse entry is keyed by the caller's qualified name, not by its file and
 * display name, which they share. */
TEST(cross_repo_graphql_reverse_callers_keyed_by_qualified_name) {
    cross_repo_fixture_t fixture;
    bool ok = cross_repo_fixture_begin(&fixture) &&
              cross_repo_seed_graphql_ex(&fixture, "__graphql__Query.ping", 2,
                                         "{\"source\":\"graphql_schema\"}", true);
    if (!ok) {
        cross_repo_fixture_end(&fixture);
        FAIL("failed to seed GraphQL same-name fixture");
    }
    const char *target = "gql-server";
    cbm_cross_repo_result_t fwd = cbm_cross_repo_match("gql-client", &target, 1);
    bool failed = fwd.failed;
    cbm_cross_repo_result_free(&fwd);
    int callers = cross_repo_query_int(&fixture, "gql-server", CROSS_REPO_GQL_CALLERS);
    cross_repo_fixture_end(&fixture);
    ASSERT_FALSE(failed);
    ASSERT_EQ(callers, 2);
    PASS();
}

/* A caller deleted from the client and relinked leaves the provider's
 * reverse callers; a link run from the provider side keeps the client's
 * callers (it used to delete the provider's GraphQL reverse rows and never
 * recreate them). */
TEST(cross_repo_graphql_reverse_callers_follow_relinks) {
    cross_repo_fixture_t fixture;
    bool ok = cross_repo_fixture_begin(&fixture) &&
              cross_repo_seed_graphql(&fixture, "__graphql__Query.ping", 2,
                                      "{\"source\":\"graphql_schema\"}");
    if (!ok) {
        cross_repo_fixture_end(&fixture);
        FAIL("failed to seed GraphQL relink fixture");
    }
    const char *server = "gql-server";
    const char *client = "gql-client";
    cbm_cross_repo_result_t r1 = cbm_cross_repo_match("gql-client", &server, 1);
    bool failed = r1.failed;
    cbm_cross_repo_result_free(&r1);
    int linked = cross_repo_query_int(&fixture, "gql-server", CROSS_REPO_GQL_CALLERS);

    cbm_cross_repo_result_t r2 = cbm_cross_repo_match("gql-server", &client, 1);
    failed = failed || r2.failed;
    cbm_cross_repo_result_free(&r2);
    int from_provider = cross_repo_query_int(&fixture, "gql-server", CROSS_REPO_GQL_CALLERS);

    char client_path[512];
    cbm_store_t *store =
        cross_repo_project_path(&fixture, "gql-client", client_path, sizeof(client_path))
            ? cbm_store_open_path(client_path)
            : NULL;
    ok = store && cbm_store_exec(store,
                                 "DELETE FROM edges WHERE source_id IN (SELECT id FROM nodes WHERE "
                                 "qualified_name = 'gql-client.caller0.load0');"
                                 "DELETE FROM nodes WHERE qualified_name = "
                                 "'gql-client.caller0.load0';") == CBM_STORE_OK;
    cbm_store_close(store);
    cbm_cross_repo_result_t r3 = cbm_cross_repo_match("gql-client", &server, 1);
    failed = failed || r3.failed;
    cbm_cross_repo_result_free(&r3);
    int relinked = cross_repo_query_int(&fixture, "gql-server", CROSS_REPO_GQL_CALLERS);
    int stale = cross_repo_query_int(
        &fixture, "gql-server",
        "SELECT COUNT(*) FROM edges e, json_each(e.properties, '$.callers') c "
        "WHERE e.type = 'CROSS_GRAPHQL_CALLS' AND json_extract(c.value, '$.function') = 'load0'");
    int top_level_stale = cross_repo_query_int(
        &fixture, "gql-server",
        "SELECT COUNT(*) FROM edges WHERE type = 'CROSS_GRAPHQL_CALLS' AND "
        "json_extract(properties, '$.target_function') = 'load0'");
    cross_repo_fixture_end(&fixture);
    ASSERT_TRUE(ok);
    ASSERT_FALSE(failed);
    ASSERT_EQ(linked, 2);
    ASSERT_EQ(from_provider, 2);
    ASSERT_EQ(relinked, 1);
    ASSERT_EQ(stale, 0);
    ASSERT_EQ(top_level_stale, 0);
    PASS();
}

/* A server Route the route pass marked ambiguous (two APIs in one checkout
 * define the field): the call is linked to the target project and marked
 * ambiguous, but names no handler (any one would be a guess), has no reverse
 * edge and is not counted as resolved. */
TEST(cross_repo_graphql_ambiguous_route_marked) {
    cross_repo_fixture_t fixture;
    bool ok = cross_repo_fixture_begin(&fixture) &&
              cross_repo_seed_graphql(&fixture, "__graphql__Query.ping", 1,
                                      "{\"source\":\"graphql_schema\",\"ambiguous\":true}");
    if (!ok) {
        cross_repo_fixture_end(&fixture);
        FAIL("failed to seed GraphQL ambiguous fixture");
    }
    const char *target = "gql-server";
    cbm_cross_repo_result_t fwd = cbm_cross_repo_match("gql-client", &target, 1);
    bool failed = fwd.failed;
    int graphql_edges = fwd.graphql_edges;
    cbm_cross_repo_result_free(&fwd);
    int fwd_marked = cross_repo_query_int(
        &fixture, "gql-client",
        "SELECT COUNT(*) FROM edges WHERE type = 'CROSS_GRAPHQL_CALLS' AND "
        "json_extract(properties, '$.ambiguous') = 1 AND "
        "json_extract(properties, '$.target_project') = 'gql-server' AND "
        "json_extract(properties, '$.target_function') = '' AND "
        "json_extract(properties, '$.target_file') = ''");
    int fwd_total = cross_repo_query_int(
        &fixture, "gql-client", "SELECT COUNT(*) FROM edges WHERE type = 'CROSS_GRAPHQL_CALLS'");
    int reverse = cross_repo_query_int(
        &fixture, "gql-server", "SELECT COUNT(*) FROM edges WHERE type = 'CROSS_GRAPHQL_CALLS'");
    cross_repo_fixture_end(&fixture);
    ASSERT_FALSE(failed);
    ASSERT_EQ(graphql_edges, 0);
    ASSERT_EQ(fwd_marked, 1);
    ASSERT_EQ(fwd_total, 1);
    ASSERT_EQ(reverse, 0);
    PASS();
}

/* A Route QN longer than the matcher's old 511-byte buffer: the exact lookup
 * uses the whole QN. A distinct server Route whose QN is the long one's
 * 511-byte prefix must not be linked in its place. */
TEST(cross_repo_graphql_long_route_qn_exact) {
    char field[506];
    memset(field, 'f', sizeof(field) - 1);
    field[sizeof(field) - 1] = '\0';
    char qn[600];
    snprintf(qn, sizeof(qn), "__graphql__Query.%s", field); /* 522 bytes */
    char prefix[512];
    snprintf(prefix, sizeof(prefix), "%s", qn); /* the first 511 */
    cross_repo_fixture_t fixture;
    bool ok = cross_repo_fixture_begin(&fixture) &&
              cross_repo_seed_graphql(&fixture, qn, 1, "{\"source\":\"graphql_schema\"}");
    char server_path[512];
    cbm_store_t *server =
        ok && cross_repo_project_path(&fixture, "gql-server", server_path, sizeof(server_path))
            ? cbm_store_open_path(server_path)
            : NULL;
    cbm_node_t decoy_route = {.project = "gql-server",
                              .label = "Route",
                              .name = prefix + strlen("__graphql__"),
                              .qualified_name = prefix,
                              .file_path = "graph/other.graphqls"};
    cbm_node_t decoy = {.project = "gql-server",
                        .label = "Function",
                        .name = "Decoy",
                        .qualified_name = "gql-server.graph.Decoy",
                        .file_path = "graph/decoy.go"};
    int64_t decoy_route_id = server ? cbm_store_upsert_node(server, &decoy_route) : 0;
    int64_t decoy_id = server ? cbm_store_upsert_node(server, &decoy) : 0;
    cbm_edge_t handles = {.project = "gql-server",
                          .source_id = decoy_id,
                          .target_id = decoy_route_id,
                          .type = "HANDLES",
                          .properties_json = "{}"};
    ok = ok && decoy_route_id > 0 && decoy_id > 0 && cbm_store_insert_edge(server, &handles) > 0;
    cbm_store_close(server);
    if (!ok) {
        cross_repo_fixture_end(&fixture);
        FAIL("failed to seed GraphQL long-QN fixture");
    }
    const char *target = "gql-server";
    cbm_cross_repo_result_t fwd = cbm_cross_repo_match("gql-client", &target, 1);
    bool failed = fwd.failed;
    int graphql_edges = fwd.graphql_edges;
    cbm_cross_repo_result_free(&fwd);
    int to_resolver = cross_repo_count_edges_like(&fixture, "gql-client", "CROSS_GRAPHQL_CALLS",
                                                  "%schema.resolvers.go%");
    int to_decoy = cross_repo_count_edges_like(&fixture, "gql-client", "CROSS_GRAPHQL_CALLS",
                                               "%decoy.go%");
    cross_repo_fixture_end(&fixture);
    ASSERT_FALSE(failed);
    ASSERT_EQ(graphql_edges, 1);
    ASSERT_EQ(to_resolver, 1);
    ASSERT_EQ(to_decoy, 0);
    PASS();
}

/* One more GraphQL server project: Route `route_qn` with props
 * `route_props`, handled by Method `handler` in `file`. */
static bool cross_repo_seed_graphql_server(const cross_repo_fixture_t *fixture,
                                           const char *project, const char *route_qn,
                                           const char *route_props, const char *handler,
                                           const char *file) {
    char path[512];
    if (!cross_repo_project_path(fixture, project, path, sizeof(path))) {
        return false;
    }
    cbm_store_t *store = cbm_store_open_path(path);
    bool ok = store && cbm_store_upsert_project(store, project, fixture->cache) == CBM_STORE_OK;
    char handler_qn[256];
    snprintf(handler_qn, sizeof(handler_qn), "%s.graph.%s", project, handler);
    cbm_node_t route = {.project = project,
                        .label = "Route",
                        .name = route_qn + strlen("__graphql__"),
                        .qualified_name = route_qn,
                        .file_path = "graph/schema.graphqls",
                        .properties_json = route_props};
    cbm_node_t method = {.project = project,
                         .label = "Method",
                         .name = handler,
                         .qualified_name = handler_qn,
                         .file_path = file};
    int64_t route_id = ok ? cbm_store_upsert_node(store, &route) : 0;
    int64_t method_id = ok ? cbm_store_upsert_node(store, &method) : 0;
    cbm_edge_t handles = {.project = project,
                          .source_id = method_id,
                          .target_id = route_id,
                          .type = "HANDLES",
                          .properties_json = "{\"via\":\"gqlgen_resolver\"}"};
    ok = ok && route_id > 0 && method_id > 0 && cbm_store_insert_edge(store, &handles) > 0;
    cbm_store_close(store);
    return ok;
}

/* A second server exposing the same field, run from its own side with the
 * client as a target, never redirects the client's links: the client linked
 * gql-server, so every forward edge still names gql-server, gql-server keeps
 * its callers, and gql-server-b gains no reverse rows. (rev-5 :1780, rev-6
 * finding 1: the provider pass used to json_patch the client's forward rows
 * to name gql-server-b.) */
TEST(cross_repo_graphql_provider_run_keeps_client_target) {
    cross_repo_fixture_t fixture;
    bool ok = cross_repo_fixture_begin(&fixture) &&
              cross_repo_seed_graphql(&fixture, "__graphql__Query.oracle", 3,
                                      "{\"source\":\"graphql_schema\"}") &&
              cross_repo_seed_graphql_server(&fixture, "gql-server-b", "__graphql__Query.oracle",
                                             "{\"source\":\"graphql_schema\"}", "Oracle",
                                             "graph/oracle.resolvers.go");
    if (!ok) {
        cross_repo_fixture_end(&fixture);
        FAIL("failed to seed two GraphQL servers");
    }
    const char *server = "gql-server";
    const char *client = "gql-client";
    cbm_cross_repo_result_t r1 = cbm_cross_repo_match("gql-client", &server, 1);
    bool failed = r1.failed;
    cbm_cross_repo_result_free(&r1);
    cbm_cross_repo_result_t r2 = cbm_cross_repo_match("gql-server-b", &client, 1);
    failed = failed || r2.failed;
    int b_edges = r2.graphql_edges;
    cbm_cross_repo_result_free(&r2);
    int to_a = cross_repo_query_int(
        &fixture, "gql-client",
        "SELECT COUNT(*) FROM edges WHERE type = 'CROSS_GRAPHQL_CALLS' AND "
        "json_extract(properties, '$.target_project') = 'gql-server'");
    int to_b = cross_repo_query_int(
        &fixture, "gql-client",
        "SELECT COUNT(*) FROM edges WHERE type = 'CROSS_GRAPHQL_CALLS' AND "
        "json_extract(properties, '$.target_project') = 'gql-server-b'");
    int a_callers = cross_repo_query_int(&fixture, "gql-server", CROSS_REPO_GQL_CALLERS);
    int b_reverse = cross_repo_query_int(
        &fixture, "gql-server-b", "SELECT COUNT(*) FROM edges WHERE type = 'CROSS_GRAPHQL_CALLS'");
    cross_repo_fixture_end(&fixture);
    ASSERT_FALSE(failed);
    ASSERT_EQ(to_a, 3);
    ASSERT_EQ(to_b, 0);
    ASSERT_EQ(a_callers, 3);
    ASSERT_EQ(b_reverse, 0);
    ASSERT_EQ(b_edges, 0);
    PASS();
}

/* The provider drops a field, is reindexed (its Route gone), and is
 * relinked from its own side: the client's forward link to it goes too.
 * (rev-5 :1724) */
TEST(cross_repo_graphql_provider_run_drops_removed_route_link) {
    cross_repo_fixture_t fixture;
    bool ok = cross_repo_fixture_begin(&fixture) &&
              cross_repo_seed_graphql(&fixture, "__graphql__Query.ping", 1,
                                      "{\"source\":\"graphql_schema\"}");
    if (!ok) {
        cross_repo_fixture_end(&fixture);
        FAIL("failed to seed GraphQL removal fixture");
    }
    const char *server = "gql-server";
    const char *client = "gql-client";
    cbm_cross_repo_result_t r1 = cbm_cross_repo_match("gql-client", &server, 1);
    bool failed = r1.failed;
    cbm_cross_repo_result_free(&r1);
    int linked = cross_repo_query_int(
        &fixture, "gql-client", "SELECT COUNT(*) FROM edges WHERE type = 'CROSS_GRAPHQL_CALLS'");
    char server_path[512];
    cbm_store_t *store =
        cross_repo_project_path(&fixture, "gql-server", server_path, sizeof(server_path))
            ? cbm_store_open_path(server_path)
            : NULL;
    ok = store && cbm_store_exec(store,
                                 "DELETE FROM edges WHERE target_id IN (SELECT id FROM nodes WHERE "
                                 "qualified_name = '__graphql__Query.ping');"
                                 "DELETE FROM nodes WHERE qualified_name = "
                                 "'__graphql__Query.ping';") == CBM_STORE_OK;
    cbm_store_close(store);
    cbm_cross_repo_result_t r2 = cbm_cross_repo_match("gql-server", &client, 1);
    failed = failed || r2.failed;
    cbm_cross_repo_result_free(&r2);
    int after = cross_repo_query_int(
        &fixture, "gql-client", "SELECT COUNT(*) FROM edges WHERE type = 'CROSS_GRAPHQL_CALLS'");
    cross_repo_fixture_end(&fixture);
    ASSERT_TRUE(ok);
    ASSERT_FALSE(failed);
    ASSERT_EQ(linked, 1);
    ASSERT_EQ(after, 0);
    PASS();
}

/* One client run against an ambiguous target and a resolved one, in either
 * order: the forward row names the resolved server, with its handler and
 * "ambiguous":false, and only it is counted. (rev-5 :1235: the flag stuck
 * through json_patch, or the ambiguous write replaced the resolved one.) */
static int cross_repo_ambiguous_then_resolved(bool resolved_first) {
    cross_repo_fixture_t fixture;
    bool ok = cross_repo_fixture_begin(&fixture) &&
              cross_repo_seed_graphql(&fixture, "__graphql__Query.ping", 1,
                                      "{\"source\":\"graphql_schema\",\"ambiguous\":true}") &&
              cross_repo_seed_graphql_server(&fixture, "gql-server-b", "__graphql__Query.ping",
                                             "{\"source\":\"graphql_schema\"}", "Ping",
                                             "graph/b.resolvers.go");
    if (!ok) {
        cross_repo_fixture_end(&fixture);
        return 0;
    }
    const char *targets[2] = {resolved_first ? "gql-server-b" : "gql-server",
                              resolved_first ? "gql-server" : "gql-server-b"};
    cbm_cross_repo_result_t r = cbm_cross_repo_match("gql-client", targets, 2);
    bool failed = r.failed;
    int counted = r.graphql_edges;
    cbm_cross_repo_result_free(&r);
    int resolved_rows = cross_repo_query_int(
        &fixture, "gql-client",
        "SELECT COUNT(*) FROM edges WHERE type = 'CROSS_GRAPHQL_CALLS' AND "
        "json_extract(properties, '$.target_project') = 'gql-server-b' AND "
        "json_extract(properties, '$.target_file') = 'graph/b.resolvers.go' AND "
        "json_extract(properties, '$.ambiguous') = 0");
    int rows = cross_repo_query_int(
        &fixture, "gql-client", "SELECT COUNT(*) FROM edges WHERE type = 'CROSS_GRAPHQL_CALLS'");
    cross_repo_fixture_end(&fixture);
    if (failed || counted != 1 || resolved_rows != 1 || rows != 1) {
        fprintf(stderr, "  resolved_first=%d failed=%d counted=%d resolved_rows=%d rows=%d\n",
                resolved_first, failed, counted, resolved_rows, rows);
        return 0;
    }
    return 1;
}

TEST(cross_repo_graphql_resolved_target_beats_ambiguous_in_one_run) {
    ASSERT_TRUE(cross_repo_ambiguous_then_resolved(false));
    ASSERT_TRUE(cross_repo_ambiguous_then_resolved(true));
    PASS();
}

TEST(cross_repo_stage_rebuilds_from_legacy_selection_without_writing_provider) {
    cross_repo_fixture_t fixture;
    char live[512], stage[512];
    bool ok = cross_repo_fixture_begin(&fixture) &&
              cross_repo_seed_http_pair(&fixture, "linked-source", "provider", "/known", "known") &&
              cross_repo_project_path(&fixture, "linked-source", live, sizeof(live)) &&
              cross_repo_project_path(&fixture, "_stage", stage, sizeof(stage));
    if (!ok) {
        cross_repo_fixture_end(&fixture);
        FAIL("cannot prepare matching project stores");
    }

    const char *target = "provider";
    cbm_cross_repo_result_t linked = cbm_cross_repo_match("linked-source", &target, 1);
    ok = !linked.failed && linked.http_edges == 1 &&
         cross_repo_exec(&fixture, "linked-source",
                         "DELETE FROM store_meta WHERE k='cross_repo_targets:linked-source'") &&
         cbm_store_backup_path(live, stage) == CBM_STORE_OK &&
         cross_repo_exec(&fixture, "_stage", "DELETE FROM edges WHERE type='CROSS_HTTP_CALLS'") &&
         cross_repo_exec(&fixture, "provider", "DELETE FROM edges WHERE type='CROSS_HTTP_CALLS'");
    cbm_cross_repo_result_free(&linked);

    int relinked =
        ok ? cbm_cross_repo_relink_stage(live, stage, "linked-source", NULL) : CBM_STORE_ERR;
    cbm_store_t *staged = cbm_store_open_path_query(stage);
    int staged_links =
        staged ? cbm_store_count_edges_by_type(staged, "linked-source", "CROSS_HTTP_CALLS") : -1;
    char *selection = NULL;
    int recorded = staged
                       ? cbm_store_meta_get(staged, "cross_repo_targets:linked-source", &selection)
                       : CBM_STORE_ERR;
    bool migrated = recorded == CBM_STORE_OK && strcmp(selection, "[\"provider\"]") == 0;
    cbm_free(CBM_MEM_CLASS_STORE, selection);
    cbm_store_close(staged);
    int provider_links = cross_repo_count_edges(&fixture, "provider", "CROSS_HTTP_CALLS");
    bool repaired = cbm_store_backup_path(stage, live) == CBM_STORE_OK &&
                    cbm_cross_repo_reverse_pending("linked-source") &&
                    cbm_cross_repo_reconcile_published("linked-source", NULL) &&
                    cbm_cross_repo_reconcile_published("linked-source", NULL) &&
                    !cbm_cross_repo_reverse_pending("linked-source");
    int reverse_links = cross_repo_count_edges(&fixture, "provider", "CROSS_HTTP_CALLS");
    cross_repo_fixture_end(&fixture);

    ASSERT_TRUE(ok);
    ASSERT_EQ(relinked, CBM_STORE_OK);
    ASSERT_EQ(staged_links, 1);
    ASSERT_EQ(provider_links, 0);
    ASSERT_TRUE(migrated);
    ASSERT_TRUE(repaired);
    ASSERT_EQ(reverse_links, 1);
    PASS();
}

TEST(cross_repo_cancel_repair_rolls_back_peer_and_keeps_pending) {
    cross_repo_fixture_t fixture;
    char live[512], stage[512];
    bool ok =
        cross_repo_fixture_begin(&fixture) &&
        cross_repo_seed_http_pair(&fixture, "cancel-source", "cancel-target-b", "/rel", "b") &&
        cross_repo_project_path(&fixture, "cancel-source", live, sizeof(live)) &&
        cross_repo_project_path(&fixture, "_stage", stage, sizeof(stage));
    if (!ok) {
        cross_repo_fixture_end(&fixture);
        FAIL("cannot prepare repair cancellation fixture");
    }

    const char *target = "cancel-target-b";
    cbm_cross_repo_result_t linked = cbm_cross_repo_match("cancel-source", &target, 1);
    ok = !linked.failed && cbm_store_backup_path(live, stage) == CBM_STORE_OK &&
         cbm_cross_repo_relink_stage(live, stage, "cancel-source", NULL) == CBM_STORE_OK &&
         cbm_store_backup_path(stage, live) == CBM_STORE_OK &&
         cross_repo_exec(&fixture, target,
                         "UPDATE edges SET properties=json_set(properties, '$.target_function',"
                         "'before-repair') WHERE type='CROSS_HTTP_CALLS'");
    cbm_cross_repo_result_free(&linked);

    atomic_int cancelled = 0;
    cross_repo_cancel_hook_t hook = {.cancelled = &cancelled, .project = target};
    cbm_cross_repo_set_after_insert_hook_for_tests(cross_repo_cancel_after_write, &hook);
    bool cancelled_repair = ok && !cbm_cross_repo_reconcile_published("cancel-source", &cancelled);
    cbm_cross_repo_set_after_insert_hook_for_tests(NULL, NULL);
    bool pending = cbm_cross_repo_reverse_pending("cancel-source");
    int old =
        cross_repo_query_int(&fixture, target,
                             "SELECT COUNT(*) FROM edges WHERE type='CROSS_HTTP_CALLS' "
                             "AND json_extract(properties, '$.target_function')='before-repair'");

    bool retried = cbm_cross_repo_reconcile_published("cancel-source", NULL) &&
                   !cbm_cross_repo_reverse_pending("cancel-source");
    int updated =
        cross_repo_query_int(&fixture, target,
                             "SELECT COUNT(*) FROM edges WHERE type='CROSS_HTTP_CALLS' "
                             "AND json_extract(properties, '$.target_function')='call_remote'");
    cross_repo_fixture_end(&fixture);

    ASSERT_TRUE(ok);
    ASSERT_TRUE(cancelled_repair);
    ASSERT_EQ(hook.fired, 1);
    ASSERT_TRUE(pending);
    ASSERT_EQ(old, 1);
    ASSERT_TRUE(retried);
    ASSERT_EQ(updated, 1);
    PASS();
}

/* A held lock outlives the repair attempt. The watcher must defer within
 * the daemon's shutdown budget even if SQLite has no progress to report. */
static bool cross_repo_repair_defers_under_lock(const char *provider_path, const char *lock_sql) {
    sqlite3 *lock = NULL;
    bool held = sqlite3_open_v2(provider_path, &lock, SQLITE_OPEN_READWRITE, NULL) == SQLITE_OK &&
                sqlite3_exec(lock, lock_sql, NULL, NULL, NULL) == SQLITE_OK;
    atomic_int stop = 0;
    uint64_t started = cbm_now_ms();
    bool deferred = held && !cbm_cross_repo_reconcile_published("busy-client", &stop);
    uint64_t elapsed = cbm_now_ms() - started;
    if (lock) {
        (void)sqlite3_exec(lock, "ROLLBACK", NULL, NULL, NULL);
        sqlite3_close(lock);
    }
    return held && deferred && elapsed < 5000 && cbm_cross_repo_reverse_pending("busy-client");
}

TEST(cross_repo_repair_defers_on_busy_open_and_write) {
    cross_repo_fixture_t fixture;
    char live[512], stage[512], provider[512];
    bool ok = cross_repo_fixture_begin(&fixture) &&
              cross_repo_seed_http_pair(&fixture, "busy-client", "busy-api", "/rel", "b") &&
              cross_repo_project_path(&fixture, "busy-client", live, sizeof(live)) &&
              cross_repo_project_path(&fixture, "_stage", stage, sizeof(stage)) &&
              cross_repo_project_path(&fixture, "busy-api", provider, sizeof(provider));
    if (!ok) {
        cross_repo_fixture_end(&fixture);
        FAIL("cannot prepare busy repair fixture");
    }

    const char *target = "busy-api";
    cbm_cross_repo_result_t linked = cbm_cross_repo_match("busy-client", &target, 1);
    ok = !linked.failed && cbm_store_backup_path(live, stage) == CBM_STORE_OK &&
         cbm_cross_repo_relink_stage(live, stage, "busy-client", NULL) == CBM_STORE_OK &&
         cbm_store_backup_path(stage, live) == CBM_STORE_OK;
    cbm_cross_repo_result_free(&linked);

    bool writer = ok && cross_repo_repair_defers_under_lock(provider, "BEGIN IMMEDIATE;");
    bool reader = ok && cross_repo_repair_defers_under_lock(
                            provider, "PRAGMA journal_mode=DELETE; BEGIN; SELECT * FROM projects;");
    bool retried = cbm_cross_repo_reconcile_published("busy-client", NULL) &&
                   !cbm_cross_repo_reverse_pending("busy-client");
    cross_repo_fixture_end(&fixture);

    ASSERT_TRUE(ok);
    ASSERT_TRUE(writer);
    ASSERT_TRUE(reader);
    ASSERT_TRUE(retried);
    PASS();
}

TEST(cross_repo_saved_repair_rolls_back_client_and_keeps_legacy_caller) {
    cross_repo_fixture_t fixture;
    const char *client = "client";
    const char *provider = "api";
    bool seeded = cross_repo_fixture_begin(&fixture) &&
                  cross_repo_seed_http_pair(&fixture, client, provider, "/outgoing", "out") &&
                  cross_repo_seed_http_pair(&fixture, "upstream", client, "/incoming", "in");
    if (!seeded) {
        cross_repo_fixture_end(&fixture);
        FAIL("cannot seed automatic repair stores");
    }
    cbm_cross_repo_result_t outgoing = cbm_cross_repo_match(client, &provider, 1);
    cbm_cross_repo_result_t incoming = cbm_cross_repo_match("upstream", &client, 1);
    bool linked = !outgoing.failed && outgoing.http_edges == 1 && !incoming.failed &&
                  incoming.http_edges == 1;
    cbm_cross_repo_result_free(&outgoing);
    cbm_cross_repo_result_free(&incoming);
    bool changed =
        linked &&
        cross_repo_exec(&fixture, client,
                        "UPDATE edges SET properties=json_remove(properties,'$.callers')"
                        " WHERE type='CROSS_HTTP_CALLS' AND "
                        "json_extract(properties,'$.target_project')='upstream'") &&
        cross_repo_exec(&fixture, provider,
                        "UPDATE nodes SET file_path='moved.c' WHERE label='Function';"
                        "UPDATE edges SET properties=json_set(properties,"
                        "'$.target_function','old-caller') WHERE type='CROSS_HTTP_CALLS';"
                        "UPDATE store_meta SET v='999' WHERE k='mutation_gen';");
    const char *old_forward = "SELECT COUNT(*) FROM edges WHERE type='CROSS_HTTP_CALLS' AND "
                              "json_extract(properties,'$.target_file')='server.c'";
    const char *new_forward = "SELECT COUNT(*) FROM edges WHERE type='CROSS_HTTP_CALLS' AND "
                              "json_extract(properties,'$.target_file')='moved.c'";
    const char *legacy = "SELECT id FROM edges WHERE type='CROSS_HTTP_CALLS' AND "
                         "json_extract(properties,'$.target_project')='upstream' AND "
                         "json_extract(properties,'$.target_function')='call_remote' AND "
                         "json_type(properties,'$.callers') IS NULL";
    const char *old_reverse = "SELECT COUNT(*) FROM edges WHERE type='CROSS_HTTP_CALLS' AND "
                              "json_extract(properties,'$.target_function')='old-caller'";
    const char *pending =
        "SELECT COUNT(*) FROM store_meta WHERE k='cross_repo_reverse_pending:client' AND v='1'";
    const char *mode = "SELECT COUNT(*) FROM store_meta WHERE k='cross_repo_reverse_mode:client'";
    const char *automatic_mode =
        "SELECT COUNT(*) FROM store_meta WHERE k='cross_repo_reverse_mode:client' "
        "AND v='automatic'";
    int legacy_id = cross_repo_query_int(&fixture, client, legacy);

    atomic_int cancelled = 0;
    cross_repo_cancel_hook_t hook = {.cancelled = &cancelled, .project = client};
    cbm_cross_repo_set_after_insert_hook_for_tests(cross_repo_cancel_after_write, &hook);
    bool client_cancelled = changed && !cbm_cross_repo_repair_saved(client, &cancelled);
    cbm_cross_repo_set_after_insert_hook_for_tests(NULL, NULL);
    bool client_rolled_back = hook.fired == 1 &&
                              cross_repo_query_int(&fixture, client, old_forward) == 1 &&
                              cross_repo_query_int(&fixture, client, new_forward) == 0 &&
                              cross_repo_query_int(&fixture, client, legacy) == legacy_id &&
                              cross_repo_query_int(&fixture, provider, old_reverse) == 1 &&
                              !cbm_cross_repo_reverse_pending(client) &&
                              cross_repo_query_int(&fixture, client, mode) == 0;

    atomic_store(&cancelled, 0);
    hook.project = provider;
    hook.fired = 0;
    cbm_cross_repo_set_after_insert_hook_for_tests(cross_repo_cancel_after_write, &hook);
    bool peer_cancelled = !cbm_cross_repo_repair_saved(client, &cancelled);
    cbm_cross_repo_set_after_insert_hook_for_tests(NULL, NULL);
    bool peer_rolled_back = hook.fired == 1 &&
                            cross_repo_query_int(&fixture, provider, old_reverse) == 1 &&
                            cross_repo_query_int(&fixture, client, new_forward) == 1 &&
                            cross_repo_query_int(&fixture, client, pending) == 1 &&
                            cross_repo_query_int(&fixture, client, automatic_mode) == 1;

    /* Pending work written by the earlier branch tip upgrades on retry. */
    bool old_marker =
        cross_repo_exec(&fixture, client,
                        "UPDATE store_meta SET v='2' "
                        "WHERE k='cross_repo_reverse_pending:client';"
                        "DELETE FROM store_meta WHERE k='cross_repo_reverse_mode:client';");
    atomic_store(&cancelled, 0);
    cbm_cross_repo_set_after_insert_hook_for_tests(cross_repo_cancel_after_write, &hook);
    bool upgraded = old_marker && !cbm_cross_repo_reconcile_published(client, &cancelled);
    cbm_cross_repo_set_after_insert_hook_for_tests(NULL, NULL);
    upgraded = upgraded && hook.fired == 2 &&
               cross_repo_query_int(&fixture, client, pending) == 1 &&
               cross_repo_query_int(&fixture, client, automatic_mode) == 1;

    /* Clearing the two keys is atomic too. */
    bool cleanup_rolled_back =
        cross_repo_exec(&fixture, client,
                        "CREATE TRIGGER block_mode_clear BEFORE DELETE ON store_meta "
                        "WHEN OLD.k='cross_repo_reverse_mode:client' "
                        "BEGIN SELECT RAISE(ABORT,'forced cleanup failure'); END;") &&
        !cbm_cross_repo_reconcile_published(client, NULL) &&
        cross_repo_query_int(&fixture, client, pending) == 1 &&
        cross_repo_query_int(&fixture, client, automatic_mode) == 1;
    bool remove_trigger = cross_repo_exec(&fixture, client, "DROP TRIGGER block_mode_clear");
    bool repaired = cbm_cross_repo_reconcile_published(client, NULL) &&
                    !cbm_cross_repo_reverse_pending(client) &&
                    cross_repo_query_int(&fixture, client, mode) == 0 &&
                    cross_repo_query_int(&fixture, provider, old_reverse) == 0 &&
                    cross_repo_query_int(&fixture, client, legacy) == legacy_id;
    cross_repo_fixture_end(&fixture);

    ASSERT_TRUE(changed);
    ASSERT_TRUE(legacy_id > 0);
    ASSERT_TRUE(client_cancelled);
    ASSERT_TRUE(client_rolled_back);
    ASSERT_TRUE(peer_cancelled);
    ASSERT_TRUE(peer_rolled_back);
    ASSERT_TRUE(upgraded);
    ASSERT_TRUE(cleanup_rolled_back);
    ASSERT_TRUE(remove_trigger);
    ASSERT_TRUE(repaired);
    PASS();
}

TEST(cross_repo_saved_repair_defers_on_busy_provider_read) {
    cross_repo_fixture_t fixture;
    char provider_path[512];
    bool seeded =
        cross_repo_fixture_begin(&fixture) &&
        cross_repo_seed_http_pair(&fixture, "busy-client", "busy-api", "/rel", "b") &&
        cross_repo_project_path(&fixture, "busy-api", provider_path, sizeof(provider_path));
    if (!seeded) {
        cross_repo_fixture_end(&fixture);
        FAIL("cannot seed busy automatic repair stores");
    }
    const char *provider = "busy-api";
    cbm_cross_repo_result_t linked = cbm_cross_repo_match("busy-client", &provider, 1);
    bool ready = !linked.failed && linked.http_edges == 1;
    cbm_cross_repo_result_free(&linked);
    sqlite3 *lock = NULL;
    bool held =
        ready && sqlite3_open_v2(provider_path, &lock, SQLITE_OPEN_READWRITE, NULL) == SQLITE_OK &&
        sqlite3_exec(lock, "PRAGMA journal_mode=DELETE; BEGIN EXCLUSIVE;", NULL, NULL, NULL) ==
            SQLITE_OK;
    uint64_t started = cbm_now_ms();
    bool deferred = held && !cbm_cross_repo_repair_saved("busy-client", NULL);
    uint64_t elapsed = cbm_now_ms() - started;
    bool rolled_back = !cbm_cross_repo_reverse_pending("busy-client") &&
                       cross_repo_count_edges(&fixture, "busy-client", "CROSS_HTTP_CALLS") == 1;
    sqlite3_close(lock);
    bool retried = cbm_cross_repo_repair_saved("busy-client", NULL);
    cross_repo_fixture_end(&fixture);
    ASSERT_TRUE(held);
    ASSERT_TRUE(deferred);
    ASSERT_TRUE(elapsed < 5000);
    ASSERT_TRUE(rolled_back);
    ASSERT_TRUE(retried);
    PASS();
}

TEST(cross_repo_status_reports_stale_after_provider_changes) {
    cross_repo_fixture_t fixture;
    bool ok = cross_repo_fixture_begin(&fixture) &&
              cross_repo_seed_http_pair(&fixture, "client", "api", "/known", "known");
    if (!ok) {
        cross_repo_fixture_end(&fixture);
        FAIL("cannot prepare matching project stores");
    }

    const char *target = "api";
    cbm_cross_repo_result_t linked = cbm_cross_repo_match("client", &target, 1);
    ok = !linked.failed &&
         cross_repo_exec(&fixture, "api", "UPDATE store_meta SET v='999' WHERE k='mutation_gen'");
    cbm_cross_repo_result_free(&linked);
    char *changed = cross_repo_status_json(&fixture, "client");
    linked = cbm_cross_repo_match("client", &target, 1);
    ok = ok && !linked.failed;
    cbm_cross_repo_result_free(&linked);
    char *relinked = cross_repo_status_json(&fixture, "client");
    cross_repo_fixture_end(&fixture);

    bool stale = changed && strstr(changed, "\"status\":\"stale\"");
    bool fresh = relinked && strstr(relinked, "\"status\":\"ran\"");
    free(changed);
    free(relinked);

    ASSERT_TRUE(ok);
    ASSERT_TRUE(stale);
    ASSERT_TRUE(fresh);
    PASS();
}

static bool cross_repo_seed_channel(const cross_repo_fixture_t *fixture, const char *project,
                                    bool emits, bool listens) {
    char path[512], function_qn[256], channel_qn[256];
    if (!cross_repo_project_path(fixture, project, path, sizeof(path))) {
        return false;
    }
    cbm_store_t *store = cbm_store_open_path(path);
    if (!store) {
        return false;
    }
    snprintf(function_qn, sizeof(function_qn), "%s.run", project);
    snprintf(channel_qn, sizeof(channel_qn), "%s.orders", project);
    cbm_node_t function = {.project = project,
                           .label = "Function",
                           .name = "run",
                           .qualified_name = function_qn,
                           .file_path = "channel.ts"};
    cbm_node_t channel = {.project = project,
                          .label = "Channel",
                          .name = "orders",
                          .qualified_name = channel_qn,
                          .file_path = "channel.ts",
                          .properties_json = "{\"transport\":\"pubsub\"}"};
    bool ok = cbm_store_upsert_project(store, project, fixture->cache) == CBM_STORE_OK;
    cbm_edge_t edge = {.project = project,
                       .source_id = cbm_store_upsert_node(store, &function),
                       .target_id = cbm_store_upsert_node(store, &channel),
                       .type = "EMITS"};
    ok = ok && edge.source_id > 0 && edge.target_id > 0 &&
         (!emits || cbm_store_insert_edge(store, &edge) > 0);
    edge.type = "LISTENS_ON";
    ok = ok && (!listens || cbm_store_insert_edge(store, &edge) > 0);
    cbm_store_close(store);
    return ok;
}

static bool cross_repo_link_channel(const char *project, const char *target, int expected) {
    cbm_cross_repo_result_t linked = cbm_cross_repo_match(project, &target, 1);
    bool ok = !linked.failed && linked.channel_edges == expected;
    cbm_cross_repo_result_free(&linked);
    return ok;
}

TEST(cross_repo_saved_repair_preserves_ambiguous_legacy_channel) {
    cross_repo_fixture_t fixture;
    bool ready = cross_repo_fixture_begin(&fixture) &&
                 cross_repo_seed_channel(&fixture, "upstream", true, false) &&
                 cross_repo_seed_channel(&fixture, "middle", true, true) &&
                 cross_repo_seed_channel(&fixture, "sink", false, true) &&
                 cross_repo_link_channel("middle", "sink", 1) &&
                 cross_repo_link_channel("upstream", "middle", 1) &&
                 cross_repo_exec(&fixture, "middle",
                                 "UPDATE edges SET properties=json_remove(properties,"
                                 "'$.callers','$.channel_forward') WHERE type='CROSS_CHANNEL'");
    const char *legacy = "SELECT id FROM edges WHERE type='CROSS_CHANNEL' AND "
                         "json_extract(properties,'$.target_project')='upstream' AND "
                         "json_type(properties,'$.callers') IS NULL AND "
                         "json_type(properties,'$.channel_forward') IS NULL";
    int before = ready ? cross_repo_query_int(&fixture, "middle", legacy) : -1;
    bool deferred = ready && !cbm_cross_repo_repair_saved("middle", NULL);
    int after = ready ? cross_repo_query_int(&fixture, "middle", legacy) : -1;
    cross_repo_fixture_end(&fixture);

    ASSERT_TRUE(ready);
    ASSERT_TRUE(before > 0);
    ASSERT_TRUE(deferred);
    ASSERT_EQ(after, before);
    PASS();
}

TEST(cross_repo_channel_repair_preserves_other_emitters_and_forward_link) {
    cross_repo_fixture_t fixture;
    char first[512], stage[512];
    bool ok = cross_repo_fixture_begin(&fixture) &&
              cross_repo_seed_channel(&fixture, "emitter-a", true, false) &&
              cross_repo_seed_channel(&fixture, "emitter-b", true, false) &&
              cross_repo_seed_channel(&fixture, "provider", true, true) &&
              cross_repo_seed_channel(&fixture, "sink", false, true) &&
              cross_repo_project_path(&fixture, "emitter-a", first, sizeof(first)) &&
              cross_repo_project_path(&fixture, "_stage", stage, sizeof(stage));
    if (!ok) {
        cross_repo_fixture_end(&fixture);
        FAIL("cannot prepare channel fixture");
    }

    const char *count_callers =
        "SELECT COUNT(*) FROM edges, json_each(edges.properties, '$.callers') "
        "WHERE edges.type='CROSS_CHANNEL'";
    ok = cross_repo_link_channel("provider", "sink", 1) &&
         cross_repo_link_channel("emitter-a", "provider", 1) &&
         cross_repo_link_channel("emitter-b", "provider", 1);
    int both_callers = cross_repo_query_int(&fixture, "provider", count_callers);

    ok = ok && cbm_store_backup_path(first, stage) == CBM_STORE_OK &&
         cross_repo_exec(&fixture, "_stage", "DELETE FROM edges WHERE type='EMITS'") &&
         cbm_cross_repo_relink_stage(first, stage, "emitter-a", NULL) == CBM_STORE_OK &&
         cbm_store_backup_path(stage, first) == CBM_STORE_OK &&
         cbm_cross_repo_reconcile_published("emitter-a", NULL);
    int remaining_callers = cross_repo_query_int(&fixture, "provider", count_callers);
    int remaining_peer = cross_repo_query_int(
        &fixture, "provider",
        "SELECT COUNT(*) FROM edges, json_each(edges.properties, '$.callers') "
        "WHERE edges.type='CROSS_CHANNEL' AND json_extract(value, '$.project')='emitter-b'");

    ok = ok && cross_repo_exec(&fixture, "emitter-b", "DELETE FROM edges WHERE type='EMITS'") &&
         cross_repo_link_channel("emitter-b", "provider", 0);
    int no_callers = cross_repo_query_int(&fixture, "provider", count_callers);
    int forward = cross_repo_query_int(&fixture, "provider",
                                       "SELECT COUNT(*) FROM edges WHERE type='CROSS_CHANNEL' "
                                       "AND json_extract(properties, '$.target_project')='sink'");
    cross_repo_fixture_end(&fixture);

    ASSERT_TRUE(ok);
    ASSERT_EQ(both_callers, 2);
    ASSERT_EQ(remaining_callers, 1);
    ASSERT_EQ(remaining_peer, 1);
    ASSERT_EQ(no_callers, 0);
    ASSERT_EQ(forward, 1);
    PASS();
}

TEST(cross_repo_relink_removes_provably_reverse_legacy_channel) {
    cross_repo_fixture_t fixture;
    bool ok = cross_repo_fixture_begin(&fixture) &&
              cross_repo_seed_channel(&fixture, "emitter", true, false) &&
              cross_repo_seed_channel(&fixture, "listener", false, true);
    if (!ok) {
        cross_repo_fixture_end(&fixture);
        FAIL("cannot prepare legacy channel fixture");
    }

    ok = cross_repo_link_channel("emitter", "listener", 1) &&
         cross_repo_exec(&fixture, "listener",
                         "UPDATE edges SET properties=json_remove(properties, '$.callers') "
                         "WHERE type='CROSS_CHANNEL'") &&
         cross_repo_exec(&fixture, "emitter", "DELETE FROM edges WHERE type='EMITS'") &&
         cross_repo_link_channel("emitter", "listener", 0);
    int reverse = cross_repo_count_edges(&fixture, "listener", "CROSS_CHANNEL");
    cross_repo_fixture_end(&fixture);

    ASSERT_TRUE(ok);
    ASSERT_EQ(reverse, 0);
    PASS();
}

static bool cross_repo_legacy_dual_role(bool valid_forward, bool long_identity) {
    cross_repo_fixture_t fixture;
    char live[512], stage[512];
    bool ok = cross_repo_fixture_begin(&fixture) &&
              cross_repo_seed_channel(&fixture, "caller", true, valid_forward) &&
              cross_repo_seed_channel(&fixture, "provider", true, true) &&
              cross_repo_project_path(&fixture, "caller", live, sizeof(live)) &&
              cross_repo_project_path(&fixture, "_stage", stage, sizeof(stage));
    if (ok && long_identity) {
        /* The UTF-8 suffix crosses a character count but fits exactly at the
         * legacy byte boundaries after C copies the name and file path. */
        ok = cross_repo_exec(
            &fixture, "caller",
            "UPDATE nodes SET name=replace(printf('%253s',''),' ','n') || char(233) || 'tail', "
            "file_path=replace(printf('%509s',''),' ','p') || char(233) || 'tail' "
            "WHERE label='Function'");
    }
    if (ok) {
        ok = cross_repo_link_channel("caller", "provider", 1) &&
             (!valid_forward || cross_repo_link_channel("provider", "caller", 1));
        if (ok && long_identity) {
            ok = cross_repo_query_int(
                     &fixture, "provider",
                     "SELECT COUNT(*) FROM edges WHERE type='CROSS_CHANNEL' AND "
                     "length(CAST(json_extract(properties,'$.target_function') AS BLOB))=255 AND "
                     "length(CAST(json_extract(properties,'$.target_file') AS BLOB))=511") == 1;
        }
        ok = ok &&
             cross_repo_exec(&fixture, "provider",
                             "UPDATE edges SET properties=json_remove(properties,"
                             "'$.callers', '$.channel_forward') WHERE type='CROSS_CHANNEL'") &&
             cbm_store_backup_path(live, stage) == CBM_STORE_OK &&
             cross_repo_exec(&fixture, "_stage", "DELETE FROM edges WHERE type='EMITS'") &&
             cbm_cross_repo_relink_stage(live, stage, "caller", NULL) == CBM_STORE_OK &&
             cbm_store_backup_path(stage, live) == CBM_STORE_OK &&
             cbm_cross_repo_reconcile_published("caller", NULL);
    }
    int links = cross_repo_count_edges(&fixture, "provider", "CROSS_CHANNEL");
    int verified_forward = cross_repo_query_int(
        &fixture, "provider",
        "SELECT COUNT(*) FROM edges WHERE type='CROSS_CHANNEL' "
        "AND json_extract(properties, '$.channel_forward.target_project')='caller'");
    cross_repo_fixture_end(&fixture);

    int expected = valid_forward ? 1 : 0;
    return ok && links == expected && verified_forward == expected;
}

TEST(cross_repo_legacy_dual_role_checks_the_remote_listener) {
    ASSERT_TRUE(cross_repo_legacy_dual_role(false, false));
    ASSERT_TRUE(cross_repo_legacy_dual_role(true, false));
    PASS();
}

TEST(cross_repo_legacy_dual_role_preserves_truncated_listener_identity) {
    ASSERT_TRUE(cross_repo_legacy_dual_role(false, true));
    ASSERT_TRUE(cross_repo_legacy_dual_role(true, true));
    PASS();
}

TEST(cross_repo_status_flags_legacy_provider_first_upgrade) {
    cross_repo_fixture_t fixture;
    bool ok = cross_repo_fixture_begin(&fixture) &&
              cross_repo_seed_http_pair(&fixture, "legacy-client", "legacy-api", "/known", "known");
    if (!ok) {
        cross_repo_fixture_end(&fixture);
        FAIL("cannot prepare legacy link fixture");
    }

    const char *target = "legacy-api";
    cbm_cross_repo_result_t linked = cbm_cross_repo_match("legacy-client", &target, 1);
    ok = !linked.failed &&
         cross_repo_exec(&fixture, "legacy-client",
                         "DELETE FROM store_meta WHERE k IN ('cross_repo_peers:legacy-client',"
                         " 'cross_repo_targets:legacy-client')") &&
         cross_repo_exec(&fixture, "legacy-api",
                         "UPDATE store_meta SET v='999' WHERE k='mutation_gen'");
    cbm_cross_repo_result_free(&linked);
    char *status = cross_repo_status_json(&fixture, "legacy-client");
    cross_repo_fixture_end(&fixture);

    bool stale = status && strstr(status, "\"status\":\"stale\"");
    free(status);
    ASSERT_TRUE(ok);
    ASSERT_TRUE(stale);
    PASS();
}

TEST(cross_repo_wildcard_replay_marks_missing_provider_until_return) {
    cross_repo_fixture_t fixture;
    char live[512], stage[512], provider[512], withheld[512];
    bool ok = cross_repo_fixture_begin(&fixture) &&
              cross_repo_seed_http_pair(&fixture, "client", "api", "/known", "known") &&
              cross_repo_project_path(&fixture, "client", live, sizeof(live)) &&
              cross_repo_project_path(&fixture, "_stage", stage, sizeof(stage)) &&
              cross_repo_project_path(&fixture, "api", provider, sizeof(provider));
    if (!ok) {
        cross_repo_fixture_end(&fixture);
        FAIL("cannot prepare wildcard link fixture");
    }

    snprintf(withheld, sizeof(withheld), "%s/api.offline", fixture.cache);
    const char *target = "*";
    cbm_cross_repo_result_t linked = cbm_cross_repo_match("client", &target, 1);
    ok = !linked.failed && linked.http_edges == 1 && cbm_rename_replace(provider, withheld) == 0 &&
         cbm_store_backup_path(live, stage) == CBM_STORE_OK &&
         cbm_cross_repo_relink_stage(live, stage, "client", NULL) == CBM_STORE_OK &&
         cbm_store_backup_path(stage, live) == CBM_STORE_OK;
    cbm_cross_repo_result_free(&linked);

    char *missing = cross_repo_status_json(&fixture, "client");
    int while_missing = cross_repo_count_edges(&fixture, "client", "CROSS_HTTP_CALLS");
    ok = ok && cbm_rename_replace(withheld, provider) == 0;
    char *returned = cross_repo_status_json(&fixture, "client");
    ok = ok && cbm_store_backup_path(live, stage) == CBM_STORE_OK &&
         cbm_cross_repo_relink_stage(live, stage, "client", NULL) == CBM_STORE_OK &&
         cbm_store_backup_path(stage, live) == CBM_STORE_OK;
    char *replayed = cross_repo_status_json(&fixture, "client");
    int after_replay = cross_repo_count_edges(&fixture, "client", "CROSS_HTTP_CALLS");
    cross_repo_fixture_end(&fixture);

    bool stale = missing && strstr(missing, "\"status\":\"stale\"") &&
                 strstr(missing, "\"projects_scanned\":0");
    bool still_stale = returned && strstr(returned, "\"status\":\"stale\"");
    bool fresh = replayed && strstr(replayed, "\"status\":\"ran\"");
    free(missing);
    free(returned);
    free(replayed);
    ASSERT_TRUE(ok);
    ASSERT_TRUE(stale);
    ASSERT_TRUE(still_stale);
    ASSERT_TRUE(fresh);
    ASSERT_EQ(while_missing, 0);
    ASSERT_EQ(after_replay, 1);
    PASS();
}

TEST(cross_repo_explicit_wildcard_keeps_missing_peer_with_online_provider) {
    cross_repo_fixture_t fixture;
    char live[512], stage[512], provider[512], withheld[512];
    bool ok = cross_repo_fixture_begin(&fixture) &&
              cross_repo_seed_http_pair(&fixture, "client", "api-a", "/a", "a") &&
              cross_repo_seed_http_pair(&fixture, "client", "api-b", "/b", "b") &&
              cross_repo_project_path(&fixture, "client", live, sizeof(live)) &&
              cross_repo_project_path(&fixture, "_stage", stage, sizeof(stage)) &&
              cross_repo_project_path(&fixture, "api-a", provider, sizeof(provider));
    if (!ok) {
        cross_repo_fixture_end(&fixture);
        FAIL("cannot prepare two-provider wildcard fixture");
    }

    snprintf(withheld, sizeof(withheld), "%s/api-a.offline", fixture.cache);
    const char *target = "*";
    cbm_cross_repo_result_t initial = cbm_cross_repo_match("client", &target, 1);
    ok = !initial.failed && initial.http_edges == 2 &&
         cbm_rename_replace(provider, withheld) == 0 &&
         cbm_store_backup_path(live, stage) == CBM_STORE_OK &&
         cbm_cross_repo_relink_stage(live, stage, "client", NULL) == CBM_STORE_OK &&
         cbm_store_backup_path(stage, live) == CBM_STORE_OK;
    cbm_cross_repo_result_free(&initial);
    cbm_cross_repo_result_t partial = cbm_cross_repo_match("client", &target, 1);
    bool partial_success = !partial.failed && partial.projects_scanned == 1 &&
                           partial.skipped_count == 1 &&
                           strcmp(partial.skipped_projects[0].project, "api-a") == 0;
    cbm_cross_repo_result_free(&partial);
    bool pending = cbm_cross_repo_reverse_pending("client");
    ok = ok && cbm_rename_replace(withheld, provider) == 0;
    char *returned = cross_repo_status_json(&fixture, "client");
    int before_refresh = cross_repo_count_edges(&fixture, "client", "CROSS_HTTP_CALLS");

    cbm_cross_repo_result_t refreshed = cbm_cross_repo_match("client", &target, 1);
    bool refreshed_all = !refreshed.failed && refreshed.projects_scanned == 2 &&
                         refreshed.http_edges == 2 && !cbm_cross_repo_reverse_pending("client");
    cbm_cross_repo_result_free(&refreshed);
    char *after = cross_repo_status_json(&fixture, "client");
    cross_repo_fixture_end(&fixture);

    bool still_stale = returned && strstr(returned, "\"status\":\"stale\"");
    bool fresh = after && strstr(after, "\"status\":\"ran\"");
    free(returned);
    free(after);
    ASSERT_TRUE(ok);
    ASSERT_TRUE(partial_success);
    ASSERT_TRUE(pending);
    ASSERT_TRUE(still_stale);
    ASSERT_EQ(before_refresh, 1);
    ASSERT_TRUE(refreshed_all);
    ASSERT_TRUE(fresh);
    PASS();
}

static int cross_repo_watch_index(const char *project, const char *root, void *context) {
    (void)context;
    cbm_pipeline_t *pipeline = cbm_pipeline_new(root, NULL, CBM_MODE_FULL);
    if (!pipeline) {
        return CBM_STORE_ERR;
    }
    cbm_pipeline_set_background(pipeline, true);
    int rc = cbm_pipeline_set_project_name(pipeline, project) ? cbm_pipeline_run(pipeline)
                                                              : CBM_STORE_ERR;
    cbm_pipeline_free(pipeline);
    return rc;
}

TEST(cross_repo_watcher_publication_keeps_caller_link) {
    cross_repo_fixture_t fixture;
    if (!cross_repo_fixture_begin(&fixture)) {
        cross_repo_fixture_end(&fixture);
        FAIL("cannot prepare isolated index cache");
    }

    char client[512], provider[512], caller_file[1024];
    snprintf(client, sizeof(client), "%s/client", fixture.cache);
    snprintf(provider, sizeof(provider), "%s/provider", fixture.cache);
    snprintf(caller_file, sizeof(caller_file), "%s/app.ts", client);
    bool ok = th_write_file(caller_file, "export function known(): Promise<Response> {"
                                         " return fetch(\"/api/known\"); }\n") == 0 &&
              th_write_file(TH_PATH(provider, "app.ts"),
                            "import express from \"express\";\n"
                            "const app = express();\n"
                            "function provideKnown(): void {}\n"
                            "app.get(\"/api/known\", provideKnown);\n") == 0 &&
              cross_repo_watch_index("watch-client", client, NULL) == 0 &&
              cross_repo_watch_index("watch-provider", provider, NULL) == 0;
    cbm_store_t *watch_store = ok ? cbm_store_open_memory() : NULL;
    cbm_watcher_t *watcher =
        watch_store ? cbm_watcher_new(watch_store, cross_repo_watch_index, NULL) : NULL;
    if (!watcher) {
        cbm_store_close(watch_store);
        cross_repo_fixture_end(&fixture);
        FAIL("cannot start watcher fixture");
    }

    cbm_watcher_set_poll_non_git(watcher, true);
    ok = cbm_watcher_watch(watcher, "watch-client", client);
    cbm_watcher_poll_once(watcher);
    cbm_watcher_touch(watcher, "watch-client");
    cbm_watcher_poll_once(watcher); /* Establish the non-Git tree signature. */

    const char *target = "watch-provider";
    cbm_cross_repo_result_t linked = cbm_cross_repo_match("watch-client", &target, 1);
    bool initially_linked = !linked.failed && linked.http_edges == 1;
    cbm_cross_repo_result_free(&linked);

    ok = ok &&
         th_append_file(caller_file, "export function changed(): number { return 42; }\n") == 0;
    cbm_watcher_touch(watcher, "watch-client");
    int reindexed = cbm_watcher_poll_once(watcher);
    int changed = cross_repo_query_int(&fixture, "watch-client",
                                       "SELECT COUNT(*) FROM nodes WHERE name='changed'");
    int links = cross_repo_count_edges(&fixture, "watch-client", "CROSS_HTTP_CALLS");

    cbm_watcher_free(watcher);
    cbm_store_close(watch_store);
    cross_repo_fixture_end(&fixture);
    ASSERT_TRUE(ok);
    ASSERT_TRUE(initially_linked);
    ASSERT_EQ(reindexed, 1);
    ASSERT_EQ(changed, 1);
    ASSERT_EQ(links, 1);
    PASS();
}

SUITE(cross_repo) {
    RUN_TEST(cross_repo_watcher_publication_keeps_caller_link);
    RUN_TEST(cross_repo_stage_rebuilds_from_legacy_selection_without_writing_provider);
    RUN_TEST(cross_repo_status_reports_stale_after_provider_changes);
    RUN_TEST(cross_repo_cancel_repair_rolls_back_peer_and_keeps_pending);
    RUN_TEST(cross_repo_repair_defers_on_busy_open_and_write);
    RUN_TEST(cross_repo_saved_repair_rolls_back_client_and_keeps_legacy_caller);
    RUN_TEST(cross_repo_saved_repair_defers_on_busy_provider_read);
    RUN_TEST(cross_repo_saved_repair_preserves_ambiguous_legacy_channel);
    RUN_TEST(cross_repo_channel_repair_preserves_other_emitters_and_forward_link);
    RUN_TEST(cross_repo_relink_removes_provably_reverse_legacy_channel);
    RUN_TEST(cross_repo_legacy_dual_role_checks_the_remote_listener);
    RUN_TEST(cross_repo_legacy_dual_role_preserves_truncated_listener_identity);
    RUN_TEST(cross_repo_status_flags_legacy_provider_first_upgrade);
    RUN_TEST(cross_repo_wildcard_replay_marks_missing_provider_until_return);
    RUN_TEST(cross_repo_explicit_wildcard_keeps_missing_peer_with_online_provider);
    RUN_TEST(cross_repo_graphql_prefers_resolver_method_handler);
    RUN_TEST(cross_repo_graphql_reverse_edge_keeps_every_caller);
    RUN_TEST(cross_repo_graphql_reverse_callers_keyed_by_qualified_name);
    RUN_TEST(cross_repo_graphql_reverse_callers_follow_relinks);
    RUN_TEST(cross_repo_graphql_provider_run_keeps_client_target);
    RUN_TEST(cross_repo_graphql_provider_run_drops_removed_route_link);
    RUN_TEST(cross_repo_graphql_resolved_target_beats_ambiguous_in_one_run);
    RUN_TEST(cross_repo_graphql_ambiguous_route_marked);
    RUN_TEST(cross_repo_graphql_long_route_qn_exact);
    RUN_TEST(cross_repo_caller_local_handler_blocks_cross_http_issue1459);
    RUN_TEST(cross_repo_methodless_call_to_local_get_route_blocks_cross_http_issue1459);
    RUN_TEST(cross_repo_caller_without_local_handler_keeps_cross_http_issue1459);
    RUN_TEST(cross_repo_angular_http_client_exact_verb_matching_issue2768);
    RUN_TEST(cross_repo_accepts_project_with_missed_shadow_row_issue1609);
    RUN_TEST(cross_repo_null_target_fails_without_dereference);
    RUN_TEST(cross_repo_wildcard_keeps_projects_containing_internal_tokens);
    RUN_TEST(cross_repo_wildcard_skips_pre768_store_issue2133);
    RUN_TEST(cross_repo_named_pre768_target_fails_before_cleanup_issue2133);
    RUN_TEST(cross_repo_status_distinguishes_never_run_from_ran_without_links);
    RUN_TEST(cross_repo_wildcard_reports_skipped_pre768_store);
    RUN_TEST(cross_repo_matches_calls_past_4096_rows);
    RUN_TEST(cross_repo_propagates_delete_failure);
    RUN_TEST(cross_repo_failed_bidirectional_insert_is_not_counted);
    RUN_TEST(cross_repo_cancel_mid_run_keeps_completed_target_and_stops_before_later_target);
    RUN_TEST(cross_repo_pre_cancel_preserves_existing_cross_edges);
    RUN_TEST(cross_repo_self_only_target_fails_and_keeps_edges_issue1133);
    RUN_TEST(cross_repo_wildcard_with_no_other_project_fails_issue1133);
}
