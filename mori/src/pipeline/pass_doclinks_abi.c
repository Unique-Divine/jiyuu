/* Adapter for the Rust REFERENCES_FILE pass. Graph and allocator ownership
 * stay with the native engine. */
#include "pipeline/pipeline.h"
#include "pipeline/pipeline_internal.h"
#include "graph_buffer/graph_buffer.h"
#include "foundation/compat_fs.h"
#include "foundation/constants.h"
#include "foundation/limits.h"
#include "foundation/log.h"
#include "foundation/mem_core.h"

#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>

typedef struct {
    int64_t id;
    const char *path;
} doclink_file_t;

typedef struct {
    uint8_t *bytes;
    size_t len;
} doclink_source_t;

typedef struct {
    doclink_file_t (*file_at)(void *data, int index);
    bool (*cancelled)(void *data);
    doclink_source_t (*read)(void *data, const char *path);
    void (*release)(uint8_t *bytes);
    bool (*insert_edge)(void *data, int64_t source, int64_t target, const char *properties);
} doclink_ops_t;

/* Internal staticlib entry point, also used by the native candidate tests. */
int mori_doc_file_links_run(void *data, const doclink_ops_t *ops, int count, bool have_repo_path);

enum {
    DOCLINK_NO_DOC_FILES = -1,
    DOCLINK_NO_REPO_PATH = -2,
};

typedef struct {
    cbm_pipeline_ctx_t *ctx;
    const cbm_gbuf_node_t **files;
} doclink_view_t;

static doclink_file_t file_at(void *data, int index) {
    const doclink_view_t *view = data;
    const cbm_gbuf_node_t *file = view->files[index];
    return (doclink_file_t){file->id, file->file_path};
}

static bool cancelled(void *data) {
    const doclink_view_t *view = data;
    return cbm_pipeline_check_cancel(view->ctx) != 0;
}

static doclink_source_t read_document(void *data, const char *path) {
    const doclink_view_t *view = data;
    cbm_pipeline_ctx_t *ctx = view->ctx;
    char abs_path[CBM_PATH_MAX];
    int n = snprintf(abs_path, sizeof(abs_path), "%s/%s", ctx->repo_path, path);
    if (n <= 0 || (size_t)n >= sizeof(abs_path)) {
        return (doclink_source_t){0};
    }

    FILE *f = cbm_fopen(abs_path, "rb");
    if (f) {
        (void)fseek(f, 0, SEEK_END);
        long size = ftell(f);
        (void)fseek(f, 0, SEEK_SET);
        if (size <= 0) {
            (void)fclose(f);
            return (doclink_source_t){0};
        }
        long cap = cbm_max_file_bytes();
        if (size > cap) {
            (void)fclose(f);
            char reason[96];
            (void)snprintf(reason, sizeof(reason), "oversized (%lld MB > %lld MB)",
                           (long long)(size / (CBM_SZ_1K * CBM_SZ_1K)),
                           (long long)(cap / (CBM_SZ_1K * CBM_SZ_1K)));
            cbm_pipeline_add_file_error(ctx->pipeline, path, reason, "oversized");
            cbm_log_warn("doclinks.file_oversized", "path", path);
            return (doclink_source_t){0};
        }
        uint8_t *buf = cbm_alloc(CBM_MEM_CLASS_EXTRACT, (size_t)size);
        if (buf) {
            size_t bytes = fread(buf, SKIP_ONE, (size_t)size, f);
            (void)fclose(f);
            return (doclink_source_t){buf, bytes};
        }
        (void)fclose(f);
    }

    cbm_pipeline_add_file_error(ctx->pipeline, path, "read failed", "read");
    cbm_log_warn("doclinks.file_unreadable", "path", path);
    return (doclink_source_t){0};
}

static void release_document(uint8_t *bytes) {
    cbm_free(CBM_MEM_CLASS_EXTRACT, bytes);
}

static bool insert_edge(void *data, int64_t source, int64_t target, const char *properties) {
    const doclink_view_t *view = data;
    return cbm_gbuf_insert_edge(view->ctx->gbuf, source, target, "REFERENCES_FILE", properties) > 0;
}

int cbm_pipeline_pass_doclinks(cbm_pipeline_ctx_t *ctx) {
    const cbm_gbuf_node_t **files = NULL;
    int count = 0;
    if (cbm_gbuf_find_by_label(ctx->gbuf, "File", &files, &count) != 0 || count == 0) {
        return 0;
    }

    static const doclink_ops_t ops = {file_at, cancelled, read_document, release_document,
                                      insert_edge};
    doclink_view_t view = {.ctx = ctx, .files = files};
    int edges = mori_doc_file_links_run(&view, &ops, count, ctx->repo_path != NULL);
    if (edges == DOCLINK_NO_DOC_FILES) {
        cbm_log_info("doclinks.skip", "reason", "no_doc_files");
        return 0;
    }
    if (edges == DOCLINK_NO_REPO_PATH) {
        cbm_log_info("doclinks.skip", "reason", "no_repo_path");
        return 0;
    }

    char count_buf[CBM_SZ_16];
    (void)snprintf(count_buf, sizeof(count_buf), "%d", edges);
    cbm_log_info("doclinks.strategy", "name", "markdown", "edges", count_buf);
    cbm_log_info("doclinks.done", "total", count_buf);
    return edges;
}
