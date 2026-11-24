/* Private ABI between the C watcher host and Rust poll policy. */
#ifndef CBM_WATCHER_RUST_POLL_H
#define CBM_WATCHER_RUST_POLL_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

typedef struct cbm_rust_watcher_state cbm_rust_watcher_state_t;
typedef struct cbm_watcher_poll_host cbm_watcher_poll_host_t;

typedef struct {
    bool registered;
    bool named;
    bool catch_up;
    bool poll_non_git;
} cbm_watcher_host_flags_t;

enum {
    CBM_WATCH_LOG_ROOT_MISSING = 0,
    CBM_WATCH_LOG_ROOT_RESTORED,
    CBM_WATCH_LOG_ROOT_STAT_ERROR,
    CBM_WATCH_LOG_TREE_WALK_FAILED,
    CBM_WATCH_LOG_BASELINE,
    CBM_WATCH_LOG_INDEX_RETRY,
    CBM_WATCH_LOG_INDEX_ERROR,
    CBM_WATCH_LOG_SUSTAINED_FAILURE,
    CBM_WATCH_LOG_COOLDOWN,
};

/* Host and state live through the synchronous call to cbm_rust_watcher_poll. */
cbm_rust_watcher_state_t *cbm_rust_watcher_new(void);
void cbm_rust_watcher_free(cbm_rust_watcher_state_t *state);
bool cbm_rust_watcher_poll(const cbm_rust_watcher_state_t *state, cbm_watcher_poll_host_t *host,
                           int64_t cycle_ns);
void cbm_rust_watcher_touch(const cbm_rust_watcher_state_t *state);
int cbm_rust_watcher_failures(const cbm_rust_watcher_state_t *state);
int64_t cbm_rust_watcher_next_poll_ns(const cbm_rust_watcher_state_t *state);
int cbm_rust_watcher_dirty_sigs(const cbm_rust_watcher_state_t *state, uint64_t *last,
                                uint64_t *pending);

cbm_watcher_host_flags_t cbm_watcher_host_flags(cbm_watcher_poll_host_t *host);
int64_t cbm_watcher_host_now_ns(void);
uint64_t cbm_watcher_host_now_ms(void);
/* Root probe: 0 for present, 1 for missing, 2 for uncertain. */
int cbm_watcher_host_root(cbm_watcher_poll_host_t *host, int *stat_errno);
/* Git probes: 0 for success, 1 for command failure, 2+ for uncertain. */
int cbm_watcher_host_git_repository(cbm_watcher_poll_host_t *host);
int cbm_watcher_host_head(cbm_watcher_poll_host_t *host, uint8_t *head, size_t size);
int cbm_watcher_host_dirty_signature(cbm_watcher_poll_host_t *host, uint64_t *signature);
int cbm_watcher_host_git_file_count(cbm_watcher_poll_host_t *host, int *count);
bool cbm_watcher_host_tree(cbm_watcher_poll_host_t *host, uint64_t *signature, int *count);
void cbm_watcher_host_publish(cbm_watcher_poll_host_t *host, int strategy, int interval_ms,
                               bool scanned);
void cbm_watcher_host_log(cbm_watcher_poll_host_t *host, int event, int value, int extra);
void cbm_watcher_host_set_catch_up(cbm_watcher_poll_host_t *host, bool needed);
bool cbm_watcher_host_index(cbm_watcher_poll_host_t *host, int strategy, int *result);
uint64_t cbm_watcher_host_prune_grace_s(void);
void cbm_watcher_host_prune(cbm_watcher_poll_host_t *host, uint64_t first_missing_ms);

#endif
