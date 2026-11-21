/*
 * pass_cross_repo.c — Cross-repo intelligence: match Routes, Channels, and
 * async topics across indexed projects to create CROSS_* edges.
 *
 * For each HTTP_CALLS/ASYNC_CALLS edge in the source project, looks up the
 * target Route QN in other project DBs. For each Channel node with EMITS
 * edges, looks for matching LISTENS_ON in other projects (and vice versa).
 *
 * Edges are written bidirectionally: both source and target project DBs
 * get a CROSS_* edge so the link is visible from either side.
 */
#include "pipeline/pass_cross_repo.h"
#include "pipeline/pipeline_internal.h" // cbm_route_canon_path
#include "foundation/constants.h"
#include "foundation/log.h"
#include "foundation/mem_core.h"
#include "foundation/platform.h"
#include "foundation/compat.h"
#include "foundation/compat_fs.h"
#include "foundation/str_util.h"

#include <sqlite3/sqlite3.h>
#include <yyjson/yyjson.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

/* ── Constants ───────────────────────────────────────────────────── */

enum {
    CR_PATH_BUF = 4096,
    CR_QN_BUF = 512,
    CR_PROPS_BUF = 2048,
    CR_DB_EXT_LEN = 3, /* strlen(".db") */
    CR_INIT_CAP = 32,
    CR_MAX_PROJECTS = 4096,
    CR_MAX_CACHE_ENTRIES = 16384,
    CR_COL_3 = 3,
    CR_COL_4 = 4,
    CR_SCHEME_SKIP = 3,      /* strlen("://") */
    CR_ROUTE_PREFIX_LEN = 9, /* strlen("__route__") */
    CR_ANY_LEN = 3,          /* strlen("ANY") */
};

#define CR_MS_PER_SEC 1000.0
#define CR_NS_PER_MS 1000000.0

typedef enum {
    CR_RUN_OK = 0,
    CR_RUN_FAILED,
    CR_RUN_CANCELLED,
} cr_run_status_t;

typedef struct {
    const atomic_int *cancelled;
    cbm_store_t *staging_store; /* NULL for an explicit bidirectional run */
    bool cancellation_observed;
    bool mutated;
} cr_run_context_t;

static CBM_TLS cbm_cross_repo_after_insert_test_hook_t cr_after_insert_test_hook = NULL;
static CBM_TLS void *cr_after_insert_test_context = NULL;
static cbm_graphql_link_policy_fn cr_graphql_link_policy;
static cbm_graphql_delete_link_fn cr_graphql_delete_link;
static cbm_graphql_upsert_link_fn cr_graphql_upsert_link;
static cbm_graphql_find_handler_fn cr_graphql_find_handler;

void cbm_cross_repo_set_graphql_backend(cbm_graphql_link_policy_fn policy,
                                        cbm_graphql_delete_link_fn delete_link,
                                        cbm_graphql_upsert_link_fn upsert_link,
                                        cbm_graphql_find_handler_fn find_handler) {
    char configured[sizeof("rust")];
    const char *impl = cbm_native_getenv("MORI_IMPL_GRAPHQL", configured, sizeof(configured), NULL);
    bool rust =
        impl && strcmp(impl, "rust") == 0 && policy && delete_link && upsert_link && find_handler;
    cr_graphql_link_policy = rust ? policy : NULL;
    cr_graphql_delete_link = rust ? delete_link : NULL;
    cr_graphql_upsert_link = rust ? upsert_link : NULL;
    cr_graphql_find_handler = rust ? find_handler : NULL;
}

void cbm_cross_repo_set_after_insert_hook_for_tests(cbm_cross_repo_after_insert_test_hook_t hook,
                                                    void *context) {
    cr_after_insert_test_hook = hook;
    cr_after_insert_test_context = context;
}

typedef struct {
    int count;
    cr_run_status_t status;
} cr_match_result_t;

static bool cr_cancel_requested(cr_run_context_t *ctx) {
    if (!ctx) {
        return false;
    }
    if (!ctx->cancellation_observed && ctx->cancelled &&
        atomic_load_explicit(ctx->cancelled, memory_order_acquire) != 0) {
        ctx->cancellation_observed = true;
    }
    return ctx->cancellation_observed;
}

static cr_match_result_t cr_match_finish(cr_run_context_t *ctx, int count, bool failed) {
    cr_match_result_t result = {
        .count = count,
        .status =
            failed ? CR_RUN_FAILED : (cr_cancel_requested(ctx) ? CR_RUN_CANCELLED : CR_RUN_OK),
    };
    return result;
}

/* TLS buffer for integer-to-string in log calls. */
static CBM_TLS char cr_ibuf[CBM_SZ_32];
static const char *cr_itoa(int v) {
    snprintf(cr_ibuf, sizeof(cr_ibuf), "%d", v);
    return cr_ibuf;
}

/* ── Helpers ─────────────────────────────────────────────────────── */

static const char *cr_cache_dir(void) {
    const char *dir = cbm_resolve_cache_dir();
    return dir ? dir : cbm_tmpdir();
}

static bool cr_db_path(const char *project, char *buf, size_t bufsz) {
    if (!project || !cbm_validate_project_name(project)) {
        return false;
    }
    int written = snprintf(buf, bufsz, "%s/%s.db", cr_cache_dir(), project);
    return written > 0 && (size_t)written < bufsz;
}

static int cr_project_compare(const void *left, const void *right) {
    const char *const *left_project = left;
    const char *const *right_project = right;
    return strcmp(*left_project, *right_project);
}

static bool cr_store_has_exact_project(cbm_store_t *store, const char *project) {
    cbm_project_t *projects = NULL;
    int count = 0;
    if (!store || !cbm_store_check_integrity(store) ||
        cbm_store_list_projects(store, &projects, &count) != CBM_STORE_OK) {
        cbm_store_free_projects(projects, count);
        return false;
    }
    /* Ignore internal shadow projects ("<name>::missed" miss-graph rows): they
     * live in the SAME db as the primary project, so any project that has ever
     * recorded a parse miss carries two rows. Requiring count == 1 over ALL rows
     * therefore made such a project unresolvable as source AND as target, and
     * cross-repo reported it as not indexed when it plainly was (#1609). This is
     * the same defect mcp.c fixed for list_projects in #1044; this site never
     * learned it.
     *
     * The single-primary requirement itself is kept: it is what proves this db
     * belongs to the project we were asked about, rather than being a shared or
     * mislabelled store. Only the shadow rows stop counting toward it. */
    int primary_count = 0;
    bool primary_matches = false;
    for (int i = 0; i < count; i++) {
        const char *name = projects[i].name;
        if (name && name[0] && !strstr(name, "::")) {
            primary_count++;
            primary_matches = strcmp(name, project) == 0;
        }
    }
    bool matches = primary_count == 1 && primary_matches;
    cbm_store_free_projects(projects, count);
    return matches;
}

typedef enum {
    CR_PROJECT_USABLE = 0,
    CR_PROJECT_MISSING, /* no store, or the store is not exactly this project */
    CR_PROJECT_LEGACY,  /* exact project, but a pre-#768 schema (reindex required) */
} cr_project_state_t;

/* Classify `project` as a cross-repo input: usable when its store holds
 * exactly that project AND carries the schema the read-write open in
 * cr_open_existing_project requires. Checking only the first let a pre-#768
 * store (still readable, so list_projects shows it) pass validation and the
 * ["*"] enumeration, then abort the whole run at its write open — after the
 * source's previous CROSS_* generation had already been deleted. The probe is
 * read-only, so the legacy store is left untouched for its reindex. (#2133) */
static cr_project_state_t cr_project_state(const char *project) {
    char path[CR_PATH_BUF];
    if (!cr_db_path(project, path, sizeof(path))) {
        return CR_PROJECT_MISSING;
    }
    cbm_store_t *store = cbm_store_open_path_query(path);
    cr_project_state_t state =
        cr_store_has_exact_project(store, project) ? CR_PROJECT_USABLE : CR_PROJECT_MISSING;
    if (state == CR_PROJECT_USABLE && !cbm_store_edges_schema_current(store)) {
        cbm_log_warn("cross_repo.project_unusable", "project", project, "reason",
                     "pre_768_schema_reindex_required");
        state = CR_PROJECT_LEGACY;
    }
    cbm_store_close(store);
    return state;
}

static bool cr_project_exists(const char *project) {
    return cr_project_state(project) == CR_PROJECT_USABLE;
}

static cbm_store_t *cr_open_existing_project(const char *project) {
    char path[CR_PATH_BUF];
    if (!cr_db_path(project, path, sizeof(path))) {
        return NULL;
    }
    cbm_store_t *store = cbm_store_open_path_existing(path);
    if (!cr_store_has_exact_project(store, project)) {
        cbm_store_close(store);
        return NULL;
    }
    return store;
}

/* Extract a JSON string property from properties_json.
 * Writes into buf, returns buf on success, NULL on miss. */
static const char *json_str_prop(const char *json, const char *key, char *buf, size_t bufsz) {
    if (!json || !key) {
        return NULL;
    }
    char pat[CBM_SZ_128];
    snprintf(pat, sizeof(pat), "\"%s\":\"", key);
    const char *start = strstr(json, pat);
    if (!start) {
        return NULL;
    }
    start += strlen(pat);
    const char *end = strchr(start, '"');
    if (!end) {
        return NULL;
    }
    size_t len = (size_t)(end - start);
    if (len >= bufsz) {
        len = bufsz - SKIP_ONE;
    }
    memcpy(buf, start, len);
    buf[len] = '\0';
    return buf;
}

/* Build CROSS_* edge properties JSON. */
static void build_cross_props(char *buf, size_t bufsz, const char *target_project,
                              const char *target_function, const char *target_file,
                              const char *url_or_channel, const char *extra_key,
                              const char *extra_val) {
    int n = snprintf(buf, bufsz,
                     "{\"target_project\":\"%s\",\"target_function\":\"%s\","
                     "\"target_file\":\"%s\"",
                     target_project ? target_project : "", target_function ? target_function : "",
                     target_file ? target_file : "");
    if (url_or_channel && url_or_channel[0]) {
        n += snprintf(buf + n, bufsz - (size_t)n, ",\"%s\":\"%s\"",
                      extra_key ? extra_key : "url_path", url_or_channel);
    }
    if (extra_val && extra_val[0]) {
        n += snprintf(buf + n, bufsz - (size_t)n, ",\"%s\":\"%s\"",
                      extra_key ? "transport" : "method", extra_val);
    }
    snprintf(buf + n, bufsz - (size_t)n, "}");
}

/* Delete all CROSS_* edges for a project from a store. Cancellation is checked
 * before every destructive statement so a pre-cancelled request never erases
 * the previous generation. */
static cr_run_status_t delete_cross_edges(cbm_store_t *store, const char *project,
                                          cr_run_context_t *ctx) {
    static const char *const edge_types[] = {
        "CROSS_HTTP_CALLS", "CROSS_ASYNC_CALLS",   "CROSS_CHANNEL",
        "CROSS_GRPC_CALLS", "CROSS_GRAPHQL_CALLS", "CROSS_TRPC_CALLS",
    };
    struct sqlite3 *db = cbm_store_get_db(store);
    if (!db) {
        return CR_RUN_FAILED;
    }
    for (size_t i = 0; i < sizeof(edge_types) / sizeof(edge_types[0]); i++) {
        if (cr_cancel_requested(ctx)) {
            return CR_RUN_CANCELLED;
        }
        int changes_before = sqlite3_total_changes(db);
        if (cbm_store_delete_edges_by_type(store, project, edge_types[i]) != CBM_STORE_OK) {
            return CR_RUN_FAILED;
        }
        if (sqlite3_total_changes(db) > changes_before) {
            ctx->mutated = true;
        }
    }
    return cr_cancel_requested(ctx) ? CR_RUN_CANCELLED : CR_RUN_OK;
}

/* Remove src_project's callers from the provider tgt's reverse CROSS_*
 * edges before src_project is matched again: an entry under "callers"
 * outlived the client function it names, because the store only merges
 * into a reverse row (json_patch) and delete_cross_edges clears the source
 * store alone. A row left with no caller is dropped, and a row naming a
 * removed caller at the top level takes a remaining one. Forward edges and
 * channel edges carry no "callers" and are left alone. The run then re-adds the
 * current callers. */
static cr_run_status_t cr_reconcile_reverse(cbm_store_t *tgt_store, const char *tgt_project,
                                            const char *src_project, cr_run_context_t *ctx) {
    static const char *const sql[] = {
        "UPDATE edges SET properties = json_set(properties, '$.callers', (SELECT "
        "json_group_object(c.key, json(c.value)) FROM json_each(edges.properties, '$.callers') c "
        "WHERE json_extract(c.value, '$.project') IS NOT ?2)) "
        "WHERE project = ?1 AND type LIKE 'CROSS\\_%' ESCAPE '\\' AND EXISTS (SELECT 1 FROM "
        "json_each(edges.properties, '$.callers') c WHERE json_extract(c.value, '$.project') = "
        "?2)",
        "DELETE FROM edges WHERE project = ?1 AND type LIKE 'CROSS\\_%' ESCAPE '\\' AND "
        "json_type(properties, '$.callers') = 'object' AND NOT EXISTS (SELECT 1 FROM "
        "json_each(edges.properties, '$.callers')) AND ?2 IS NOT NULL",
        "UPDATE edges SET properties = (SELECT json_set(edges.properties, '$.target_project', "
        "json_extract(c.value, '$.project'), '$.target_function', json_extract(c.value, "
        "'$.function'), '$.target_file', json_extract(c.value, '$.file')) FROM "
        "json_each(edges.properties, '$.callers') c ORDER BY c.key LIMIT 1) "
        "WHERE project = ?1 AND type LIKE 'CROSS\\_%' ESCAPE '\\' AND "
        "json_extract(properties, '$.target_project') = ?2 AND json_type(properties, "
        "'$.callers') = 'object'",
    };
    struct sqlite3 *db = cbm_store_get_db(tgt_store);
    if (!db) {
        return CR_RUN_FAILED;
    }
    for (size_t i = 0; i < sizeof(sql) / sizeof(sql[0]); i++) {
        if (cr_cancel_requested(ctx)) {
            return CR_RUN_CANCELLED;
        }
        sqlite3_stmt *st = NULL;
        int changes_before = sqlite3_total_changes(db);
        bool ok = sqlite3_prepare_v2(db, sql[i], CBM_NOT_FOUND, &st, NULL) == SQLITE_OK &&
                  sqlite3_bind_text(st, SKIP_ONE, tgt_project, CBM_NOT_FOUND, SQLITE_STATIC) ==
                      SQLITE_OK &&
                  sqlite3_bind_text(st, PAIR_LEN, src_project, CBM_NOT_FOUND, SQLITE_STATIC) ==
                      SQLITE_OK &&
                  sqlite3_step(st) == SQLITE_DONE;
        if (sqlite3_finalize(st) != SQLITE_OK || !ok) {
            return CR_RUN_FAILED;
        }
        if (sqlite3_total_changes(db) > changes_before) {
            ctx->mutated = true;
        }
    }
    return CR_RUN_OK;
}

/* Insert a CROSS_* edge into a store. Idempotent by construction: the edges
 * table is UNIQUE(source_id, target_id, type) and cbm_store_insert_edge
 * upserts on conflict, so a pair reached from both match directions or on a
 * repeat run never duplicates a row. (#523) */
static bool insert_cross_edge(cbm_store_t *store, const char *project, int64_t from_id,
                              int64_t to_id, const char *edge_type, const char *props,
                              cr_run_context_t *ctx) {
    if (cr_cancel_requested(ctx)) {
        return false;
    }
    if (ctx->staging_store && store != ctx->staging_store) {
        return true;
    }
    bool inserted;
    if (cr_graphql_upsert_link && strcmp(edge_type, "CROSS_GRAPHQL_CALLS") == 0) {
        sqlite3_stmt *stmt = cbm_store_prepare_edge_upsert(store);
        inserted = stmt && cr_graphql_upsert_link(stmt, project, from_id, to_id, props);
    } else {
        cbm_edge_t edge = {
            .project = project,
            .source_id = from_id,
            .target_id = to_id,
            .type = edge_type,
            .properties_json = props,
        };
        inserted = cbm_store_insert_edge(store, &edge) > 0;
    }
    if (inserted) {
        ctx->mutated = true;
        if (cr_after_insert_test_hook) {
            cr_after_insert_test_hook(project, edge_type, cr_after_insert_test_context);
        }
    }
    return inserted;
}

/* Strip "scheme://host[:port]" from a stored HTTP_CALLS url, returning the
 * path. url_path property values are stored raw from the call's first string
 * argument, so they can be full URLs ("scheme://host:port/v2/x") — and
 * cbm_route_canon_path only canonicalizes placeholder syntax, never strips
 * authorities. Returns "/" for a URL with no path after the host (a request
 * against the bare base URL targets the root route). (#523) */
static const char *cr_url_path(const char *url) {
    if (!url) {
        return url;
    }
    const char *scheme_end = strstr(url, "://");
    if (!scheme_end) {
        return url; /* already a bare path */
    }
    const char *path_start = strchr(scheme_end + CR_SCHEME_SKIP, '/');
    return path_start ? path_start : "/";
}

/* Look up a node's name and file_path by id. */
static bool lookup_node_info(struct sqlite3 *db, int64_t node_id, char *name_out, size_t name_sz,
                             char *file_out, size_t file_sz) {
    name_out[0] = '\0';
    file_out[0] = '\0';
    if (!db) {
        return false;
    }
    sqlite3_stmt *st = NULL;
    if (sqlite3_prepare_v2(db, "SELECT name, file_path FROM nodes WHERE id = ?1", CBM_NOT_FOUND,
                           &st, NULL) != SQLITE_OK) {
        return false;
    }
    if (sqlite3_bind_int64(st, SKIP_ONE, node_id) != SQLITE_OK) {
        sqlite3_finalize(st);
        return false;
    }
    int step_rc = sqlite3_step(st);
    if (step_rc == SQLITE_ROW) {
        const char *nm = (const char *)sqlite3_column_text(st, 0);
        const char *fp = (const char *)sqlite3_column_text(st, SKIP_ONE);
        if (nm) {
            snprintf(name_out, name_sz, "%s", nm);
        }
        if (fp) {
            snprintf(file_out, file_sz, "%s", fp);
        }
    }
    int finalize_rc = sqlite3_finalize(st);
    return (step_rc == SQLITE_ROW || step_rc == SQLITE_DONE) && finalize_rc == SQLITE_OK;
}

/* ── Phase A: HTTP Route matching ────────────────────────────────── */

/* Find a Route node in target_store by QN and return the handler function's
 * node id, name, and file_path via HANDLES edges. Returns 0 if not found. */
static int64_t find_route_handler(cbm_store_t *target_store, const char *route_qn,
                                  char *handler_name, size_t name_sz, char *handler_file,
                                  size_t file_sz, bool *failed) {
    *failed = false;
    handler_name[0] = '\0';
    handler_file[0] = '\0';
    struct sqlite3 *db = cbm_store_get_db(target_store);
    if (!db) {
        *failed = true;
        return 0;
    }

    /* Find Route node by QN */
    sqlite3_stmt *s = NULL;
    if (sqlite3_prepare_v2(
            db, "SELECT id FROM nodes WHERE qualified_name = ?1 AND label = 'Route' LIMIT 1",
            CBM_NOT_FOUND, &s, NULL) != SQLITE_OK) {
        *failed = true;
        return 0;
    }
    if (sqlite3_bind_text(s, SKIP_ONE, route_qn, CBM_NOT_FOUND, SQLITE_STATIC) != SQLITE_OK) {
        sqlite3_finalize(s);
        *failed = true;
        return 0;
    }
    int64_t route_id = 0;
    int step_rc = sqlite3_step(s);
    if (step_rc == SQLITE_ROW) {
        route_id = sqlite3_column_int64(s, 0);
    } else if (step_rc != SQLITE_DONE) {
        *failed = true;
    }
    if (sqlite3_finalize(s) != SQLITE_OK) {
        *failed = true;
    }
    if (*failed || route_id == 0) {
        return 0;
    }

    /* Follow HANDLES edge to find the handler function. A Route can have
     * several handlers (a GraphQL field: its resolver method and, in a
     * monorepo, a second schema's Field); pick one by a fixed order, code
     * (Method, then Function) before a schema or config node, so the link
     * lands on the code an agent reads and never depends on row order. */
    if (sqlite3_prepare_v2(db,
                           "SELECT n.id, n.name, n.file_path FROM edges e "
                           "JOIN nodes n ON n.id = e.source_id "
                           "WHERE e.target_id = ?1 AND e.type = 'HANDLES' "
                           "ORDER BY CASE n.label WHEN 'Method' THEN 0 "
                           "WHEN 'Function' THEN 1 ELSE 2 END, "
                           "n.qualified_name, n.id LIMIT 1",
                           CBM_NOT_FOUND, &s, NULL) != SQLITE_OK) {
        *failed = true;
        return 0;
    }
    if (sqlite3_bind_int64(s, SKIP_ONE, route_id) != SQLITE_OK) {
        sqlite3_finalize(s);
        *failed = true;
        return 0;
    }
    int64_t handler_id = 0;
    step_rc = sqlite3_step(s);
    if (step_rc == SQLITE_ROW) {
        handler_id = sqlite3_column_int64(s, 0);
        const char *n = (const char *)sqlite3_column_text(s, SKIP_ONE);
        const char *f = (const char *)sqlite3_column_text(s, PAIR_LEN);
        if (n) {
            snprintf(handler_name, name_sz, "%s", n);
        }
        if (f) {
            snprintf(handler_file, file_sz, "%s", f);
        }
    } else if (step_rc != SQLITE_DONE) {
        *failed = true;
    }
    if (sqlite3_finalize(s) != SQLITE_OK) {
        *failed = true;
    }
    return handler_id;
}

/* Segment-wise match of a concrete path against a route template path, where a
 * "{...}" segment in the template matches any single non-empty concrete
 * segment. Both inputs are bare paths (no method prefix, no authority).
 * Leading/trailing slashes are insignificant. Returns true on a full match. */
static bool cr_path_matches_template(const char *concrete, const char *templ) {
    const char *c = concrete;
    const char *t = templ;
    while (*c && *t) {
        if (*c == '/') {
            c++;
        }
        if (*t == '/') {
            t++;
        }
        const char *cseg = c;
        while (*c && *c != '/') {
            c++;
        }
        const char *tseg = t;
        while (*t && *t != '/') {
            t++;
        }
        size_t clen = (size_t)(c - cseg);
        size_t tlen = (size_t)(t - tseg);
        bool t_is_param = (tlen >= PAIR_LEN && tseg[0] == '{' && tseg[tlen - 1] == '}');
        if (!t_is_param) {
            if (clen != tlen || strncmp(cseg, tseg, clen) != 0) {
                return false;
            }
        } else if (clen == 0) {
            return false; /* a parameter never matches an empty segment */
        }
    }
    while (*c == '/') {
        c++;
    }
    while (*t == '/') {
        t++;
    }
    return *c == '\0' && *t == '\0';
}

/* Fallback for when the exact route-QN lookup misses: a concrete client path
 * ("/v2/orders/123") never exact-matches a templated route QN
 * ("__route__GET__/v2/orders/{}"). Enumerate the target store's Route nodes
 * and segment-match the concrete path against each template. On a match, copy
 * the route QN into out_qn and return the handler id; returns 0 on no match.
 *
 * COST: this scans every Route node of the target project once per unmatched
 * HTTP_CALLS edge — O(calls × routes) per project pair. It only runs for edges
 * the exact lookup missed; no row count stops a scan (a cap decided which
 * calls were matched at all); revisit with a prepared template index if
 * cross-repo matching ever shows up in profiles. (#523) */
static int64_t find_route_handler_fuzzy(cbm_store_t *target_store, const char *concrete_path,
                                        const char *method, char *out_qn, size_t out_qn_sz,
                                        char *handler_name, size_t name_sz, char *handler_file,
                                        size_t file_sz, bool *failed, cr_run_context_t *ctx) {
    *failed = false;
    struct sqlite3 *db = cbm_store_get_db(target_store);
    if (!db || !concrete_path || !concrete_path[0]) {
        if (!db) {
            *failed = true;
        }
        return 0;
    }
    sqlite3_stmt *s = NULL;
    if (sqlite3_prepare_v2(db, "SELECT qualified_name FROM nodes WHERE label = 'Route' ORDER BY id",
                           CBM_NOT_FOUND, &s, NULL) != SQLITE_OK) {
        *failed = true;
        return 0;
    }
    int64_t found = 0;
    int step_rc = SQLITE_DONE;
    while (!cr_cancel_requested(ctx) && (step_rc = sqlite3_step(s)) == SQLITE_ROW) {
        const char *qn = (const char *)sqlite3_column_text(s, 0);
        if (!qn || strncmp(qn, "__route__", CR_ROUTE_PREFIX_LEN) != 0) {
            continue;
        }
        /* Split "__route__<METHOD>__<path>" */
        const char *rest = qn + CR_ROUTE_PREFIX_LEN;
        const char *sep = strstr(rest, "__");
        if (!sep) {
            continue;
        }
        size_t mlen = (size_t)(sep - rest);
        const char *rpath = sep + PAIR_LEN;
        /* Method gate: the route's method must equal the caller's, or be ANY.
         * A missing caller method matches any route method. */
        if (method && method[0]) {
            bool same_method = (strncmp(rest, method, mlen) == 0 && method[mlen] == '\0');
            bool route_any = (mlen == CR_ANY_LEN && strncmp(rest, "ANY", CR_ANY_LEN) == 0);
            if (!same_method && !route_any) {
                continue;
            }
        }
        if (!cr_path_matches_template(concrete_path, rpath)) {
            continue;
        }
        /* A concrete path can match more than one stored template (e.g. a raw
         * "{id}" variant and its canonical "{}" form). Only accept a Route that
         * actually has a HANDLES edge — the handler is attached to the
         * canonical node. Keep scanning otherwise. */
        int64_t hid = find_route_handler(target_store, qn, handler_name, name_sz, handler_file,
                                         file_sz, failed);
        if (*failed) {
            step_rc = SQLITE_DONE;
            break;
        }
        if (hid != 0) {
            snprintf(out_qn, out_qn_sz, "%s", qn);
            found = hid;
            step_rc = SQLITE_DONE;
            break;
        }
    }
    if (!cr_cancel_requested(ctx) && step_rc != SQLITE_DONE) {
        *failed = true;
    }
    if (sqlite3_finalize(s) != SQLITE_OK) {
        *failed = true;
    }
    return found;
}

static char *lookup_node_qn(struct sqlite3 *db, int64_t node_id, bool *failed);

/* Reverse-edge props `rev` (a JSON object) plus this caller's entry under
 * "callers", keyed "<project>:<qualified name>" (two same-named methods of
 * one file are distinct callers); heap-allocated, NULL on failure. Every caller of one handler and Route shares the reverse edge
 * (the store keeps one row per source, target and type), and the store
 * merges a re-inserted edge's props with json_patch: the top-level target_*
 * keys name the last caller, while the nested "callers" object merges key by
 * key and keeps them all. */
static char *cr_props_with_caller(const char *rev, const char *project, const char *qn,
                                  const char *function, const char *file) {
    yyjson_mut_doc *doc = yyjson_mut_doc_new(NULL);
    if (!doc) {
        return NULL;
    }
    yyjson_mut_val *root = yyjson_mut_obj(doc);
    yyjson_mut_doc_set_root(doc, root);
    yyjson_mut_val *callers = yyjson_mut_obj_add_obj(doc, root, "callers");
    yyjson_mut_val *entry = yyjson_mut_obj(doc);
    size_t key_len = strlen(project) + strlen(qn) + PAIR_LEN;
    char *key = callers && entry ? malloc(key_len) : NULL;
    char *out = NULL;
    if (key) {
        (void)snprintf(key, key_len, "%s:%s", project, qn);
        yyjson_mut_obj_add_strcpy(doc, entry, "project", project);
        yyjson_mut_obj_add_strcpy(doc, entry, "qualified_name", qn);
        yyjson_mut_obj_add_strcpy(doc, entry, "function", function);
        yyjson_mut_obj_add_strcpy(doc, entry, "file", file);
        yyjson_mut_obj_put(callers, yyjson_mut_strcpy(doc, key), entry);
        free(key);
        size_t callers_len = 0;
        char *json = yyjson_mut_write(doc, 0, &callers_len);
        size_t rev_len = strlen(rev);
        if (json && rev_len >= PAIR_LEN && rev[rev_len - SKIP_ONE] == '}' && callers_len > 0) {
            /* rev without its closing brace, a comma, then callers without
             * its opening one. */
            out = malloc(rev_len + callers_len + SKIP_ONE);
            if (out) {
                memcpy(out, rev, rev_len - SKIP_ONE);
                out[rev_len - SKIP_ONE] = ',';
                memcpy(out + rev_len, json + SKIP_ONE, callers_len - SKIP_ONE);
                out[rev_len + callers_len - SKIP_ONE] = '\0';
            }
        }
        free(json);
    }
    yyjson_mut_doc_free(doc);
    return out;
}

/* Set "ambiguous" on cross-edge props (a JSON object in buf), true or false.
 * A typed-route forward row is upserted with json_patch, so the flag is
 * always written, or a resolved write would keep an earlier `true`. */
static void cr_set_ambiguous(char *buf, size_t bufsz, bool ambiguous) {
    const char *mark = ambiguous ? ",\"ambiguous\":true}" : ",\"ambiguous\":false}";
    size_t mark_size = strlen(mark) + SKIP_ONE;
    size_t len = strlen(buf);
    if (len > 0 && buf[len - SKIP_ONE] == '}' && len - SKIP_ONE + mark_size <= bufsz) {
        memcpy(buf + len - SKIP_ONE, mark, mark_size);
    }
}

/* Emit CROSS_* edge for a route match: forward into source, reverse into target.
 * A pair is successful only when both writes complete. */
static bool emit_cross_route_bidirectional(
    cbm_store_t *src_store, const char *src_project, struct sqlite3 *src_db, int64_t caller_id,
    int64_t local_route_id, cbm_store_t *tgt_store, const char *tgt_project, int64_t handler_id,
    const char *route_qn, const char *handler_name, const char *handler_file, const char *url_path,
    const char *method, const char *edge_type, bool resolved_flag, cr_run_context_t *ctx) {
    /* Forward: caller → local Route in source DB */
    char fwd[CR_PROPS_BUF];
    build_cross_props(fwd, sizeof(fwd), tgt_project, handler_name, handler_file, url_path,
                      "url_path", method);
    if (resolved_flag) {
        cr_set_ambiguous(fwd, sizeof(fwd), false);
    }
    if (!insert_cross_edge(src_store, src_project, caller_id, local_route_id, edge_type, fwd,
                           ctx)) {
        return false;
    }

    if (cr_cancel_requested(ctx)) {
        return false;
    }

    /* Reverse: handler → Route in target DB */
    struct sqlite3 *tgt_db = cbm_store_get_db(tgt_store);
    if (!tgt_db) {
        return false;
    }
    sqlite3_stmt *rq = NULL;
    if (sqlite3_prepare_v2(tgt_db, "SELECT id FROM nodes WHERE qualified_name = ?1 LIMIT 1",
                           CBM_NOT_FOUND, &rq, NULL) != SQLITE_OK) {
        return false;
    }
    if (sqlite3_bind_text(rq, SKIP_ONE, route_qn, CBM_NOT_FOUND, SQLITE_STATIC) != SQLITE_OK) {
        sqlite3_finalize(rq);
        return false;
    }
    int64_t tgt_route_id = 0;
    int step_rc = sqlite3_step(rq);
    if (step_rc == SQLITE_ROW) {
        tgt_route_id = sqlite3_column_int64(rq, 0);
    }
    int finalize_rc = sqlite3_finalize(rq);
    if ((step_rc != SQLITE_ROW && step_rc != SQLITE_DONE) || finalize_rc != SQLITE_OK ||
        tgt_route_id == 0) {
        return false;
    }

    char caller_name[CBM_SZ_256] = {0};
    char caller_file[CBM_SZ_512] = {0};
    if (!lookup_node_info(src_db, caller_id, caller_name, sizeof(caller_name), caller_file,
                          sizeof(caller_file))) {
        return false;
    }

    char rev[CR_PROPS_BUF];
    build_cross_props(rev, sizeof(rev), src_project, caller_name, caller_file, url_path, "url_path",
                      method);
    bool qn_failed = false;
    char *caller_qn = lookup_node_qn(src_db, caller_id, &qn_failed);
    char *rev_callers = caller_qn ? cr_props_with_caller(rev, src_project, caller_qn, caller_name,
                                                         caller_file)
                                  : NULL;
    free(caller_qn);
    if (!rev_callers) {
        return false;
    }
    bool inserted =
        insert_cross_edge(tgt_store, tgt_project, handler_id, tgt_route_id, edge_type, rev_callers,
                          ctx);
    free(rev_callers);
    return inserted;
}

/* A resolved route: the matched Route QN plus its handler's name and file. */
typedef struct {
    char qn[CR_QN_BUF];
    char name[CBM_SZ_256];
    char file[CBM_SZ_512];
} cr_route_hit_t;

/* Resolve a client call (method + canonical path) to a handled Route in
 * store: exact method QN, then ANY, then segment-wise template matching,
 * since a concrete client path ("/v2/orders/123") never exact-matches a
 * templated route ("/v2/orders/{}") (#523). Returns the handler id, or 0. */
static int64_t resolve_route_handler(cbm_store_t *store, const char *method, const char *curl,
                                     cr_route_hit_t *hit, bool *failed, cr_run_context_t *ctx) {
    snprintf(hit->qn, sizeof(hit->qn), "__route__%s__%s", method[0] ? method : "ANY", curl);
    int64_t handler_id = find_route_handler(store, hit->qn, hit->name, sizeof(hit->name), hit->file,
                                            sizeof(hit->file), failed);
    if (!*failed && handler_id == 0) {
        snprintf(hit->qn, sizeof(hit->qn), "__route__ANY__%s", curl);
        handler_id = find_route_handler(store, hit->qn, hit->name, sizeof(hit->name), hit->file,
                                        sizeof(hit->file), failed);
    }
    if (!*failed && handler_id == 0) {
        handler_id = find_route_handler_fuzzy(store, curl, method[0] ? method : NULL, hit->qn,
                                              sizeof(hit->qn), hit->name, sizeof(hit->name),
                                              hit->file, sizeof(hit->file), failed, ctx);
    }
    return *failed ? 0 : handler_id;
}

/* True when the caller's own project has a HANDLES edge into the local Route
 * node its HTTP_CALLS points at. Sets *failed on a query error. (#1459) */
static bool route_node_handled(struct sqlite3 *src_db, const char *src_project, int64_t route_id,
                               bool *failed) {
    *failed = false;
    sqlite3_stmt *s = NULL;
    if (sqlite3_prepare_v2(src_db,
                           "SELECT 1 FROM edges WHERE project = ?1 AND target_id = ?2 "
                           "AND type = 'HANDLES' LIMIT 1",
                           CBM_NOT_FOUND, &s, NULL) != SQLITE_OK) {
        *failed = true;
        return false;
    }
    if (sqlite3_bind_text(s, SKIP_ONE, src_project, CBM_NOT_FOUND, SQLITE_STATIC) != SQLITE_OK ||
        sqlite3_bind_int64(s, PAIR_LEN, route_id) != SQLITE_OK) {
        sqlite3_finalize(s);
        *failed = true;
        return false;
    }
    int step_rc = sqlite3_step(s);
    if (step_rc != SQLITE_ROW && step_rc != SQLITE_DONE) {
        *failed = true;
    }
    if (sqlite3_finalize(s) != SQLITE_OK) {
        *failed = true;
    }
    return !*failed && step_rc == SQLITE_ROW;
}

/* Caller-local floor (#1459): a call the caller's own project serves is an
 * in-project call — another project exposing the same path is no evidence of
 * a cross-service call. "Served" is the HANDLES edge on the Route node the
 * HTTP_CALLS points at, or the same method/ANY/template resolution the target
 * lookup uses, applied to the caller's own store: a method-less fetch() points
 * at the ANY node while the local handler sits on the GET node. */
static bool call_served_locally(cbm_store_t *src_store, struct sqlite3 *src_db,
                                const char *src_project, int64_t route_id, const char *method,
                                const char *curl, bool *failed, cr_run_context_t *ctx) {
    if (route_node_handled(src_db, src_project, route_id, failed) || *failed) {
        return !*failed;
    }
    cr_route_hit_t local;
    return resolve_route_handler(src_store, method, curl, &local, failed, ctx) != 0;
}

static cr_match_result_t match_http_routes(cbm_store_t *src_store, const char *src_project,
                                           cbm_store_t *tgt_store, const char *tgt_project,
                                           cr_run_context_t *ctx) {
    struct sqlite3 *src_db = cbm_store_get_db(src_store);
    if (!src_db) {
        return cr_match_finish(ctx, 0, true);
    }

    /* Find all HTTP_CALLS edges in source project */
    sqlite3_stmt *s = NULL;
    if (sqlite3_prepare_v2(src_db,
                           "SELECT e.source_id, e.target_id, e.properties FROM edges e "
                           "WHERE e.project = ?1 AND e.type = 'HTTP_CALLS' ORDER BY e.id",
                           CBM_NOT_FOUND, &s, NULL) != SQLITE_OK) {
        return cr_match_finish(ctx, 0, true);
    }
    if (sqlite3_bind_text(s, SKIP_ONE, src_project, CBM_NOT_FOUND, SQLITE_STATIC) != SQLITE_OK) {
        sqlite3_finalize(s);
        return cr_match_finish(ctx, 0, true);
    }

    int count = 0;
    int step_rc = SQLITE_DONE;
    bool failed = false;
    while (!cr_cancel_requested(ctx) && (step_rc = sqlite3_step(s)) == SQLITE_ROW) {
        int64_t caller_id = sqlite3_column_int64(s, 0);
        int64_t route_id = sqlite3_column_int64(s, SKIP_ONE);
        const char *props = (const char *)sqlite3_column_text(s, PAIR_LEN);

        char url_path[CBM_SZ_256] = {0};
        char method[CBM_SZ_32] = {0};
        json_str_prop(props, "url_path", url_path, sizeof(url_path));
        json_str_prop(props, "method", method, sizeof(method));
        if (!method[0]) {
            char callee[CBM_SZ_256] = {0};
            json_str_prop(props, "callee", callee, sizeof(callee));
            if (callee[0]) {
                const char *m = cbm_service_pattern_http_method(callee);
                if (m) {
                    snprintf(method, sizeof(method), "%s", m);
                }
            }
        }
        if (!url_path[0]) {
            continue;
        }

        /* Canonical client path: authority-stripped and param-canonicalized so
         * it matches a server handler regardless of base URL and framework
         * placeholder syntax. */
        char cpath[CBM_SZ_256];
        const char *curl = cbm_route_canon_path(cr_url_path(url_path), cpath, sizeof(cpath));

        bool query_failed = false;
        bool served_locally = call_served_locally(src_store, src_db, src_project, route_id, method,
                                                  curl, &query_failed, ctx);
        if (query_failed) {
            failed = true;
            break;
        }
        if (served_locally) {
            continue;
        }

        cr_route_hit_t hit;
        int64_t handler_id =
            resolve_route_handler(tgt_store, method, curl, &hit, &query_failed, ctx);
        if (query_failed) {
            failed = true;
            break;
        }
        if (handler_id == 0) {
            continue;
        }

        if (cr_cancel_requested(ctx)) {
            break;
        }

        if (!emit_cross_route_bidirectional(src_store, src_project, src_db, caller_id, route_id,
                                            tgt_store, tgt_project, handler_id, hit.qn, hit.name,
                                            hit.file, url_path, method, "CROSS_HTTP_CALLS",
                                            false, ctx)) {
            failed = !cr_cancel_requested(ctx);
            break;
        }

        count++;
    }
    if (!failed && !cr_cancel_requested(ctx) && step_rc != SQLITE_DONE) {
        failed = true;
    }
    if (sqlite3_finalize(s) != SQLITE_OK) {
        failed = true;
    }
    return cr_match_finish(ctx, count, failed);
}

/* ── Phase B: Async matching ─────────────────────────────────────── */

static cr_match_result_t match_async_routes(cbm_store_t *src_store, const char *src_project,
                                            cbm_store_t *tgt_store, const char *tgt_project,
                                            cr_run_context_t *ctx) {
    struct sqlite3 *src_db = cbm_store_get_db(src_store);
    if (!src_db) {
        return cr_match_finish(ctx, 0, true);
    }

    sqlite3_stmt *s = NULL;
    if (sqlite3_prepare_v2(src_db,
                           "SELECT e.source_id, e.target_id, e.properties FROM edges e "
                           "WHERE e.project = ?1 AND e.type = 'ASYNC_CALLS' ORDER BY e.id",
                           CBM_NOT_FOUND, &s, NULL) != SQLITE_OK) {
        return cr_match_finish(ctx, 0, true);
    }
    if (sqlite3_bind_text(s, SKIP_ONE, src_project, CBM_NOT_FOUND, SQLITE_STATIC) != SQLITE_OK) {
        sqlite3_finalize(s);
        return cr_match_finish(ctx, 0, true);
    }

    int count = 0;
    int step_rc = SQLITE_DONE;
    bool failed = false;
    while (!cr_cancel_requested(ctx) && (step_rc = sqlite3_step(s)) == SQLITE_ROW) {
        int64_t caller_id = sqlite3_column_int64(s, 0);
        int64_t route_id = sqlite3_column_int64(s, SKIP_ONE);
        const char *props = (const char *)sqlite3_column_text(s, PAIR_LEN);

        char url_path[CBM_SZ_256] = {0};
        char broker[CBM_SZ_128] = {0};
        json_str_prop(props, "url_path", url_path, sizeof(url_path));
        json_str_prop(props, "broker", broker, sizeof(broker));
        if (!url_path[0]) {
            continue;
        }

        char route_qn[CR_QN_BUF];
        snprintf(route_qn, sizeof(route_qn), "__route__%s__%s", broker[0] ? broker : "async",
                 url_path);

        char handler_name[CBM_SZ_256] = {0};
        char handler_file[CBM_SZ_512] = {0};
        bool query_failed = false;
        int64_t handler_id =
            find_route_handler(tgt_store, route_qn, handler_name, sizeof(handler_name),
                               handler_file, sizeof(handler_file), &query_failed);
        if (query_failed) {
            failed = true;
            break;
        }
        if (handler_id == 0) {
            continue;
        }

        if (cr_cancel_requested(ctx)) {
            break;
        }

        char edge_props[CR_PROPS_BUF];
        build_cross_props(edge_props, sizeof(edge_props), tgt_project, handler_name, handler_file,
                          url_path, "url_path", broker);
        if (!insert_cross_edge(src_store, src_project, caller_id, route_id, "CROSS_ASYNC_CALLS",
                               edge_props, ctx)) {
            failed = !cr_cancel_requested(ctx);
            break;
        }
        count++;
    }
    if (!failed && !cr_cancel_requested(ctx) && step_rc != SQLITE_DONE) {
        failed = true;
    }
    if (sqlite3_finalize(s) != SQLITE_OK) {
        failed = true;
    }
    return cr_match_finish(ctx, count, failed);
}

/* ── Phase C: Channel matching ───────────────────────────────────── */

/* Try to find a matching listener in target DB for a channel name. Returns 1
 * for a complete bidirectional pair, 0 for no match, and CBM_STORE_ERR when a
 * query or write fails. */
static int try_match_channel_listener(cbm_store_t *src_store, const char *src_project,
                                      cbm_store_t *tgt_store, const char *tgt_project,
                                      const char *channel_name, const char *transport,
                                      int64_t emitter_id, int64_t channel_id,
                                      cr_run_context_t *ctx) {
    if (cr_cancel_requested(ctx)) {
        return CBM_STORE_NOT_FOUND;
    }
    struct sqlite3 *tgt_db = cbm_store_get_db(tgt_store);
    if (!tgt_db) {
        return CBM_STORE_ERR;
    }
    sqlite3_stmt *tq = NULL;
    if (sqlite3_prepare_v2(tgt_db,
                           "SELECT n.id, e.source_id, fn.name, fn.file_path FROM nodes n "
                           "JOIN edges e ON e.target_id = n.id AND e.type = 'LISTENS_ON' "
                           "JOIN nodes fn ON fn.id = e.source_id "
                           "WHERE n.project = ?1 AND n.name = ?2 AND n.label = 'Channel' LIMIT 1",
                           CBM_NOT_FOUND, &tq, NULL) != SQLITE_OK) {
        return CBM_STORE_ERR;
    }
    if (sqlite3_bind_text(tq, SKIP_ONE, tgt_project, CBM_NOT_FOUND, SQLITE_STATIC) != SQLITE_OK ||
        sqlite3_bind_text(tq, PAIR_LEN, channel_name, CBM_NOT_FOUND, SQLITE_STATIC) != SQLITE_OK) {
        sqlite3_finalize(tq);
        return CBM_STORE_ERR;
    }

    int64_t tgt_channel_id = 0;
    int64_t listener_id = 0;
    char listener_name[CBM_SZ_256] = {0};
    char listener_file[CBM_SZ_512] = {0};
    int step_rc = sqlite3_step(tq);
    if (step_rc == SQLITE_ROW) {
        tgt_channel_id = sqlite3_column_int64(tq, 0);
        listener_id = sqlite3_column_int64(tq, SKIP_ONE);
        const char *name = (const char *)sqlite3_column_text(tq, PAIR_LEN);
        const char *file = (const char *)sqlite3_column_text(tq, CR_COL_3);
        snprintf(listener_name, sizeof(listener_name), "%s", name ? name : "");
        snprintf(listener_file, sizeof(listener_file), "%s", file ? file : "");
    }
    int finalize_rc = sqlite3_finalize(tq);
    if ((step_rc != SQLITE_ROW && step_rc != SQLITE_DONE) || finalize_rc != SQLITE_OK) {
        return CBM_STORE_ERR;
    }
    if (step_rc == SQLITE_DONE) {
        return 0;
    }

    char caller_name[CBM_SZ_256] = {0};
    char caller_file[CBM_SZ_512] = {0};
    if (!lookup_node_info(cbm_store_get_db(src_store), emitter_id, caller_name, sizeof(caller_name),
                          caller_file, sizeof(caller_file))) {
        return CBM_STORE_ERR;
    }

    /* Forward edge: emitter → local Channel */
    char fwd[CR_PROPS_BUF];
    build_cross_props(fwd, sizeof(fwd), tgt_project, listener_name, listener_file, channel_name,
                      "channel_name", transport);
    if (!insert_cross_edge(src_store, src_project, emitter_id, channel_id, "CROSS_CHANNEL", fwd,
                           ctx)) {
        return cr_cancel_requested(ctx) ? CBM_STORE_NOT_FOUND : CBM_STORE_ERR;
    }
    if (cr_cancel_requested(ctx)) {
        return CBM_STORE_NOT_FOUND;
    }

    /* Reverse edge: listener → target Channel */
    char rev[CR_PROPS_BUF];
    build_cross_props(rev, sizeof(rev), src_project, caller_name, caller_file, channel_name,
                      "channel_name", transport);
    return insert_cross_edge(tgt_store, tgt_project, listener_id, tgt_channel_id, "CROSS_CHANNEL",
                             rev, ctx)
               ? 1
               : (cr_cancel_requested(ctx) ? CBM_STORE_NOT_FOUND : CBM_STORE_ERR);
}

static cr_match_result_t match_channels(cbm_store_t *src_store, const char *src_project,
                                        cbm_store_t *tgt_store, const char *tgt_project,
                                        cr_run_context_t *ctx) {
    struct sqlite3 *src_db = cbm_store_get_db(src_store);
    if (!src_db) {
        return cr_match_finish(ctx, 0, true);
    }

    sqlite3_stmt *s = NULL;
    if (sqlite3_prepare_v2(src_db,
                           "SELECT DISTINCT n.id, n.name, n.qualified_name, n.properties, "
                           "e.source_id FROM nodes n "
                           "JOIN edges e ON e.target_id = n.id AND e.type = 'EMITS' "
                           "WHERE n.project = ?1 AND n.label = 'Channel' "
                           "ORDER BY n.id, e.source_id",
                           CBM_NOT_FOUND, &s, NULL) != SQLITE_OK) {
        return cr_match_finish(ctx, 0, true);
    }
    if (sqlite3_bind_text(s, SKIP_ONE, src_project, CBM_NOT_FOUND, SQLITE_STATIC) != SQLITE_OK) {
        sqlite3_finalize(s);
        return cr_match_finish(ctx, 0, true);
    }

    int count = 0;
    int step_rc = SQLITE_DONE;
    bool failed = false;
    while (!cr_cancel_requested(ctx) && (step_rc = sqlite3_step(s)) == SQLITE_ROW) {
        const char *channel_name = (const char *)sqlite3_column_text(s, SKIP_ONE);
        const char *channel_qn = (const char *)sqlite3_column_text(s, PAIR_LEN);
        if (!channel_name || !channel_qn) {
            continue;
        }
        int64_t channel_id = sqlite3_column_int64(s, 0);
        const char *channel_props = (const char *)sqlite3_column_text(s, CR_COL_3);
        int64_t emitter_id = sqlite3_column_int64(s, CR_COL_4);

        char *channel_name_copy = cbm_strdup(channel_name);
        if (!channel_name_copy) {
            failed = true;
            break;
        }
        char transport[CBM_SZ_64] = {0};
        json_str_prop(channel_props, "transport", transport, sizeof(transport));

        int matched =
            try_match_channel_listener(src_store, src_project, tgt_store, tgt_project,
                                       channel_name_copy, transport, emitter_id, channel_id, ctx);
        free(channel_name_copy);
        if (matched == CBM_STORE_ERR) {
            failed = true;
            break;
        }
        if (matched == CBM_STORE_NOT_FOUND && cr_cancel_requested(ctx)) {
            break;
        }
        if (matched > 0) {
            count++;
        }
    }
    if (!failed && !cr_cancel_requested(ctx) && step_rc != SQLITE_DONE) {
        failed = true;
    }
    if (sqlite3_finalize(s) != SQLITE_OK) {
        failed = true;
    }
    return cr_match_finish(ctx, count, failed);
}

/* ── Phase D: Generic route-type matcher (gRPC, GraphQL, tRPC) ──── */

/* Look up a node's qualified_name by id: a heap copy of the whole name
 * (never truncated: the lookup that follows is exact), or NULL. Reports SQL
 * and allocation failures separately from an ordinary miss. */
static char *lookup_node_qn(struct sqlite3 *db, int64_t node_id, bool *failed) {
    *failed = false;
    if (!db) {
        *failed = true;
        return NULL;
    }
    sqlite3_stmt *st = NULL;
    if (sqlite3_prepare_v2(db, "SELECT qualified_name FROM nodes WHERE id = ?1", CBM_NOT_FOUND, &st,
                           NULL) != SQLITE_OK) {
        *failed = true;
        return NULL;
    }
    if (sqlite3_bind_int64(st, SKIP_ONE, node_id) != SQLITE_OK) {
        sqlite3_finalize(st);
        *failed = true;
        return NULL;
    }
    char *found = NULL;
    int step_rc = sqlite3_step(st);
    if (step_rc == SQLITE_ROW) {
        const char *qn = (const char *)sqlite3_column_text(st, 0);
        if (qn) {
            found = strdup(qn);
            *failed = found == NULL;
        }
    } else if (step_rc != SQLITE_DONE) {
        *failed = true;
    }
    if (sqlite3_finalize(st) != SQLITE_OK) {
        *failed = true;
    }
    if (*failed) {
        free(found);
        return NULL;
    }
    return found;
}

/* Is the target's Route `route_qn` marked "ambiguous" (pass_route_nodes.c:
 * several schemas define the GraphQL field and no one resolver owns it)? */
static bool cr_route_ambiguous(cbm_store_t *store, const char *route_qn, bool *failed) {
    *failed = false;
    struct sqlite3 *db = cbm_store_get_db(store);
    sqlite3_stmt *st = NULL;
    if (!db || sqlite3_prepare_v2(db,
                                  "SELECT json_extract(properties, '$.ambiguous') FROM nodes "
                                  "WHERE qualified_name = ?1 AND label = 'Route' LIMIT 1",
                                  CBM_NOT_FOUND, &st, NULL) != SQLITE_OK) {
        *failed = true;
        return false;
    }
    bool ambiguous = false;
    if (sqlite3_bind_text(st, SKIP_ONE, route_qn, CBM_NOT_FOUND, SQLITE_STATIC) != SQLITE_OK) {
        *failed = true;
    } else {
        int step_rc = sqlite3_step(st);
        if (step_rc == SQLITE_ROW) {
            ambiguous = sqlite3_column_int(st, 0) != 0;
        } else if (step_rc != SQLITE_DONE) {
            *failed = true;
        }
    }
    if (sqlite3_finalize(st) != SQLITE_OK) {
        *failed = true;
    }
    return ambiguous && !*failed;
}

/* A call into a target Route several handlers claim (a GraphQL field two
 * APIs in one checkout define, which the route pass marks "ambiguous"): the
 * forward edge names the target project and marks the link ambiguous, but
 * names no handler, since any one would be a guess; there is no reverse
 * edge, as no handler is known to serve this caller. */
static bool cr_emit_ambiguous(cbm_store_t *src_store, const char *src_project, int64_t caller_id,
                              int64_t local_route_id, const char *tgt_project,
                              const char *url_path, const char *method, const char *edge_type,
                              cr_run_context_t *ctx) {
    char fwd[CR_PROPS_BUF];
    build_cross_props(fwd, sizeof(fwd), tgt_project, "", "", url_path, "url_path", method);
    cr_set_ambiguous(fwd, sizeof(fwd), true);
    return insert_cross_edge(src_store, src_project, caller_id, local_route_id, edge_type, fwd,
                             ctx);
}

/* The forward row a client already holds for (caller, local Route, type):
 * whether it exists, the project it names and its ambiguous flag. False on
 * a query error. */
typedef struct {
    bool exists;
    bool ambiguous;
    char project[CBM_SZ_256];
} cr_forward_row_t;

static bool cr_forward_row(struct sqlite3 *db, int64_t caller_id, int64_t route_id,
                           const char *edge_type, cr_forward_row_t *row) {
    memset(row, 0, sizeof(*row));
    sqlite3_stmt *st = NULL;
    if (sqlite3_prepare_v2(db,
                           "SELECT json_extract(properties, '$.target_project'), "
                           "json_extract(properties, '$.ambiguous') FROM edges WHERE "
                           "source_id = ?1 AND target_id = ?2 AND type = ?3 LIMIT 1",
                           CBM_NOT_FOUND, &st, NULL) != SQLITE_OK) {
        return false;
    }
    bool ok = sqlite3_bind_int64(st, SKIP_ONE, caller_id) == SQLITE_OK &&
              sqlite3_bind_int64(st, PAIR_LEN, route_id) == SQLITE_OK &&
              sqlite3_bind_text(st, CBM_SZ_3, edge_type, CBM_NOT_FOUND, SQLITE_STATIC) == SQLITE_OK;
    int step_rc = ok ? sqlite3_step(st) : SQLITE_ERROR;
    if (step_rc == SQLITE_ROW) {
        const char *project = (const char *)sqlite3_column_text(st, 0);
        row->exists = true;
        row->ambiguous = sqlite3_column_int(st, SKIP_ONE) != 0;
        snprintf(row->project, sizeof(row->project), "%s", project ? project : "");
    }
    ok = (step_rc == SQLITE_ROW || step_rc == SQLITE_DONE) && sqlite3_finalize(st) == SQLITE_OK;
    return ok;
}

/* Drop a client's forward row (its provider no longer serves the call). */
static bool cr_delete_forward_row(cbm_store_t *store, int64_t caller_id, int64_t route_id,
                                  const char *edge_type, cr_run_context_t *ctx) {
    struct sqlite3 *db = cbm_store_get_db(store);
    sqlite3_stmt *st = NULL;
    if (!db || sqlite3_prepare_v2(db,
                                  "DELETE FROM edges WHERE source_id = ?1 AND target_id = ?2 AND "
                                  "type = ?3",
                                  CBM_NOT_FOUND, &st, NULL) != SQLITE_OK) {
        return false;
    }
    bool ok = sqlite3_bind_int64(st, SKIP_ONE, caller_id) == SQLITE_OK &&
              sqlite3_bind_int64(st, PAIR_LEN, route_id) == SQLITE_OK &&
              sqlite3_bind_text(st, CBM_SZ_3, edge_type, CBM_NOT_FOUND, SQLITE_STATIC) ==
                  SQLITE_OK &&
              sqlite3_step(st) == SQLITE_DONE;
    ok = sqlite3_finalize(st) == SQLITE_OK && ok;
    if (ok && sqlite3_changes(db) > 0) {
        ctx->mutated = true;
    }
    return ok;
}

/* Matchers call the policy after filtering out forward rows owned by another
 * provider. On a caller's run, an ambiguous match ranks below an existing
 * resolved match from another target. */
static cbm_cross_route_action_t cr_plan_typed_route(bool provider_side, bool has_handler,
                                                    bool ambiguous, bool names_other,
                                                    bool existing_ambiguous) {
    if (!has_handler) {
        return provider_side ? CBM_CROSS_ROUTE_DROP : CBM_CROSS_ROUTE_KEEP;
    }
    if (!ambiguous) {
        return CBM_CROSS_ROUTE_LINK_RESOLVED;
    }
    if (!provider_side && names_other && !existing_ambiguous) {
        return CBM_CROSS_ROUTE_KEEP;
    }
    return CBM_CROSS_ROUTE_LINK_AMBIGUOUS;
}

/* Match edges of a given type against Route nodes with a given QN prefix.
 * Reuses the same infrastructure as HTTP/async matching.
 *
 * provider_side: this run belongs to the provider (tgt_*) and src_* is a
 * client store. The client chose its targets on its own runs, so the
 * provider only maintains the forward rows that already name it: it
 * refreshes them (and rewrites its own reverse rows, which its run's
 * cleanup removed), drops them when it no longer serves the call, and never
 * creates a row or touches one naming another server. On the client's own
 * run (provider_side false) a resolved target wins over an ambiguous one
 * for the same row, and the flag is written both ways. */
static cr_match_result_t match_typed_routes(cbm_store_t *src_store, const char *src_project,
                                            cbm_store_t *tgt_store, const char *tgt_project,
                                            const char *edge_type, const char *svc_key,
                                            const char *method_key, const char *cross_edge_type,
                                            bool provider_side, cr_run_context_t *ctx) {
    struct sqlite3 *src_db = cbm_store_get_db(src_store);
    if (!src_db) {
        return cr_match_finish(ctx, 0, true);
    }

    char sql[CBM_SZ_256];
    snprintf(sql, sizeof(sql),
             "SELECT e.source_id, e.target_id, e.properties FROM edges e "
             "WHERE e.project = ?1 AND e.type = '%s' ORDER BY e.id",
             edge_type);

    sqlite3_stmt *s = NULL;
    if (sqlite3_prepare_v2(src_db, sql, CBM_NOT_FOUND, &s, NULL) != SQLITE_OK) {
        return cr_match_finish(ctx, 0, true);
    }
    if (sqlite3_bind_text(s, SKIP_ONE, src_project, CBM_NOT_FOUND, SQLITE_STATIC) != SQLITE_OK) {
        sqlite3_finalize(s);
        return cr_match_finish(ctx, 0, true);
    }

    int count = 0;
    int step_rc = SQLITE_DONE;
    bool failed = false;
    bool use_rust = cr_graphql_link_policy && strcmp(edge_type, "GRAPHQL_CALLS") == 0;
    cbm_graphql_link_policy_fn plan = use_rust ? cr_graphql_link_policy : cr_plan_typed_route;
    while (!cr_cancel_requested(ctx) && (step_rc = sqlite3_step(s)) == SQLITE_ROW) {
        int64_t caller_id = sqlite3_column_int64(s, 0);
        int64_t route_id = sqlite3_column_int64(s, SKIP_ONE);
        const char *props = (const char *)sqlite3_column_text(s, PAIR_LEN);

        char svc_val[CBM_SZ_256] = {0};
        char meth_val[CBM_SZ_256] = {0};
        json_str_prop(props, svc_key, svc_val, sizeof(svc_val));
        json_str_prop(props, method_key, meth_val, sizeof(meth_val));
        if (!svc_val[0] && !meth_val[0]) {
            continue;
        }

        /* Look up the Route QN from the target node (already points to the Route). */
        bool query_failed = false;
        char *route_qn = lookup_node_qn(src_db, route_id, &query_failed);
        if (!route_qn) {
            if (query_failed) {
                failed = true;
                break;
            }
            continue;
        }

        cr_forward_row_t row;
        if (!cr_forward_row(src_db, caller_id, route_id, cross_edge_type, &row)) {
            free(route_qn);
            failed = true;
            break;
        }
        bool names_other = row.exists && strcmp(row.project, tgt_project) != 0;
        if (provider_side && (!row.exists || names_other)) {
            free(route_qn); /* not this provider's link to maintain */
            continue;
        }
        char handler_name[CBM_SZ_256] = {0};
        char handler_file[CBM_SZ_512] = {0};
        cbm_graphql_handler_t handler;
        if (use_rust) {
            handler =
                cr_graphql_find_handler(cbm_store_get_db(tgt_store), route_qn, handler_name,
                                        sizeof(handler_name), handler_file, sizeof(handler_file));
        } else {
            handler.handler_id =
                find_route_handler(tgt_store, route_qn, handler_name, sizeof(handler_name),
                                   handler_file, sizeof(handler_file), &query_failed);
            handler.ambiguous = !query_failed && handler.handler_id != 0 &&
                                cr_route_ambiguous(tgt_store, route_qn, &query_failed);
            handler.failed = query_failed;
        }
        query_failed = handler.failed;
        int64_t handler_id = handler.handler_id;
        bool ambiguous = handler.ambiguous;
        bool cancelled = !query_failed && cr_cancel_requested(ctx);
        cbm_cross_route_action_t action =
            query_failed || cancelled
                ? CBM_CROSS_ROUTE_KEEP
                : plan(provider_side, handler_id != 0, ambiguous, names_other, row.ambiguous);
        bool emitted = false;
        switch (action) {
        case CBM_CROSS_ROUTE_KEEP:
            break;
        case CBM_CROSS_ROUTE_DROP:
            if (ctx->staging_store && src_store != ctx->staging_store) {
                emitted = true;
            } else if (use_rust) {
                int deleted = cr_graphql_delete_link(src_db, caller_id, route_id);
                emitted = deleted >= 0;
                ctx->mutated = ctx->mutated || deleted > 0;
            } else {
                emitted =
                    cr_delete_forward_row(src_store, caller_id, route_id, cross_edge_type, ctx);
            }
            break;
        case CBM_CROSS_ROUTE_LINK_AMBIGUOUS:
            emitted = cr_emit_ambiguous(src_store, src_project, caller_id, route_id, tgt_project,
                                        svc_val, svc_key, cross_edge_type, ctx);
            break;
        case CBM_CROSS_ROUTE_LINK_RESOLVED:
            emitted = emit_cross_route_bidirectional(src_store, src_project, src_db, caller_id,
                                                     route_id, tgt_store, tgt_project, handler_id,
                                                     route_qn, handler_name, handler_file, svc_val,
                                                     svc_key, cross_edge_type, true, ctx);
            break;
        }
        free(route_qn);
        if (query_failed) {
            failed = true;
            break;
        }
        if (cancelled) {
            break;
        }
        if (action == CBM_CROSS_ROUTE_KEEP) {
            continue;
        }
        if (!emitted) {
            failed = !cr_cancel_requested(ctx);
            break;
        }
        /* An ambiguous Route is linked but not resolved, and a dropped row is
         * no link: neither is counted. */
        count += action == CBM_CROSS_ROUTE_LINK_RESOLVED ? SKIP_ONE : 0;
    }
    if (!failed && !cr_cancel_requested(ctx) && step_rc != SQLITE_DONE) {
        failed = true;
    }
    if (sqlite3_finalize(s) != SQLITE_OK) {
        failed = true;
    }
    return cr_match_finish(ctx, count, failed);
}

/* ── Collect target projects ─────────────────────────────────────── */

static void free_project_list(char **projects, int count);

/* Remember a store the ["*"] enumeration skipped, so the caller can name it
 * instead of the run silently covering fewer projects than exist. */
static bool cr_record_skip(cbm_cross_repo_result_t *result, const char *project,
                           const char *reason) {
    if (!result) {
        return true;
    }
    cbm_cross_repo_skip_t *grown =
        cbm_realloc(CBM_MEM_CLASS_OTHER, result->skipped_projects,
                    ((size_t)result->skipped_count + 1U) * sizeof(*result->skipped_projects));
    if (!grown) {
        return false;
    }
    result->skipped_projects = grown;
    char *name = cbm_mem_strdup(CBM_MEM_CLASS_OTHER, project);
    if (!name) {
        return false;
    }
    grown[result->skipped_count].project = name;
    grown[result->skipped_count].reason = reason;
    result->skipped_count++;
    return true;
}

/* When target_projects = ["*"], scan the cache directory for all .db files. */
static int collect_all_projects(char ***out, cr_run_context_t *ctx,
                                cbm_cross_repo_result_t *result) {
    *out = NULL;
    const char *dir = cr_cache_dir();
    cbm_dir_t *d = cbm_opendir(dir);
    if (!d) {
        return -1;
    }

    int cap = CR_INIT_CAP;
    int count = 0;
    char **projects = malloc((size_t)cap * sizeof(char *));
    if (!projects) {
        cbm_closedir(d);
        return -1;
    }

    bool failed = false;
    int entries_scanned = 0;

    cbm_dirent_t *ent;
    while ((ent = cbm_readdir(d)) != NULL) {
        if (cr_cancel_requested(ctx)) {
            failed = true;
            break;
        }
        entries_scanned++;
        if (entries_scanned > CR_MAX_CACHE_ENTRIES) {
            failed = true;
            break;
        }
        /* Internal stores are exact filenames (cbm_is_internal_cache_db).
         * Substring filtering would hide legitimate projects such as
         * orders_config_service or api-wal. */
        if (!cbm_is_project_index_db(ent->name)) {
            continue;
        }
        size_t len = strlen(ent->name);
        if (count >= CR_MAX_PROJECTS) {
            failed = true;
            break;
        }
        char project[CBM_DIRENT_NAME_MAX];
        size_t project_length = len - CR_DB_EXT_LEN;
        if (project_length == 0 || project_length >= sizeof(project)) {
            continue;
        }
        memcpy(project, ent->name, project_length);
        project[project_length] = '\0';
        if (!cbm_validate_project_name(project)) {
            continue;
        }
        cr_project_state_t state = cr_project_state(project);
        if (state == CR_PROJECT_LEGACY && !cr_record_skip(result, project, "pre_768_schema")) {
            failed = true;
            break;
        }
        if (state != CR_PROJECT_USABLE) {
            continue;
        }
        if (count >= cap) {
            cap *= PAIR_LEN;
            char **tmp = realloc(projects, (size_t)cap * sizeof(char *));
            if (!tmp) {
                failed = true;
                break;
            }
            projects = tmp;
        }
        projects[count] = cbm_strdup(project);
        if (!projects[count]) {
            failed = true;
            break;
        }
        count++;
    }
    cbm_closedir(d);

    if (failed) {
        free_project_list(projects, count);
        return cr_cancel_requested(ctx) ? CBM_STORE_NOT_FOUND : CBM_STORE_ERR;
    }
    qsort(projects, (size_t)count, sizeof(*projects), cr_project_compare);
    int unique_count = 0;
    for (int i = 0; i < count; i++) {
        if (unique_count == 0 || strcmp(projects[i], projects[unique_count - 1]) != 0) {
            projects[unique_count++] = projects[i];
        } else {
            free(projects[i]);
        }
    }
    *out = projects;
    return unique_count;
}

static void free_project_list(char **projects, int count) {
    for (int i = 0; i < count; i++) {
        free(projects[i]);
    }
    free(projects);
}

static int collect_named_projects(const char **targets, int target_count, char ***out,
                                  cr_run_context_t *ctx) {
    *out = NULL;
    if (!targets || target_count <= 0 || target_count > CR_MAX_PROJECTS) {
        return -1;
    }
    char **projects = calloc((size_t)target_count, sizeof(*projects));
    if (!projects) {
        return -1;
    }
    int count = 0;
    for (int i = 0; i < target_count; i++) {
        if (cr_cancel_requested(ctx)) {
            free_project_list(projects, count);
            return CBM_STORE_NOT_FOUND;
        }
        if (!targets[i] || strcmp(targets[i], "*") == 0 || !cbm_validate_project_name(targets[i])) {
            free_project_list(projects, count);
            return -1;
        }
        projects[count] = cbm_strdup(targets[i]);
        if (!projects[count]) {
            free_project_list(projects, count);
            return -1;
        }
        count++;
    }
    qsort(projects, (size_t)count, sizeof(*projects), cr_project_compare);
    int unique_count = 0;
    for (int i = 0; i < count; i++) {
        if (unique_count == 0 || strcmp(projects[i], projects[unique_count - 1]) != 0) {
            projects[unique_count++] = projects[i];
        } else {
            free(projects[i]);
        }
    }
    *out = projects;
    return unique_count;
}

/* ── Last-run marker ─────────────────────────────────────────────── */

static bool cr_status_key(const char *project, char *buf, size_t bufsz) {
    int written = snprintf(buf, bufsz, "cross_repo_last_run:%s", project);
    return written > 0 && (size_t)written < bufsz;
}

/* A failed explicit run updates its result marker but does not replace the
 * selection to replay on the next index. */
static bool cr_targets_key(const char *project, char *buf, size_t size) {
    int written = snprintf(buf, size, "cross_repo_targets:%s", project);
    return written > 0 && (size_t)written < size;
}

static bool cr_record_targets(cbm_store_t *store, const char *project, const char **targets,
                              int target_count) {
    char key[CR_QN_BUF];
    if (!cr_targets_key(project, key, sizeof(key))) {
        return false;
    }
    yyjson_mut_doc *doc = yyjson_mut_doc_new(NULL);
    yyjson_mut_val *arr = doc ? yyjson_mut_arr(doc) : NULL;
    bool ok = arr != NULL;
    for (int i = 0; ok && i < target_count; i++) {
        ok = yyjson_mut_arr_append(arr, yyjson_mut_strcpy(doc, targets[i]));
    }
    if (arr) {
        yyjson_mut_doc_set_root(doc, arr);
    }
    char *json = ok ? yyjson_mut_write(doc, 0, NULL) : NULL;
    ok = json && cbm_store_meta_put(store, key, json) == CBM_STORE_OK;
    free(json);
    yyjson_mut_doc_free(doc);
    return ok;
}

/* Returns zero for an index that has not recorded a successful selection. */
static int cr_read_targets(cbm_store_t *store, const char *project, char ***targets) {
    *targets = NULL;
    char key[CR_QN_BUF];
    if (!cr_targets_key(project, key, sizeof(key))) {
        return CBM_STORE_ERR;
    }
    char *json = NULL;
    int rc = cbm_store_meta_get(store, key, &json);
    bool legacy = rc == CBM_STORE_NOT_FOUND;
    if (legacy) {
        if (!cr_status_key(project, key, sizeof(key))) {
            return CBM_STORE_ERR;
        }
        rc = cbm_store_meta_get(store, key, &json);
    }
    if (rc == CBM_STORE_NOT_FOUND) {
        return 0;
    }
    if (rc != CBM_STORE_OK) {
        return CBM_STORE_ERR;
    }
    yyjson_doc *doc = yyjson_read(json, strlen(json), 0);
    cbm_free(CBM_MEM_CLASS_STORE, json);
    yyjson_val *root = yyjson_doc_get_root(doc);
    yyjson_val *arr = root;
    if (legacy) {
        const char *outcome = yyjson_get_str(yyjson_obj_get(root, "outcome"));
        if (!outcome || strcmp(outcome, "complete") != 0) {
            yyjson_doc_free(doc);
            return 0;
        }
        arr = yyjson_obj_get(root, "targets");
    }
    size_t count = yyjson_arr_size(arr);
    if (!yyjson_is_arr(arr) || count == 0 || count > CR_MAX_PROJECTS) {
        yyjson_doc_free(doc);
        return CBM_STORE_ERR;
    }
    char **names = calloc(count, sizeof(*names));
    size_t i = 0;
    for (; names && i < count; i++) {
        const char *name = yyjson_get_str(yyjson_arr_get(arr, i));
        if (!name ||
            (!(count == 1 && strcmp(name, "*") == 0) && !cbm_validate_project_name(name))) {
            break;
        }
        names[i] = cbm_strdup(name);
        if (!names[i]) {
            break;
        }
    }
    yyjson_doc_free(doc);
    if (i != count) {
        free_project_list(names, (int)i);
        return CBM_STORE_ERR;
    }
    *targets = names;
    return (int)count;
}

static bool cr_peers_key(const char *project, char *buf, size_t size) {
    int written = snprintf(buf, size, "cross_repo_peers:%s", project);
    return written > 0 && (size_t)written < size;
}

static cbm_store_t *cr_open_peer(const char *name) {
    char path[CR_PATH_BUF];
    return cr_db_path(name, path, sizeof(path)) ? cbm_store_open_path_query(path) : NULL;
}

static bool cr_save_peers(cbm_store_t *source, const char *project, yyjson_mut_doc *peers) {
    char key[CR_QN_BUF];
    if (!cr_peers_key(project, key, sizeof(key))) {
        return false;
    }
    char *json = yyjson_mut_write(peers, 0, NULL);
    bool ok = json && cbm_store_meta_put(source, key, json) == CBM_STORE_OK;
    free(json);
    return ok;
}

static yyjson_doc *cr_read_peers(cbm_store_t *source, const char *project) {
    char key[CR_QN_BUF];
    char *json = NULL;
    if (!cr_peers_key(project, key, sizeof(key)) ||
        cbm_store_meta_get(source, key, &json) != CBM_STORE_OK) {
        return NULL;
    }
    yyjson_doc *doc = yyjson_read(json, strlen(json), 0);
    cbm_free(CBM_MEM_CLASS_STORE, json);
    return doc;
}

static bool cr_reverse_pending_key(const char *project, char *buf, size_t size) {
    int written = snprintf(buf, size, "cross_repo_reverse_pending:%s", project);
    return written > 0 && (size_t)written < size;
}

static bool cr_reverse_pending(cbm_store_t *source, const char *project) {
    char key[CR_QN_BUF];
    char *value = NULL;
    bool pending = cr_reverse_pending_key(project, key, sizeof(key)) &&
                   cbm_store_meta_get(source, key, &value) == CBM_STORE_OK &&
                   strcmp(value, "1") == 0;
    cbm_free(CBM_MEM_CLASS_STORE, value);
    return pending;
}

static bool cr_set_reverse_pending(cbm_store_t *source, const char *project, bool pending) {
    char key[CR_QN_BUF];
    return cr_reverse_pending_key(project, key, sizeof(key)) &&
           cbm_store_meta_put(source, key, pending ? "1" : "0") == CBM_STORE_OK;
}

bool cbm_cross_repo_reverse_pending(const char *project) {
    cbm_store_t *store = cr_open_peer(project);
    bool pending = store && cr_reverse_pending(store, project);
    cbm_store_close(store);
    return pending;
}

/* Write generation observations against the same connection that the
 * matcher reads. An unavailable provider is represented by JSON null. */
static bool cr_remember_peer(yyjson_mut_doc *peers, const char *name, cbm_store_t *provider) {
    yyjson_mut_val *root = yyjson_mut_doc_get_root(peers);
    if (!provider) {
        return yyjson_mut_obj_add_null(peers, root, name);
    }
    char generation[CBM_GRAPH_COMPARE_GENERATION_SIZE];
    return cbm_store_generation(provider, generation, sizeof(generation)) == CBM_STORE_OK &&
           yyjson_mut_obj_add_strcpy(peers, root, name, generation);
}

static bool cr_peer_is_stale(const char *name, const char *observed) {
    if (!observed) {
        return true;
    }
    cbm_store_t *peer = cr_open_peer(name);
    char generation[CBM_GRAPH_COMPARE_GENERATION_SIZE];
    bool same = peer &&
                cbm_store_generation(peer, generation, sizeof(generation)) == CBM_STORE_OK &&
                strcmp(observed, generation) == 0;
    cbm_store_close(peer);
    return !same;
}

static bool cr_has_stale_peers(cbm_store_t *store, const char *project,
                               bool observations_required) {
    yyjson_doc *peers = cr_read_peers(store, project);
    yyjson_val *observations = yyjson_doc_get_root(peers);
    yyjson_obj_iter iter = yyjson_obj_iter_with(observations);
    bool stale = observations_required && !yyjson_is_obj(observations);
    yyjson_val *key;
    while (!stale && (key = yyjson_obj_iter_next(&iter))) {
        stale = cr_peer_is_stale(yyjson_get_str(key), yyjson_get_str(yyjson_obj_iter_get_val(key)));
    }
    yyjson_doc_free(peers);
    return stale;
}

/* Record this run in the SOURCE store's store_meta, next to the CROSS_*
 * edges it just rewrote: a reindex that rebuilds the store drops both
 * together, so the marker can never outlive the links it describes. Written
 * once the source's previous CROSS_* generation was cleaned, whatever the
 * outcome, because from then on the old summary no longer matches the graph.
 * Project names are validated ([A-Za-z0-9._-]) or "*", so no JSON escaping is
 * needed. Best-effort: a marker failure is logged, never fails the run. */
static bool cr_record_last_run(cbm_store_t *src_store, const char *project, const char **targets,
                               int target_count, const cbm_cross_repo_result_t *result) {
    char key[CR_QN_BUF];
    if (!cr_status_key(project, key, sizeof(key))) {
        return false;
    }
    size_t cap = CR_PROPS_BUF;
    for (int i = 0; i < target_count; i++) {
        cap += (targets[i] ? strlen(targets[i]) : 0) + CR_COL_4;
    }
    char *json = cbm_alloc(CBM_MEM_CLASS_OTHER, cap);
    if (!json) {
        return false;
    }
    char when[CBM_SZ_32];
    time_t now = time(NULL);
    struct tm tm_now;
    if (!cbm_gmtime_r(&now, &tm_now) ||
        strftime(when, sizeof(when), "%Y-%m-%dT%H:%M:%SZ", &tm_now) == 0) {
        when[0] = '\0';
    }
    const char *outcome = result->failed       ? "failed"
                          : result->cancelled  ? "cancelled"
                          : result->no_targets ? "incomplete"
                                               : "complete";
    size_t len = (size_t)snprintf(
        json, cap, "{\"last_run_at\":\"%s\",\"outcome\":\"%s\",\"targets\":[", when, outcome);
    for (int i = 0; i < target_count && len < cap; i++) {
        len += (size_t)snprintf(json + len, cap - len, "%s\"%s\"", i ? "," : "",
                                targets[i] ? targets[i] : "");
    }
    if (len < cap) {
        int total = result->http_edges + result->async_edges + result->channel_edges +
                    result->grpc_edges + result->graphql_edges + result->trpc_edges;
        len += (size_t)snprintf(json + len, cap - len,
                                "],\"projects_scanned\":%d,\"total_cross_edges\":%d,"
                                "\"skipped_projects\":%d}",
                                result->projects_scanned, total, result->skipped_count);
    }
    bool ok = len < cap && cbm_store_meta_put(src_store, key, json) == CBM_STORE_OK;
    if (!ok) {
        cbm_log_warn("cross_repo.status_unrecorded", "project", project);
    }
    cbm_free(CBM_MEM_CLASS_OTHER, json);
    return ok;
}

bool cbm_cross_repo_add_status_json(struct yyjson_mut_doc *doc, struct yyjson_mut_val *parent,
                                    cbm_store_t *store, const char *project) {
    char key[CR_QN_BUF];
    if (!doc || !parent || !store || !project || !cr_status_key(project, key, sizeof(key))) {
        return false;
    }
    char *raw = NULL;
    int rc = cbm_store_meta_get(store, key, &raw);
    yyjson_doc *recorded = rc == CBM_STORE_OK ? yyjson_read(raw, strlen(raw), 0) : NULL;
    cbm_free(CBM_MEM_CLASS_STORE, raw);
    yyjson_val *recorded_root = yyjson_doc_get_root(recorded);
    const char *outcome = yyjson_get_str(yyjson_obj_get(recorded_root, "outcome"));
    bool complete = outcome && strcmp(outcome, "complete") == 0;
    bool incomplete = outcome && strcmp(outcome, "incomplete") == 0;
    yyjson_mut_val *status = NULL;
    if (yyjson_is_obj(recorded_root)) {
        status = yyjson_val_mut_copy(doc, recorded_root);
    }
    if (!status) {
        status = yyjson_mut_obj(doc);
    }
    yyjson_doc_free(recorded);
    if (!status) {
        return false;
    }
    /* "never_run" is the honest reading of an absent marker: no run with this
     * project as the source has completed its cleanup since the store was
     * (re)built. A target-side run still leaves its CROSS_* edges here. */
    bool stale = incomplete || cr_has_stale_peers(store, project, complete);
    const char *state = stale                             ? "stale"
                        : rc == CBM_STORE_NOT_FOUND       ? "never_run"
                        : yyjson_mut_obj_size(status) > 0 ? "ran"
                                                          : "unavailable";
    yyjson_mut_obj_add_str(doc, status, "status", state);
    if (cr_reverse_pending(store, project)) {
        yyjson_mut_obj_add_bool(doc, status, "reverse_pending", true);
    }
    return yyjson_mut_obj_add_val(doc, parent, "cross_repo", status);
}

void cbm_cross_repo_result_free(cbm_cross_repo_result_t *result) {
    if (!result) {
        return;
    }
    for (int i = 0; i < result->skipped_count; i++) {
        cbm_free(CBM_MEM_CLASS_OTHER, result->skipped_projects[i].project);
    }
    cbm_free(CBM_MEM_CLASS_OTHER, result->skipped_projects);
    result->skipped_projects = NULL;
    result->skipped_count = 0;
}

static cr_run_status_t add_match_count(int *total, cr_match_result_t matched) {
    *total += matched.count;
    return matched.status;
}

static cr_run_status_t cr_match_project_pair(cbm_store_t *src_store, const char *project,
                                             cbm_store_t *tgt_store, const char *tgt,
                                             cr_run_context_t *run,
                                             cbm_cross_repo_result_t *result) {
    cr_run_status_t match_status =
        run->staging_store ? CR_RUN_OK : cr_reconcile_reverse(tgt_store, tgt, project, run);
    if (match_status == CR_RUN_OK) {
        match_status = add_match_count(&result->http_edges,
                                       match_http_routes(src_store, project, tgt_store, tgt, run));
    }
    /* Reverse direction: when this pass runs from the provider side, the
     * consumer's HTTP_CALLS live in tgt, not src — the forward pass above
     * finds nothing because the provider has no outbound calls. This also
     * re-creates the provider-side reverse edges that delete_cross_edges
     * just wiped, which a provider-side run previously destroyed for good.
     * A caller edge lives in exactly one DB, so the two directions scan
     * disjoint edge sets and never double-count a pair; the store's
     * (source, target, type) upsert keeps re-recorded rows unique. (#523) */
    if (match_status == CR_RUN_OK) {
        match_status = add_match_count(&result->http_edges,
                                       match_http_routes(tgt_store, tgt, src_store, project, run));
    }
    if (match_status == CR_RUN_OK) {
        match_status = add_match_count(&result->async_edges,
                                       match_async_routes(src_store, project, tgt_store, tgt, run));
    }
    if (match_status == CR_RUN_OK) {
        match_status = add_match_count(&result->channel_edges,
                                       match_channels(src_store, project, tgt_store, tgt, run));
    }
    if (match_status == CR_RUN_OK) {
        match_status = add_match_count(&result->grpc_edges,
                                       match_typed_routes(src_store, project, tgt_store, tgt,
                                                          "GRPC_CALLS", "service", "method",
                                                          "CROSS_GRPC_CALLS", false, run));
    }
    if (match_status == CR_RUN_OK) {
        match_status = add_match_count(&result->graphql_edges,
                                       match_typed_routes(src_store, project, tgt_store, tgt,
                                                          "GRAPHQL_CALLS", "operation", "operation",
                                                          "CROSS_GRAPHQL_CALLS", false, run));
    }
    if (match_status == CR_RUN_OK) {
        match_status = add_match_count(&result->trpc_edges,
                                       match_typed_routes(src_store, project, tgt_store, tgt,
                                                          "TRPC_CALLS", "procedure", "procedure",
                                                          "CROSS_TRPC_CALLS", false, run));
    }
    /* Provider side: the consumer's typed calls live in tgt. This run
     * maintains only the consumer's forward rows that already name this
     * provider (match_typed_routes, provider_side) and rewrites its own
     * reverse rows for them, which delete_cross_edges just wiped. Unlike
     * HTTP above, it never creates a consumer link or redirects one that
     * names another server. */
    if (match_status == CR_RUN_OK) {
        match_status =
            add_match_count(&result->grpc_edges,
                            match_typed_routes(tgt_store, tgt, src_store, project, "GRPC_CALLS",
                                               "service", "method", "CROSS_GRPC_CALLS", true, run));
    }
    if (match_status == CR_RUN_OK) {
        match_status = add_match_count(&result->graphql_edges,
                                       match_typed_routes(tgt_store, tgt, src_store, project,
                                                          "GRAPHQL_CALLS", "operation", "operation",
                                                          "CROSS_GRAPHQL_CALLS", true, run));
    }
    if (match_status == CR_RUN_OK) {
        match_status = add_match_count(&result->trpc_edges,
                                       match_typed_routes(tgt_store, tgt, src_store, project,
                                                          "TRPC_CALLS", "procedure", "procedure",
                                                          "CROSS_TRPC_CALLS", true, run));
    }

    return match_status;
}

/* Wildcard enumeration only returns usable projects. Keep unavailable peers
 * from the previous generation visible while the wildcard is being replayed. */
static bool cr_record_unavailable_peers(yyjson_mut_doc *current, yyjson_doc *previous,
                                        cr_run_context_t *run, cbm_cross_repo_result_t *result) {
    yyjson_mut_val *observations = yyjson_mut_doc_get_root(current);
    yyjson_obj_iter iter = yyjson_obj_iter_with(yyjson_doc_get_root(previous));
    yyjson_val *key;
    while ((key = yyjson_obj_iter_next(&iter))) {
        const char *target = yyjson_get_str(key);
        if (cr_cancel_requested(run)) {
            return false;
        }
        if (cbm_validate_project_name(target) && !yyjson_mut_obj_get(observations, target) &&
            (yyjson_mut_obj_size(observations) >= CR_MAX_PROJECTS ||
             !cr_remember_peer(current, target, NULL) ||
             !cr_record_skip(result, target, "target_unavailable"))) {
            return false;
        }
    }
    return true;
}

/* The live generation owns the previous selection. The stage may be a new
 * database, or an incremental copy whose link rows need to be replaced. */
int cbm_cross_repo_relink_stage(const char *live_path, const char *stage_path, const char *project,
                                const atomic_int *cancelled) {
    cbm_store_t *previous = cbm_store_open_path_query(live_path);
    if (!previous) {
        /* Keep the normal recovery path for an unreadable serving index. */
        return CBM_STORE_OK;
    }
    char **targets = NULL;
    int target_count = cr_read_targets(previous, project, &targets);
    bool wildcard = target_count == 1 && strcmp(targets[0], "*") == 0;
    yyjson_doc *previous_peers = wildcard ? cr_read_peers(previous, project) : NULL;
    cbm_store_close(previous);
    if (target_count <= 0) {
        return target_count < 0 ? CBM_STORE_ERR : CBM_STORE_OK;
    }

    cbm_store_t *staging = cbm_store_open_path_existing(stage_path);
    if (!staging) {
        yyjson_doc_free(previous_peers);
        free_project_list(targets, target_count);
        return CBM_STORE_ERR;
    }
    cr_run_context_t run = {.cancelled = cancelled, .staging_store = staging};
    cbm_cross_repo_result_t result = {0};
    char **resolved = NULL;
    int count = wildcard
                    ? collect_all_projects(&resolved, &run, &result)
                    : collect_named_projects((const char **)targets, target_count, &resolved, &run);
    yyjson_mut_doc *peers = yyjson_mut_doc_new(NULL);
    yyjson_mut_val *observations = peers ? yyjson_mut_obj(peers) : NULL;
    if (observations) {
        yyjson_mut_doc_set_root(peers, observations);
    }
    bool ok = count >= 0 && observations && delete_cross_edges(staging, project, &run) == CR_RUN_OK;
    for (int i = 0; ok && i < count; i++) {
        const char *target = resolved[i];
        if (strcmp(target, project) == 0) {
            continue;
        }
        cbm_store_t *provider = cr_open_peer(target);
        if (!provider || !cr_store_has_exact_project(provider, target) ||
            !cbm_store_edges_schema_current(provider)) {
            cbm_store_close(provider);
            ok = cr_record_skip(&result, target, "target_unavailable") &&
                 cr_remember_peer(peers, target, NULL);
            continue;
        }
        cbm_store_read_scope_t *scope = NULL;
        ok = cbm_store_read_scope_open(provider, NULL, NULL, &scope) == CBM_STORE_OK &&
             cr_remember_peer(peers, target, provider) &&
             cr_match_project_pair(staging, project, provider, target, &run, &result) == CR_RUN_OK;
        ok = cbm_store_read_scope_close(scope) == CBM_STORE_OK && ok;
        cbm_store_close(provider);
        if (ok) {
            result.projects_scanned++;
        }
    }
    result.no_targets = result.projects_scanned == 0;
    ok = ok && cr_record_unavailable_peers(peers, previous_peers, &run, &result) &&
         !cr_cancel_requested(&run) &&
         cr_record_targets(staging, project, (const char **)targets, target_count) &&
         cr_save_peers(staging, project, peers) && cr_set_reverse_pending(staging, project, true) &&
         cr_record_last_run(staging, project, (const char **)targets, target_count, &result);
    yyjson_mut_doc_free(peers);
    yyjson_doc_free(previous_peers);
    cbm_cross_repo_result_free(&result);
    cbm_store_close(staging);
    free_project_list(resolved, count);
    free_project_list(targets, target_count);
    return ok ? CBM_STORE_OK : CBM_STORE_ERR;
}

/* Repair only the provider's reverse references. The source generation is
 * already published and its outgoing links must survive a repair failure.
 * Typed matches restrict this pass to the provider the source selected. */
static bool cr_repair_peer(cbm_store_t *source, const char *project, cbm_store_t *provider,
                           const char *target) {
    cr_run_context_t run = {.staging_store = provider};
    if (cbm_store_begin(provider) != CBM_STORE_OK) {
        return false;
    }
    bool ok = cr_reconcile_reverse(provider, target, project, &run) == CR_RUN_OK &&
              match_http_routes(source, project, provider, target, &run).status == CR_RUN_OK &&
              match_channels(source, project, provider, target, &run).status == CR_RUN_OK &&
              match_typed_routes(source, project, provider, target, "GRPC_CALLS", "service",
                                 "method", "CROSS_GRPC_CALLS", true, &run)
                      .status == CR_RUN_OK &&
              match_typed_routes(source, project, provider, target, "GRAPHQL_CALLS", "operation",
                                 "operation", "CROSS_GRAPHQL_CALLS", true, &run)
                      .status == CR_RUN_OK &&
              match_typed_routes(source, project, provider, target, "TRPC_CALLS", "procedure",
                                 "procedure", "CROSS_TRPC_CALLS", true, &run)
                      .status == CR_RUN_OK;
    if (ok && cbm_store_commit(provider) == CBM_STORE_OK) {
        return true;
    }
    (void)cbm_store_rollback(provider);
    return false;
}

bool cbm_cross_repo_reconcile_published(const char *project) {
    cbm_store_t *source = cr_open_existing_project(project);
    if (!source) {
        return false;
    }
    if (!cr_reverse_pending(source, project)) {
        cbm_store_close(source);
        return true;
    }
    yyjson_doc *peers = cr_read_peers(source, project);
    yyjson_val *expected = yyjson_doc_get_root(peers);
    bool ok = yyjson_is_obj(expected);
    yyjson_obj_iter iter = yyjson_obj_iter_with(expected);
    yyjson_val *key;
    while ((key = yyjson_obj_iter_next(&iter))) {
        const char *target = yyjson_get_str(key);
        const char *observed = yyjson_get_str(yyjson_obj_iter_get_val(key));
        cbm_store_t *provider = observed ? cr_open_existing_project(target) : NULL;
        char current[CBM_GRAPH_COMPARE_GENERATION_SIZE];
        bool matched =
            provider && cbm_store_generation(provider, current, sizeof(current)) == CBM_STORE_OK &&
            strcmp(current, observed) == 0 && cr_repair_peer(source, project, provider, target);
        cbm_store_close(provider);
        ok = matched && ok;
    }
    yyjson_doc_free(peers);
    if (ok) {
        ok = cr_set_reverse_pending(source, project, false);
    }
    cbm_store_close(source);
    return ok;
}

/* ── Entry point ─────────────────────────────────────────────────── */

cbm_cross_repo_result_t cbm_cross_repo_match_cancellable(const char *project,
                                                         const char **target_projects,
                                                         int target_count,
                                                         const atomic_int *cancelled) {
    cbm_cross_repo_result_t result = {0};
    cr_run_context_t run = {.cancelled = cancelled};
    struct timespec t0;
    clock_gettime(CLOCK_MONOTONIC, &t0);

    if (!project || !cbm_validate_project_name(project) || !target_projects || target_count <= 0 ||
        (target_count == SKIP_ONE && !target_projects[0]) || !cr_project_exists(project)) {
        result.failed = true;
        return result;
    }
    if (cr_cancel_requested(&run)) {
        result.cancelled = true;
        return result;
    }

    /* Resolve target projects */
    char **resolved = NULL;
    int resolved_count = 0;

    if (target_count == SKIP_ONE && target_projects[0] && strcmp(target_projects[0], "*") == 0) {
        resolved_count = collect_all_projects(&resolved, &run, &result);
    } else {
        resolved_count = collect_named_projects(target_projects, target_count, &resolved, &run);
    }
    if (resolved_count < 0) {
        result.cancelled = cr_cancel_requested(&run);
        result.failed = !result.cancelled;
        return result;
    }
    int other_targets = 0;
    for (int i = 0; i < resolved_count; i++) {
        if (cr_cancel_requested(&run)) {
            result.cancelled = true;
            free_project_list(resolved, resolved_count);
            return result;
        }
        if (strcmp(resolved[i], project) == 0) {
            continue;
        }
        if (!cr_project_exists(resolved[i])) {
            result.failed = true;
            free_project_list(resolved, resolved_count);
            return result;
        }
        other_targets++;
    }
    /* Nothing but the source itself to match against (a self-only list, or
     * ["*"] in a store holding only the source). Reporting that as success
     * with projects_scanned:0 is indistinguishable from "these services share
     * no routes", and the source's existing CROSS_* edges would be wiped below
     * with nothing to rebuild them. Fail before any write. (#1133) */
    if (other_targets == 0) {
        result.failed = true;
        result.no_targets = true;
        free_project_list(resolved, resolved_count);
        return result;
    }

    /* Every input is known to exist before destructive source cleanup. The
     * read-write open itself also omits CREATE so a race cannot create ghosts. */
    cbm_store_t *src_store = cr_open_existing_project(project);
    if (!src_store) {
        result.failed = true;
        free_project_list(resolved, resolved_count);
        return result;
    }

    if (cr_cancel_requested(&run)) {
        result.cancelled = true;
        cbm_store_close(src_store);
        free_project_list(resolved, resolved_count);
        return result;
    }

    char **previous_targets = NULL;
    int previous_count = cr_read_targets(src_store, project, &previous_targets);
    bool saved_previous =
        previous_count == 0 ||
        (previous_count > 0 &&
         cr_record_targets(src_store, project, (const char **)previous_targets, previous_count));
    free_project_list(previous_targets, previous_count);
    if (!saved_previous) {
        result.failed = true;
        cbm_store_close(src_store);
        free_project_list(resolved, resolved_count);
        return result;
    }

    yyjson_mut_doc *peers = yyjson_mut_doc_new(NULL);
    yyjson_mut_val *observations = peers ? yyjson_mut_obj(peers) : NULL;
    if (!observations) {
        result.failed = true;
        yyjson_mut_doc_free(peers);
        cbm_store_close(src_store);
        free_project_list(resolved, resolved_count);
        return result;
    }
    yyjson_mut_doc_set_root(peers, observations);

    /* Clean existing CROSS_* edges for this project */
    cr_run_status_t cleanup_status = delete_cross_edges(src_store, project, &run);
    if (cleanup_status != CR_RUN_OK) {
        result.cancelled = cleanup_status == CR_RUN_CANCELLED;
        result.partial_results = result.cancelled && run.mutated;
        result.failed = cleanup_status == CR_RUN_FAILED;
        yyjson_mut_doc_free(peers);
        cbm_store_close(src_store);
        free_project_list(resolved, resolved_count);
        return result;
    }

    /* Match against each target */
    for (int i = 0; i < resolved_count; i++) {
        if (cr_cancel_requested(&run)) {
            result.cancelled = true;
            result.partial_results = run.mutated;
            break;
        }
        const char *tgt = resolved[i];
        if (strcmp(tgt, project) == 0) {
            continue; /* skip self */
        }

        /* Open target store read-write (for bidirectional edge writes) */
        cbm_store_t *tgt_store = cr_open_existing_project(tgt);
        if (!tgt_store) {
            result.failed = true;
            break;
        }

        cr_run_status_t match_status =
            cr_remember_peer(peers, tgt, tgt_store)
                ? cr_match_project_pair(src_store, project, tgt_store, tgt, &run, &result)
                : CR_RUN_FAILED;

        cbm_store_close(tgt_store);
        if (match_status == CR_RUN_FAILED) {
            result.failed = true;
            break;
        }
        if (match_status == CR_RUN_CANCELLED || cr_cancel_requested(&run)) {
            result.cancelled = true;
            result.partial_results = run.mutated;
            break;
        }
        result.projects_scanned++;
    }

    if (!result.failed && !result.cancelled &&
        (!cr_save_peers(src_store, project, peers) ||
         !cr_record_targets(src_store, project, target_projects, target_count) ||
         !cr_set_reverse_pending(src_store, project, false))) {
        result.failed = true;
    }
    (void)cr_record_last_run(src_store, project, target_projects, target_count, &result);
    yyjson_mut_doc_free(peers);
    cbm_store_close(src_store);

    free_project_list(resolved, resolved_count);

    struct timespec t1;
    clock_gettime(CLOCK_MONOTONIC, &t1);
    result.elapsed_ms = ((double)(t1.tv_sec - t0.tv_sec) * CR_MS_PER_SEC) +
                        ((double)(t1.tv_nsec - t0.tv_nsec) / CR_NS_PER_MS);

    if (result.cancelled) {
        cbm_log_info("cross_repo.cancelled", "project", project, "partial_results",
                     result.partial_results ? "true" : "false");
    } else {
        int total = result.http_edges + result.async_edges + result.channel_edges +
                    result.grpc_edges + result.graphql_edges + result.trpc_edges;
        cbm_log_info("cross_repo.done", "project", project, "total", cr_itoa(total));
    }

    return result;
}

cbm_cross_repo_result_t cbm_cross_repo_match(const char *project, const char **target_projects,
                                             int target_count) {
    return cbm_cross_repo_match_cancellable(project, target_projects, target_count, NULL);
}
