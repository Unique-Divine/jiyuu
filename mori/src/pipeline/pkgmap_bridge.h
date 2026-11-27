#ifndef CBM_PKGMAP_BRIDGE_H
#define CBM_PKGMAP_BRIDGE_H

#include "pipeline/pipeline_internal.h"
#include <yyjson/yyjson.h>

void *cbm_pkgmap_tracked_calloc(size_t size);
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

bool cbm_pkgmap_skip_dir(const char *name, const char *rel, char *const *excluded, int count);

#endif
