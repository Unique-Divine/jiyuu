/* The Rust similarity pass reads fingerprints and returns SIMILAR_TO edges. */
#include "pipeline/pipeline_internal.h"
#include "pipeline/worker_pool.h"
#include "foundation/log.h"
#include "foundation/platform.h"
#include "graph_buffer/graph_buffer.h"
#include "simhash/minhash.h"

#define XXH_INLINE_ALL
#include "xxhash/xxhash.h"

#include <stdio.h>

typedef void (*similarity_node_fn)(void *, int64_t, const char *, const char *, const char *);
extern int mori_similarity_buffer(cbm_gbuf_t *graph, int workers);

void cbm_similarity_read_buffer(const cbm_gbuf_t *graph, void *data, similarity_node_fn read) {
    static const char *const labels[] = {"Function", "Method"};
    for (size_t label = 0; label < sizeof(labels) / sizeof(labels[0]); label++) {
        const cbm_gbuf_node_t **nodes = NULL;
        int count = 0;
        cbm_gbuf_find_by_label(graph, labels[label], &nodes, &count);
        for (int i = 0; i < count; i++) {
            const cbm_gbuf_node_t *node = nodes[i];
            if (node->properties_json) {
                read(data, node->id, node->qualified_name, node->file_path, node->properties_json);
            }
        }
    }
}

void cbm_similarity_hash_bands(const uint32_t fingerprint[CBM_MINHASH_K],
                               uint16_t bands[CBM_LSH_BANDS]) {
    for (int band = 0; band < CBM_LSH_BANDS; band++) {
        const uint32_t *rows = &fingerprint[band * CBM_LSH_ROWS];
        bands[band] = (uint16_t)XXH3_64bits(rows, CBM_LSH_ROWS * sizeof(*rows));
    }
}

void cbm_similarity_run_workers(int count, cbm_parallel_fn run, void *data) {
    cbm_parallel_for_opts_t opts = {.max_workers = count, .force_pthreads = false};
    cbm_parallel_for(count, run, data, opts);
}

int cbm_pipeline_pass_similarity(cbm_pipeline_ctx_t *ctx) {
    cbm_log_info("pass.start", "pass", "similarity");
    int count = mori_similarity_buffer(ctx->gbuf, cbm_default_worker_count(false));
    char edges[CBM_SZ_32];
    snprintf(edges, sizeof(edges), "%d", count);
    cbm_log_info("pass.done", "pass", "similarity", "edges", edges, "implementation", "rust");
    return 0;
}
