#ifndef CBM_MCP_DETECT_CHANGES_H
#define CBM_MCP_DETECT_CHANGES_H

#include "pipeline/pipeline.h"
#include "store/store_impact.h"
#include "mcp/test_impact.h"

enum { DETECT_MODCAP = 256 };

typedef struct {
    const char *qn;
    const char *label;
    const char *file;
    const char *reason;
    int start;
    int end;
} detect_seed_row_t;

typedef struct {
    const char *name;
    int count;
} detect_module_row_t;

typedef struct {
    cbm_store_t *store;
    const char *project;
    const char *const *files;
    int file_count;
    const cbm_changed_hunk_t *hunks;
    int hunk_count;
    const cbm_test_config_t *product_config; /* NULL for unfiltered seeds */
    bool list_seeds;
    bool want_symbols;
    const char *direction;
    int depth;
    int bfs_limit;
    const cbm_impact_policy_t *policy; /* NULL for bounded default BFS */
    bool via;
} detect_graph_request_t;

typedef struct {
    const detect_seed_row_t *seeds;
    int seed_total;
    int seed_count; /* includes seeds when list_seeds is false */
    const cbm_node_hop_t *impacted;
    int impacted_count;
    bool saturated;
    const cbm_impact_walk_t *via_walk;
    const detect_module_row_t *modules;
    int module_count;
    int module_overflow;
    const char *error;
} detect_graph_view_t;

typedef struct detect_graph detect_graph_t;

/* Reads the request's existing store. The view borrows the returned owner
 * until free, which must occur before releasing that store. */
detect_graph_t *cbm_rust_detect_graph(const detect_graph_request_t *request,
                                    detect_graph_view_t *view);
void cbm_rust_detect_graph_free(detect_graph_t *graph);

/* Match the native property reader's yyjson type and string rules. */
bool cbm_detect_seed_is_test(const cbm_node_t *node);

#endif
