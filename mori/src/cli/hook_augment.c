/* Platform I/O and yyjson accessors for the Rust hook augmenter. */
#include "cli/cli.h"
#include "foundation/compat.h"
#include "foundation/compat_fs.h"
#include "foundation/constants.h"
#include "foundation/mem.h"
#include "foundation/platform.h"
#include "yyjson/yyjson.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifndef _WIN32
#include <fcntl.h>
#include <signal.h>
#include <sys/time.h>
#include <unistd.h>
#else
#include <direct.h>
#include <windows.h>
#endif

#define HA_STDIN_CAP (256 * 1024) /* hook payloads are tiny; cap defensively */

/* ── Hard deadline ────────────────────────────────────────────────
 * A slow SQLite open or query must never stall the agent. When the timer
 * fires we _exit(0) immediately. Output is written exactly once at the very
 * end, so firing mid-work simply yields a clean no-op (no partial JSON).
 *
 * Observability (#858): a fired deadline is otherwise indistinguishable from
 * "no matches", so the handler first write()s a pre-formatted breadcrumb to
 * <cache_dir>/logs/hook-augment-timeouts.log (by default
 * ~/.cache/codebase-memory-mcp/logs) and to stderr (fd and message prepared at
 * arm time — only async-signal-safe write/_exit in the handler). */
#ifndef _WIN32
/* Rust owns the POSIX deadline settings. The timer and signal handler
 * stay native so the handler can only call async-signal-safe operations. */
extern int mori_hook_deadline_ms(void);

int cbm_hook_augment_deadline_ms_for_testing(void) {
    return mori_hook_deadline_ms();
}

static int g_ha_crumb_fd = -1;
static char g_ha_crumb_msg[160];
static size_t g_ha_crumb_len = 0;

static void ha_deadline_exit(int sig) {
    (void)sig;
    /* async-signal-safe only: write() the prepared breadcrumb to the timeouts
     * log (when it opened) and ALWAYS to stderr, so a fired deadline is a
     * visible event, never a silent 0-byte exit(0). */
    if (g_ha_crumb_len > 0) {
        if (g_ha_crumb_fd >= 0) {
            ssize_t w = write(g_ha_crumb_fd, g_ha_crumb_msg, g_ha_crumb_len);
            (void)w;
        }
        ssize_t e = write(STDERR_FILENO, g_ha_crumb_msg, g_ha_crumb_len);
        (void)e;
    }
    _exit(0);
}

/* Whether snprintf wrote the whole string into a buffer of `cap` bytes. */
static bool ha_fits(int written, size_t cap) {
    return written > 0 && (size_t)written < cap;
}

static void ha_open_crumb_log(int deadline_ms) {
    /* Pre-format the breadcrumb unconditionally: the handler may only write() a
     * prepared buffer, and formatting it before the log path is resolved means
     * stderr still gets the line even when the file cannot be opened (no HOME,
     * unwritable cache dir) — the "never silent" guarantee holds regardless. */
    int n = snprintf(g_ha_crumb_msg, sizeof(g_ha_crumb_msg),
                     "hook-augment: deadline_exceeded ms=%d pid=%ld (raise via "
                     "CBM_HOOK_DEADLINE_MS)\n",
                     deadline_ms, (long)getpid());
    g_ha_crumb_len = (n > 0 && n < (int)sizeof(g_ha_crumb_msg)) ? (size_t)n : 0;

    const char *override = getenv("CBM_HOOK_TIMEOUT_LOG"); /* tests + power users */
    char path[CBM_SZ_1K];
    /* A path cut off at the buffer names some other file, and O_CREAT would
     * create it. Such a path means no breadcrumb; the hook itself runs on. */
    if (override && override[0]) {
        if (!ha_fits(snprintf(path, sizeof(path), "%s", override), sizeof(path))) {
            return;
        }
    } else {
        /* <cache_dir>/logs, where every cbm component logs: the cache dir is
         * CBM_CACHE_DIR, else HOME (then USERPROFILE) + the default. */
        const char *cache = cbm_resolve_cache_dir();
        char dir[CBM_SZ_1K];
        if (!cache || !ha_fits(snprintf(dir, sizeof(dir), "%s/logs", cache), sizeof(dir)) ||
            !ha_fits(snprintf(path, sizeof(path), "%s/hook-augment-timeouts.log", dir),
                     sizeof(path))) {
            return;
        }
        cbm_mkdir_p_ex(dir, 0755, CBM_MKDIR_FOLLOW_OWNED);
    }
    /* Startup may re-arm after resolving saved settings. Keep the original
     * diagnostic descriptor; the deadline text above is refreshed on every arm. */
    if (g_ha_crumb_fd < 0) {
        g_ha_crumb_fd = open(path, O_WRONLY | O_CREAT | O_APPEND, 0644);
    }
}

void cbm_hook_augment_arm_deadline(void) {
    int ms = mori_hook_deadline_ms();
    ha_open_crumb_log(ms);

    struct sigaction sa;
    memset(&sa, 0, sizeof(sa));
    sa.sa_handler = ha_deadline_exit;
    sigaction(SIGALRM, &sa, NULL);

    struct itimerval it;
    memset(&it, 0, sizeof(it));
    it.it_value.tv_sec = ms / 1000;
    it.it_value.tv_usec = (ms % 1000) * 1000;
    setitimer(ITIMER_REAL, &it, NULL);
}
#else
#define HA_WINDOWS_DEADLINE_MS 300

static VOID CALLBACK ha_deadline_exit_windows(PVOID context, BOOLEAN fired) {
    (void)context;
    (void)fired;
    ExitProcess(0U);
}

void cbm_hook_augment_arm_deadline(void) {
    HANDLE timer = NULL;
    (void)CreateTimerQueueTimer(&timer, NULL, ha_deadline_exit_windows, NULL, HA_WINDOWS_DEADLINE_MS, 0U,
                                WT_EXECUTEONLYONCE);
}
#endif

/* ── stdin ────────────────────────────────────────────────────────── */

/* main() reads stdin EARLY on the hook-client path so the no-op gate can run
 * before executable-identity hashing; the consumed bytes are handed back here
 * so every downstream reader sees them exactly once. */
static char *g_ha_prefetched_stdin = NULL;

void cbm_hook_augment_prefetch_stdin(char *owned) {
    free(g_ha_prefetched_stdin);
    g_ha_prefetched_stdin = owned;
}

char *cbm_hook_augment_read_stdin(void) {
    if (g_ha_prefetched_stdin) {
        char *out = g_ha_prefetched_stdin;
        g_ha_prefetched_stdin = NULL;
        return out;
    }
    char *buf = malloc(HA_STDIN_CAP + 1);
    if (!buf) {
        return NULL;
    }
    size_t total = 0;
    size_t n;
    while (total < HA_STDIN_CAP && (n = fread(buf + total, 1, HA_STDIN_CAP - total, stdin)) > 0) {
        total += n;
    }
    buf[total] = '\0';
    return buf;
}


/* Use the same CRT and allocation route as the native callers. */
char *mori_hook_alloc(size_t size) {
    return malloc(size);
}

void mori_hook_free(void *ptr) {
    free(ptr);
}

void mori_hook_write(const char *output) {
    fputs(output, stdout);
}

bool mori_hook_getcwd(char *buffer, size_t cap) {
#ifdef _WIN32
    return _getcwd(buffer, (int)cap) != NULL;
#else
    return getcwd(buffer, cap) != NULL;
#endif
}

bool mori_hook_fs_resolve(const char *input, char *buffer, size_t cap) {
#ifdef _WIN32
    return _fullpath(buffer, input, cap) != NULL;
#else
    (void)cap;
    return realpath(input, buffer) != NULL;
#endif
}

/* yyjson owns the parsed document. Values borrow its storage. Keeping the
 * codec also preserves first-key lookup and embedded-NUL behavior. */
yyjson_doc *mori_hook_json_read(const char *bytes, size_t size) {
    return yyjson_read(bytes, size, 0);
}

void mori_hook_json_free(yyjson_doc *doc) {
    yyjson_doc_free(doc);
}

yyjson_val *mori_hook_json_root(const yyjson_doc *doc) {
    return yyjson_doc_get_root(doc);
}

bool mori_hook_json_is_object(const yyjson_val *val) {
    return yyjson_is_obj(val);
}

yyjson_val *mori_hook_json_get(const yyjson_val *val, const char *key) {
    return yyjson_obj_get(val, key);
}

yyjson_val *mori_hook_json_at(const yyjson_val *val, size_t index) {
    return yyjson_arr_get(val, index);
}

size_t mori_hook_json_array_len(const yyjson_val *val) {
    return yyjson_arr_size(val);
}

bool mori_hook_json_is_array(const yyjson_val *val) {
    return yyjson_is_arr(val);
}

const char *mori_hook_json_string(const yyjson_val *val, size_t *len) {
    *len = yyjson_get_len(val);
    return yyjson_get_str(val);
}

int mori_hook_json_bool(const yyjson_val *val) {
    return yyjson_is_bool(val) ? yyjson_is_true(val) : -1;
}

bool mori_hook_json_int(const yyjson_val *val, int64_t *out) {
    if (!yyjson_is_int(val)) {
        return false;
    }
    *out = yyjson_get_sint(val);
    return true;
}

yyjson_val *mori_hook_json_next(const yyjson_val *val) {
    return unsafe_yyjson_get_next(val);
}
