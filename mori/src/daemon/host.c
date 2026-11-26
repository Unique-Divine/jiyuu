/*
 * host.c — Lifecycle owner for the mandatory per-account CBM daemon.
 */
#include "daemon/host.h"
#include "daemon/host_internal.h"
#include "daemon/http_host.h"

#include "daemon/application.h"
#include "daemon/bootstrap.h"
#include "daemon/runtime.h"
#include "daemon/project_lock.h"
#include "daemon/version_cohort.h"
#include "cli/cli.h"
#include "foundation/compat.h"
#include "foundation/compat_fs.h"
#include "foundation/compat_thread.h"
#include "foundation/diagnostics.h"
#include "foundation/log.h"
#include "foundation/mem.h"
#include "foundation/platform.h"
#include "foundation/secure_random.h"
#include "foundation/sha256.h"
#include "mcp/index_supervisor.h"
#include "store/store.h"
#include "ui/config.h"
#include "ui/embedded_assets.h"
#include "ui/http_server.h"
#include "watcher/watcher.h"
#include "yyjson/yyjson.h"

#include <stdbool.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

enum {
    HOST_MAX_CLIENTS = 64,
    HOST_LEASE_TIMEOUT_MS = 30000,
    HOST_REQUEST_TIMEOUT_MS = 5000,
    HOST_RUNTIME_SHUTDOWN_MS = 10000,
    HOST_APPLICATION_SHUTDOWN_MS = 10000,
    HOST_COORDINATION_CLEANUP_MS = 500,
    HOST_INITIAL_CLIENT_TIMEOUT_MS = 10000,
    /* A just-superseded daemon generation can still hold the cohort claim for
     * the brief window between going client-terminal (which lets the next
     * generation start) and releasing its cohort locks. Wait out that handoff
     * rather than failing startup on a transient BUSY. Bounded well under the
     * client's 30 s daemon-start budget. */
    HOST_DAEMON_CLAIM_TIMEOUT_MS = 12000,
    HOST_WAIT_TICK_MS = 100,
    HOST_WATCH_INTERVAL_MS = 5000,
    HOST_NAMED_WATCH_CONFIG_POLL_MS = 1000,
    HOST_NAMED_WATCH_CONFIG_CAP = 64 * 1024,
    /* Age-based temp cleaners need days of inactivity (macOS tmp_cleaner: 3);
     * a minute bounds the blind window after a long sleep (#2178). */
    HOST_COORDINATION_TOUCH_MS = 60000,
    HOST_CONFLICT_LOG_CAP = 1024 * 1024,
    HOST_OPERATION_LOG_CAP = 5 * 1024 * 1024,
    HOST_PATH_CAP = 4096,
#ifdef _WIN32
    /* LockFileEx has no non-owning F_GETLK equivalent. A lifetime probe must
     * momentarily take and release the private file range. The child can also
     * briefly lose the legacy-mutex handoff to another waiting starter, so it
     * retries both bounded observation/compatibility windows. */
    HOST_WINDOWS_LIFETIME_ACQUIRE_MS = 250,
#endif
};

typedef struct host_state host_state_t;

typedef struct {
    yyjson_doc *document;
    size_t count;
    cbm_daemon_named_watch_t entries[CBM_DAEMON_NAMED_WATCH_LIMIT];
} host_named_watch_snapshot_t;

struct host_state {
    /* Mirrors cbm_daemon_host_config_t.permanent for prepare-time consumers. */
    bool permanent;
    cbm_daemon_application_t *application;
    cbm_watcher_t *watcher;
    cbm_store_t *watch_store;
    cbm_config_t *runtime_config;
    cbm_project_lock_manager_t *project_locks;
    mori_http_host_t http;
    cbm_thread_t watcher_thread;
    cbm_thread_t http_thread;
    bool watcher_started;
    char named_watch_path[HOST_PATH_CAP];
    uint64_t named_watch_next_load_ms;
    char named_watch_digest[CBM_SHA256_HEX_LEN + 1];
    host_named_watch_snapshot_t *named_watch_candidate;
    host_named_watch_snapshot_t *named_watch_accepted;
    bool named_watch_unavailable;
    bool named_watch_apply_warned;
    bool named_watch_pending;
    bool named_watch_suspended;
    const mori_http_host_ops_t *http_ops;
    uint8_t ui_readiness_secret[CBM_SHA256_DIGEST_LEN];
};

static FILE *g_host_log_file = NULL;
static cbm_mutex_t g_host_log_mutex;
static bool g_host_log_mutex_initialized = false;

static _Noreturn void host_force_terminate(const char *component);
static uint64_t host_deadline_from(uint64_t now_ms, uint32_t delay_ms);

static void host_log_sink(const char *line) {
    cbm_ui_log_append(line);
    if (!g_host_log_file || !g_host_log_mutex_initialized) {
        return;
    }
    cbm_mutex_lock(&g_host_log_mutex);
    (void)fprintf(g_host_log_file, "%s\n", line ? line : "");
    (void)fflush(g_host_log_file);
    cbm_mutex_unlock(&g_host_log_mutex);
}

static bool host_log_open(char conflict_log_out[HOST_PATH_CAP]) {
    const char *cache = cbm_resolve_cache_dir();
    if (!cache || !cache[0] || !conflict_log_out) {
        return false;
    }
    char logs[HOST_PATH_CAP];
    int written = snprintf(logs, sizeof(logs), "%s/logs", cache);
    if (written <= 0 || written >= (int)sizeof(logs)) {
        return false;
    }
    written = snprintf(conflict_log_out, HOST_PATH_CAP, "%s/daemon-conflicts.ndjson", logs);
    if (written <= 0 || written >= HOST_PATH_CAP) {
        return false;
    }
    g_host_log_file =
        cbm_daemon_ipc_private_log_open(logs, "cbm-daemon.log", HOST_OPERATION_LOG_CAP);
    if (!g_host_log_file) {
        conflict_log_out[0] = '\0';
        return false;
    }
    cbm_mutex_init(&g_host_log_mutex);
    g_host_log_mutex_initialized = true;
    cbm_log_set_sink(host_log_sink);
    return true;
}

/* #1828: a detached daemon has no stderr; its start failure must reach the
 * client that is waiting for it, or that client burns its whole deadline and
 * reports the opposite of the truth. */
static void host_start_failure_record(const cbm_daemon_ipc_endpoint_t *endpoint,
                                      const char *component) {
    char logs[HOST_PATH_CAP];
    if (!cbm_daemon_bootstrap_log_directory(logs, sizeof(logs)) ||
        !cbm_daemon_bootstrap_start_failure_record(logs, endpoint, component)) {
        cbm_log_error("daemon.start_failure_record_failed", "component", component);
    }
}

static void host_log_close(void) {
    cbm_log_set_sink(NULL);
    cbm_ui_log_close();
    if (g_host_log_mutex_initialized) {
        cbm_mutex_lock(&g_host_log_mutex);
    }
    FILE *file = g_host_log_file;
    g_host_log_file = NULL;
    if (file) {
        (void)fclose(file);
    }
    if (g_host_log_mutex_initialized) {
        cbm_mutex_unlock(&g_host_log_mutex);
        cbm_mutex_destroy(&g_host_log_mutex);
        g_host_log_mutex_initialized = false;
    }
}

static uint64_t host_deadline_after(uint32_t timeout_ms) {
    uint64_t now = cbm_now_ms();
    return now > UINT64_MAX - timeout_ms ? UINT64_MAX : now + timeout_ms;
}

static uint64_t host_current_process_id(void) {
#ifdef _WIN32
    return (uint64_t)GetCurrentProcessId();
#else
    return (uint64_t)getpid();
#endif
}

static _Noreturn void host_cleanup_force_terminate(const char *component) {
    /* Early startup failures can precede the ordinary operation-log setup.
     * Make one best-effort attempt to establish that same owner-only durable
     * sink before the process-level escape releases every native claim. */
    if (!g_host_log_file) {
        char conflict_log[HOST_PATH_CAP];
        cbm_ui_log_init();
        (void)host_log_open(conflict_log);
    }
    host_force_terminate(component);
}

static void host_cleanup_release_until_complete(cbm_daemon_host_cleanup_release_for_test_fn release,
                                                void *context, const char *component) {
    if (!release) {
        return;
    }
    uint64_t deadline = host_deadline_after(HOST_COORDINATION_CLEANUP_MS);
    while (!release(context)) {
        if (cbm_now_ms() >= deadline) {
            host_cleanup_force_terminate(component);
        }
        cbm_usleep(1000);
    }
}

void cbm_daemon_host_cleanup_release_until_complete_for_test(
    cbm_daemon_host_cleanup_release_for_test_fn release, void *context) {
    host_cleanup_release_until_complete(release, context, "coordination_cleanup");
}

static bool host_cohort_lease_release_once(void *context) {
    cbm_version_cohort_lease_t **lease = context;
    if (!lease || !*lease) {
        return true;
    }
    (void)cbm_version_cohort_lease_release(lease);
    return *lease == NULL;
}

static bool host_cohort_manager_free_once(void *context) {
    cbm_version_cohort_manager_t **manager = context;
    if (!manager || !*manager) {
        return true;
    }
    (void)cbm_version_cohort_manager_free(manager);
    return *manager == NULL;
}

static void host_cohort_close(cbm_version_cohort_lease_t **lease,
                              cbm_version_cohort_manager_t **manager) {
    host_cleanup_release_until_complete(host_cohort_lease_release_once, lease,
                                        "cohort_lease_cleanup");
    host_cleanup_release_until_complete(host_cohort_manager_free_once, manager,
                                        "cohort_manager_cleanup");
}

static bool host_daemon_claim_release_once(void *context) {
    cbm_version_cohort_daemon_claim_t **claim = context;
    if (!claim || !*claim) {
        return true;
    }
    (void)cbm_version_cohort_daemon_claim_release(claim);
    return *claim == NULL;
}

static void host_daemon_claim_close(cbm_version_cohort_daemon_claim_t **claim) {
    host_cleanup_release_until_complete(host_daemon_claim_release_once, claim,
                                        "daemon_claim_cleanup");
}

static bool host_participant_guard_release_once(void *context) {
    cbm_daemon_ipc_participant_guard_t **guard = context;
    if (!guard || !*guard) {
        return true;
    }
    (void)cbm_daemon_ipc_participant_guard_release(guard);
    return *guard == NULL;
}

static void host_participant_guard_close(cbm_daemon_ipc_participant_guard_t **guard) {
    host_cleanup_release_until_complete(host_participant_guard_release_once, guard,
                                        "participant_guard_cleanup");
}

static int host_lifetime_reservation_acquire(
    const cbm_daemon_ipc_endpoint_t *endpoint,
    cbm_daemon_ipc_lifetime_reservation_t **reservation_out) {
#ifdef _WIN32
    uint64_t now = cbm_now_ms();
    uint64_t deadline = now > UINT64_MAX - HOST_WINDOWS_LIFETIME_ACQUIRE_MS
                            ? UINT64_MAX
                            : now + HOST_WINDOWS_LIFETIME_ACQUIRE_MS;
    do {
        int status = cbm_daemon_ipc_lifetime_reservation_try_acquire(endpoint, reservation_out);
        if (status != 0) {
            return status;
        }
        cbm_usleep(1000);
    } while (cbm_now_ms() < deadline);
    return 0;
#else
    return cbm_daemon_ipc_lifetime_reservation_try_acquire(endpoint, reservation_out);
#endif
}

static void *host_watcher_thread(void *opaque) {
    cbm_watcher_run(opaque, HOST_WATCH_INTERVAL_MS);
    return NULL;
}

static void *host_http_thread(void *opaque) {
    cbm_http_server_run(opaque);
    return NULL;
}

static int host_watcher_index(const char *project_name, const char *root_path, void *opaque) {
    host_state_t *host = opaque;
    return host && host->application
               ? cbm_daemon_application_watcher_index(project_name, root_path, host->application)
               : -1;
}

static void host_named_watch_free(host_named_watch_snapshot_t *snapshot) {
    if (snapshot) {
        yyjson_doc_free(snapshot->document);
        free(snapshot);
    }
}

static host_named_watch_snapshot_t *host_named_watch_parse(const char *text, size_t length) {
    yyjson_doc *doc = yyjson_read(text, length, 0);
    yyjson_val *root = yyjson_doc_get_root(doc);
    size_t count = yyjson_arr_size(root);
    if (!yyjson_is_arr(root) || count > CBM_DAEMON_NAMED_WATCH_LIMIT) {
        yyjson_doc_free(doc);
        return NULL;
    }
    host_named_watch_snapshot_t *snapshot = calloc(1, sizeof(*snapshot));
    if (!snapshot) {
        yyjson_doc_free(doc);
        return NULL;
    }
    snapshot->document = doc;
    snapshot->count = count;
    for (size_t i = 0; i < count; i++) {
        yyjson_val *item = yyjson_arr_get(root, i);
        yyjson_val *project = yyjson_obj_get(item, "project");
        yyjson_val *path = yyjson_obj_get(item, "root");
        const char *project_name = yyjson_get_str(project);
        const char *root_path = yyjson_get_str(path);
        if (yyjson_obj_size(item) != 2 || !project_name || !root_path ||
            strlen(project_name) != yyjson_get_len(project) ||
            strlen(root_path) != yyjson_get_len(path)) {
            host_named_watch_free(snapshot);
            return NULL;
        }
        snapshot->entries[i] =
            (cbm_daemon_named_watch_t){.project = project_name, .root = root_path};
    }
    return snapshot;
}

static void host_named_watch_unavailable(host_state_t *host) {
    if (!host->named_watch_unavailable) {
        cbm_log_warn("daemon.named_watch.file_unavailable", "path", host->named_watch_path);
        host->named_watch_unavailable = true;
        host_named_watch_free(host->named_watch_candidate);
        host->named_watch_candidate = NULL;
        host->named_watch_digest[0] = '\0';
    }
}

static void host_named_watch_read(host_state_t *host) {
    cbm_path_info_t info;
    if (cbm_path_info_utf8(host->named_watch_path, &info) != CBM_PATH_INFO_OK || !info.is_regular ||
        info.size < 0 || info.size >= HOST_NAMED_WATCH_CONFIG_CAP) {
        host_named_watch_unavailable(host);
        return;
    }
    FILE *input = cbm_fopen(host->named_watch_path, "rb");
    if (!input) {
        host_named_watch_unavailable(host);
        return;
    }
    char text[HOST_NAMED_WATCH_CONFIG_CAP];
    size_t length = fread(text, 1, sizeof(text), input);
    bool stable = !ferror(input) && (int64_t)length == info.size;
    fclose(input);
    cbm_path_info_t after;
    if (!stable || cbm_path_info_utf8(host->named_watch_path, &after) != CBM_PATH_INFO_OK ||
        !after.is_regular || after.size != info.size || after.mtime_ns != info.mtime_ns) {
        host_named_watch_unavailable(host);
        return;
    }
    host->named_watch_unavailable = false;

    /* Equal size and mtime do not identify an atomically replaced file. */
    char digest[CBM_SHA256_HEX_LEN + 1];
    cbm_sha256_hex(text, length, digest);
    if (strcmp(digest, host->named_watch_digest) == 0) {
        return;
    }
    host_named_watch_free(host->named_watch_candidate);
    host->named_watch_candidate = host_named_watch_parse(text, length);
    if (!host->named_watch_candidate) {
        cbm_log_warn("daemon.named_watch.invalid_snapshot", "path", host->named_watch_path);
    }
    memcpy(host->named_watch_digest, digest, sizeof(digest));
    host->named_watch_apply_warned = false;
}

static void host_named_watch_report_missing(const host_named_watch_snapshot_t *snapshot) {
    for (size_t i = 0; i < snapshot->count; i++) {
        const cbm_daemon_named_watch_t *entry = &snapshot->entries[i];
        cbm_path_info_t info;
        if (cbm_path_info_utf8(entry->root, &info) == CBM_PATH_INFO_ABSENT) {
            cbm_log_warn("daemon.named_watch.root_missing", "project", entry->project, "path",
                         entry->root);
        }
    }
}

static void host_named_watch_reload_at(host_state_t *host, uint64_t now_ms) {
    if (!host->named_watch_path[0] || now_ms < host->named_watch_next_load_ms) {
        return;
    }
    host->named_watch_next_load_ms = host_deadline_from(now_ms, HOST_NAMED_WATCH_CONFIG_POLL_MS);
    host_named_watch_read(host);
    if (!cbm_config_get_bool(host->runtime_config, CBM_CONFIG_AUTO_WATCH, true)) {
        if (!host->named_watch_suspended) {
            (void)cbm_daemon_application_reconcile_named_watches(host->application, NULL, 0, NULL);
            host->named_watch_suspended = true;
            cbm_log_info("daemon.named_watch.suspended");
        }
        return;
    }

    host_named_watch_snapshot_t *candidate = host->named_watch_candidate;
    bool pending = false;
    if (candidate) {
        if (cbm_daemon_application_reconcile_named_watches(host->application, candidate->entries,
                                                           candidate->count, &pending)) {
            host_named_watch_free(host->named_watch_accepted);
            host->named_watch_accepted = candidate;
            host->named_watch_candidate = NULL;
            host->named_watch_pending = pending;
            host->named_watch_suspended = false;
            if (pending) {
                host_named_watch_report_missing(candidate);
            }
            cbm_log_info("daemon.named_watch.applied", "path", host->named_watch_path);
            return;
        }
        if (!host->named_watch_apply_warned) {
            cbm_log_warn("daemon.named_watch.apply_pending", "path", host->named_watch_path);
            host->named_watch_apply_warned = true;
        }
    }
    host_named_watch_snapshot_t *accepted = host->named_watch_accepted;
    if (accepted &&
        (host->named_watch_suspended || host->named_watch_pending ||
         !cbm_daemon_application_named_watches_match(host->application, accepted->entries,
                                                     accepted->count)) &&
        cbm_daemon_application_reconcile_named_watches(host->application, accepted->entries,
                                                       accepted->count, &pending)) {
        if (host->named_watch_suspended) {
            cbm_log_info("daemon.named_watch.resumed");
        }
        host->named_watch_pending = pending;
        host->named_watch_suspended = false;
    }
}

static int host_ui_index(void *opaque, const char *root_path, const char *project_name) {
    host_state_t *host = opaque;
    return host && host->application
               ? cbm_daemon_application_index(host->application, project_name ? project_name : "",
                                              root_path)
               : -1;
}

static bool host_ui_mutation_begin(void *opaque, const char *project) {
    host_state_t *host = opaque;
    return host && host->application &&
           cbm_daemon_application_project_mutation_try_begin(host->application, project);
}

static void host_ui_mutation_end(void *opaque, const char *project) {
    host_state_t *host = opaque;
    if (host && host->application) {
        cbm_daemon_application_project_mutation_end(host->application, project);
    }
}

static void host_http_config_load_default(void *context, cbm_ui_config_t *config_out) {
    (void)context;
    cbm_ui_config_load(config_out);
}

static cbm_http_server_t *host_http_server_new_default(void *context, int port) {
    (void)context;
    return cbm_http_server_new(port);
}

static void host_http_server_configure_default(void *context, cbm_http_server_t *server,
                                               void *owner) {
    (void)context;
    host_state_t *host = owner;
    cbm_http_server_set_watcher(server, host->watcher);
    cbm_http_server_set_index_executor(server, host_ui_index, host);
    cbm_http_server_set_project_mutation_guard(server, host_ui_mutation_begin, host_ui_mutation_end,
                                               host);
    cbm_http_server_set_readiness_secret(server, host->ui_readiness_secret);
}

static void host_http_server_stop_default(void *context, cbm_http_server_t *server) {
    (void)context;
    cbm_http_server_stop(server);
}

static bool host_http_server_free_default(void *context, cbm_http_server_t *server) {
    (void)context;
    return cbm_http_server_free(server);
}

typedef int (*host_http_thread_create_fn)(void *context, cbm_thread_t *thread,
                                          void *(*entry)(void *), void *entry_context);

static int host_http_thread_create_default(void *context, cbm_thread_t *thread,
                                           void *(*entry)(void *), void *entry_context) {
    (void)context;
    return cbm_thread_create(thread, 0, entry, entry_context);
}

static int host_http_thread_schedule_create(cbm_thread_t *thread, void *(*entry)(void *),
                                            cbm_http_server_t *server,
                                            host_http_thread_create_fn create,
                                            void *create_context) {
    if (!server || !create || !cbm_http_server_schedule_run(server))
        return -1;
    int rc = create(create_context, thread, entry, server);
    if (rc != 0 && !cbm_http_server_cancel_scheduled_run(server))
        return -1;
    return rc;
}

static int host_http_thread_start_default(void *context, void *owner, cbm_http_server_t *server) {
    (void)context;
    host_state_t *host = owner;
    return host_http_thread_schedule_create(&host->http_thread, host_http_thread, server,
                                            host_http_thread_create_default, NULL);
}

static int host_http_thread_join_default(void *context, void *owner) {
    (void)context;
    host_state_t *host = owner;
    return cbm_thread_join(&host->http_thread);
}

static void host_http_log_retry(const char *reason, uint32_t delay_ms) {
    char delay_text[32];
    (void)snprintf(delay_text, sizeof(delay_text), "%u", delay_ms);
    cbm_log_warn("ui.retry_scheduled", "reason", reason, "delay_ms", delay_text);
}

static void host_http_log_no_assets(void) {
    cbm_log_warn("ui.no_assets", "hint", "rebuild with: make -f Makefile.cbm cbm-with-ui");
}

static const mori_http_host_ops_t g_host_http_default_ops = {
    .context = NULL,
    .config_load = host_http_config_load_default,
    .server_new = host_http_server_new_default,
    .server_configure = host_http_server_configure_default,
    .server_stop = host_http_server_stop_default,
    .server_free = host_http_server_free_default,
    .thread_start = host_http_thread_start_default,
    .thread_join = host_http_thread_join_default,
    .log_retry = host_http_log_retry,
    .log_no_assets = host_http_log_no_assets,
};

static uint64_t host_deadline_from(uint64_t now_ms, uint32_t delay_ms) {
    return now_ms > UINT64_MAX - delay_ms ? UINT64_MAX : now_ms + delay_ms;
}

static void host_http_reconcile_at(host_state_t *host, uint64_t now_ms, bool force_config_load) {
    if (host && host->http_ops) {
        mori_http_host_reconcile(&host->http, host->http_ops, host, now_ms, force_config_load);
    }
}

static void host_background_stop(host_state_t *host) {
    mori_http_host_stop(&host->http, host->http_ops);
    if (host->watcher) {
        cbm_watcher_stop(host->watcher);
    }
}

static bool host_background_join(host_state_t *host) {
    if (!mori_http_host_join(&host->http, host->http_ops, host)) {
        return false;
    }
    if (host->watcher_started) {
        if (cbm_thread_join(&host->watcher_thread) != 0) {
            return false;
        }
        host->watcher_started = false;
    }
    return true;
}

static bool host_project_lock_manager_free_once(void *context) {
    cbm_project_lock_manager_t **manager = context;
    if (!manager || !*manager) {
        return true;
    }
    (void)cbm_project_lock_manager_free(manager);
    return *manager == NULL;
}

static void host_state_free(host_state_t *host) {
    host_named_watch_free(host->named_watch_candidate);
    host_named_watch_free(host->named_watch_accepted);
    if (!mori_http_host_dispose(&host->http, host->http_ops, host)) {
        /* The retained server or one of its callbacks can still borrow every
         * dependency below. Process containment is the only safe teardown. */
        host_force_terminate("http_cleanup");
    }
    if (host->application) {
        if (!cbm_daemon_application_free(host->application)) {
            /* The application still owns work or a borrowed callback context.
             * Freeing any dependency below would turn the retained application
             * into UAF. The daemon is the ultimate containment boundary. */
            host_cleanup_force_terminate("application_cleanup");
        }
        host->application = NULL;
    }
    cbm_watcher_free(host->watcher);
    host->watcher = NULL;
    cbm_store_close(host->watch_store);
    host->watch_store = NULL;
    cbm_config_close(host->runtime_config);
    host->runtime_config = NULL;
    if (host->project_locks) {
        host_cleanup_release_until_complete(host_project_lock_manager_free_once,
                                            &host->project_locks, "project_lock_manager_cleanup");
    }
    cbm_secure_zero(host->ui_readiness_secret, sizeof(host->ui_readiness_secret));
}

static bool host_state_prepare(host_state_t *host, const cbm_daemon_ipc_endpoint_t *endpoint) {
    host->http_ops = &g_host_http_default_ops;
    host->http.assets_available = CBM_EMBEDDED_FILE_COUNT > 0;
    if (!cbm_secure_random(host->ui_readiness_secret, sizeof(host->ui_readiness_secret))) {
        cbm_log_error("daemon.readiness_secret_failed", "reason", "system_rng_unavailable");
        return false;
    }
    size_t aggregate_memory_budget_bytes = cbm_mem_budget();
    const char *cache = cbm_resolve_cache_dir();
    if (!cache || !cache[0]) {
        cbm_log_error("daemon.runtime_config_open_failed", "reason", "cache_dir_unavailable");
        return false;
    }
    host->runtime_config = cbm_config_open(cache);
    if (!host->runtime_config) {
        cbm_log_error("daemon.runtime_config_open_failed", "reason", "config_db_unavailable");
        return false;
    }
    host->project_locks = cbm_project_lock_manager_new(endpoint);
    /* #335: watcher_enabled (default true) is the master switch for the
     * background watcher. When false the daemon never builds the watcher or the
     * in-memory store backing it, so the poll thread never starts
     * (host_background_start has nothing to run) and no project ever registers
     * (register_watcher_if_enabled early-returns on a NULL watcher). The daemon
     * still starts and still owns everything else: IPC, HTTP/UI, project locks,
     * and manual index_repository. Read once here at daemon startup, so a change
     * takes effect when the daemon next starts — see docs/CONFIGURATION.md. */
    bool watcher_enabled = cbm_config_watcher_enabled(host->runtime_config);
    if (watcher_enabled) {
        host->watch_store = cbm_store_open_memory();
        host->watcher = cbm_watcher_new(host->watch_store, host_watcher_index, host);
        /* #1948: non-git roots stay unwatched unless watch_non_git opts in.
         * Same read-once-at-startup contract as watcher_enabled. */
        if (host->watcher && cbm_config_watch_non_git(host->runtime_config)) {
            cbm_watcher_set_poll_non_git(host->watcher, true);
            cbm_log_info("watcher.non_git", "mode", "tree_poll");
        }
    } else {
        cbm_log_info("watcher.disabled", "reason", "config");
    }
    cbm_daemon_application_config_t application_config = {
        .watcher = host->watcher,
        .config = host->runtime_config,
        .aggregate_memory_budget_bytes = aggregate_memory_budget_bytes,
        .project_locks = host->project_locks,
        .ui_readiness_secret = host->ui_readiness_secret,
        .ui_readiness_secret_length = sizeof(host->ui_readiness_secret),
    };
    host->application = cbm_daemon_application_new(&application_config);
    if (host->application && host->permanent) {
        cbm_daemon_application_set_permanent(host->application, true);
        const char *path = cbm_safe_getenv("MORI_NAMED_WATCH_FILE", host->named_watch_path,
                                           sizeof(host->named_watch_path), NULL);
        if (path && path[0]) {
            (void)cbm_daemon_application_reconcile_named_watches(host->application, NULL, 0, NULL);
            if (!cbm_hook_path_is_abs(path)) {
                cbm_log_warn("daemon.named_watch.file_unavailable", "reason",
                             "absolute_path_required");
                host->named_watch_path[0] = '\0';
            }
        }
    }
    if (!host->project_locks || !host->application) {
        return false;
    }
    /* A watcher deliberately left unbuilt by watcher_enabled=false is not a
     * startup failure; an allocation failure while it is enabled still is. */
    if (watcher_enabled && (!host->watch_store || !host->watcher)) {
        return false;
    }
    return true;
}

bool cbm_daemon_host_state_prepare_for_test(const cbm_daemon_ipc_endpoint_t *endpoint) {
    if (!endpoint) {
        return false;
    }
    host_state_t host = {0};
    bool prepared = host_state_prepare(&host, endpoint);
    host_state_free(&host);
    return prepared;
}

typedef struct {
    size_t create_failures;
    size_t thread_start_failures;
    size_t server_free_failures;
    size_t config_change_after_load;
    size_t config_loads;
    size_t server_create_attempts;
    size_t thread_start_attempts;
    size_t server_stops;
    size_t server_frees;
    size_t thread_joins;
} host_http_reconcile_test_context_t;

static void host_http_test_config_load(void *opaque, cbm_ui_config_t *config_out) {
    host_http_reconcile_test_context_t *context = opaque;
    context->config_loads++;
    config_out->ui_enabled = true;
    bool change_port = context->config_change_after_load > 0 &&
                       context->config_loads == context->config_change_after_load;
    config_out->ui_port = change_port ? CBM_UI_DEFAULT_PORT + 1 : CBM_UI_DEFAULT_PORT;
}

static cbm_http_server_t *host_http_test_server_new(void *opaque, int port) {
    (void)port;
    host_http_reconcile_test_context_t *context = opaque;
    context->server_create_attempts++;
    return context->server_create_attempts <= context->create_failures
               ? NULL
               : (cbm_http_server_t *)context;
}

static void host_http_test_server_configure(void *opaque, cbm_http_server_t *server, void *owner) {
    (void)opaque;
    (void)server;
    (void)owner;
}

/* WHY: mori_http_host_ops_t is also implemented by production callbacks that mutate
 * the server; this test double must retain that callback ABI even though it
 * only observes whether the opaque fake server is present. */
// cppcheck-suppress constParameterCallback
static void host_http_test_server_stop(void *opaque, cbm_http_server_t *server) {
    if (server) {
        ((host_http_reconcile_test_context_t *)opaque)->server_stops++;
    }
}

/* WHY: See host_http_test_server_stop; changing only this implementation to a
 * pointer-to-const parameter would make it incompatible with mori_http_host_ops_t. */
// cppcheck-suppress constParameterCallback
static bool host_http_test_server_free(void *opaque, cbm_http_server_t *server) {
    host_http_reconcile_test_context_t *context = opaque;
    if (server) {
        context->server_frees++;
    }
    return !server || context->server_frees > context->server_free_failures;
}

// cppcheck-suppress constParameterCallback
static int host_http_test_thread_start(void *opaque, void *owner, cbm_http_server_t *server) {
    (void)owner;
    (void)server;
    host_http_reconcile_test_context_t *context = opaque;
    context->thread_start_attempts++;
    return context->thread_start_attempts <= context->thread_start_failures ? -1 : 0;
}

static int host_http_test_thread_join(void *opaque, void *owner) {
    (void)owner;
    host_http_reconcile_test_context_t *context = opaque;
    context->thread_joins++;
    return 0;
}

bool cbm_daemon_host_http_reconcile_sequence_for_test(
    const uint64_t *timestamps_ms, size_t timestamp_count, size_t create_failures,
    size_t thread_start_failures, cbm_daemon_host_http_reconcile_test_result_t *result_out) {
    if (!timestamps_ms || timestamp_count == 0 || !result_out) {
        return false;
    }
    for (size_t i = 1; i < timestamp_count; i++) {
        if (timestamps_ms[i] < timestamps_ms[i - 1]) {
            return false;
        }
    }

    host_http_reconcile_test_context_t context = {
        .create_failures = create_failures,
        .thread_start_failures = thread_start_failures,
    };
    const mori_http_host_ops_t ops = {
        .context = &context,
        .config_load = host_http_test_config_load,
        .server_new = host_http_test_server_new,
        .server_configure = host_http_test_server_configure,
        .server_stop = host_http_test_server_stop,
        .server_free = host_http_test_server_free,
        .thread_start = host_http_test_thread_start,
        .thread_join = host_http_test_thread_join,
        .log_retry = host_http_log_retry,
        .log_no_assets = host_http_log_no_assets,
    };
    host_state_t host = {
        .http = {.assets_available = true},
        .http_ops = &ops,
    };
    for (size_t i = 0; i < timestamp_count; i++) {
        host_http_reconcile_at(&host, timestamps_ms[i], i == 0);
    }
    bool active = host.http.started;
    uint32_t largest_retry = host.http.largest_scheduled_retry_ms;
    uint64_t next_retry = host.http.retry_at_ms;
    if (!mori_http_host_dispose(&host.http, &ops, &host)) {
        return false;
    }

    *result_out = (cbm_daemon_host_http_reconcile_test_result_t){
        .config_loads = context.config_loads,
        .server_create_attempts = context.server_create_attempts,
        .thread_start_attempts = context.thread_start_attempts,
        .server_stops = context.server_stops,
        .server_frees = context.server_frees,
        .thread_joins = context.thread_joins,
        .largest_scheduled_retry_ms = largest_retry,
        .next_retry_ms = next_retry,
        .active_after_sequence = active,
    };
    return true;
}

bool cbm_daemon_host_http_reconcile_free_refusal_for_test(
    cbm_daemon_host_http_free_refusal_test_result_t *result_out) {
    if (!result_out) {
        return false;
    }
    host_http_reconcile_test_context_t context = {
        .server_free_failures = 1,
        .config_change_after_load = 2,
    };
    const mori_http_host_ops_t ops = {
        .context = &context,
        .config_load = host_http_test_config_load,
        .server_new = host_http_test_server_new,
        .server_configure = host_http_test_server_configure,
        .server_stop = host_http_test_server_stop,
        .server_free = host_http_test_server_free,
        .thread_start = host_http_test_thread_start,
        .thread_join = host_http_test_thread_join,
        .log_retry = host_http_log_retry,
        .log_no_assets = host_http_log_no_assets,
    };
    host_state_t host = {
        .http = {.assets_available = true},
        .http_ops = &ops,
    };

    host_http_reconcile_at(&host, 0, true);
    host_http_reconcile_at(&host, 1000, false);
    bool retained_after_refusal =
        host.http.server != NULL && !host.http.started && context.server_create_attempts == 1;
    size_t creates_after_refusal = context.server_create_attempts;
    host_http_reconcile_at(&host, 2000, false);
    bool replacement_active = host.http.server != NULL && host.http.started;

    *result_out = (cbm_daemon_host_http_free_refusal_test_result_t){
        .server_create_attempts_after_refusal = creates_after_refusal,
        .server_create_attempts = context.server_create_attempts,
        .thread_start_attempts = context.thread_start_attempts,
        .server_stops = context.server_stops,
        .server_free_attempts = context.server_frees,
        .thread_joins = context.thread_joins,
        .retained_after_refusal = retained_after_refusal,
        .replacement_active_after_retry = replacement_active,
    };
    return mori_http_host_dispose(&host.http, &ops, &host);
}

typedef struct {
    cbm_http_server_t *server;
    bool free_refused_while_scheduled;
} host_http_thread_create_failure_test_t;

/* WHY: the signature is fixed by the host_http_thread_create_fn type;
 * a const pointee would make the function-pointer types incompatible. */
static int host_http_thread_create_failure_for_test(
    void *opaque, cbm_thread_t *thread, void *(*entry)(void *),
    void *entry_context) { // cppcheck-suppress constParameterCallback
    (void)thread;
    (void)entry;
    host_http_thread_create_failure_test_t *test = opaque;
    if (!test || entry_context != test->server)
        return -1;
    test->free_refused_while_scheduled = !cbm_http_server_free(test->server);
    return -1;
}

bool cbm_daemon_host_http_thread_create_failure_lifecycle_for_test(void) {
    cbm_http_server_t *server = cbm_http_server_new(0);
    if (!server)
        return false;
    host_http_thread_create_failure_test_t test = {
        .server = server,
    };
    cbm_thread_t unused_thread = {0};
    int rc = host_http_thread_schedule_create(&unused_thread, host_http_thread, server,
                                              host_http_thread_create_failure_for_test, &test);
    bool free_refused = test.free_refused_while_scheduled;
    bool final_free_succeeded = free_refused && cbm_http_server_free(server);
    return rc != 0 && free_refused && final_free_succeeded;
}

static bool host_background_start(host_state_t *host) {
    /* No watcher object when watcher_enabled=false (#335) — nothing to run, and
     * the daemon must still come up with its remaining subsystems. */
    if (host->watcher) {
        host_named_watch_reload_at(host, cbm_now_ms());
        if (cbm_thread_create(&host->watcher_thread, 0, host_watcher_thread, host->watcher) != 0) {
            return false;
        }
        host->watcher_started = true;
    }

    host_http_reconcile_at(host, cbm_now_ms(), true);
    return true;
}

/* Coordination handles the host retains for the generation's whole lifetime. */
typedef struct {
    const cbm_daemon_ipc_endpoint_t *endpoint;
    cbm_version_cohort_lease_t *cohort_lease;
    cbm_version_cohort_daemon_claim_t *daemon_claim;
    cbm_daemon_ipc_participant_guard_t *participant_guard;
} host_coordination_t;

typedef struct {
    const char *lost;      /* first held file whose path no longer names it */
    const char *transient; /* first still-valid file that could not be refreshed */
} host_touch_result_t;

static void host_touch_note(host_touch_result_t *result, const char *file, int status) {
    if (status == 0 && !result->lost) {
        result->lost = file;
    } else if (status < 0 && !result->transient) {
        result->transient = file;
    }
}

static int host_cohort_touch_status(cbm_version_cohort_status_t status) {
    return status == CBM_VERSION_COHORT_OK ? 1 : (status == CBM_VERSION_COHORT_IO ? -1 : 0);
}

/* Refresh every long-held runtime file. A held lock whose path now names
 * another inode coordinates nothing: forked index workers read the daemon as
 * uncoordinated and a peer can start a second generation (#2178). Every file
 * is visited even after one fails so the rest keep being refreshed. */
static host_touch_result_t host_coordination_touch(const host_coordination_t *coordination,
                                                   cbm_daemon_runtime_service_t *service) {
    host_touch_result_t result = {0};
    host_touch_note(&result, "cohort_daemon_claim",
                    host_cohort_touch_status(
                        cbm_version_cohort_daemon_claim_touch(coordination->daemon_claim)));
    host_touch_note(
        &result, "cohort_lease",
        host_cohort_touch_status(cbm_version_cohort_lease_touch(coordination->cohort_lease)));
    host_touch_note(&result, "participant_guard",
                    cbm_daemon_ipc_participant_guard_touch(coordination->endpoint,
                                                           coordination->participant_guard));
    host_touch_note(&result, "listener", cbm_daemon_runtime_service_touch_listener(service));
    return result;
}

static bool host_wait_for_lifetime(cbm_daemon_runtime_service_t *service,
                                   atomic_int *stop_requested, host_state_t *host, bool permanent,
                                   const host_coordination_t *coordination) {
    uint64_t initial_deadline = cbm_now_ms() + HOST_INITIAL_CLIENT_TIMEOUT_MS;
    uint64_t stopping_deadline = 0;
    uint64_t next_touch = 0;
    bool touch_degraded = false;
    for (;;) {
        cbm_daemon_runtime_service_state_t state = cbm_daemon_runtime_service_state(service);
        if (state == CBM_DAEMON_RUNTIME_SERVICE_EXITED) {
            cbm_log_info("daemon.lifetime_end", "reason", "runtime_exited");
            return true;
        }
        if (state == CBM_DAEMON_RUNTIME_SERVICE_STOPPING) {
            if (stopping_deadline == 0) {
                stopping_deadline = cbm_now_ms() + HOST_RUNTIME_SHUTDOWN_MS;
            }
            if (cbm_now_ms() >= stopping_deadline) {
                return false;
            }
            (void)cbm_daemon_runtime_service_wait_exited(service, HOST_WAIT_TICK_MS);
            continue;
        }
        /* The nobody-ever-connected window latches on the monotonic admission
         * total, not the live client count: a short first session can begin
         * and end entirely between two polls of this loop, and a sampled
         * count then reads zero forever — stopping a daemon whose client is
         * mid-conversation between per-request connections. */
        bool admitted = cbm_daemon_runtime_service_clients_admitted_total(service) > 0;
        bool stop = stop_requested && atomic_load(stop_requested);
        /* A permanent generation (`daemon start`) skips the nobody-ever-
         * connected window — it exists precisely to idle ahead of clients.
         * Explicit stop requests (signals) are still honored. */
        if (stop || (!permanent && !admitted && cbm_now_ms() >= initial_deadline)) {
            cbm_log_info("daemon.lifetime_end", "reason",
                         stop ? "stop_requested" : "initial_window_expired");
            return cbm_daemon_runtime_service_stop(service, HOST_RUNTIME_SHUTDOWN_MS);
        }
        /* Permanent generations too: exiting lets the next client start a
         * coordinated generation, while re-claiming would race that client. */
        if (cbm_now_ms() >= next_touch) {
            next_touch = cbm_now_ms() + HOST_COORDINATION_TOUCH_MS;
            host_touch_result_t touched = host_coordination_touch(coordination, service);
            if (touched.lost) {
                cbm_log_warn("daemon.lifetime_end", "reason", "coordination_file_lost", "file",
                             touched.lost);
                return cbm_daemon_runtime_service_stop(service, HOST_RUNTIME_SHUTDOWN_MS);
            }
            /* A refresh failure on a still-valid file (read-only remount,
             * EMFILE) is not loss: keep serving, retry next tick, and log
             * only transitions so a persistent condition cannot flood. */
            if ((touched.transient != NULL) != touch_degraded) {
                touch_degraded = touched.transient != NULL;
                if (touch_degraded) {
                    cbm_log_warn("daemon.coordination_touch_failed", "file", touched.transient);
                } else {
                    cbm_log_info("daemon.coordination_touch_recovered");
                }
            }
        }
        uint64_t now_ms = cbm_now_ms();
        host_http_reconcile_at(host, now_ms, false);
        host_named_watch_reload_at(host, now_ms);
        /* Retire an ephemeral generation that lingered for cold-storm cohort
         * participants once they drain, or once its bounded linger elapses.
         * A no-op unless a cohort-participant hook armed a linger. */
        cbm_daemon_runtime_service_reconcile_lifetime(service);
        (void)cbm_daemon_runtime_service_wait_exited(service, HOST_WAIT_TICK_MS);
    }
}

bool cbm_daemon_host_wait_for_lifetime_for_test(cbm_daemon_runtime_service_t *service,
                                                const cbm_daemon_ipc_endpoint_t *endpoint,
                                                cbm_version_cohort_lease_t *cohort_lease,
                                                cbm_version_cohort_daemon_claim_t *daemon_claim,
                                                cbm_daemon_ipc_participant_guard_t *guard) {
    host_state_t host = {0};
    atomic_int stop_requested = 0;
    host_coordination_t coordination = {
        .endpoint = endpoint,
        .cohort_lease = cohort_lease,
        .daemon_claim = daemon_claim,
        .participant_guard = guard,
    };
    return host_wait_for_lifetime(service, &stop_requested, &host, false, &coordination);
}

static bool host_application_shutdown(host_state_t *host) {
    if (cbm_daemon_application_shutdown(host->application, HOST_APPLICATION_SHUTDOWN_MS)) {
        return true;
    }
    cbm_log_error("daemon.shutdown_timeout", "component", "operations");
    return false;
}

static bool host_runtime_stop_free(cbm_daemon_runtime_service_t *service) {
    if (cbm_daemon_runtime_service_state(service) != CBM_DAEMON_RUNTIME_SERVICE_EXITED) {
        if (!cbm_daemon_runtime_service_stop(service, HOST_RUNTIME_SHUTDOWN_MS)) {
            cbm_log_error("daemon.shutdown_timeout", "component", "runtime");
            return false;
        }
    }
    if (!cbm_daemon_runtime_service_free(service)) {
        cbm_log_error("daemon.cleanup_timeout", "component", "runtime");
        return false;
    }
    return true;
}

static _Noreturn void host_force_terminate(const char *component) {
    /* The operation-log sink flushes every record before returning. Do not run
     * ordinary teardown after the cooperative deadline: a callback thread may
     * still own application storage. Process termination releases native
     * locks, and supervised worker groups observe their daemon parent's death. */
    cbm_log_error("daemon.forced_shutdown", "component", component);
#ifdef _WIN32
    (void)TerminateProcess(GetCurrentProcess(), EXIT_FAILURE);
    abort();
#else
    _exit(EXIT_FAILURE);
#endif
}

_Noreturn void cbm_daemon_host_force_terminate_for_test(const char *component) {
    char conflict_log[HOST_PATH_CAP];
    cbm_ui_log_init();
    if (host_log_open(conflict_log)) {
        host_force_terminate(component ? component : "test");
    }
#ifdef _WIN32
    (void)TerminateProcess(GetCurrentProcess(), EXIT_FAILURE);
    abort();
#else
    _exit(EXIT_FAILURE);
#endif
}

int cbm_daemon_host_run(const cbm_daemon_host_config_t *config) {
    if (!config || !config->endpoint || !config->executable_path ||
        !config->identity.semantic_version || !config->identity.build_fingerprint) {
        return -1;
    }
    cbm_ui_log_init();
    char conflict_log[HOST_PATH_CAP];
    if (!host_log_open(conflict_log)) {
        (void)fprintf(stderr, "mori: daemon log path is not private or safe\n");
        return -1;
    }
    cbm_version_cohort_manager_t *cohort_manager = cbm_version_cohort_manager_new(config->endpoint);
    cbm_version_cohort_lease_t *cohort_lease = NULL;
    cbm_daemon_conflict_t cohort_conflict;
    uint64_t now_ms = cbm_now_ms();
    uint64_t cohort_deadline = now_ms > UINT64_MAX - HOST_INITIAL_CLIENT_TIMEOUT_MS
                                   ? UINT64_MAX
                                   : now_ms + HOST_INITIAL_CLIENT_TIMEOUT_MS;
    cbm_version_cohort_status_t cohort_status =
        cohort_manager
            ? cbm_version_cohort_acquire(cohort_manager, &config->identity, cohort_deadline,
                                         &cohort_lease, &cohort_conflict)
            : CBM_VERSION_COHORT_IO;
    if (cohort_status != CBM_VERSION_COHORT_OK) {
        cbm_log_error("daemon.start_failed", "component", "cohort");
        char message[CBM_DAEMON_CONFLICT_MESSAGE_SIZE];
        bool formatted = cohort_status == CBM_VERSION_COHORT_CONFLICT &&
                         cbm_daemon_conflict_format(&cohort_conflict, message, sizeof(message));
        if (cohort_status == CBM_VERSION_COHORT_CONFLICT) {
            (void)cbm_version_cohort_log_conflict(&cohort_conflict);
        }
        (void)fprintf(stderr, "mori: %s\n",
                      formatted ? message : "daemon exact-build admission failed");
        host_cohort_close(&cohort_lease, &cohort_manager);
        host_log_close();
        return -1;
    }
    cbm_daemon_ipc_participant_guard_t *participant_guard = NULL;
    if (cbm_daemon_ipc_participant_guard_try_join(config->endpoint, &participant_guard) != 1) {
        cbm_log_error("daemon.start_failed", "component", "participant");
        (void)fprintf(stderr, "mori: daemon participant admission failed\n");
        host_participant_guard_close(&participant_guard);
        host_cohort_close(&cohort_lease, &cohort_manager);
        host_log_close();
        return -1;
    }
    cbm_version_cohort_daemon_claim_t *daemon_claim = NULL;
    uint64_t claim_deadline = host_deadline_after(HOST_DAEMON_CLAIM_TIMEOUT_MS);
    cbm_version_cohort_status_t claim_status =
        cbm_version_cohort_daemon_claim_acquire(cohort_manager, &daemon_claim);
    while (claim_status == CBM_VERSION_COHORT_BUSY && cbm_now_ms() < claim_deadline) {
        /* The previous generation is releasing; retry until the claim is free
         * or the handoff window elapses (a persistent BUSY = a real conflict). */
        cbm_usleep(HOST_WAIT_TICK_MS * 1000);
        claim_status = cbm_version_cohort_daemon_claim_acquire(cohort_manager, &daemon_claim);
    }
    if (claim_status != CBM_VERSION_COHORT_OK) {
        cbm_log_error("daemon.start_failed", "component", "claim");
        host_participant_guard_close(&participant_guard);
        host_daemon_claim_close(&daemon_claim);
        host_cohort_close(&cohort_lease, &cohort_manager);
        host_log_close();
        return -1;
    }
    cbm_daemon_ipc_lifetime_reservation_t *lifetime_reservation = NULL;
    if (host_lifetime_reservation_acquire(config->endpoint, &lifetime_reservation) != 1) {
        cbm_log_error("daemon.start_failed", "component", "lifetime");
        cbm_daemon_ipc_lifetime_reservation_release(lifetime_reservation);
        host_participant_guard_close(&participant_guard);
        host_daemon_claim_close(&daemon_claim);
        host_cohort_close(&cohort_lease, &cohort_manager);
        host_log_close();
        return -1;
    }
    cbm_mem_init(cbm_mem_ram_fraction_for_total(cbm_system_info().total_ram));
    cbm_http_server_set_binary_path(config->executable_path);
    cbm_index_supervisor_mark_host();

    host_state_t host = {0};
    host.permanent = config->permanent;
    if (!host_state_prepare(&host, config->endpoint)) {
        cbm_log_error("daemon.start_failed", "component", "application");
        host_state_free(&host);
        host_log_close();
        cbm_daemon_ipc_lifetime_reservation_release(lifetime_reservation);
        host_participant_guard_close(&participant_guard);
        host_daemon_claim_close(&daemon_claim);
        host_cohort_close(&cohort_lease, &cohort_manager);
        return -1;
    }
    cbm_daemon_runtime_application_callbacks_t callbacks =
        cbm_daemon_application_runtime_callbacks(host.application);
    cbm_daemon_runtime_service_config_t runtime_config = {
        .endpoint = config->endpoint,
        .identity = config->identity,
        .conflict_log_path = conflict_log,
        .conflict_log_cap_bytes = HOST_CONFLICT_LOG_CAP,
        .max_clients = HOST_MAX_CLIENTS,
        .lease_timeout_ms = HOST_LEASE_TIMEOUT_MS,
        .request_timeout_ms = HOST_REQUEST_TIMEOUT_MS,
        .shutdown_timeout_ms = HOST_RUNTIME_SHUTDOWN_MS,
        .application = callbacks,
        .permanent = config->permanent,
    };
    cbm_daemon_runtime_service_t *service =
        cbm_daemon_runtime_service_start_reserved(&runtime_config, &lifetime_reservation);
    if (!service) {
        cbm_log_error("daemon.start_failed", "component", "runtime");
        /* Recorded while the lifetime reservation is still held: a client
         * that watches this generation vanish finds the cause already there
         * and never launches a doomed replacement. */
        host_start_failure_record(config->endpoint, "runtime");
    }
    cbm_daemon_ipc_lifetime_reservation_release(lifetime_reservation);
    lifetime_reservation = NULL;
    if (!service) {
        host_state_free(&host);
        host_log_close();
        host_participant_guard_close(&participant_guard);
        host_daemon_claim_close(&daemon_claim);
        host_cohort_close(&cohort_lease, &cohort_manager);
        return -1;
    }
    if (!host_background_start(&host)) {
        cbm_log_error("daemon.start_failed", "component", "background");
        if (!host_application_shutdown(&host)) {
            host_force_terminate("operations");
        }
        host_background_stop(&host);
        if (!host_background_join(&host)) {
            host_force_terminate("background");
        }
        if (!host_runtime_stop_free(service)) {
            host_force_terminate("runtime");
        }
        host_state_free(&host);
        host_log_close();
        host_participant_guard_close(&participant_guard);
        host_daemon_claim_close(&daemon_claim);
        host_cohort_close(&cohort_lease, &cohort_manager);
        return -1;
    }
    cbm_diag_start();

    char process_id[32];
    char memory_budget[32];
    char physical_job_limit[32];
    char worker_memory_budget[32];
    (void)snprintf(process_id, sizeof(process_id), "%llu",
                   (unsigned long long)host_current_process_id());
    (void)snprintf(memory_budget, sizeof(memory_budget), "%zu", cbm_mem_budget());
    (void)snprintf(physical_job_limit, sizeof(physical_job_limit), "%zu",
                   cbm_daemon_application_physical_job_limit(host.application));
    (void)snprintf(worker_memory_budget, sizeof(worker_memory_budget), "%zu",
                   cbm_daemon_application_worker_memory_budget_bytes(host.application));
    cbm_log_info("daemon.start", "version", config->identity.semantic_version, "pid", process_id,
                 "cache_fingerprint",
                 config->identity.cache_fingerprint ? config->identity.cache_fingerprint
                                                    : "unavailable",
                 "memory_budget_bytes", memory_budget, "physical_job_limit", physical_job_limit,
                 "worker_memory_budget_bytes", worker_memory_budget);
    host_coordination_t coordination = {
        .endpoint = config->endpoint,
        .cohort_lease = cohort_lease,
        .daemon_claim = daemon_claim,
        .participant_guard = participant_guard,
    };
    if (!host_wait_for_lifetime(service, config->stop_requested, &host, config->permanent,
                                &coordination)) {
        host_force_terminate("runtime");
    }

    /* Prevent any new UI/watcher operation, then cancel/reap every physical
     * job before those background loops and the daemon process disappear. */
    if (!host_application_shutdown(&host)) {
        host_force_terminate("operations");
    }
    host_background_stop(&host);
    if (!host_background_join(&host)) {
        host_force_terminate("background");
    }
    if (!host_runtime_stop_free(service)) {
        host_force_terminate("runtime");
    }
    host_state_free(&host);
    cbm_diag_stop();
    cbm_log_info("daemon.stop");
    host_log_close();
    /* Keep the exact-build lifetime lease through listener/lifetime teardown,
     * application destruction, diagnostics shutdown, the durable stop record,
     * participant release, and daemon-claim release. An activation transaction
     * may treat exclusive acquisition as proof that coordinated cleanup for
     * this generation has completed. */
    host_participant_guard_close(&participant_guard);
    host_daemon_claim_close(&daemon_claim);
    host_cohort_close(&cohort_lease, &cohort_manager);
    return 0;
}
