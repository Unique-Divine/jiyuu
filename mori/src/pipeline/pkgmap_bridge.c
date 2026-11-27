/* ABI for C layouts and yyjson's inline API used by the Rust package map. */
#include "discover/discover.h"
#include "foundation/compat.h"
#include "foundation/compat_fs.h"
#include "foundation/constants.h"
#include "foundation/platform.h"
#include "foundation/win_utf8.h"
#include "pipeline/pkgmap_bridge.h"
#include <sys/stat.h>

void *cbm_pkgmap_tracked_calloc(size_t size) {
    return cbm_calloc(CBM_MEM_CLASS_OTHER, size);
}

char *cbm_pkgmap_tracked_strdup(const char *value) {
    return cbm_mem_strdup(CBM_MEM_CLASS_OTHER, value);
}

void cbm_pkgmap_tracked_free(char *value) {
    cbm_free(CBM_MEM_CLASS_OTHER, value);
}

const char *cbm_pkgmap_file_at(const cbm_file_info_t *files, int index, const char **abs) {
    *abs = files[index].path;
    return files[index].rel_path;
}

yyjson_doc *cbm_pkgmap_json_read(const char *source, size_t len) {
    return yyjson_read(source, len, 0);
}

void cbm_pkgmap_json_free(yyjson_doc *doc) {
    yyjson_doc_free(doc);
}

const yyjson_val *cbm_pkgmap_json_root(const yyjson_doc *doc) {
    return yyjson_doc_get_root(doc);
}

const yyjson_val *cbm_pkgmap_json_get(const yyjson_val *object, const char *key) {
    return yyjson_obj_get(object, key);
}

const char *cbm_pkgmap_json_string(const yyjson_val *value) {
    return yyjson_get_str(value);
}

const yyjson_val *cbm_pkgmap_json_first_key(const yyjson_val *object, size_t *count) {
    yyjson_obj_iter iter = yyjson_obj_iter_with(object);
    *count = iter.max;
    return iter.cur;
}

const yyjson_val *cbm_pkgmap_json_next_pair(const yyjson_val **next) {
    const yyjson_val *value = yyjson_obj_iter_get_val((yyjson_val *)*next);
    *next = unsafe_yyjson_get_next(value);
    return value;
}

/* Read an entire file into a core-owned buffer (class extract; release with
 * cbm_free(CBM_MEM_CLASS_EXTRACT, ...)). Returns NULL on failure. */
char *cbm_pkgmap_read_manifest(const char *path, int *out_len) {
    FILE *f = cbm_fopen(path, "rb");
    if (!f) {
        return NULL;
    }
    (void)fseek(f, 0, SEEK_END);
    long size = ftell(f);
    (void)fseek(f, 0, SEEK_SET);
    if (size <= 0 || size > (long)CBM_SZ_1K * CBM_SZ_1K) { /* 1MB cap for manifests */
        (void)fclose(f);
        return NULL;
    }
    char *buf = (char *)cbm_alloc(CBM_MEM_CLASS_EXTRACT, (size_t)size + SKIP_ONE);
    if (!buf) {
        (void)fclose(f);
        return NULL;
    }
    size_t nread = fread(buf, SKIP_ONE, (size_t)size, f);
    (void)fclose(f);
    buf[nread] = '\0';
    *out_len = (int)nread;
    return buf;
}

void cbm_pkgmap_free_manifest(char *source) {
    cbm_free(CBM_MEM_CLASS_EXTRACT, source);
}

const char *cbm_pkgmap_dir_next(cbm_dir_t *dir) {
    const cbm_dirent_t *entry = cbm_readdir(dir);
    return entry ? entry->name : NULL;
}

int cbm_pkgmap_path_kind(const char *path) {
    enum { PKGMAP_SKIP_ENTRY = 0, PKGMAP_DIRECTORY = 1, PKGMAP_REGULAR_FILE = 2 };
#ifdef _WIN32
    /* The compatibility layer normalizes / in extended-length paths. */
    wchar_t *wide = cbm_path_to_wide_in(CBM_MEM_CLASS_OTHER, path);
    if (!wide) {
        return PKGMAP_SKIP_ENTRY;
    }
    struct _stat64 info;
    int rc = _wstat64(wide, &info);
    int kind = PKGMAP_SKIP_ENTRY;
    if (rc == 0 && S_ISDIR(info.st_mode)) {
        DWORD attributes = GetFileAttributesW(wide);
        if (attributes == INVALID_FILE_ATTRIBUTES || !(attributes & FILE_ATTRIBUTE_REPARSE_POINT)) {
            kind = PKGMAP_DIRECTORY;
        }
    } else if (rc == 0 && S_ISREG(info.st_mode)) {
        kind = PKGMAP_REGULAR_FILE;
    }
    cbm_free(CBM_MEM_CLASS_OTHER, wide);
    return kind;
#else
    cbm_path_info_t info;
    if (cbm_path_info_utf8(path, &info) != CBM_PATH_INFO_OK || info.is_symlink) {
        return PKGMAP_SKIP_ENTRY;
    }
    if (info.is_directory) {
        return PKGMAP_DIRECTORY;
    }
    return info.is_regular ? PKGMAP_REGULAR_FILE : PKGMAP_SKIP_ENTRY;
#endif
}

bool cbm_pkgmap_skip_dir(const char *name, const char *rel, char *const *excluded, int count) {
    return cbm_should_skip_dir(name, CBM_MODE_FULL) ||
           cbm_pipeline_relpath_is_excluded(rel, excluded, count);
}
