/* Native objects whose representation is private to the pipeline or allocator. */
#include "pipeline/doc_links.h"

#include "doclink.h"
#include "foundation/arena.h"
#include "foundation/mem_core.h"

/* Keep the Rust vtable and the numeric values read from native records in
 * sync with their C declarations. */
_Static_assert(sizeof(CBMLanguage) == sizeof(int) && CBM_DOCLINK_RESOLVER_LANGS == 6 &&
                   CBM_LANG_JAVA == 6 && CBM_LANG_KOTLIN == 12,
               "JVM resolver language ABI");
_Static_assert(CBM_DOCLINK_FLAG_FILE == 1 && CBM_DOCLINK_JAVA_VALUE == 10 &&
                   CBM_DOCLINK_KT_KDOC_LINK == 11 && CBM_DOCLINK_KT_SAMPLE == 14,
               "JVM reference ABI");
_Static_assert(sizeof(cbm_doclink_kind_t) == sizeof(int) && CBM_DOCLINK_EDGE == 0 &&
                   CBM_DOCLINK_UNRESOLVED == 1 && CBM_DOCLINK_LOCAL == 2 &&
                   CBM_DOCLINK_REASON_MISSING == 0 && CBM_DOCLINK_REASON_AMBIGUOUS == 1 &&
                   CBM_DOCLINK_REASON_EXTERNAL == 2 && CBM_DOCLINK_REASON_TEST_ONLY == 3 &&
                   CBM_DOCLINK_REASON_NOT_INDEXED == 4 && CBM_DOCLINK_REASON_GRAPH_GAP == 5 &&
                   CBM_DOCLINK_REASON_UNPARSEABLE == 6,
               "JVM resolution outcome ABI");
_Static_assert(CBM_DOCLINK_DELTA_LOCAL == 0 && CBM_DOCLINK_DELTA_GLOBAL == 1,
               "JVM incremental delta ABI");

const char *mori_jvm_doclinks_project(const cbm_pipeline_ctx_t *ctx) {
    return ctx->project_name;
}

int mori_jvm_doclinks_mode(const cbm_pipeline_ctx_t *ctx) {
    return ctx->mode;
}

CBMArena *mori_jvm_doclinks_arena_new(void) {
    CBMArena *arena = cbm_alloc(CBM_MEM_CLASS_OTHER, sizeof(*arena));
    if (arena) {
        cbm_arena_init(arena);
    }
    return arena;
}

void mori_jvm_doclinks_arena_drop(CBMArena *arena) {
    if (arena) {
        cbm_arena_destroy(arena);
        cbm_free(CBM_MEM_CLASS_OTHER, arena);
    }
}
