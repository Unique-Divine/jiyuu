/* Private ABI between the Rust stdio session and the native daemon. */
#ifndef CBM_DAEMON_FRONTEND_SESSION_H
#define CBM_DAEMON_FRONTEND_SESSION_H

#include "daemon/frontend.h"
#include "foundation/compat_thread.h"

typedef cbm_thread_t mori_frontend_thread_t;

int mori_frontend_session_run(cbm_daemon_runtime_client_t *client,
                              cbm_version_cohort_manager_t *manager,
                              const cbm_daemon_frontend_session_config_t *session, FILE *in,
                              FILE *out);

mori_frontend_thread_t *mori_frontend_thread_start(void *(*run)(void *), void *context);
bool mori_frontend_thread_join(mori_frontend_thread_t *thread);
_Noreturn void mori_frontend_fail_stop(void);
cbm_daemon_maintenance_monitor_t *mori_frontend_monitor_start(
    cbm_version_cohort_manager_t *manager, cbm_daemon_maintenance_cancel_fn cancel, void *context,
    int exit_code, const char *participant, bool exit_when_cancel_not_needed);
void mori_frontend_note_idle(void);
void mori_frontend_note_cancellation(void);

void *mori_frontend_alloc(size_t size);
void mori_frontend_free(void *buffer);
ptrdiff_t mori_frontend_read_bytes(FILE *in, uint8_t *bytes, size_t size);
bool mori_frontend_write_bytes(FILE *out, const uint8_t *bytes, size_t size);
bool mori_frontend_flush(FILE *out);
bool mori_frontend_read_id(const char *message, bool *has_id, int64_t *id, char **id_str);
/* True transfers params (possibly NULL) to the caller. */
bool mori_frontend_cancel_params(const char *message, const char **params);
char *mori_frontend_cancelled_response(const char *message, int64_t id, const char *id_str);

cbm_daemon_runtime_client_t *mori_frontend_bootstrap(
    const cbm_daemon_frontend_session_config_t *session);
bool mori_frontend_configure(cbm_daemon_runtime_client_t *client,
                             const cbm_daemon_frontend_session_config_t *session);

#endif
