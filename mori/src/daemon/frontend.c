/*
 * frontend.c — Stateless stdio bridge for a daemon-backed MCP session.
 */
#include "daemon/frontend.h"

#include "daemon/application.h"
#include "daemon/frontend_session.h"
#include "foundation/compat.h"
#include "foundation/compat_thread.h"
#include "foundation/platform.h"
#include "foundation/sanitized.h"
#include "foundation/subprocess.h"
#include "mcp/mcp.h"

#include <assert.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

enum {
    FRONTEND_WAIT_US = 1000,
    FRONTEND_MAINTENANCE_POLL_MS = 10,
    /* Cadence for the quiet no-maintenance steady state. Every presence probe
     * try-acquires the maintenance marker lock, and on Windows that
     * revalidates the whole ancestor directory chain (a CreateFileW +
     * identity check per path component). At the old 10 ms cadence this
     * burned ~20% of a core for the life of every MCP session — 7762
     * CPU-seconds on one 9.5 h session in the 2026-08-29 incident. The
     * budget: an activation drains participants within
     * CLI_ACTIVATION_DRAIN_TIMEOUT_MS (15 s) and the monitor's own exit grace
     * is FRONTEND_MAINTENANCE_GRACE_MS (3 s), so a 250 ms detection latency
     * spends under 2% of that window while cutting the probe cost 25-fold.
     * The 10 ms cadence still paces the monitor's post-detection grace loop,
     * which does no lock probing. */
    FRONTEND_MAINTENANCE_IDLE_POLL_MS = 250,
    /* The owner thread may be draining a supervised process tree. Preserve the
     * supervisor's complete graceful + forced-settle window before the monitor
     * fail-stops the process, plus scheduling/teardown margin. */
    FRONTEND_MAINTENANCE_GRACE_MS =
        CBM_SUBPROCESS_MAX_CANCEL_GRACE_MS + CBM_SUBPROCESS_FORCE_SETTLE_MS + 1000,
    FRONTEND_PARTICIPANT_NAME_CAP = 64,
};

struct cbm_daemon_maintenance_monitor {
    cbm_thread_t thread;
    cbm_version_cohort_manager_t *manager;
    cbm_daemon_maintenance_cancel_fn cancel;
    void *cancel_context;
    atomic_bool stopping;
    bool exit_when_cancel_not_needed;
    int exit_code;
    char participant[FRONTEND_PARTICIPANT_NAME_CAP];
};

#if defined(CBM_ENABLE_TEST_SEAMS) && CBM_ENABLE_TEST_SEAMS
static atomic_bool frontend_test_hold_monitor = ATOMIC_VAR_INIT(false);
static atomic_bool frontend_test_monitor_is_waiting = ATOMIC_VAR_INIT(false);
static atomic_uint_fast64_t frontend_test_monitor_observation_count = ATOMIC_VAR_INIT(0);
static atomic_uint_fast64_t frontend_test_worker_idle_count = ATOMIC_VAR_INIT(0);
static atomic_uint_fast64_t frontend_test_routed_cancellation_count = ATOMIC_VAR_INIT(0);
static atomic_uint_fast32_t frontend_test_replay_budget_ms = ATOMIC_VAR_INIT(0);
static atomic_uint_fast32_t frontend_test_monitor_idle_sleep = ATOMIC_VAR_INIT(0);

void cbm_daemon_frontend_test_observer_reset(bool hold_monitor) {
    atomic_store_explicit(&frontend_test_monitor_observation_count, 0, memory_order_release);
    atomic_store_explicit(&frontend_test_monitor_idle_sleep, 0, memory_order_release);
    atomic_store_explicit(&frontend_test_worker_idle_count, 0, memory_order_release);
    atomic_store_explicit(&frontend_test_monitor_is_waiting, false, memory_order_release);
    atomic_store_explicit(&frontend_test_hold_monitor, hold_monitor, memory_order_release);
}

void cbm_daemon_frontend_test_observer_release(void) {
    atomic_store_explicit(&frontend_test_hold_monitor, false, memory_order_release);
}

bool cbm_daemon_frontend_test_monitor_waiting(void) {
    return atomic_load_explicit(&frontend_test_monitor_is_waiting, memory_order_acquire);
}

uint64_t cbm_daemon_frontend_test_monitor_observations(void) {
    return atomic_load_explicit(&frontend_test_monitor_observation_count, memory_order_acquire);
}

uint64_t cbm_daemon_frontend_test_worker_observations(void) {
    return 0;
}

uint64_t cbm_daemon_frontend_test_worker_idle_cycles(void) {
    return atomic_load_explicit(&frontend_test_worker_idle_count, memory_order_acquire);
}

uint64_t cbm_daemon_frontend_test_routed_cancellations(void) {
    return atomic_load_explicit(&frontend_test_routed_cancellation_count, memory_order_acquire);
}

uint32_t cbm_daemon_frontend_test_replay_timeout_ms(void) {
    return (uint32_t)atomic_load_explicit(&frontend_test_replay_budget_ms, memory_order_acquire);
}

uint32_t cbm_daemon_frontend_test_monitor_idle_sleep_ms(void) {
    return (uint32_t)atomic_load_explicit(&frontend_test_monitor_idle_sleep, memory_order_acquire);
}
#endif

static cbm_version_cohort_maintenance_presence_t frontend_observe_maintenance(
    cbm_version_cohort_manager_t *manager) {
#if defined(CBM_ENABLE_TEST_SEAMS) && CBM_ENABLE_TEST_SEAMS
    atomic_fetch_add_explicit(&frontend_test_monitor_observation_count, 1, memory_order_release);
    if (atomic_load_explicit(&frontend_test_hold_monitor, memory_order_acquire)) {
        atomic_store_explicit(&frontend_test_monitor_is_waiting, true, memory_order_release);
        while (atomic_load_explicit(&frontend_test_hold_monitor, memory_order_acquire)) {
            cbm_usleep(FRONTEND_WAIT_US);
        }
        atomic_store_explicit(&frontend_test_monitor_is_waiting, false, memory_order_release);
    }
#endif
    return cbm_version_cohort_maintenance_presence_terminal(manager);
}

static void *frontend_maintenance_monitor_worker(void *opaque) {
    cbm_daemon_maintenance_monitor_t *monitor = opaque;
    while (!atomic_load_explicit(&monitor->stopping, memory_order_acquire)) {
        cbm_version_cohort_maintenance_presence_t presence =
            frontend_observe_maintenance(monitor->manager);
        if (presence == CBM_VERSION_COHORT_MAINTENANCE_ABSENT) {
            uint32_t idle_ms = FRONTEND_MAINTENANCE_IDLE_POLL_MS;
#if defined(CBM_ENABLE_TEST_SEAMS) && CBM_ENABLE_TEST_SEAMS
            atomic_store_explicit(&frontend_test_monitor_idle_sleep, idle_ms, memory_order_release);
#endif
            cbm_usleep(idle_ms * 1000U);
            continue;
        }

        if (presence == CBM_VERSION_COHORT_MAINTENANCE_REQUESTED) {
            bool cancel_requested = false;
            if (monitor->cancel) {
                cancel_requested = monitor->cancel(monitor->cancel_context);
            }
            /* Never log, write to, or flush agent stdio from this monitor.
             * Structured logging itself writes to stderr, and this thread's
             * reason for existing is to remain runnable when another frontend
             * thread is blocked on a full stdout/stderr pipe. The activation
             * owner records the maintenance event durably. */

            if (monitor->exit_when_cancel_not_needed && !cancel_requested) {
                _Exit(monitor->exit_code);
            }
            uint64_t now = cbm_now_ms();
            uint64_t deadline = now > UINT64_MAX - FRONTEND_MAINTENANCE_GRACE_MS
                                    ? UINT64_MAX
                                    : now + FRONTEND_MAINTENANCE_GRACE_MS;
            while (!atomic_load_explicit(&monitor->stopping, memory_order_acquire) &&
                   cbm_now_ms() < deadline) {
                cbm_usleep(FRONTEND_MAINTENANCE_POLL_MS * 1000U);
            }
            if (atomic_load_explicit(&monitor->stopping, memory_order_acquire)) {
                return NULL;
            }
            _Exit(monitor->exit_code);
        }

        /* An observer that cannot prove absence must not let local work
         * survive into a binary mutation window. Fail closed and let native
         * process teardown release SQLite and cohort ownership. */
        _Exit(EXIT_FAILURE);
    }
    return NULL;
}

cbm_daemon_maintenance_monitor_t *mori_frontend_monitor_start(
    cbm_version_cohort_manager_t *manager, cbm_daemon_maintenance_cancel_fn cancel,
    void *cancel_context, int exit_code, const char *participant,
    bool exit_when_cancel_not_needed) {
    if (!manager || exit_code < 0 || !participant || !participant[0]) {
        return NULL;
    }
    cbm_daemon_maintenance_monitor_t *monitor = calloc(1, sizeof(*monitor));
    if (!monitor) {
        return NULL;
    }
    monitor->manager = manager;
    monitor->cancel = cancel;
    monitor->cancel_context = cancel_context;
    monitor->exit_when_cancel_not_needed = exit_when_cancel_not_needed;
    monitor->exit_code = exit_code;
    atomic_init(&monitor->stopping, false);
    int written = snprintf(monitor->participant, sizeof(monitor->participant), "%s", participant);
    if (written <= 0 || written >= (int)sizeof(monitor->participant) ||
        cbm_thread_create(&monitor->thread, 0, frontend_maintenance_monitor_worker, monitor) != 0) {
        free(monitor);
        return NULL;
    }
    return monitor;
}

cbm_daemon_maintenance_monitor_t *cbm_daemon_maintenance_monitor_start(
    cbm_version_cohort_manager_t *manager, cbm_daemon_maintenance_cancel_fn cancel,
    void *cancel_context, int exit_code, const char *participant) {
    return mori_frontend_monitor_start(manager, cancel, cancel_context, exit_code, participant,
                                       false);
}

bool cbm_daemon_maintenance_monitor_stop(cbm_daemon_maintenance_monitor_t **monitor_io) {
    if (!monitor_io || !*monitor_io) {
        return false;
    }
    cbm_daemon_maintenance_monitor_t *monitor = *monitor_io;
    atomic_store_explicit(&monitor->stopping, true, memory_order_release);
    if (cbm_thread_join(&monitor->thread) != 0) {
        return false;
    }
    free(monitor);
    *monitor_io = NULL;
    return true;
}

const char *cbm_daemon_frontend_cancelled_error_message(const char *request_message) {
    static const char plain[] = "Request cancelled";
    static const char index_call[] = "Request cancelled. " CBM_MCP_INDEX_ASYNC_HINT;
    cbm_jsonrpc_request_t request = {0};
    if (!request_message || cbm_jsonrpc_parse(request_message, &request) != 0) {
        return plain;
    }
    char *tool = request.method && strcmp(request.method, "tools/call") == 0 && request.params_raw
                     ? cbm_mcp_get_string_arg(request.params_raw, "name")
                     : NULL;
    bool index_repository = tool && strcmp(tool, "index_repository") == 0;
    safe_free(tool);
    cbm_jsonrpc_request_free(&request);
    return index_repository ? index_call : plain;
}

static bool frontend_parse_cancellation(const char *message, cbm_jsonrpc_request_t *request_out) {
    if (!message || !request_out) {
        return false;
    }
    memset(request_out, 0, sizeof(*request_out));
    if (cbm_jsonrpc_parse(message, request_out) != 0) {
        return false;
    }
    bool cancellation = !request_out->has_id && request_out->method &&
                        strcmp(request_out->method, "notifications/cancelled") == 0;
    if (!cancellation) {
        cbm_jsonrpc_request_free(request_out);
    }
    return cancellation;
}

bool cbm_daemon_frontend_is_cancellation_notification(const char *message) {
    cbm_jsonrpc_request_t request = {0};
    bool cancellation = frontend_parse_cancellation(message, &request);
    if (cancellation) {
        cbm_jsonrpc_request_free(&request);
    }
    return cancellation;
}

bool cbm_daemon_frontend_cancellation_matches_request(const char *message, int64_t active_id,
                                                      const char *active_id_str) {
    cbm_jsonrpc_request_t request = {0};
    bool cancellation = frontend_parse_cancellation(message, &request);
    bool matches = cancellation &&
                   cbm_mcp_cancel_request_matches(request.params_raw, active_id, active_id_str);
    if (cancellation) {
        cbm_jsonrpc_request_free(&request);
    }
    return matches;
}

bool mori_frontend_configure(cbm_daemon_runtime_client_t *client,
                             const cbm_daemon_frontend_session_config_t *session) {
    /* The replay restores what startup set, so it waits within the startup
     * budget like the first set_context and set_ui_config in main.c. The
     * connect budget only bounds a probe (1 s in production): a replacement
     * daemon whose UI save runs longer would end an otherwise good recovery. */
    uint32_t timeout_ms = session->bootstrap.startup_timeout_ms;
#if defined(CBM_ENABLE_TEST_SEAMS) && CBM_ENABLE_TEST_SEAMS
    atomic_store_explicit(&frontend_test_replay_budget_ms, timeout_ms, memory_order_release);
#endif
    if (cbm_daemon_application_client_set_context(
            client, session->session_root, session->allowed_root, session->tool_profile, NULL, NULL,
            timeout_ms) != CBM_DAEMON_RUNTIME_APPLICATION_OK) {
        return false;
    }
    return session->ui_update_mask == 0 ||
           cbm_daemon_application_client_set_ui_config(
               client, session->ui_update_mask, session->ui_enabled, session->ui_port,
               timeout_ms) == CBM_DAEMON_RUNTIME_APPLICATION_OK;
}

/* Native adapters used by the Rust session. The platform thread wrapper
 * retains the existing stack floor, thread handle and allocator teardown. */
mori_frontend_thread_t *mori_frontend_thread_start(void *(*run)(void *), void *context) {
    mori_frontend_thread_t *thread = malloc(sizeof(*thread));
    if (thread && cbm_thread_create(thread, 0, run, context) != 0) {
        free(thread);
        return NULL;
    }
    return thread;
}

bool mori_frontend_thread_join(mori_frontend_thread_t *thread) {
    if (cbm_thread_join(thread) != 0) {
        return false;
    }
    free(thread);
    return true;
}

_Noreturn void mori_frontend_fail_stop(void) {
    _Exit(EXIT_FAILURE);
}

void mori_frontend_note_idle(void) {
#if defined(CBM_ENABLE_TEST_SEAMS) && CBM_ENABLE_TEST_SEAMS
    atomic_fetch_add_explicit(&frontend_test_worker_idle_count, 1, memory_order_release);
#endif
}

void mori_frontend_note_cancellation(void) {
#if defined(CBM_ENABLE_TEST_SEAMS) && CBM_ENABLE_TEST_SEAMS
    atomic_fetch_add_explicit(&frontend_test_routed_cancellation_count, 1, memory_order_release);
#endif
}

void *mori_frontend_alloc(size_t size) {
    return malloc(size);
}

void mori_frontend_free(void *buffer) {
    free(buffer);
}

/* Read and write through the CRT that owns the borrowed FILE, including
 * Windows builds that link a Rust archive into the native executable. */
ptrdiff_t mori_frontend_read_bytes(FILE *in, uint8_t *bytes, size_t size) {
    size_t count = 0;
    if (size == 1) {
        int byte = fgetc(in);
        if (byte != EOF) {
            *bytes = (uint8_t)byte;
            count = 1;
        }
    } else {
        count = fread(bytes, 1, size, in);
    }
    return count == 0 && ferror(in) ? -1 : (ptrdiff_t)count;
}

bool mori_frontend_write_bytes(FILE *out, const uint8_t *bytes, size_t size) {
    CBM_MSAN_WRITTEN(bytes, size);
    return fwrite(bytes, 1, size, out) == size;
}

bool mori_frontend_flush(FILE *out) {
    return fflush(out) == 0;
}

bool mori_frontend_read_id(const char *message, bool *has_id, int64_t *id, char **id_str) {
    cbm_jsonrpc_request_t request = {0};
    if (cbm_jsonrpc_parse(message, &request) != 0) {
        return true; /* Forward malformed JSON for the daemon's reply. */
    }
    *has_id = request.has_id;
    *id = request.id;
    *id_str = request.id_str ? cbm_strdup(request.id_str) : NULL;
    bool copied = !request.id_str || *id_str;
    cbm_jsonrpc_request_free(&request);
    return copied;
}

bool mori_frontend_cancel_params(const char *message, const char **params) {
    cbm_jsonrpc_request_t request = {0};
    if (!frontend_parse_cancellation(message, &request)) {
        return false;
    }
    *params = request.params_raw;
    request.params_raw = NULL;
    cbm_jsonrpc_request_free(&request);
    return true;
}

char *mori_frontend_cancelled_response(const char *message, int64_t id, const char *id_str) {
    char cancelled_error[512];
    (void)snprintf(cancelled_error, sizeof(cancelled_error), "{\"code\":-32800,\"message\":\"%s\"}",
                   cbm_daemon_frontend_cancelled_error_message(message));
    cbm_jsonrpc_response_t response = {
        .id = id,
        .id_str = id_str,
        .error_json = cancelled_error,
        .error_code = -32800,
    };
    return cbm_jsonrpc_format_response(&response);
}

cbm_daemon_runtime_client_t *mori_frontend_bootstrap(
    const cbm_daemon_frontend_session_config_t *session) {
    cbm_daemon_bootstrap_result_t bootstrap = {0};
    return cbm_daemon_bootstrap_execute(&session->bootstrap, &bootstrap) ==
                   CBM_DAEMON_BOOTSTRAP_CONNECTED
               ? bootstrap.client
               : NULL;
}

/* The runtime enums cross the private ABI as C integers. */
static_assert(CBM_DAEMON_RUNTIME_APPLICATION_TRANSPORT_ERROR == 0, "frontend transport status");
static_assert(CBM_DAEMON_RUNTIME_APPLICATION_OK == 1, "frontend application status");
static_assert(CBM_DAEMON_RUNTIME_APPLICATION_CANCELLED == 6, "frontend cancelled status");
static_assert(CBM_DAEMON_RUNTIME_CANCEL_ERROR == -1, "frontend cancel error");
static_assert(CBM_DAEMON_RUNTIME_CANCEL_ACCEPTED == 1, "frontend cancel accepted");
static_assert(CBM_DAEMON_RUNTIME_APPLICATION_TOKEN_INVALID == 0, "frontend invalid token");

int cbm_daemon_frontend_mcp_run(cbm_daemon_runtime_client_t *client,
                                cbm_version_cohort_manager_t *cohort_manager,
                                const cbm_daemon_frontend_session_config_t *session, FILE *in,
                                FILE *out) {
    if (!client || !cohort_manager || !session || !session->bootstrap.endpoint ||
        !session->bootstrap.identity || !session->session_root || !session->session_root[0] ||
        !in || !out) {
        return -1;
    }
    return mori_frontend_session_run(client, cohort_manager, session, in, out);
}
