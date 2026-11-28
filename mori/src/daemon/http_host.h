#ifndef CBM_DAEMON_HTTP_HOST_H
#define CBM_DAEMON_HTTP_HOST_H

#include "ui/config.h"
#include "ui/http_server.h"

#include <stdbool.h>
#include <stdint.h>

/* Inline state owned by the daemon loop. Zero-init before the first call.
 * Keep the layout in sync with the Rust definitions in ui/http_host.rs. */
typedef struct {
    cbm_http_server_t *server;
    bool started;
    bool retiring;
    bool config_loaded;
    bool assets_available;
    cbm_ui_config_t config;
    uint64_t next_config_load_ms;
    uint64_t retry_at_ms;
    uint32_t retry_delay_ms;
    uint32_t largest_scheduled_retry_ms;
} mori_http_host_t;

typedef struct {
    void *context;
    void (*config_load)(void *context, cbm_ui_config_t *config_out);
    cbm_http_server_t *(*server_new)(void *context, int port);
    void (*server_configure)(void *context, cbm_http_server_t *server, void *owner);
    void (*server_stop)(void *context, cbm_http_server_t *server);
    bool (*server_free)(void *context, cbm_http_server_t *server);
    int (*thread_start)(void *context, void *owner, cbm_http_server_t *server);
    int (*thread_join)(void *context, void *owner);
    void (*log_retry)(const char *reason, uint32_t delay_ms);
    void (*log_no_assets)(void);
} mori_http_host_ops_t;

/* The owner holds platform handles and dependency contexts. Callbacks must
 * leave the inline Rust state alone and must not re-enter these functions. */
void mori_http_host_reconcile(mori_http_host_t *state, const mori_http_host_ops_t *ops, void *owner,
                              uint64_t now_ms, bool force_config_load);
void mori_http_host_stop(const mori_http_host_t *state, const mori_http_host_ops_t *ops);
bool mori_http_host_join(mori_http_host_t *state, const mori_http_host_ops_t *ops, void *owner);
bool mori_http_host_dispose(mori_http_host_t *state, const mori_http_host_ops_t *ops, void *owner);

#endif
