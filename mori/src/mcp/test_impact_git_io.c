#include "mcp/test_impact_git.h"
#include "mcp/test_impact.h"

#include "foundation/arena.h"
#include "foundation/compat.h"
#include "foundation/compat_fs.h"
#include "foundation/platform.h"
#include "foundation/sanitized.h"
#include "foundation/secure_random.h"
#include "foundation/subprocess.h"

#include <errno.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
#include <windows.h>
#include <aclapi.h>
#include "foundation/win_utf8.h"
#else
#include <fcntl.h>
#include <sys/stat.h>
#include <unistd.h>
#endif

enum {
    GF_ARENA_BLOCK = 8192,
    GF_PATH_CAP = 4096,
    GF_ARG_CAP = 96,
    GF_FLAG_CAP = 32,
    GF_INPUT_MAX = 65536,
    GF_POLL_US = 10000
};

typedef struct {
    size_t arena_limit, arena_used;
    size_t input_limit, input_used;
} gf_batch_budget_t;

typedef struct {
    const char *commit;
    cbm_git_tree_inventory_t inventory;
} gf_inventory_cache_t;

typedef struct {
    cbm_git_facts_identity_t identity;
    const char
        *command_git_dir; /* worktree directory while resolving refs, common directory later */
    const char *diff_flags[GF_FLAG_CAP];
    int diff_flag_count;
    const char *graft_paths[2];
    const char *shallow_paths[2];
    gf_inventory_cache_t inventory_cache[2];
} gf_projection_t;

struct cbm_git_facts {
    CBMArena arena;
    cbm_git_facts_options_t options;
    gf_projection_t projection;
    const char *git_pins[GF_FLAG_CAP];
    int git_pin_count;
    unsigned commands;
    size_t output_bytes;
    cbm_git_facts_error_t stopped;
    gf_batch_budget_t *batch_budget; /* borrowed stack state during one serialized call */
    bool no_lazy_fetch; /* the executable takes --no-lazy-fetch (gf_probe_no_lazy_fetch) */
};

typedef struct {
    cbm_git_bytes_t out;
    cbm_git_bytes_t err;
    int exit_code;
} gf_result_t;

typedef struct {
    char directory[GF_PATH_CAP];
    char out[GF_PATH_CAP];
    char err[GF_PATH_CAP];
    char in[GF_PATH_CAP];
} gf_capture_t;

static void gf_error_clear(cbm_git_facts_error_t *error) {
    memset(error, 0, sizeof(*error));
    error->exit_code = -1;
}

static bool gf_error(cbm_git_facts_error_t *error, cbm_git_facts_status_t status,
                     const char *message) {
    error->status = status;
    (void)snprintf(error->diagnostic, sizeof(error->diagnostic), "%s", message);
    return false;
}

/* Exhausting any query budget makes the handle terminal. In particular, an
 * observed over-cap capture cannot be retried without charging its bytes. */
static bool gf_limit(cbm_git_facts_t *facts, cbm_git_facts_error_t *error, const char *message) {
    gf_error(error, CBM_GIT_FACTS_LIMIT, message);
    facts->stopped = *error;
    return false;
}

static size_t gf_input_length(const char *text) {
    if (!text)
        return SIZE_MAX;
    size_t n = strnlen(text, (size_t)GF_INPUT_MAX + 1);
    return n <= GF_INPUT_MAX ? n : SIZE_MAX;
}

static bool gf_absolute(const char *path) {
    if (!path || !path[0])
        return false;
#ifdef _WIN32
    bool has_prefix = path[1] && path[2];
    bool drive = has_prefix &&
                 ((path[0] >= 'A' && path[0] <= 'Z') || (path[0] >= 'a' && path[0] <= 'z')) &&
                 path[1] == ':' && (path[2] == '/' || path[2] == '\\');
    bool unc = has_prefix && (path[0] == '/' || path[0] == '\\') &&
               (path[1] == '/' || path[1] == '\\') && path[2] != '/' && path[2] != '\\';
    return drive || unc;
#else
    return path[0] == '/';
#endif
}

static bool gf_native_executable(const char *path) {
    if (!gf_absolute(path))
        return false;
#ifdef _WIN32
    size_t n = strlen(path);
    return n >= 4 && path[n - 4] == '.' && (path[n - 3] == 'e' || path[n - 3] == 'E') &&
           (path[n - 2] == 'x' || path[n - 2] == 'X') && (path[n - 1] == 'e' || path[n - 1] == 'E');
#else
    return true; /* trusted internal resolution, not an arbitrary user program */
#endif
}

static bool gf_gate(cbm_git_facts_t *facts, cbm_git_facts_error_t *error) {
    if (facts->stopped.status != CBM_GIT_FACTS_OK) {
        *error = facts->stopped;
        return false;
    }
    if (facts->options.cancelled && facts->options.cancelled(facts->options.cancel_context)) {
        gf_error(error, CBM_GIT_FACTS_CANCELLED, "Git facts request cancelled");
        facts->stopped = *error;
        return false;
    }
    if (cbm_now_ms() >= facts->options.deadline_ms)
        return gf_error(error, CBM_GIT_FACTS_DEADLINE, "Git facts deadline expired");
    return true;
}

/* A batch charges logical requests before allocating, including allocations
 * in inherited guards. Legacy calls retain their original allocation policy. */
static void *gf_allocate(cbm_git_facts_t *facts, CBMArena *arena, size_t size,
                         cbm_git_facts_error_t *error) {
    gf_batch_budget_t *budget = facts->batch_budget;
    if (budget) {
        if (!gf_gate(facts, error))
            return NULL;
        if (size > budget->arena_limit - budget->arena_used) {
            gf_limit(facts, error, "Git batch arena limit exceeded");
            return NULL;
        }
        budget->arena_used += size;
    }
    void *out = cbm_arena_alloc(arena, size);
    if (!out) {
        gf_error(error, CBM_GIT_FACTS_OOM, "Cannot retain Git facts data");
        return NULL;
    }
    if (budget && !gf_gate(facts, error))
        return NULL;
    return out;
}

/* These are compile-time project constants containing simple space-separated
 * argv tokens, not user command text. No shell quoting or expansion is used. */
static bool gf_split_flags(cbm_git_facts_t *facts, const char *literal, const char **tokens,
                           int *count) {
    *count = 0;
    char *copy = cbm_arena_strdup(&facts->arena, literal);
    if (!copy)
        return false;
    char *p = copy;
    while (*p) {
        while (*p == ' ')
            p++;
        if (!*p)
            break;
        if (*count == GF_FLAG_CAP)
            return false;
        tokens[(*count)++] = p;
        while (*p && *p != ' ') {
            if (*p == '\t' || *p == '\n' || *p == '\r' || *p == '\'' || *p == '"')
                return false;
            p++;
        }
        if (*p)
            *p++ = '\0';
    }
    return true;
}

#ifdef _WIN32
/* Same owner-only, protected, inheritable ACL as compat.c's private helper,
 * but deliberately no inherited-security fallback. TOKEN_USER storage is
 * arena-owned; Windows owns the ACL returned by SetEntriesInAclW. */
static bool gf_private_directory(cbm_git_facts_t *facts, char path[GF_PATH_CAP],
                                 cbm_git_facts_error_t *error) {
    CBMArena scratch;
    cbm_arena_init_lazy(&scratch, 1024);
    HANDLE token = NULL;
    TOKEN_USER *user = NULL;
    PACL acl = NULL;
    DWORD needed = 0;
    bool created = false;
    const char *tmp = cbm_tmpdir();
    if (!gf_absolute(tmp)) {
        gf_error(error, CBM_GIT_FACTS_IO, "An absolute private temp location is required");
        goto done;
    }
    if (!OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &token) ||
        GetTokenInformation(token, TokenUser, NULL, 0, &needed) ||
        GetLastError() != ERROR_INSUFFICIENT_BUFFER || needed == 0 || needed > 65536) {
        gf_error(error, CBM_GIT_FACTS_IO, "Cannot establish private capture owner");
        goto done;
    }
    user = gf_allocate(facts, &scratch, needed, error);
    if (!user) {
        if (error->status == CBM_GIT_FACTS_OOM)
            gf_error(error, CBM_GIT_FACTS_OOM, "Cannot retain private capture owner");
        goto done;
    }
    if (!GetTokenInformation(token, TokenUser, user, needed, &needed) || !user->User.Sid ||
        !IsValidSid(user->User.Sid)) {
        gf_error(error, CBM_GIT_FACTS_IO, "Cannot establish private capture owner");
        goto done;
    }
    EXPLICIT_ACCESSW access;
    memset(&access, 0, sizeof(access));
    access.grfAccessPermissions = GENERIC_ALL;
    access.grfAccessMode = SET_ACCESS;
    access.grfInheritance = SUB_CONTAINERS_AND_OBJECTS_INHERIT;
    access.Trustee.TrusteeForm = TRUSTEE_IS_SID;
    access.Trustee.TrusteeType = TRUSTEE_IS_USER;
    access.Trustee.ptstrName = (LPWSTR)user->User.Sid;
    SECURITY_DESCRIPTOR descriptor;
    if (SetEntriesInAclW(1, &access, NULL, &acl) != ERROR_SUCCESS ||
        !InitializeSecurityDescriptor(&descriptor, SECURITY_DESCRIPTOR_REVISION) ||
        !SetSecurityDescriptorDacl(&descriptor, TRUE, acl, FALSE) ||
        !SetSecurityDescriptorOwner(&descriptor, user->User.Sid, FALSE) ||
        !SetSecurityDescriptorControl(&descriptor, SE_DACL_PROTECTED, SE_DACL_PROTECTED)) {
        gf_error(error, CBM_GIT_FACTS_IO, "Cannot establish private capture permissions");
        goto done;
    }
    SECURITY_ATTRIBUTES attributes = {sizeof(attributes), &descriptor, FALSE};
    static const char hex[] = "0123456789abcdef";
    for (int attempt = 0; attempt < 16; attempt++) {
        if (!gf_gate(facts, error))
            goto done;
        unsigned char random[16];
        char suffix[33];
        if (!cbm_secure_random(random, sizeof(random)))
            break;
        for (size_t i = 0; i < sizeof(random); i++) {
            suffix[i * 2] = hex[random[i] >> 4];
            suffix[i * 2 + 1] = hex[random[i] & 15];
        }
        suffix[32] = '\0';
        int n = snprintf(path, GF_PATH_CAP, "%s/cbm-git-facts-%s", tmp, suffix);
        if (n < 0 || n >= GF_PATH_CAP)
            break;
        wchar_t *wide = cbm_path_to_wide_in(CBM_MEM_CLASS_OTHER, path);
        if (!wide)
            break;
        created = CreateDirectoryW(wide, &attributes) != 0;
        DWORD why = GetLastError();
        cbm_free(CBM_MEM_CLASS_OTHER, wide);
        if (created || (why != ERROR_ALREADY_EXISTS && why != ERROR_FILE_EXISTS))
            break;
    }
    if (!created)
        gf_error(error, CBM_GIT_FACTS_IO, "Private capture directory creation failed");
done:
    if (acl)
        (void)LocalFree(acl);
    if (token)
        (void)CloseHandle(token);
    cbm_arena_destroy(&scratch);
    if (!created)
        path[0] = '\0';
    return created;
}
#else
static bool gf_private_directory(cbm_git_facts_t *facts, char path[GF_PATH_CAP],
                                 cbm_git_facts_error_t *error) {
    if (!gf_gate(facts, error))
        return false;
    int n = snprintf(path, GF_PATH_CAP, "%s/cbm-git-facts-XXXXXX", cbm_tmpdir());
    if (n < 0 || n >= GF_PATH_CAP || !cbm_mkdtemp(path)) {
        path[0] = '\0';
        return gf_error(error, CBM_GIT_FACTS_IO, "Private capture directory creation failed");
    }
    return true;
}
#endif

static bool gf_capture_cleanup(gf_capture_t *capture) {
    bool ok = true;
    if (capture->out[0] && cbm_unlink(capture->out) != 0 && errno != ENOENT)
        ok = false;
    if (capture->err[0] && cbm_unlink(capture->err) != 0 && errno != ENOENT)
        ok = false;
    if (capture->in[0] && cbm_unlink(capture->in) != 0 && errno != ENOENT)
        ok = false;
    if (capture->directory[0] && cbm_rmdir(capture->directory) != 0 && errno != ENOENT)
        ok = false;
    return ok;
}

static bool gf_capture_create(cbm_git_facts_t *facts, gf_capture_t *capture,
                              cbm_git_facts_error_t *error) {
    memset(capture, 0, sizeof(*capture));
    if (!gf_private_directory(facts, capture->directory, error))
        return false;
    int a = snprintf(capture->out, sizeof(capture->out), "%s/stdout", capture->directory);
    int b = snprintf(capture->err, sizeof(capture->err), "%s/stderr", capture->directory);
    if (a < 0 || b < 0 || (size_t)a >= sizeof(capture->out) || (size_t)b >= sizeof(capture->err)) {
        capture->out[0] = capture->err[0] = '\0';
        (void)gf_capture_cleanup(capture);
        return gf_error(error, CBM_GIT_FACTS_IO, "Capture path exceeds supported length");
    }
    return true;
}

/* The private writer is closed before spawn. Only canonical OID lines reach
 * this transport; the child opens its independent read-only binary handle. */
static bool gf_capture_input(cbm_git_facts_t *facts, gf_capture_t *capture,
                             const cbm_git_bytes_t *input, cbm_git_facts_error_t *error) {
    if (!input)
        return true;
    gf_batch_budget_t *budget = facts->batch_budget;
    if (!budget || (input->length && !input->data))
        return gf_error(error, CBM_GIT_FACTS_INVALID, "Invalid internal Git batch input");
    if (!gf_gate(facts, error))
        return false;
    if (input->length > budget->input_limit - budget->input_used)
        return gf_limit(facts, error, "Git batch stdin limit exceeded");
    int n = snprintf(capture->in, sizeof(capture->in), "%s/stdin", capture->directory);
    if (n < 0 || (size_t)n >= sizeof(capture->in)) {
        capture->in[0] = '\0';
        return gf_error(error, CBM_GIT_FACTS_IO, "Batch input path exceeds supported length");
    }
    FILE *file = cbm_fopen(capture->in, "wb");
    if (!file)
        return gf_error(error, CBM_GIT_FACTS_IO, "Cannot create private Git batch input");
    bool ok = true;
    size_t at = 0;
    while (at < input->length) {
        if (!gf_gate(facts, error)) {
            ok = false;
            break;
        }
        size_t chunk = input->length - at;
        if (chunk > 65536)
            chunk = 65536;
        size_t written = fwrite(input->data + at, 1, chunk, file);
        at += written;
        budget->input_used += written;
        if (written != chunk) {
            ok = gf_error(error, CBM_GIT_FACTS_IO, "Cannot write private Git batch input");
            break;
        }
    }
    if (fclose(file) != 0 && ok)
        ok = gf_error(error, CBM_GIT_FACTS_IO, "Cannot close private Git batch input");
    return ok && gf_gate(facts, error);
}

static bool gf_capture_sizes(cbm_git_facts_t *facts, const gf_capture_t *capture, size_t *out_size,
                             size_t *err_size, cbm_git_facts_error_t *error) {
    int64_t a = cbm_file_size(capture->out), b = cbm_file_size(capture->err);
    if (a < 0 || b < 0)
        return gf_error(error, CBM_GIT_FACTS_IO, "Cannot inspect captured output");
    if ((uint64_t)a > facts->options.stdout_limit || (uint64_t)b > facts->options.stderr_limit ||
        (uint64_t)a > SIZE_MAX - (uint64_t)b)
        return gf_limit(facts, error, "Git command output limit exceeded");
    size_t total = (size_t)a + (size_t)b;
    if (facts->output_bytes > facts->options.total_output_limit ||
        total > facts->options.total_output_limit - facts->output_bytes)
        return gf_limit(facts, error, "Git request output limit exceeded");
    if (out_size)
        *out_size = (size_t)a;
    if (err_size)
        *err_size = (size_t)b;
    return true;
}

static bool gf_read_capture(cbm_git_facts_t *facts, const char *path, size_t length,
                            cbm_git_bytes_t *out, cbm_git_facts_error_t *error) {
    *out = (cbm_git_bytes_t){0};
    if (length == SIZE_MAX)
        return gf_limit(facts, error, "Captured output is too large");
    unsigned char *bytes = gf_allocate(facts, &facts->arena, length + 1, error);
    if (!bytes) {
        if (error->status == CBM_GIT_FACTS_OOM)
            gf_error(error, CBM_GIT_FACTS_OOM, "Cannot retain captured output");
        return false;
    }
    FILE *file = cbm_fopen(path, "rb");
    if (!file)
        return gf_error(error, CBM_GIT_FACTS_IO, "Cannot read captured output");
    bool ok = true;
    size_t used = 0;
    while (used < length) {
        if (!gf_gate(facts, error)) {
            ok = false;
            break;
        }
        size_t chunk = length - used;
        if (chunk > 65536)
            chunk = 65536;
        size_t got = fread(bytes + used, 1, chunk, file);
        used += got;
        if (got != chunk) {
            ok = gf_error(error, CBM_GIT_FACTS_IO, "Captured output was truncated or unreadable");
            break;
        }
    }
    if (ok && (fgetc(file) != EOF || ferror(file)))
        ok = gf_error(error, CBM_GIT_FACTS_IO, "Captured output changed while being read");
    if (fclose(file) != 0 && ok)
        ok = gf_error(error, CBM_GIT_FACTS_IO, "Cannot close captured output");
    if (ok)
        ok = gf_gate(facts, error);
    if (!ok)
        return false;
    bytes[length] = '\0'; /* convenience sentinel; length remains authoritative */
    *out = (cbm_git_bytes_t){bytes, length};
    return true;
}

static void gf_command_diagnostic(cbm_git_facts_error_t *error, const gf_result_t *result) {
    error->exit_code = result->exit_code;
    size_t at = strlen(error->diagnostic);
    if (result->err.length && at + 2 < sizeof(error->diagnostic)) {
        error->diagnostic[at++] = ':';
        error->diagnostic[at++] = ' ';
    }
    for (size_t i = 0; i < result->err.length && at + 1 < sizeof(error->diagnostic); i++) {
        unsigned char c = result->err.data[i];
        error->diagnostic[at++] = c >= 32 && c < 127 ? (char)c : ' ';
    }
    error->diagnostic[at] = '\0';
}

/* Observed file-size limits request cancellation. They are not instantaneous
 * disk quotas; final retained bytes are bounded independently after quiescence. */
static bool gf_run_input(cbm_git_facts_t *facts, const char *const *arguments, int count,
                         bool allow_exit_one, const cbm_git_bytes_t *input, gf_result_t *out,
                         cbm_git_facts_error_t *error) {
    memset(out, 0, sizeof(*out));
    out->exit_code = -1;
    if (!gf_gate(facts, error))
        return false;
    if (facts->commands >= facts->options.command_limit)
        return gf_limit(facts, error, "Git command-count limit exceeded");
    const char *argv[GF_ARG_CAP];
    int argc = 0;
    const char *prefix[] = {facts->options.git_executable,
                            "--no-replace-objects",
                            "--no-pager",
                            "--literal-pathspecs",
                            "-C",
                            facts->projection.identity.root ? facts->projection.identity.root
                                                            : facts->options.root,
                            "-c",
                            "protocol.allow=never",
                            "-c",
                            "core.fsmonitor=false"};
    size_t prefix_count = sizeof(prefix) / sizeof(prefix[0]);
    size_t directory_count = facts->projection.command_git_dir ? 4 : 0;
    if (count < 1 || count >= GF_ARG_CAP ||
        (size_t)count + prefix_count + directory_count + (size_t)facts->git_pin_count + 2 >
            GF_ARG_CAP)
        return gf_error(error, CBM_GIT_FACTS_INVALID, "Invalid internal Git command");
    for (size_t i = 0; i < prefix_count; i++)
        argv[argc++] = prefix[i];
    if (facts->no_lazy_fetch)
        argv[argc++] = "--no-lazy-fetch";
    if (facts->projection.command_git_dir) {
        argv[argc++] = "--git-dir";
        argv[argc++] = facts->projection.command_git_dir;
        argv[argc++] = "--work-tree";
        argv[argc++] = facts->projection.identity.root;
    }
    for (int i = 0; i < facts->git_pin_count; i++)
        argv[argc++] = facts->git_pins[i];
    for (int i = 0; i < count; i++)
        argv[argc++] = arguments[i];
    argv[argc] = NULL;
    gf_capture_t capture;
    if (!gf_capture_create(facts, &capture, error))
        return false;
    bool ok = gf_gate(facts, error) && gf_capture_input(facts, &capture, input, error);
    cbm_subprocess_t *process = NULL;
    cbm_proc_result_t result = {0};
    if (ok) {
        cbm_proc_opts_t options = {.bin = facts->options.git_executable,
                                   .argv = argv,
                                   .log_file = capture.err,
                                   .quiet_timeout_ms = 0,
                                   .cancel_grace_ms = CBM_SUBPROCESS_DEFAULT_CANCEL_GRACE_MS,
                                   .strip_git_repo_env = true,
                                   .stdout_file = capture.out,
                                   .stdin_file = input ? capture.in : NULL};
        facts->commands++;
        if (cbm_subprocess_spawn(&options, &process) != 0)
            ok = gf_error(error, CBM_GIT_FACTS_SPAWN, "Cannot spawn native Git process");
    }
    if (process) {
        for (;;) {
            if (ok)
                ok = gf_gate(facts, error) && gf_capture_sizes(facts, &capture, NULL, NULL, error);
            if (!ok)
                (void)cbm_subprocess_request_cancel(process);
            cbm_proc_poll_t polled = cbm_subprocess_poll(process, &result);
            if (polled == CBM_PROC_POLL_TERMINAL)
                break;
            if (polled == CBM_PROC_POLL_ERROR && ok)
                ok = gf_error(error, CBM_GIT_FACTS_SUPERVISION, "Git process supervision failed");
            /* Valid owned handles never have an ERROR transition. Continue
             * driving cancellation rather than abandon a possibly live tree. */
            cbm_usleep(GF_POLL_US);
        }
        cbm_subprocess_destroy(process);
        if (!result.tree_quiesced || result.supervision_failed) {
            gf_error(error, CBM_GIT_FACTS_SUPERVISION, "Git process tree did not quiesce");
            (void)snprintf(error->diagnostic, sizeof(error->diagnostic),
                           "Git containment failed; retained capture directory: %.400s",
                           capture.directory);
            facts->stopped = *error;
            return false; /* do not claim files with possible writers were cleaned */
        }
        if (ok && (result.cancellation_requested || result.forced))
            ok = gf_error(error, CBM_GIT_FACTS_CANCELLED, "Git process was cancelled");
    }
    size_t stdout_size = 0, stderr_size = 0;
    if (ok)
        ok = gf_gate(facts, error) &&
             gf_capture_sizes(facts, &capture, &stdout_size, &stderr_size, error);
    gf_result_t captured = {.exit_code = result.exit_code};
    if (ok) {
        facts->output_bytes += stdout_size + stderr_size;
        ok = gf_read_capture(facts, capture.err, stderr_size, &captured.err, error) &&
             gf_read_capture(facts, capture.out, stdout_size, &captured.out, error);
    }
    /* New batches classify a known command failure before cleanup. Cleanup
     * must not replace an established error; legacy calls keep their ordering. */
    if (ok && facts->batch_budget && !(result.outcome == CBM_PROC_CLEAN && result.exit_code == 0) &&
        !(allow_exit_one && result.outcome == CBM_PROC_EXIT_NONZERO && result.exit_code == 1)) {
        ok = gf_error(error, CBM_GIT_FACTS_COMMAND, "Git command failed");
        gf_command_diagnostic(error, &captured);
    }
    bool cleaned = gf_capture_cleanup(&capture);
    if (!cleaned && ok) {
        ok = gf_error(error, CBM_GIT_FACTS_IO, "Cannot clean captured Git output");
        (void)snprintf(error->diagnostic, sizeof(error->diagnostic),
                       "Cannot clean retained capture directory: %.400s", capture.directory);
    }
    if (ok)
        ok = gf_gate(facts, error);
    if (!ok)
        return false;
    bool success = result.outcome == CBM_PROC_CLEAN && result.exit_code == 0;
    bool one = allow_exit_one && result.outcome == CBM_PROC_EXIT_NONZERO && result.exit_code == 1;
    if (!success && !one) {
        gf_error(error, CBM_GIT_FACTS_COMMAND, "Git command failed");
        gf_command_diagnostic(error, &captured);
        return false;
    }
    *out = captured;
    return true;
}

#ifdef _WIN32
/* MSYS2 and Cygwin git print absolute paths in their own namespace:
 * /c/Users/x (Cygwin: /cygdrive/c/Users/x). The drive form maps one to one
 * onto C:/Users/x, which the native file checks below need. Any other path of
 * that namespace (/tmp, /usr) needs the runtime's mount table, so it stays
 * unmapped and is refused as not absolute. */
static const char *gf_native_metadata_path(cbm_git_facts_t *facts, const char *path) {
    const char *drive = strncmp(path, "/cygdrive/", 10) == 0 ? path + 9 : path;
    if (drive[0] != '/' || !drive[1])
        return path;
    bool letter = (drive[1] >= 'A' && drive[1] <= 'Z') || (drive[1] >= 'a' && drive[1] <= 'z');
    if (!letter || (drive[2] != '/' && drive[2] != '\0'))
        return path;
    char upper = (char)(drive[1] >= 'a' ? drive[1] - 'a' + 'A' : drive[1]);
    return cbm_arena_sprintf(&facts->arena, "%c:/%s", upper, drive[2] ? drive + 3 : "");
}
#endif

/* --no-replace-objects is not treated as a promise about legacy info/grafts.
 * Check graft and shallow state in both per-worktree and common Git locations.
 * No graft parsing or environment mutation; uncertain files block proof. */
static bool gf_history_file_absent_or_empty(const char *path, cbm_git_facts_error_t *error) {
#ifdef _WIN32
    wchar_t *wide = cbm_path_to_wide_in(CBM_MEM_CLASS_OTHER, path);
    if (!wide)
        return gf_error(error, CBM_GIT_FACTS_IO, "Cannot inspect Git history override path");
    HANDLE file =
        CreateFileW(wide, GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    NULL, OPEN_EXISTING, FILE_FLAG_OPEN_REPARSE_POINT, NULL);
    DWORD open_error = GetLastError();
    cbm_free(CBM_MEM_CLASS_OTHER, wide);
    if (file == INVALID_HANDLE_VALUE) {
        if (open_error == ERROR_FILE_NOT_FOUND || open_error == ERROR_PATH_NOT_FOUND)
            return true;
        return gf_error(error, CBM_GIT_FACTS_IO, "Cannot inspect Git history override state");
    }
    BY_HANDLE_FILE_INFORMATION info;
    bool ok =
        GetFileType(file) == FILE_TYPE_DISK && GetFileInformationByHandle(file, &info) != 0 &&
        (info.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)) == 0 &&
        info.nFileSizeHigh == 0 && info.nFileSizeLow == 0;
    bool closed = CloseHandle(file) != 0;
#else
    int flags = O_RDONLY | O_NONBLOCK;
#ifdef O_CLOEXEC
    flags |= O_CLOEXEC;
#endif
#ifdef O_NOFOLLOW
    flags |= O_NOFOLLOW;
#endif
    int file = open(path, flags);
    if (file < 0) {
        if (errno == ENOENT)
            return true;
        return gf_error(error, CBM_GIT_FACTS_IO, "Cannot inspect Git history override state");
    }
    struct stat info;
    bool ok = fstat(file, &info) == 0 && S_ISREG(info.st_mode) && info.st_size == 0;
    bool closed = close(file) == 0;
#endif
    if (!ok)
        return gf_error(error, CBM_GIT_FACTS_UNSUPPORTED,
                        "Graft or shallow state prevents history certification");
    return closed ||
           gf_error(error, CBM_GIT_FACTS_IO, "Cannot close Git history override inspection");
}

/* The native bridge owns allocation, command supervision and platform paths.
 * Rust owns the query plan and reads/writes the projection state by this ABI. */
extern bool mori_git_valid_oid(const char *oid, size_t length);
extern bool mori_git_open_identity(cbm_git_facts_t *facts, const char *base_ref,
                                   const char *expected_head, cbm_git_facts_error_t *error);

gf_projection_t *mori_git_io_projection(cbm_git_facts_t *facts);
bool mori_git_io_gate(cbm_git_facts_t *facts, cbm_git_facts_error_t *error);
bool mori_git_io_limit(cbm_git_facts_t *facts, cbm_git_facts_error_t *error, const char *message);
bool mori_git_io_run(cbm_git_facts_t *facts, const char *const *args, int count, bool allow_one,
                     const cbm_git_bytes_t *input, gf_result_t *out, cbm_git_facts_error_t *error);
void *mori_git_io_allocate(cbm_git_facts_t *facts, size_t size, cbm_git_facts_error_t *error);
const char *mori_git_io_metadata_path(cbm_git_facts_t *facts, const char *path,
                                      cbm_git_facts_error_t *error);
bool mori_git_io_history_file(const char *path, cbm_git_facts_error_t *error);
size_t mori_git_io_output_budget(const cbm_git_facts_t *facts, size_t *remaining);
bool mori_git_io_begin_batch(cbm_git_facts_t *facts, gf_batch_budget_t *budget);
void mori_git_io_end_batch(cbm_git_facts_t *facts, const cbm_git_facts_error_t *error);
void mori_git_io_msan_written(const void *address, size_t size);

void mori_git_io_msan_written(const void *address, size_t size) {
    (void)address;
    (void)size;
    CBM_MSAN_WRITTEN(address, size);
}

gf_projection_t *mori_git_io_projection(cbm_git_facts_t *facts) {
    return &facts->projection;
}

bool mori_git_io_gate(cbm_git_facts_t *facts, cbm_git_facts_error_t *error) {
    return gf_gate(facts, error);
}

bool mori_git_io_limit(cbm_git_facts_t *facts, cbm_git_facts_error_t *error, const char *message) {
    return gf_limit(facts, error, message);
}

bool mori_git_io_run(cbm_git_facts_t *facts, const char *const *args, int count, bool allow_one,
                     const cbm_git_bytes_t *input, gf_result_t *out, cbm_git_facts_error_t *error) {
    return gf_run_input(facts, args, count, allow_one, input, out, error);
}

void *mori_git_io_allocate(cbm_git_facts_t *facts, size_t size, cbm_git_facts_error_t *error) {
    return gf_allocate(facts, &facts->arena, size, error);
}

const char *mori_git_io_metadata_path(cbm_git_facts_t *facts, const char *path,
                                      cbm_git_facts_error_t *error) {
#ifdef _WIN32
    path = gf_native_metadata_path(facts, path);
    if (!path) {
        gf_error(error, CBM_GIT_FACTS_OOM, "Cannot retain Git metadata path");
        return NULL;
    }
#else
    (void)facts;
#endif
    if (!gf_absolute(path)) {
        gf_error(error, CBM_GIT_FACTS_COMMAND, "Git metadata path is not absolute");
        return NULL;
    }
    return path;
}

bool mori_git_io_history_file(const char *path, cbm_git_facts_error_t *error) {
    return gf_history_file_absent_or_empty(path, error);
}

size_t mori_git_io_output_budget(const cbm_git_facts_t *facts, size_t *remaining) {
    *remaining = facts->options.total_output_limit - facts->output_bytes;
    return facts->options.stdout_limit;
}

bool mori_git_io_begin_batch(cbm_git_facts_t *facts, gf_batch_budget_t *budget) {
    if (facts->batch_budget)
        return false;
    facts->batch_budget = budget;
    return true;
}

void mori_git_io_end_batch(cbm_git_facts_t *facts, const cbm_git_facts_error_t *error) {
    facts->batch_budget = NULL;
    if (error->status == CBM_GIT_FACTS_CANCELLED)
        facts->stopped = *error;
}

/* --no-lazy-fetch (git 2.44) keeps an object missing from a partial clone
 * from being fetched. Older git rejects the option, which failed every engine
 * command (Ubuntu 24.04 ships git 2.43). There `-c protocol.allow=never`
 * already turns such a fetch into an error, which is what the option asks
 * for, so it is passed only when the executable accepts it: one probe per
 * facts session, outside the command budget, output discarded. */
static bool gf_probe_no_lazy_fetch(const char *git) {
    const char *argv[] = {git, "--no-lazy-fetch", "--version", NULL};
    cbm_proc_opts_t options = {.bin = git,
                               .argv = argv,
                               .cancel_grace_ms = CBM_SUBPROCESS_DEFAULT_CANCEL_GRACE_MS,
                               .strip_git_repo_env = true};
    cbm_proc_result_t result = {0};
    return cbm_subprocess_run(&options, &result) == 0 && result.outcome == CBM_PROC_CLEAN;
}

cbm_git_facts_t *cbm_git_facts_open(const cbm_git_facts_options_t *options,
                                    cbm_git_facts_error_t *error) {
    cbm_git_facts_error_t local;
    if (!error)
        error = &local;
    gf_error_clear(error);
    if (!options || gf_input_length(options->root) == SIZE_MAX ||
        gf_input_length(options->base_ref) == SIZE_MAX ||
        gf_input_length(options->git_executable) == SIZE_MAX ||
        (options->expected_head && gf_input_length(options->expected_head) == SIZE_MAX) ||
        !gf_absolute(options->root) || !gf_native_executable(options->git_executable) ||
        !options->base_ref[0] || options->base_ref[0] == '-' || !options->deadline_ms ||
        !options->command_limit || !options->stdout_limit || !options->stderr_limit ||
        !options->total_output_limit) {
        gf_error(error, CBM_GIT_FACTS_INVALID,
                 "Invalid Git facts options or missing finite limits");
        return NULL;
    }
    if (options->expected_head &&
        !mori_git_valid_oid(options->expected_head, strlen(options->expected_head))) {
        gf_error(error, CBM_GIT_FACTS_INVALID, "Expected HEAD must be a full object ID");
        return NULL;
    }
    CBMArena arena;
    cbm_arena_init_sized(&arena, GF_ARENA_BLOCK);
    cbm_git_facts_t *facts = cbm_arena_calloc(&arena, sizeof(*facts));
    if (!facts) {
        cbm_arena_destroy(&arena);
        gf_error(error, CBM_GIT_FACTS_OOM, "Cannot allocate Git facts");
        return NULL;
    }
    facts->arena = arena;
    facts->options = *options;
    facts->options.root = cbm_arena_strdup(&facts->arena, options->root);
    facts->options.base_ref = cbm_arena_strdup(&facts->arena, options->base_ref);
    facts->options.git_executable = cbm_arena_strdup(&facts->arena, options->git_executable);
    facts->options.expected_head =
        options->expected_head ? cbm_arena_strdup(&facts->arena, options->expected_head) : NULL;
    if (!facts->options.root || !facts->options.base_ref || !facts->options.git_executable ||
        (options->expected_head && !facts->options.expected_head) ||
        !gf_split_flags(facts, CBM_TEST_IMPACT_GIT_PINS, facts->git_pins, &facts->git_pin_count) ||
        !gf_split_flags(facts, CBM_TEST_IMPACT_DIFF_FLAGS, facts->projection.diff_flags,
                        &facts->projection.diff_flag_count)) {
        gf_error(error, CBM_GIT_FACTS_OOM, "Cannot retain Git facts options");
        cbm_git_facts_free(facts);
        return NULL;
    }
    facts->no_lazy_fetch = gf_probe_no_lazy_fetch(facts->options.git_executable);
    if (!mori_git_open_identity(facts, facts->options.base_ref, facts->options.expected_head, error)) {
        cbm_git_facts_free(facts);
        return NULL;
    }
    return facts;
}

void cbm_git_facts_free(cbm_git_facts_t *facts) {
    if (facts) {
        CBMArena arena = facts->arena;
        cbm_arena_destroy(&arena);
    }
}

const cbm_git_facts_identity_t *cbm_git_facts_identity(const cbm_git_facts_t *facts) {
    return facts ? &facts->projection.identity : NULL;
}
