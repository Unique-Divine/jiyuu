#ifndef CBM_MCP_INTERNAL_H
#define CBM_MCP_INTERNAL_H

#include "foundation/index_policy.h"
#include "mcp/mcp.h"
#include "mcp/compact_out.h"
#include "pipeline/pipeline.h" /* cbm_changed_hunk_t */
#include "store/store.h"       /* cbm_node_t */

#include <yyjson/yyjson.h>

/* The Rust request opens the native directory, pulls one owned row at a
 * time, and hydrates only selected rows before releasing them. A failed
 * open returns an owned MCP error through error. */
typedef struct cbm_mcp_project_catalog cbm_mcp_project_catalog_t;
typedef enum {
    CBM_MCP_CATALOG_END = 0,
    CBM_MCP_CATALOG_RECORD = 1,
    CBM_MCP_CATALOG_FAILED = 2,
    CBM_MCP_CATALOG_CANCELLED = 3,
} cbm_mcp_project_catalog_result_t;

cbm_mcp_project_catalog_t *cbm_mcp_project_catalog_open(cbm_mcp_server_t *server, char **error);
cbm_mcp_project_catalog_result_t cbm_mcp_project_catalog_next(
    cbm_mcp_project_catalog_t *catalog, cbm_mcp_project_record_t *record);
cbm_mcp_project_catalog_result_t cbm_mcp_project_catalog_hydrate(
    cbm_mcp_project_catalog_t *catalog, cbm_mcp_project_record_t *record, bool include_stats);
void cbm_mcp_project_catalog_release_record(cbm_mcp_project_record_t *record);
void cbm_mcp_project_catalog_close(cbm_mcp_project_catalog_t *catalog);

char *cbm_rust_mcp_list_projects(cbm_mcp_server_t *server, const char *args_json);

typedef struct {
    const char *fts_query;
    const char *project;
    const char *file_like;
    const char *label;
    const char *kind;
    const char *ns;
    double label_weight;
} cbm_rank_search_filters_t;
typedef struct {
    const cbm_rank_search_filters_t *filters;
    const char *exact_query;
    int limit;
    int offset;
    int candidate_limit;
} cbm_rank_search_request_t;
typedef void (*cbm_rank_search_row_fn)(void *context, struct sqlite3_stmt *row);
/* False means FTS is unavailable and the caller can use the pattern path. */
bool mori_rank_search(struct sqlite3 *db, const cbm_rank_search_request_t *request, int *total,
                      bool *window_saturated, void *context, cbm_rank_search_row_fn emit);

/* The trace pager borrows the request strings and reads the filtered legs
 * through a row accessor. Rows are in (hop, node ID) order. */
typedef struct {
    const char *project;
    const char *function_name;
    const char *direction;
    const char *mode;
    const char *parameter_name;
    int depth;
    int limit;
    bool include_tests;
    bool risk_labels;
    bool include_evidence;
    bool include_overrides;
} cbm_trace_params_t;

typedef struct {
    uint64_t hash;
    int64_t node_id;
    int hop;
    char leg;
} cbm_trace_pagination_t;

typedef struct {
    const void *rows;
    int count;
} cbm_trace_leg_t;

typedef struct {
    int outbound;
    int inbound;
} cbm_trace_page_start_t;

typedef struct {
    int outbound;
    int inbound;
    bool has_more;
    char next[192];
} cbm_trace_page_t;

typedef struct {
    int64_t node_id;
    int hop;
} cbm_trace_watermark_t;

typedef const char *(*cbm_trace_next_edge_fn)(void *context);
typedef cbm_trace_watermark_t (*cbm_trace_read_row_fn)(const void *rows, int index);

const char *mori_trace_cursor(const cbm_trace_params_t *params, const char *generation,
                              const char *token, cbm_trace_next_edge_fn next_edge, void *edges,
                              cbm_trace_pagination_t *state);
const char *mori_trace_page_start(const cbm_trace_pagination_t *state, const cbm_trace_leg_t *out,
                                  const cbm_trace_leg_t *in, cbm_trace_read_row_fn read,
                                  cbm_trace_page_start_t *start);
void mori_trace_page(const cbm_trace_pagination_t *state, const char *generation,
                     const cbm_trace_leg_t *out, const cbm_trace_leg_t *in,
                     cbm_trace_read_row_fn read, const cbm_trace_page_start_t *start, int budget,
                     cbm_trace_page_t *page);

/* Snippet inputs are borrowed until Rust returns a C-owned MCP result. */
typedef struct {
    const cbm_node_t *node;
    const cbm_mcp_snippet_window_t *window;
    cbm_store_t *store;
    const char *display_path;
    const char *generation;
    const char *source;
    const char *freshness;
    const char *match_method;
    const cbm_node_t *members;
    const cbm_coverage_row_t *coverage;
    char **caller_names;
    char **callee_names;
    int member_count;
    int coverage_count;
    int caller_count;
    int callee_count;
    int member_offset;
    int member_limit;
    int callers;
    int callees;
    bool content_verified;
    bool source_drift;
    bool json_format;
} cbm_mcp_snippet_input_t;

typedef struct {
    const cbm_node_t *node;
    double probability;
    const char *kind;
    const char *position;
    int shared_terms;
    int name_tokens;
    bool has_shared_terms;
    bool has_name_tokens;
} cbm_mcp_snippet_candidate_t;

typedef void (*cbm_mcp_snippet_candidate_sink_fn)(void *context,
                                                  const cbm_mcp_snippet_candidate_t *candidate);
void cbm_mcp_snippet_visit_candidates(cbm_store_t *store, const cbm_node_t *node, bool by_section,
                                      cbm_mcp_snippet_candidate_sink_fn emit, void *context);
void cbm_mcp_snippet_free_tree(char *tree);
char *mori_snippet_response(const cbm_mcp_snippet_input_t *input);
char *mori_snippet_suggestions(const char *input, const cbm_node_t *nodes, int count);

/* Compact table rendering retains the C emitter's escaping and column profiles. */
void cbm_mcp_render_project_table(cbm_sb_t *sb, const cbm_mcp_project_record_t *records,
                                   int page_count, bool metadata_only, bool include_stats);

/* White-box fault injection for deterministic cross-platform quarantine
 * safety tests. This header is internal and is not part of the MCP API. */
typedef bool (*cbm_mcp_quarantine_test_hook_fn)(void *context, const char *step);
typedef bool (*cbm_mcp_command_test_hook_fn)(void *context, const char *command);
#ifdef CBM_ENABLE_TEST_SEAMS
/* One-shot sample JSON string-copy allocation failure, not a backing-pool
 * growth event: 0 is next, 4 is fifth, -1 disables. Setting clears consumption. */
void cbm_mcp_doc_links_test_fail_sample_alloc_after(int successful_copies);
bool cbm_mcp_doc_links_test_sample_alloc_failed(void);

typedef void (*cbm_mcp_auto_index_count_test_hook_fn)(void *context);
#endif

void cbm_mcp_server_set_quarantine_test_hook(cbm_mcp_server_t *srv,
                                             cbm_mcp_quarantine_test_hook_fn hook, void *context);
void cbm_mcp_server_set_command_test_hook(cbm_mcp_server_t *srv, cbm_mcp_command_test_hook_fn hook,
                                          void *context);
void cbm_mcp_server_set_search_output_limit_for_test(cbm_mcp_server_t *srv, size_t limit);
#ifdef CBM_ENABLE_TEST_SEAMS
void cbm_mcp_server_set_auto_index_count_test_hook(cbm_mcp_server_t *srv,
                                                   cbm_mcp_auto_index_count_test_hook_fn hook,
                                                   void *context);
#endif
void cbm_mcp_server_set_search_scan_command_for_test(cbm_mcp_server_t *srv, const char *command);
void cbm_mcp_server_set_search_scan_timeout_for_test(cbm_mcp_server_t *srv, uint64_t timeout_ms,
                                                     bool override_set);
/* Parent directory for search_code's private cbm-search-XXXXXX scratch
 * (NULL restores cbm_tmpdir()). Tests point it at a private directory so a
 * scratch count cannot see other processes' entries in the shared temp dir.
 * The server borrows dir until it is reset or the server is freed. */
void cbm_mcp_server_set_search_scratch_dir_for_test(cbm_mcp_server_t *srv, const char *dir);
/* search_code's in-process scan of `filelist_path` (records separated as the
 * scoped list is written: NUL on POSIX, newline on Windows) into `out_path`.
 * Returns the scan cause in mcp_scan_cause_t order: 0 success, 1 command
 * failure, 3 output limit, 4 deadline, 5 cancelled. */
int cbm_mcp_search_scan_for_test(cbm_mcp_server_t *srv, const char *filelist_path,
                                 const char *pattern, bool use_regex, const char *out_path,
                                 size_t output_limit, uint64_t deadline_ms);

/* Release only the constructor-created pristine in-memory store. Public
 * cbm_mcp_server_new(NULL) semantics remain unchanged; daemon sessions use
 * this immediately before publication so idle sessions retain no SQLite DB. */
bool cbm_mcp_server_release_pristine_memory_store(cbm_mcp_server_t *srv);

/* Prepend one daemon-owned notice to a successful JSON-RPC tool response.
 * On success replaces and frees *response_io; on failure it is unchanged. */
bool cbm_mcp_jsonrpc_response_prepend_notice(char **response_io, const char *notice);

/* Encode the complete trusted policy on an internal worker request. Callers
 * must remove any untrusted field with the same name before invoking this. */
bool cbm_mcp_index_policy_add_to_args(yyjson_mut_doc *doc, yyjson_mut_val *root,
                                      const cbm_index_resource_policy_t *policy);
bool cbm_mcp_index_policy_from_internal_args(const char *args, cbm_index_resource_policy_t *policy,
                                             char *error, size_t error_size);

/* Request-scoped facts a worker resource failure is reported against. One
 * supervised request can run several worker attempts (crash/hang recovery), and
 * serving_index_preserved describes the request, not only its last attempt:
 * it is true only when a servable index served the project when the request
 * began AND no attempt of the request committed to replacing it (the
 * publication fence, index_supervisor.h) AND that index is still servable. */
typedef struct {
    bool serving_index_existed;
    bool publication_committed;
} cbm_mcp_index_request_facts_t;

/* Capture the facts at the start of a request, before its first worker. */
void cbm_mcp_index_request_facts_begin(const char *args, cbm_mcp_index_request_facts_t *facts);
/* Fold one finished attempt into the request facts. */
void cbm_mcp_index_request_facts_record(cbm_mcp_index_request_facts_t *facts,
                                        const cbm_index_worker_result_t *worker_result);
/* NULL facts claim nothing: serving_index_preserved is then false. */
char *cbm_mcp_index_worker_resource_response(const char *args,
                                             const cbm_index_worker_result_t *worker_result,
                                             const cbm_mcp_index_request_facts_t *facts);

/* Encode a doc_candidates setting the user set to a boolean on an internal
 * worker request (the worker reads no config). Unset or unparseable adds no
 * key, so the worker's index mode decides. False only when encoding fails. */
bool cbm_mcp_index_doc_candidates_add_to_args(yyjson_mut_doc *doc, yyjson_mut_val *root,
                                              struct cbm_config *config);

enum { CBM_MCP_DEFAULT_AUTO_INDEX_LIMIT = 50000 };

/* Count indexable files with the pipeline's native full-mode discovery policy,
 * without retaining per-file results. A false result means the count exceeded
 * file_limit or could not be established before the bounded deadline; every
 * such failure is fail-closed because this is the memory-admission guard. */
/* Map an internal resolver strategy (as recorded on a CALLS edge by
 * pass_calls.c) to the CLOSED public class published by trace_path's
 * include_evidence output: "lsp" | "language_rule" | "heuristic" |
 * "unresolved". NULL only for a NULL/empty strategy.
 *
 * Exposed so tests/test_mcp.c can pin every strategy production can emit to a
 * known class — a new resolver KIND must fail there rather than leaking an
 * unmapped internal name into a user-visible field. */
const char *cbm_mcp_edge_strategy_class(const char *strategy);

bool cbm_mcp_auto_index_within_file_limit(const char *root_path, int file_limit,
                                          int *file_count_out);

/* True when the `ignore_worktrees` config key is on for this server (default
 * off). Callers pair it with cbm_git_is_linked_worktree() to decide whether an
 * automatic index of a linked worktree should be skipped. */
bool cbm_mcp_ignore_worktrees_enabled(const cbm_mcp_server_t *srv);

/* The same key when srv may have no store (the lifecycle hook). Never attaches
 * a store to srv and never creates the runtime store; reads it only when it
 * already exists. */
bool cbm_mcp_ignore_worktrees_runtime_enabled(const cbm_mcp_server_t *srv);

/* detect_changes seed scoping (#1363): does `node`'s line range overlap any
 * recorded hunk for `file`? Exposed for direct unit testing of the overlap
 * logic, independent of the git/subprocess/index plumbing around it. */
bool cbm_detect_node_in_hunks(const cbm_node_t *node, const cbm_changed_hunk_t *hunks,
                              int hunk_count, const char *file);

/* search_code Windows pre-scan optimization: only simple suffix globs can be
 * moved ahead of
 * Select-String without changing the existing full-path
 * PowerShell -like contract. Exposed for
 * direct boundary tests only. */
bool cbm_search_code_file_pattern_can_prefilter(const char *file_pattern);
bool cbm_search_code_windows_path_matches_prefilter(const char *path, const char *file_pattern);

/* Internal command builder exposed so tests can pin the PowerShell pipeline
 * ordering without
 * starting an external shell. */
void cbm_search_code_build_grep_cmd(char *cmd, size_t cmd_sz, bool use_regex, bool scoped,
                                    const char *file_pattern, const char *tmpfile,
                                    const char *filelist, const char *root_path);
#ifdef CBM_ENABLE_TEST_SEAMS
/* Reject opening one changed file while detect_changes fingerprints its live
 * snapshot. This makes otherwise platform-specific permission/read races
 * deterministic without changing production filesystem behavior. */
typedef bool (*cbm_mcp_snapshot_read_test_hook_fn)(void *context, const char *absolute_path);
void cbm_mcp_server_set_snapshot_read_test_hook(cbm_mcp_server_t *srv,
                                                cbm_mcp_snapshot_read_test_hook_fn hook,
                                                void *context);

/* Runs after get_code_snippet has read and verified a file's bytes and before
 * it slices them, so a test can write the file at exactly that point (#2585). */
typedef void (*cbm_mcp_snippet_capture_test_hook_fn)(void *context, const char *rel_path);
void cbm_mcp_server_set_snippet_capture_test_hook(cbm_mcp_server_t *srv,
                                                  cbm_mcp_snippet_capture_test_hook_fn hook,
                                                  void *context);

/* Runs before a cache-directory scan opens each project database, so a test
 * can cancel the request at an exact point of the scan (#2796). */
typedef void (*cbm_mcp_project_scan_test_hook_fn)(void *context, const char *db_file);
void cbm_mcp_server_set_project_scan_test_hook(cbm_mcp_server_t *srv,
                                               cbm_mcp_project_scan_test_hook_fn hook,
                                               void *context);

/* Filesystem PATH_MAX prevents a portable end-to-end fixture for multi-KiB
 * stored paths. This seam drives the normal search result ownership,
 * directory aggregation, and renderer pipeline with synthetic identities. */
char *cbm_mcp_render_search_rows_for_testing(const char *const *qualified_names,
                                             const char *const *file_paths, int row_count,
                                             bool json_format);

/* Drive the production raw-source slicer and renderer without relying on an
 * external grep/PowerShell text decoder. This keeps malformed-byte paging
 * deterministic on every test platform. */
char *cbm_mcp_render_raw_preview_for_testing(const char *content, bool content_offset_set,
                                             size_t content_offset, bool json_format);

/* Exercise search_graph's semantic continuation metadata without allocating
 * a 100k-row vector fixture. The production emitters remain the code under
 * test; this seam only supplies their already-ranked page descriptor. */
char *cbm_mcp_render_semantic_paging_for_testing(int total, int offset, int returned, int limit,
                                                 bool total_exact, bool json_format);
#endif

#endif
