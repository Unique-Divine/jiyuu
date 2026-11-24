/* The standalone C index decision drivers and the default linked fallback. */
#include "pipeline/pipeline_internal.h"

int cbm_pipeline_full_build_reference(cbm_full_build_t *run, const cbm_full_build_ops_t *ops,
                                      int file_count, int parallel_threshold) {
    int rc = ops->structure(run);
    if (rc != 0) {
        return rc;
    }
    int workers = ops->workers(run);
    rc = ops->extract(run, workers, workers > 1 && file_count > parallel_threshold);
    if (rc != 0) {
        return rc;
    }
    rc = ops->tests(run);
    if (rc != 0) {
        return rc;
    }
    rc = ops->history(run);
    if (rc != 0) {
        return rc;
    }
    ops->predump(run);
    rc = ops->fence(run);
    return rc != 0 ? rc : ops->stage_write(run);
}

int cbm_pipeline_source_publish_reference(cbm_source_publish_t *run,
                                          const cbm_source_publish_ops_t *ops,
                                          int rc, bool live_noop, bool final_existed) {
    if (rc != 0) {
        return rc;
    }
    if (ops->cancelled(run)) {
        return ops->abort_preserve_db;
    }
    if (live_noop) {
        return ops->finish_noop(run);
    }
    if (final_existed && !ops->relink(run)) {
        return ops->cancelled(run) ? ops->abort_preserve_db : ops->persist_failed;
    }
    if (!ops->seal(run)) {
        return ops->persist_failed;
    }
    ops->before_publish(run);
    if (ops->cancelled(run)) {
        return ops->abort_preserve_db;
    }
    /* A hook can reopen SQLite in WAL mode. Install a standalone main file. */
    if (!ops->seal(run)) {
        return ops->persist_failed;
    }
    if (!ops->source_matches(run) || !ops->claim_lease(run)) {
        return ops->abort_preserve_db;
    }
    return ops->commit(run);
}

bool cbm_pipeline_stage_write_reference(cbm_stage_write_t *run, const cbm_stage_write_ops_t *ops,
                                        bool surfaces_in_place, bool doc_candidates_in_place,
                                        bool fts_wholesale) {
    bool ok = ops->hashes(run) && (surfaces_in_place || ops->surfaces(run)) &&
              (doc_candidates_in_place || ops->doc_candidates(run)) && ops->format(run);
    ops->mark(run, "writes");
    ok = ops->coverage(run) && ok;
    ops->mark(run, "coverage_replace");
    ok = ok && ops->doc_links(run);
    ops->mark(run, "doc_links");
    ok = (!fts_wholesale || ops->fts(run)) && ok;
    ops->mark(run, "fts");
    ok = ok && ops->advance(run);
    ops->mark(run, "generation");
    ok = ok && ops->integrity(run);
    ops->mark(run, "integrity");
    ok = ok && ops->seal(run);
    ops->mark(run, "seal");
    return ok;
}

int cbm_pipeline_stage_finalize_reference(cbm_stage_finalize_t *run,
                                          const cbm_stage_finalize_ops_t *ops) {
    if (!ops->remove_sidecars(run)) {
        ops->discard(run);
        return ops->persist_failed;
    }
    if (ops->cancelled(run)) {
        ops->discard(run);
        return ops->abort_preserve_db;
    }
    if (!ops->prepare(run)) {
        ops->discard(run);
        return ops->persist_failed;
    }
    if (ops->cancelled(run)) {
        bool restored = ops->rollback(run);
        ops->discard(run);
        return restored ? ops->abort_preserve_db : ops->persist_failed;
    }
    if (!ops->rename(run)) {
        (void)ops->rollback(run);
        ops->discard(run);
        return ops->persist_failed;
    }
    ops->release(run);
    return 0;
}
