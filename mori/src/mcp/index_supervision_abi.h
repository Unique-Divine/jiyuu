/* Internal bridge between the worker supervisor and Rust policy. */
#ifndef MORI_INDEX_SUPERVISION_ABI_H
#define MORI_INDEX_SUPERVISION_ABI_H

#include <stdbool.h>

typedef struct mori_index_supervision mori_index_supervision_t;

/* Rust owns this allocation. Requests may race with the polling owner, but
 * destruction requires all calls to be finished. */
mori_index_supervision_t *mori_index_supervision_new(void);
void mori_index_supervision_free(mori_index_supervision_t *state);

enum {
    MORI_FENCE_CREATED,
    MORI_FENCE_EXISTS,
    MORI_FENCE_ERROR,
};
typedef int (*mori_fence_create_fn)(const char *path, int *error_out);
typedef bool (*mori_index_cancel_fn)(void *context);

enum {
    MORI_PUBLICATION_ALLOWED,
    MORI_PUBLICATION_CLAIMED,
    MORI_PUBLICATION_REFUSED_LIMIT,
    MORI_PUBLICATION_REFUSED_ERROR,
};
int mori_index_worker_publication(bool already_claimed, const char *path,
                                  mori_fence_create_fn create);

bool mori_index_supervision_checks_limits(const mori_index_supervision_t *state);

enum {
    MORI_LIMIT_IGNORED,
    MORI_LIMIT_ACCEPTED,
    MORI_LIMIT_AFTER_PUBLICATION,
    MORI_LIMIT_FENCE_ERROR,
};
int mori_index_supervision_limit(const mori_index_supervision_t *state, const char *fence_path,
                                 mori_fence_create_fn create, int *error_out,
                                 mori_index_cancel_fn cancel, void *context);
bool mori_index_supervision_cancel(const mori_index_supervision_t *state,
                                   mori_index_cancel_fn cancel, void *context);

enum {
    MORI_WORKER_RESPONSE_OK,
    MORI_WORKER_RESPONSE_MISSING,
    MORI_WORKER_RESPONSE_TOO_LARGE,
};

typedef struct {
    bool clean;
    bool cancellation_requested;
    bool tree_quiesced;
    bool supervision_failed;
    bool resource_violation;
    bool resource_probe_failed;
} mori_index_terminal_facts_t;

enum {
    MORI_TERMINAL_CLEAN,
    MORI_TERMINAL_WORKER_FAILED,
    MORI_TERMINAL_CONTAINMENT_FAILED,
    MORI_TERMINAL_CANCELLED,
    MORI_TERMINAL_RESOURCE_LIMIT,
    MORI_TERMINAL_RESOURCE_PROBE_FAILED,
    MORI_TERMINAL_RESPONSE_MISSING,
    MORI_TERMINAL_RESPONSE_REJECTED,
};
typedef struct {
    int disposition;
    bool cancellation_requested;
    bool publication_committed;
    bool remove_fence;
} mori_index_terminal_decision_t;

/* Poll owner only, after process termination and log relay. Only a clean
 * disposition admits reading the C-owned response. */
mori_index_terminal_decision_t mori_index_supervision_terminal(
    const mori_index_supervision_t *state, const char *fence_path, mori_fence_create_fn create,
    mori_index_terminal_facts_t facts);
int mori_index_supervision_response_disposition(int response_status);

#endif
