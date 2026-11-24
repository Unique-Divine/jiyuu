/* Rust owns the complexity pass. Keep C access to the graph and store in
 * this bridge until the Rust runner supplies a generation view and writer. */
#include "pipeline/pipeline_internal.h"
#include "graph_buffer/graph_buffer.h"
#include "foundation/log.h"
#include "store/store.h"
#include "sqlite3.h"

#include <inttypes.h>
#include <stdio.h>

typedef void (*complexity_node_fn)(void *, int64_t, const char *, const char *, int, const char *,
                                   bool);
typedef bool (*complexity_call_fn)(void *, int64_t, int64_t);

extern int64_t mori_complexity_buffer(cbm_gbuf_t *gb);
extern int64_t mori_complexity_stage(sqlite3 *db, const char *project);

static void emit_node(void *data, complexity_node_fn node_fn, const cbm_gbuf_node_t *node,
                      bool seed) {
    node_fn(data, node->id, node->qualified_name, node->file_path, node->start_line,
            seed ? node->properties_json : NULL, seed);
}

void cbm_complexity_read_buffer(const cbm_gbuf_t *gb, void *data, complexity_node_fn node_fn,
                                complexity_call_fn call_fn) {
    int64_t next_id = cbm_gbuf_next_id(gb);
    static const char *const labels[] = {"Function", "Method"};
    for (size_t l = 0; l < sizeof(labels) / sizeof(labels[0]); l++) {
        const cbm_gbuf_node_t **nodes = NULL;
        int count = 0;
        if (cbm_gbuf_find_by_label(gb, labels[l], &nodes, &count) != 0) {
            continue;
        }
        for (int i = 0; i < count; i++) {
            if (nodes[i]->id >= 1 && nodes[i]->id < next_id) {
                emit_node(data, node_fn, nodes[i], true);
            }
        }
    }
    const cbm_gbuf_edge_t **edges = NULL;
    int count = 0;
    if (cbm_gbuf_find_edges_by_type(gb, "CALLS", &edges, &count) != 0) {
        return;
    }
    for (int i = 0; i < count; i++) {
        int64_t source = edges[i]->source_id;
        int64_t target = edges[i]->target_id;
        if (source < 1 || source >= next_id || target < 1 || target >= next_id) {
            continue;
        }
        if (call_fn(data, source, target)) {
            const cbm_gbuf_node_t *node = cbm_gbuf_find_by_id(gb, target);
            if (node) {
                emit_node(data, node_fn, node, false);
            }
        }
    }
}

int cbm_complexity_write_node(cbm_gbuf_t *gb, int64_t id, const char *properties) {
    cbm_gbuf_node_t *node = (cbm_gbuf_node_t *)cbm_gbuf_find_by_id(gb, id);
    return node ? cbm_gbuf_node_set_properties_json(node, properties) : -1;
}

static void log_result(const char *event, int64_t count) {
    if (count < 0) {
        cbm_log_error(event, "implementation", "rust", "error", "complexity_update_failed");
        return;
    }
    char val[32];
    snprintf(val, sizeof(val), "%" PRId64, count);
    cbm_log_info(event, "functions", val, "implementation", "rust");
}

void cbm_pipeline_pass_complexity(cbm_pipeline_ctx_t *ctx) {
    log_result("pass.complexity", mori_complexity_buffer(ctx->gbuf));
}

int cbm_pipeline_complexity_recompute_store(cbm_store_t *store, const char *project) {
    sqlite3 *db = store && project ? cbm_store_get_db(store) : NULL;
    if (!db) {
        return -1;
    }
    int64_t count = mori_complexity_stage(db, project);
    log_result("pass.complexity_store", count);
    return count < 0 ? -1 : 0;
}
