#ifndef CBM_TEST_IMPACT_ENGINE_REACH_H
#define CBM_TEST_IMPACT_ENGINE_REACH_H

#include "mcp/test_impact_engine_internal.h"

/* All inputs are borrowed for the duration of the Rust call. The store is
 * the one bound to scope; neither Rust nor its callbacks close the scope. */
typedef struct {
    te_ctx_t *context;
    cbm_store_t *store;
    cbm_store_read_scope_t *scope;
    cbm_ti_graph_t *graph;
    const cbm_ti_source_t *source;
    const cbm_test_model_t *model;
    const cbm_test_policy_t *policy;
    const cbm_ti_seeds_t *seeds;
    te_match_t *matches;
    int match_count;
    const char *const *fixture_paths;
    const int *fixture_rules;
    int fixture_count;
} te_reach_input_t;

bool cbm_test_impact_rust_reach(const te_reach_input_t *input);
void cbm_test_impact_reach_free(void *items);
/* Adopt both arrays from the extract memory class. Their names still borrow
 * from the request's model. */
void cbm_test_impact_reach_publish(te_ctx_t *c, cbm_test_reach_t *rows, int row_count,
                                   cbm_test_suite_trigger_t *triggers, int trigger_count);

#endif
