/*
 * pass_cross_repo.h — Cross-repo intelligence: match Routes, Channels, and
 * async topics across indexed projects to create CROSS_* edges.
 */
#ifndef CBM_PASS_CROSS_REPO_H
#define CBM_PASS_CROSS_REPO_H

#include "store/store.h"

#include <stdatomic.h>

typedef enum {
    CBM_CROSS_ROUTE_KEEP,
    CBM_CROSS_ROUTE_DROP,
    CBM_CROSS_ROUTE_LINK_AMBIGUOUS,
    CBM_CROSS_ROUTE_LINK_RESOLVED,
} cbm_cross_route_action_t;

typedef cbm_cross_route_action_t (*cbm_graphql_link_policy_fn)(bool provider_side, bool has_handler,
                                                               bool ambiguous, bool names_other,
                                                               bool existing_ambiguous);

/* Return -1 on SQLite error, 0 if the row is absent, and 1 after a delete.
 * The caller owns the SQLite connection and its transaction. */
typedef int (*cbm_graphql_delete_link_fn)(struct sqlite3 *db, int64_t caller_id, int64_t route_id);

/* Bind, run and reset the borrowed graph-store upsert statement. */
typedef bool (*cbm_graphql_upsert_link_fn)(struct sqlite3_stmt *stmt, const char *project,
                                           int64_t source_id, int64_t target_id,
                                           const char *properties);

typedef struct {
    int64_t handler_id; /* zero if the target has no matching handler */
    bool ambiguous;
    bool failed;
} cbm_graphql_handler_t;

typedef cbm_graphql_handler_t (*cbm_graphql_find_handler_fn)(struct sqlite3 *db,
                                                             const char *route_qn,
                                                             char *handler_name, size_t name_size,
                                                             char *handler_file, size_t file_size);

/* Register GraphQL link callbacks before starting the C runtime. */
void cbm_cross_repo_set_graphql_backend(cbm_graphql_link_policy_fn policy,
                                        cbm_graphql_delete_link_fn delete_link,
                                        cbm_graphql_upsert_link_fn upsert_link,
                                        cbm_graphql_find_handler_fn find_handler);

/* A store the ["*"] enumeration skipped instead of linking. */
typedef struct {
    char *project;      /* heap-owned */
    const char *reason; /* static, e.g. "pre_768_schema" (reindex required) */
} cbm_cross_repo_skip_t;

/* Result of a cross-repo matching run. */
typedef struct {
    int http_edges;    /* CROSS_HTTP_CALLS edges created */
    int async_edges;   /* CROSS_ASYNC_CALLS edges created */
    int channel_edges; /* CROSS_CHANNEL edges created */
    int grpc_edges;    /* CROSS_GRPC_CALLS edges created */
    int graphql_edges; /* CROSS_GRAPHQL_CALLS edges created */
    int trpc_edges;    /* CROSS_TRPC_CALLS edges created */
    int projects_scanned;
    double elapsed_ms;
    bool failed;          /* source/target validation, open, or allocation failed */
    bool no_targets;      /* failed: nothing but the source project resolved (#1133) */
    bool cancelled;       /* stopped at a bounded cancellation checkpoint */
    bool partial_results; /* committed writes before cancellation were retained */
    cbm_cross_repo_skip_t *skipped_projects; /* ["*"] only; free with ..._result_free */
    int skipped_count;
} cbm_cross_repo_result_t;

/* Release the heap-owned parts of a result (safe on a zeroed result). */
void cbm_cross_repo_result_free(cbm_cross_repo_result_t *result);

struct yyjson_mut_doc;
struct yyjson_mut_val;

/* Add a "cross_repo" object to `parent` describing the last cross-repo run
 * with `project` as the source, as recorded in that project's store:
 *   {"status":"never_run"}  -- no run recorded since the store was (re)built
 *   {"status":"ran","last_run_at","outcome","targets","projects_scanned",
 *    "total_cross_edges","skipped_projects"}
 *   {"status":"stale", ...} -- a recorded provider generation changed
 *   "reverse_pending": true -- a staged publication still needs reverse repair
 * Read-only (works on a query store). Returns false only on invalid input or
 * allocation failure. */
bool cbm_cross_repo_add_status_json(struct yyjson_mut_doc *doc, struct yyjson_mut_val *parent,
                                    cbm_store_t *store, const char *project);

/* Restore the last successful selection into a private staging graph. Only
 * the staged store is writable; peer graphs are used as matching inputs. */
int cbm_cross_repo_relink_stage(const char *live_path, const char *stage_path, const char *project,
                                const atomic_int *cancelled);

/* Read the current indexed generation into a CBM_GRAPH_COMPARE_GENERATION_SIZE
 * buffer. A missing, busy, or unreadable graph returns false. */
bool cbm_cross_repo_read_generation(const char *project, char *generation, size_t capacity);

typedef void (*cbm_cross_repo_client_visitor_t)(void *context, const char *client,
                                                const char *const *targets, int target_count);

/* Visit clients with an explicit saved target set that includes this provider
 * and records an unknown or older provider generation. The visitor borrows
 * each client name and its complete targets for the duration of the call.
 * Selection is advisory. Dispatch must reread the graph under its write lease. */
void cbm_cross_repo_visit_stale_clients(const char *provider, const char *generation,
                                        cbm_cross_repo_client_visitor_t visitor, void *context,
                                        const atomic_int *cancelled);

/* Check whether this published generation still needs peer-side writes. */
bool cbm_cross_repo_reverse_pending(const char *project);

/* Complete peer-side writes after publication. Caller must hold the wildcard
 * project mutation lease. A cancellation rolls back the current peer write
 * and leaves the repair pending. */
bool cbm_cross_repo_reconcile_published(const char *project, const atomic_int *cancelled);

/* Run cross-repo matching for `project` against `target_projects`.
 * If target_count == 1 and target_projects[0] == "*", matches against all
 * indexed projects. Writes CROSS_* edges bidirectionally into both the
 * source and target project DBs.
 *
 * `project` must already be indexed (its .db must exist).
 * Returns result with edge counts. */
cbm_cross_repo_result_t cbm_cross_repo_match(const char *project, const char **target_projects,
                                             int target_count);

/* Cancellation-aware form used by session-owned frontends. The caller-owned
 * flag must outlive the call. Cancellation is not a multi-database rollback:
 * already committed target writes remain and are reported as partial results. */
cbm_cross_repo_result_t cbm_cross_repo_match_cancellable(const char *project,
                                                         const char **target_projects,
                                                         int target_count,
                                                         const atomic_int *cancelled);

#endif /* CBM_PASS_CROSS_REPO_H */
