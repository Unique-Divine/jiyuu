/* Native bindings for the Rust Ansible pass and the closure classifier. */
#include "foundation/constants.h"
#include "foundation/log.h"
#include "pipeline/pipeline_internal.h"
#include "cbm.h"
#include "lang_specs.h"

#include <stdio.h>

/* Matches Stats in src/pipeline/ansible/mod.rs. */
typedef struct {
    int imports;
    int tasks;
    int notifies;
    int deploys;
} ansible_stats_t;

extern bool cbm_rust_ansible_run(cbm_pipeline_ctx_t *ctx, cbm_gbuf_t *graph,
                                 const cbm_file_info_t *files, int count, const TSLanguage *yaml,
                                 bool (*cancelled)(cbm_pipeline_ctx_t *), ansible_stats_t *stats);
extern bool cbm_rust_ansible_is_sequence(const char *src);

static bool ansible_cancelled(cbm_pipeline_ctx_t *ctx) {
    return cbm_pipeline_check_cancel(ctx) != 0;
}

bool cbm_ansible_is_sequence(const char *src) {
    return cbm_rust_ansible_is_sequence(src);
}

void cbm_pipeline_pass_ansible(cbm_pipeline_ctx_t *ctx, const cbm_file_info_t *files,
                               int file_count) {
    ansible_stats_t stats = {0};
    if (!cbm_rust_ansible_run(ctx, ctx->gbuf, files, file_count, cbm_ts_language(CBM_LANG_YAML),
                              ansible_cancelled, &stats) ||
        (stats.imports == 0 && stats.tasks == 0)) {
        return;
    }
    char imports[CBM_SZ_32], tasks[CBM_SZ_32], notifies[CBM_SZ_32], deploys[CBM_SZ_32];
    snprintf(imports, sizeof(imports), "%d", stats.imports);
    snprintf(tasks, sizeof(tasks), "%d", stats.tasks);
    snprintf(notifies, sizeof(notifies), "%d", stats.notifies);
    snprintf(deploys, sizeof(deploys), "%d", stats.deploys);
    cbm_log_info("pass.ansible", "imports", imports, "tasks", tasks, "notifies", notifies,
                 "deploys", deploys);
}
