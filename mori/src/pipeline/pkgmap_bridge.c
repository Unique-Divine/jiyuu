/* ABI for C layouts and yyjson's inline API used by the Rust package map. */
#include "discover/discover.h"
#include "foundation/compat.h"
#include "foundation/compat_fs.h"
#include "foundation/constants.h"
#include "foundation/platform.h"
#include "foundation/win_utf8.h"
#include "native_tree_ffi.h"
#include "pipeline/pkgmap_bridge.h"
#include <sys/stat.h>

void *cbm_pkgmap_tracked_calloc(size_t size) {
    return cbm_calloc(CBM_MEM_CLASS_OTHER, size);
}

void *cbm_pkgmap_binding_scratch_alloc(size_t size) {
    return cbm_alloc(CBM_MEM_CLASS_OTHER, size);
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

int cbm_pkgmap_language(const char *name) {
    switch (cbm_language_for_filename(name)) {
    case CBM_LANG_GO:
        return 1;
    case CBM_LANG_PYTHON:
        return 2;
    case CBM_LANG_RUST:
        return 3;
    case CBM_LANG_PHP:
        return 4;
    case CBM_LANG_KUSTOMIZE:
        return 5;
    default:
        return 0;
    }
}

const cbm_gbuf_t *cbm_pkgmap_context(const cbm_pipeline_ctx_t *ctx, const char **project,
                                     const cbm_path_alias_collection_t **aliases) {
    *project = ctx ? ctx->project_name : NULL;
    *aliases = ctx ? ctx->path_aliases : NULL;
    return ctx ? ctx->gbuf : NULL;
}

const char *cbm_pkgmap_namespace(const CBMFileResult *result) {
    return result ? result->namespace_name : NULL;
}

const CBMImportArray *cbm_pkgmap_imports(const CBMFileResult *result) {
    return &result->imports;
}

const TSTree *cbm_pkgmap_site_source(const CBMFileResult *result, const char **source, int *len) {
    *source = result->source;
    *len = result->source_len;
    return result->cached_tree;
}

bool cbm_pkgmap_node_at(const TSTree *tree, uint32_t start, uint32_t end, TSNode *out) {
    *out = ts_node_descendant_for_byte_range(ts_tree_root_node(tree), start, end);
    return !ts_node_is_null(*out);
}

bool cbm_pkgmap_node_parent(const TSNode *node, TSNode *out) {
    cbm_ffi_ts_node_written(node);
    *out = ts_node_parent(*node);
    return !ts_node_is_null(*out);
}

bool cbm_pkgmap_node_first_named_child(const TSNode *node, TSNode *out) {
    cbm_ffi_ts_node_written(node);
    *out = ts_node_named_child(*node, 0);
    return !ts_node_is_null(*out);
}

const char *cbm_pkgmap_call_occurrence(const CBMCall *call, cbm_pkgmap_occurrence_t *site) {
    *site = (cbm_pkgmap_occurrence_t){
        call->site_start_byte,
        call->site_end_byte,
        call->source_origin,
    };
    return call->callee_name;
}

const char *cbm_pkgmap_usage_occurrence(const CBMUsage *usage, cbm_pkgmap_occurrence_t *site) {
    *site = (cbm_pkgmap_occurrence_t){
        usage->site_start_byte,
        usage->site_end_byte,
        usage->source_origin,
    };
    return usage->ref_name;
}

bool cbm_pkgmap_json_is_null(const yyjson_val *value) {
    return yyjson_is_null(value);
}

bool cbm_pkgmap_json_is_object(const yyjson_val *value) {
    return yyjson_is_obj(value);
}

size_t cbm_pkgmap_json_len(const yyjson_val *value) {
    return yyjson_get_len(value);
}

bool cbm_pkgmap_json_int(const yyjson_val *value, int *number) {
    *number = yyjson_get_int(value);
    return yyjson_is_int(value);
}

bool cbm_pkgmap_json_bool(const yyjson_val *value, bool *boolean) {
    *boolean = yyjson_get_bool(value);
    return yyjson_is_bool(value);
}

bool cbm_pkgmap_json_array_size(const yyjson_val *value, size_t *count) {
    *count = yyjson_arr_size(value);
    return yyjson_is_arr(value);
}

const yyjson_val *cbm_pkgmap_json_array_at(const yyjson_val *value, size_t index) {
    return yyjson_arr_get(value, index);
}

yyjson_mut_val *cbm_pkgmap_json_object(yyjson_mut_doc *doc) {
    return yyjson_mut_obj(doc);
}

yyjson_mut_val *cbm_pkgmap_json_array(yyjson_mut_doc *doc) {
    return yyjson_mut_arr(doc);
}

yyjson_mut_val *cbm_pkgmap_json_copy_string(yyjson_mut_doc *doc, const char *string) {
    return string ? yyjson_mut_strcpy(doc, string) : yyjson_mut_null(doc);
}

yyjson_mut_val *cbm_pkgmap_json_integer(yyjson_mut_doc *doc, int64_t number) {
    return yyjson_mut_int(doc, number);
}

yyjson_mut_val *cbm_pkgmap_json_boolean(yyjson_mut_doc *doc, bool value) {
    return yyjson_mut_bool(doc, value);
}

bool cbm_pkgmap_json_put(yyjson_mut_doc *doc, yyjson_mut_val *object, const char *key,
                         yyjson_mut_val *value) {
    return yyjson_mut_obj_add(object, yyjson_mut_strcpy(doc, key), value);
}

bool cbm_pkgmap_json_append(yyjson_mut_val *array, yyjson_mut_val *value) {
    return yyjson_mut_arr_add_val(array, value);
}

char *cbm_pkgmap_json_write(const yyjson_mut_val *value) {
    return yyjson_mut_val_write(value, 0, NULL);
}

bool cbm_pkgmap_skip_dir(const char *name, const char *rel, char *const *excluded, int count) {
    return cbm_should_skip_dir(name, CBM_MODE_FULL) ||
           cbm_pipeline_relpath_is_excluded(rel, excluded, count);
}
