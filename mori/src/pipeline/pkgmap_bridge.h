#ifndef CBM_PKGMAP_BRIDGE_H
#define CBM_PKGMAP_BRIDGE_H

#include "foundation/compat_fs.h"
#include "pipeline/pipeline_internal.h"
#include <yyjson/yyjson.h>

void *cbm_pkgmap_tracked_calloc(size_t size);
void *cbm_pkgmap_binding_scratch_alloc(size_t size);
char *cbm_pkgmap_tracked_strdup(const char *value);
void cbm_pkgmap_tracked_free(char *value);
const char *cbm_pkgmap_file_at(const cbm_file_info_t *files, int index, const char **abs);
yyjson_doc *cbm_pkgmap_json_read(const char *source, size_t len);
void cbm_pkgmap_json_free(yyjson_doc *doc);
const yyjson_val *cbm_pkgmap_json_root(const yyjson_doc *doc);
const yyjson_val *cbm_pkgmap_json_get(const yyjson_val *object, const char *key);
const char *cbm_pkgmap_json_string(const yyjson_val *value);
const yyjson_val *cbm_pkgmap_json_first_key(const yyjson_val *object, size_t *count);
const yyjson_val *cbm_pkgmap_json_next_pair(const yyjson_val **next);

char *cbm_pkgmap_read_manifest(const char *path, int *len);
void cbm_pkgmap_free_manifest(char *source);

const char *cbm_pkgmap_dir_next(cbm_dir_t *dir);
int cbm_pkgmap_path_kind(const char *path);
int cbm_pkgmap_language(const char *name);
const cbm_gbuf_t *cbm_pkgmap_context(const cbm_pipeline_ctx_t *ctx, const char **project,
                                     const cbm_path_alias_collection_t **aliases);
const char *cbm_pkgmap_namespace(const CBMFileResult *result);
const CBMImportArray *cbm_pkgmap_imports(const CBMFileResult *result);

typedef struct {
    uint32_t start;
    uint32_t end;
    unsigned int origin;
} cbm_pkgmap_occurrence_t;

const TSTree *cbm_pkgmap_site_source(const CBMFileResult *result, const char **source, int *len);
bool cbm_pkgmap_node_at(const TSTree *tree, uint32_t start, uint32_t end, TSNode *out);
bool cbm_pkgmap_node_parent(const TSNode *node, TSNode *out);
bool cbm_pkgmap_node_first_named_child(const TSNode *node, TSNode *out);
const char *cbm_pkgmap_call_occurrence(const CBMCall *call, cbm_pkgmap_occurrence_t *site);
const char *cbm_pkgmap_usage_occurrence(const CBMUsage *usage, cbm_pkgmap_occurrence_t *site);

bool cbm_pkgmap_json_is_null(const yyjson_val *value);
bool cbm_pkgmap_json_is_object(const yyjson_val *value);
size_t cbm_pkgmap_json_len(const yyjson_val *value);
bool cbm_pkgmap_json_int(const yyjson_val *value, int *number);
bool cbm_pkgmap_json_bool(const yyjson_val *value, bool *boolean);
bool cbm_pkgmap_json_array_size(const yyjson_val *value, size_t *count);
const yyjson_val *cbm_pkgmap_json_array_at(const yyjson_val *value, size_t index);
yyjson_mut_val *cbm_pkgmap_json_object(yyjson_mut_doc *doc);
yyjson_mut_val *cbm_pkgmap_json_array(yyjson_mut_doc *doc);
yyjson_mut_val *cbm_pkgmap_json_copy_string(yyjson_mut_doc *doc, const char *string);
yyjson_mut_val *cbm_pkgmap_json_integer(yyjson_mut_doc *doc, int64_t number);
yyjson_mut_val *cbm_pkgmap_json_boolean(yyjson_mut_doc *doc, bool value);
bool cbm_pkgmap_json_put(yyjson_mut_doc *doc, yyjson_mut_val *object, const char *key,
                         yyjson_mut_val *value);
bool cbm_pkgmap_json_append(yyjson_mut_val *array, yyjson_mut_val *value);
char *cbm_pkgmap_json_write(const yyjson_mut_val *value);

bool cbm_pkgmap_skip_dir(const char *name, const char *rel, char *const *excluded, int count);

#endif
