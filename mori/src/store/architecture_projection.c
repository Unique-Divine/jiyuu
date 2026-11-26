/* Bounded architecture projections over a completed graph generation. */
#include "store/architecture_projection_ffi.h"
#include "foundation/platform.h"
#include "foundation/mem_core.h"
#include "foundation/sanitized.h"
#include <sqlite3.h>
#include <yyjson/yyjson.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

enum {
    AP_TEXT_BUDGET = 64 * 1024 * 1024,
    AP_ENTRYPOINTS = 128,
    AP_JSON_BUDGET = 32 * 1024 * 1024,
    AP_ARGUMENT_LIMIT = 8,
    AP_SIGNATURE_BYTES = 4096,
    AP_VALUE_BYTES = 1024,
    AP_PARAMETER_LIMIT = 64
};
enum { AP_EVIDENCE_CONTEXT, AP_EVIDENCE_CORRIDOR, AP_EVIDENCE_PATH, AP_EVIDENCE_SCOPES };

typedef struct {
    size_t allocated;
    bool failed;
} ap_json_budget;

static void *ap_json_malloc(void *context, size_t size) {
    ap_json_budget *budget = context;
    if (size > AP_JSON_BUDGET - budget->allocated) {
        budget->failed = true;
        return NULL;
    }
    void *p = cbm_alloc(CBM_MEM_CLASS_OTHER, size);
    if (p)
        budget->allocated += size;
    else
        budget->failed = true;
    return p;
}

static void *ap_json_realloc(void *context, void *ptr, size_t old_size, size_t size) {
    ap_json_budget *budget = context;
    size_t growth = size > old_size ? size - old_size : 0;
    if (growth > AP_JSON_BUDGET - budget->allocated) {
        budget->failed = true;
        return NULL;
    }
    void *p = cbm_realloc(CBM_MEM_CLASS_OTHER, ptr, size);
    if (p)
        budget->allocated += growth;
    else
        budget->failed = true;
    return p;
}

static void ap_json_free(void *context, void *ptr) {
    (void)context;
    cbm_free(CBM_MEM_CLASS_OTHER, ptr);
}

/* Presence and references are deliberately separate from execution. */
static const char *const ap_types[] = {
    "",           "CALLS",      "HTTP_CALLS", "ASYNC_CALLS",    "CALL_REFERENCE", "IMPORTS",
    "IMPLEMENTS", "INHERITS",   "CONFIGURES", "WRITES",         "READS",          "USAGE",
    "PUBLISHES",  "SUBSCRIBES", "DEFINES",    "DEFINES_METHOD", "CONTAINS",       "EMITS",
    "LISTENS_ON", "HANDLES",    "DATA_FLOWS", "INFRA_MAPS"};
typedef struct {
    sqlite3 *db;
    const char *project;
    char **out_json;
    cbm_architecture_projection_options_t options;
    uint64_t started;
    bool cancelled, expired;
    size_t text_bytes;
    const char *limited;
    int64_t total_nodes, total_edges, total_files;
    ap_node *nodes;
    ap_edge *edges;
    int n, m;
    ap_component *components;
    int component_count, accounted, structural, unmodeled_edges, relationship_edges;
    int cycle_count, omitted_cycles;
    ap_overview_group groups[66];
    int group_count, grouping_depth;
    sqlite3_stmt *provenance;
    sqlite3_stmt *symbol_metadata;
    yyjson_doc **symbol_properties;
    unsigned char *symbol_properties_loaded;
    ap_json_budget metadata_budget;
    size_t evidence_bytes[AP_EVIDENCE_SCOPES];
    int evidence_scope;
    bool evidence_omitted, evidence_suppressed;
} ap_context;

static int ap_bound(int requested, int fallback, int ceiling) {
    return requested <= 0 ? fallback : requested > ceiling ? ceiling : requested;
}

static bool ap_stopped(void *context) {
    ap_context *c = context;
    if (c->options.cancel && c->options.cancel(c->options.cancel_context))
        c->cancelled = true;
    if (cbm_now_ms() >= c->options.deadline_ms)
        c->expired = true;
    return c->cancelled || c->expired;
}

static char *ap_copy(ap_context *c, const unsigned char *text) {
    const char *s = text ? (const char *)text : "";
    size_t bytes = strlen(s) + 1;
    if (bytes > AP_TEXT_BUDGET - c->text_bytes) {
        c->limited = "Indexed symbol text exceeds the analysis memory budget.";
        return NULL;
    }
    char *result = cbm_alloc(CBM_MEM_CLASS_OTHER, bytes);
    if (!result)
        return NULL;
    memcpy(result, s, bytes);
    c->text_bytes += bytes;
    return result;
}

static int ap_find(const ap_context *c, int64_t id) {
    int lo = 0, hi = c->n;
    while (lo < hi) {
        int mid = lo + (hi - lo) / 2;
        if (c->nodes[mid].id < id)
            lo = mid + 1;
        else
            hi = mid;
    }
    return lo < c->n && c->nodes[lo].id == id ? lo : -1;
}

static int ap_type(const char *type) {
    for (int i = 1; i < (int)(sizeof(ap_types) / sizeof(ap_types[0])); i++)
        if (strcmp(type, ap_types[i]) == 0)
            return i;
    return 0;
}

static bool ap_execution(int type) {
    return type >= 1 && type <= 3;
}
static int ap_counts(ap_context *c, const char *project) {
    sqlite3_stmt *stmt = NULL;
    const char *sql = "SELECT (SELECT count(*) FROM nodes WHERE project=?1),"
                      "(SELECT count(*) FROM edges WHERE project=?1),"
                      "(SELECT count(*) FROM nodes WHERE project=?1 AND label='File') "
                      "FROM projects WHERE name=?1";
    if (sqlite3_prepare_v2(c->db, sql, -1, &stmt, NULL) != SQLITE_OK)
        return CBM_STORE_ERR;
    sqlite3_bind_text(stmt, 1, project, -1, SQLITE_TRANSIENT);
    int step = sqlite3_step(stmt);
    if (step == SQLITE_ROW) {
        c->total_nodes = sqlite3_column_int64(stmt, 0);
        c->total_edges = sqlite3_column_int64(stmt, 1);
        c->total_files = sqlite3_column_int64(stmt, 2);
    }
    sqlite3_finalize(stmt);
    if (step == SQLITE_DONE)
        return CBM_STORE_NOT_FOUND;
    if (step != SQLITE_ROW)
        return CBM_STORE_ERR;
    if (c->total_nodes > c->options.max_nodes)
        c->limited = "Graph exceeds the node budget; no sampled architecture was inferred.";
    else if (c->total_edges > c->options.max_edges)
        c->limited = "Graph exceeds the edge budget; no sampled architecture was inferred.";
    return CBM_STORE_OK;
}

static int ap_load(ap_context *c, const char *project) {
    c->nodes = cbm_calloc(CBM_MEM_CLASS_OTHER, ((size_t)c->total_nodes + 1) * sizeof(*c->nodes));
    c->edges = cbm_calloc(CBM_MEM_CLASS_OTHER, ((size_t)c->total_edges + 1) * sizeof(*c->edges));
    if (!c->nodes || !c->edges)
        return CBM_STORE_ERR;
    sqlite3_stmt *stmt = NULL;
    const char *sql =
        "SELECT id,name,qualified_name,file_path,label,start_line,end_line,"
        "CASE WHEN json_valid(properties) THEN json_extract(properties,'$.is_entry_point') "
        "ELSE 0 END, CASE WHEN json_valid(properties) THEN json_extract(properties,'$.is_test') "
        "ELSE 0 END FROM nodes WHERE project=?1 ORDER BY id";
    if (sqlite3_prepare_v2(c->db, sql, -1, &stmt, NULL) != SQLITE_OK)
        return CBM_STORE_ERR;
    sqlite3_bind_text(stmt, 1, project, -1, SQLITE_TRANSIENT);
    int step;
    while ((step = sqlite3_step(stmt)) == SQLITE_ROW) {
        if (c->n >= c->total_nodes || ap_stopped(c))
            break;
        ap_node *n = &c->nodes[c->n++];
        n->id = sqlite3_column_int64(stmt, 0);
        n->name = ap_copy(c, sqlite3_column_text(stmt, 1));
        n->qn = ap_copy(c, sqlite3_column_text(stmt, 2));
        n->file = ap_copy(c, sqlite3_column_text(stmt, 3));
        n->label = ap_copy(c, sqlite3_column_text(stmt, 4));
        if (!n->name || !n->qn || !n->file || !n->label)
            break;
        n->line = sqlite3_column_int(stmt, 5);
        n->end = sqlite3_column_int(stmt, 6);
        n->entry = sqlite3_column_int(stmt, 7) != 0;
        n->test_property = sqlite3_column_int(stmt, 8) != 0;
        n->component = -1;
    }
    sqlite3_finalize(stmt);
    if (step != SQLITE_DONE)
        return CBM_STORE_ERR;
    sql = "SELECT id,source_id,target_id,type FROM edges WHERE project=?1 ORDER BY id";
    if (sqlite3_prepare_v2(c->db, sql, -1, &stmt, NULL) != SQLITE_OK)
        return CBM_STORE_ERR;
    sqlite3_bind_text(stmt, 1, project, -1, SQLITE_TRANSIENT);
    int scanned = 0;
    while ((step = sqlite3_step(stmt)) == SQLITE_ROW) {
        if (scanned++ >= c->total_edges || ap_stopped(c))
            break;
        int type = ap_type((const char *)sqlite3_column_text(stmt, 3));
        if (!type) {
            c->unmodeled_edges++;
            continue;
        }
        int source = ap_find(c, sqlite3_column_int64(stmt, 1));
        int target = ap_find(c, sqlite3_column_int64(stmt, 2));
        if (source < 0 || target < 0)
            continue;
        c->edges[c->m++] = (ap_edge){sqlite3_column_int64(stmt, 0), source, target, type};
    }
    sqlite3_finalize(stmt);
    return step == SQLITE_DONE ? CBM_STORE_OK : CBM_STORE_ERR;
}

void mori_architecture_scratch_add(size_t bytes) {
    cbm_mem_class_add_external(CBM_MEM_CLASS_OTHER, bytes);
}

void mori_architecture_scratch_remove(size_t bytes) {
    /* Publish even short-lived scratch before the thread-local delta nets out. */
    cbm_mem_class_flush_thread();
    cbm_mem_class_remove_external(CBM_MEM_CLASS_OTHER, bytes);
}

void mori_architecture_directory_written(const unsigned char *directory, size_t length) {
    CBM_MSAN_WRITTEN(directory, length);
    (void)directory;
    (void)length;
}

static char *ap_copy_group_text(const char *source, size_t bytes) {
    CBM_MSAN_WRITTEN(source, bytes);
    char *copy = cbm_alloc(CBM_MEM_CLASS_OTHER, bytes);
    if (copy)
        memcpy(copy, source, bytes);
    return copy;
}

static bool ap_store_group(void *context, const ap_overview_group *source, size_t id_bytes,
                           size_t label_bytes) {
    CBM_MSAN_WRITTEN(source, sizeof(*source));
    ap_context *c = context;
    ap_overview_group *group = &c->groups[c->group_count++];
    *group = *source;
    group->id = ap_copy_group_text(source->id, id_bytes);
    group->label = ap_copy_group_text(source->label, label_bytes);
    return group->id && group->label;
}

static int ap_partition_graph(ap_context *c) {
    c->components = cbm_calloc(CBM_MEM_CLASS_OTHER, ((size_t)c->n + 1) * sizeof(*c->components));
    if (!c->components)
        return CBM_STORE_ERR;
    ap_partition partition = {
        .nodes = c->nodes, .edges = c->edges, .components = c->components,
        .node_count = c->n, .edge_count = c->m, .text_bytes = c->text_bytes,
        .text_budget = AP_TEXT_BUDGET,
    };
    int rc = mori_architecture_partition(&partition, c, ap_stopped, ap_store_group);
    /* The output fields start at zero and can be updated before an error. */
    CBM_MSAN_WRITTEN(&partition.component_count, sizeof(partition.component_count));
    CBM_MSAN_WRITTEN(&partition.accounted, sizeof(partition.accounted));
    CBM_MSAN_WRITTEN(&partition.grouping_depth, sizeof(partition.grouping_depth));
    CBM_MSAN_WRITTEN(&partition.relationship_edges, sizeof(partition.relationship_edges));
    if (rc == CBM_STORE_OK) {
        CBM_MSAN_WRITTEN(c->nodes, (size_t)c->n * sizeof(*c->nodes));
        CBM_MSAN_WRITTEN(c->components, (size_t)partition.component_count * sizeof(*c->components));
    }
    c->component_count = partition.component_count;
    c->accounted = partition.accounted;
    c->structural = c->n - c->accounted;
    c->grouping_depth = partition.grouping_depth;
    c->relationship_edges = partition.relationship_edges;
    if (rc == CBM_STORE_SCAN_LIMIT)
        c->limited = "Indexed symbol text exceeds the analysis memory budget.";
    return rc;
}

static void ap_component_id(const ap_context *c, int component, char id[48]) {
    if (component < 0) {
        id[0] = '\0';
        return;
    }
    snprintf(id, 48, "component-%lld", (long long)c->nodes[c->components[component].root].id);
}

/* Only opt-in, displayed symbols load indexed properties. Cache each decoded
 * object once across representatives, paths and witnesses in this response.
 * Source code is never opened or parsed. Oversized values are omitted whole. */
static yyjson_val *ap_symbol_properties(ap_context *c, int index) {
    if (!c->options.include_behavior_evidence)
        return NULL;
    if (c->symbol_properties_loaded && c->symbol_properties_loaded[index])
        return c->symbol_properties[index] ? yyjson_doc_get_root(c->symbol_properties[index])
                                           : NULL;
    if (c->metadata_budget.failed)
        return NULL;
    if (!c->symbol_properties) {
        c->symbol_properties =
            cbm_calloc(CBM_MEM_CLASS_OTHER, ((size_t)c->n) * sizeof(*c->symbol_properties));
        c->symbol_properties_loaded = cbm_calloc(CBM_MEM_CLASS_OTHER, (size_t)c->n);
        if (!c->symbol_properties || !c->symbol_properties_loaded) {
            cbm_free(CBM_MEM_CLASS_OTHER, c->symbol_properties);
            cbm_free(CBM_MEM_CLASS_OTHER, c->symbol_properties_loaded);
            c->symbol_properties = NULL;
            c->symbol_properties_loaded = NULL;
            c->metadata_budget.failed = true;
            return NULL;
        }
    }
    c->symbol_properties_loaded[index] = 1;
    if (!c->symbol_metadata &&
        sqlite3_prepare_v2(c->db,
                           "SELECT CASE WHEN length(CAST(properties AS BLOB))<=16384 THEN "
                           "properties END FROM nodes WHERE id=?1",
                           -1, &c->symbol_metadata, NULL) != SQLITE_OK)
        return NULL;
    sqlite3_bind_int64(c->symbol_metadata, 1, c->nodes[index].id);
    if (sqlite3_step(c->symbol_metadata) == SQLITE_ROW) {
        char *text = (char *)sqlite3_column_text(c->symbol_metadata, 0);
        yyjson_alc allocator = {ap_json_malloc, ap_json_realloc, ap_json_free, &c->metadata_budget};
        if (text)
            c->symbol_properties[index] = yyjson_read_opts(
                text, (size_t)sqlite3_column_bytes(c->symbol_metadata, 0), 0, &allocator, NULL);
    }
    sqlite3_reset(c->symbol_metadata);
    return c->symbol_properties[index] ? yyjson_doc_get_root(c->symbol_properties[index]) : NULL;
}

static bool ap_bounded_string(yyjson_val *value, size_t maximum) {
    return yyjson_is_str(value) && yyjson_get_len(value) <= maximum;
}

static bool ap_parameter_array(yyjson_val *value) {
    if (!yyjson_is_arr(value) || yyjson_arr_size(value) > AP_PARAMETER_LIMIT)
        return false;
    size_t index, maximum;
    yyjson_val *item;
    yyjson_arr_foreach(value, index, maximum,
                       item) if (!ap_bounded_string(item, AP_VALUE_BYTES)) return false;
    return true;
}

/* Optional copies must leave room for the base graph and its serialized copy.
 * Charge mutable values plus worst-case JSON escaping before allocation. The
 * separate pools keep overview/catalog duplicates from consuming path evidence.
 * The shared allocator remains the final hard bound on the entire response. */
static size_t ap_evidence_limit(const ap_context *c) {
    static const size_t limits[AP_EVIDENCE_SCOPES] = {256 * 1024, 768 * 1024, 1024 * 1024};
    return limits[c->evidence_scope];
}

static size_t ap_evidence_string_cost(yyjson_val *value) {
    return 128 + 8 * yyjson_get_len(value);
}

static bool ap_evidence_admit(ap_context *c, size_t bytes) {
    if (c->evidence_suppressed)
        return false;
    if (bytes > ap_evidence_limit(c) - c->evidence_bytes[c->evidence_scope]) {
        c->evidence_omitted = true;
        return false;
    }
    c->evidence_bytes[c->evidence_scope] += bytes;
    return true;
}

static void ap_declaration_evidence(yyjson_mut_doc *doc, ap_context *c, int index,
                                    yyjson_mut_val *value) {
    if (c->evidence_suppressed)
        return;
    if (ap_evidence_limit(c) - c->evidence_bytes[c->evidence_scope] < 128) {
        c->evidence_omitted = true;
        return;
    }
    yyjson_val *properties = ap_symbol_properties(c, index);
    if (!yyjson_is_obj(properties))
        return;
    yyjson_val *signature = yyjson_obj_get(properties, "signature"),
               *returns = yyjson_obj_get(properties, "return_type");
    if (ap_bounded_string(signature, AP_SIGNATURE_BYTES) &&
        ap_evidence_admit(c, ap_evidence_string_cost(signature)))
        yyjson_mut_obj_add_val(doc, value, "signature", yyjson_val_mut_copy(doc, signature));
    if (ap_bounded_string(returns, AP_VALUE_BYTES) &&
        ap_evidence_admit(c, ap_evidence_string_cost(returns)))
        yyjson_mut_obj_add_val(doc, value, "return_type", yyjson_val_mut_copy(doc, returns));
    yyjson_val *names = yyjson_obj_get(properties, "param_names"),
               *types = yyjson_obj_get(properties, "param_types"),
               *count = yyjson_obj_get(properties, "param_count");
    if (ap_parameter_array(names) && ap_parameter_array(types) && yyjson_is_int(count) &&
        yyjson_get_sint(count) >= 0 && yyjson_get_sint(count) <= INT_MAX &&
        yyjson_arr_size(names) <= (size_t)yyjson_get_sint(count) &&
        yyjson_arr_size(types) <= (size_t)yyjson_get_sint(count)) {
        size_t bytes = 384, at, maximum;
        yyjson_val *item;
        yyjson_arr_foreach(names, at, maximum, item) bytes += ap_evidence_string_cost(item);
        yyjson_arr_foreach(types, at, maximum, item) bytes += ap_evidence_string_cost(item);
        if (!ap_evidence_admit(c, bytes))
            return;
        yyjson_mut_val *parameters = yyjson_mut_obj(doc);
        yyjson_mut_obj_add_val(doc, parameters, "names", yyjson_val_mut_copy(doc, names));
        yyjson_mut_obj_add_val(doc, parameters, "types", yyjson_val_mut_copy(doc, types));
        yyjson_mut_obj_add_val(doc, parameters, "count", yyjson_val_mut_copy(doc, count));
        yyjson_mut_obj_add_val(doc, value, "parameters", parameters);
    }
}

static yyjson_mut_val *ap_symbol(yyjson_mut_doc *doc, ap_context *c, int index) {
    const ap_node *n = &c->nodes[index];
    yyjson_mut_val *value = yyjson_mut_obj(doc);
    char id[48];
    ap_component_id(c, n->component, id);
    yyjson_mut_obj_add_sint(doc, value, "id", n->id);
    yyjson_mut_obj_add_strcpy(doc, value, "name", n->name);
    yyjson_mut_obj_add_strcpy(doc, value, "qualified_name", n->qn);
    yyjson_mut_obj_add_strcpy(doc, value, "label", n->label);
    yyjson_mut_obj_add_strcpy(doc, value, "file_path", n->file);
    yyjson_mut_obj_add_int(doc, value, "start_line", n->line);
    yyjson_mut_obj_add_int(doc, value, "end_line", n->end);
    yyjson_mut_obj_add_strcpy(doc, value, "component_id", id);
    if (n->component >= 0 && c->group_count)
        yyjson_mut_obj_add_strcpy(doc, value, "group_id",
                                  c->groups[c->components[n->component].overview_group].id);
    if (c->options.include_behavior_evidence)
        ap_declaration_evidence(doc, c, index, value);
    return value;
}

/* CALLS stores at most eight indexed expressions, without keyword names.
 * Preserve stored indices/strings; do not infer parameter binding or values
 * for absent arguments. Even a short array cannot establish completeness. */
static void ap_argument_evidence(yyjson_mut_doc *doc, ap_context *c, yyjson_val *properties,
                                 yyjson_mut_val *value) {
    if (c->evidence_suppressed)
        return;
    yyjson_val *args = yyjson_obj_get(properties, "args");
    if (!yyjson_is_arr(args))
        return;
    size_t count = yyjson_arr_size(args);
    if (count > AP_ARGUMENT_LIMIT)
        count = AP_ARGUMENT_LIMIT;
    size_t bytes = 384;
    for (size_t at = 0; at < count; at++) {
        yyjson_val *arg = yyjson_arr_get(args, at), *index = yyjson_obj_get(arg, "i"),
                   *expression = yyjson_obj_get(arg, "e"), *resolved = yyjson_obj_get(arg, "v");
        if (!yyjson_is_int(index) || yyjson_get_sint(index) < 0 ||
            yyjson_get_sint(index) > INT_MAX || !ap_bounded_string(expression, AP_VALUE_BYTES))
            continue;
        bytes += 256 + ap_evidence_string_cost(expression);
        if (ap_bounded_string(resolved, AP_VALUE_BYTES))
            bytes += ap_evidence_string_cost(resolved);
    }
    if (!ap_evidence_admit(c, bytes))
        return;
    yyjson_mut_val *arguments = yyjson_mut_arr(doc);
    for (size_t at = 0; at < count; at++) {
        yyjson_val *arg = yyjson_arr_get(args, at), *index = yyjson_obj_get(arg, "i"),
                   *expression = yyjson_obj_get(arg, "e"), *resolved = yyjson_obj_get(arg, "v");
        if (!yyjson_is_int(index) || yyjson_get_sint(index) < 0 ||
            yyjson_get_sint(index) > INT_MAX || !ap_bounded_string(expression, AP_VALUE_BYTES))
            continue;
        yyjson_mut_val *copy = yyjson_mut_obj(doc);
        yyjson_mut_obj_add_val(doc, copy, "i", yyjson_val_mut_copy(doc, index));
        yyjson_mut_obj_add_val(doc, copy, "e", yyjson_val_mut_copy(doc, expression));
        if (ap_bounded_string(resolved, AP_VALUE_BYTES))
            yyjson_mut_obj_add_val(doc, copy, "v", yyjson_val_mut_copy(doc, resolved));
        yyjson_mut_arr_add_val(arguments, copy);
    }
    yyjson_mut_obj_add_val(doc, value, "arguments", arguments);
    yyjson_mut_obj_add_int(doc, value, "argument_limit", AP_ARGUMENT_LIMIT);
    yyjson_mut_obj_add_bool(doc, value, "arguments_complete", false);
}

/* Look up only displayed witnesses. Large/unavailable properties are omitted;
 * resolver confidence is an uncalibrated score, never execution probability. */
static void ap_provenance(yyjson_mut_doc *doc, ap_context *c, int edge, yyjson_mut_val *value) {
    if (!c->provenance &&
        sqlite3_prepare_v2(c->db,
                           "SELECT CASE WHEN length(CAST(properties AS BLOB))<=16384 THEN "
                           "properties END FROM edges WHERE id=?1",
                           -1, &c->provenance, NULL) != SQLITE_OK)
        return;
    sqlite3_bind_int64(c->provenance, 1, c->edges[edge].id);
    yyjson_doc *properties = NULL;
    if (sqlite3_step(c->provenance) == SQLITE_ROW) {
        const char *text = (const char *)sqlite3_column_text(c->provenance, 0);
        if (text)
            properties = yyjson_read(text, (size_t)sqlite3_column_bytes(c->provenance, 0), 0);
    }
    sqlite3_reset(c->provenance);
    if (!properties)
        return;
    yyjson_val *root = yyjson_doc_get_root(properties), *line = yyjson_obj_get(root, "line");
    const char *file = c->nodes[c->edges[edge].source].file;
    if (yyjson_is_int(line) && yyjson_get_sint(line) > 0 && file[0]) {
        yyjson_mut_val *site = yyjson_mut_obj(doc);
        yyjson_mut_obj_add_strcpy(doc, site, "file_path", file);
        yyjson_mut_obj_add_sint(doc, site, "line", yyjson_get_sint(line));
        yyjson_mut_obj_add_val(doc, value, "callsite", site);
    }
    yyjson_val *strategy = yyjson_obj_get(root, "strategy"),
               *confidence = yyjson_obj_get(root, "confidence"),
               *candidates = yyjson_obj_get(root, "candidates");
    yyjson_mut_val *resolution = yyjson_mut_obj(doc);
    bool any = false;
    if (yyjson_is_str(strategy) && yyjson_get_len(strategy) <= 128) {
        yyjson_mut_obj_add_strcpy(doc, resolution, "strategy", yyjson_get_str(strategy));
        any = true;
    }
    if (yyjson_is_num(confidence)) {
        yyjson_mut_obj_add_real(doc, resolution, "confidence", yyjson_get_num(confidence));
        any = true;
    }
    if (yyjson_is_int(candidates) && yyjson_get_sint(candidates) >= 0) {
        yyjson_mut_obj_add_sint(doc, resolution, "candidates", yyjson_get_sint(candidates));
        any = true;
    }
    if (any)
        yyjson_mut_obj_add_val(doc, value, "resolution", resolution);
    if (c->options.include_behavior_evidence && c->edges[edge].type == 1)
        ap_argument_evidence(doc, c, root, value);
    yyjson_doc_free(properties);
}

static yyjson_mut_val *ap_path_edge(yyjson_mut_doc *doc, ap_context *c, int edge) {
    const ap_edge *e = &c->edges[edge];
    yyjson_mut_val *value = yyjson_mut_obj(doc);
    yyjson_mut_obj_add_sint(doc, value, "id", e->id);
    yyjson_mut_obj_add_sint(doc, value, "source_id", c->nodes[e->source].id);
    yyjson_mut_obj_add_sint(doc, value, "target_id", c->nodes[e->target].id);
    yyjson_mut_obj_add_str(doc, value, "type", ap_types[e->type]);
    ap_provenance(doc, c, edge, value);
    return value;
}

static yyjson_mut_val *ap_witness(yyjson_mut_doc *doc, ap_context *c, int edge) {
    const ap_edge *e = &c->edges[edge];
    yyjson_mut_val *value = yyjson_mut_obj(doc);
    yyjson_mut_obj_add_sint(doc, value, "edge_id", e->id);
    yyjson_mut_obj_add_val(doc, value, "source", ap_symbol(doc, c, e->source));
    yyjson_mut_obj_add_val(doc, value, "target", ap_symbol(doc, c, e->target));
    yyjson_mut_obj_add_str(doc, value, "type", ap_types[e->type]);
    ap_provenance(doc, c, edge, value);
    return value;
}

/* Iterative Kosaraju. A component SCC is a dependency cycle, not evidence of
 * a runtime loop or a contiguous path through the member symbols. */
static int ap_cycles(ap_context *c, yyjson_mut_doc *doc, yyjson_mut_val *cycles, const ap_dep *deps,
                     int count) {
    int n = c->component_count;
    int *out = cbm_calloc(CBM_MEM_CLASS_OTHER, ((size_t)n + 2) * sizeof(int));
    int *in = cbm_calloc(CBM_MEM_CLASS_OTHER, ((size_t)n + 2) * sizeof(int));
    int *forward = cbm_alloc(CBM_MEM_CLASS_OTHER, ((size_t)count + 1) * sizeof(int));
    int *reverse = cbm_alloc(CBM_MEM_CLASS_OTHER, ((size_t)count + 1) * sizeof(int));
    int *cursor = cbm_calloc(CBM_MEM_CLASS_OTHER, ((size_t)n + 1) * sizeof(int));
    int *stack = cbm_alloc(CBM_MEM_CLASS_OTHER, ((size_t)n + 1) * sizeof(int));
    int *order = cbm_alloc(CBM_MEM_CLASS_OTHER, ((size_t)n + 1) * sizeof(int));
    unsigned char *seen = cbm_calloc(CBM_MEM_CLASS_OTHER, (size_t)n + 1);
    if (!out || !in || !forward || !reverse || !cursor || !stack || !order || !seen) {
        cbm_free(CBM_MEM_CLASS_OTHER, out);
        cbm_free(CBM_MEM_CLASS_OTHER, in);
        cbm_free(CBM_MEM_CLASS_OTHER, forward);
        cbm_free(CBM_MEM_CLASS_OTHER, reverse);
        cbm_free(CBM_MEM_CLASS_OTHER, cursor);
        cbm_free(CBM_MEM_CLASS_OTHER, stack);
        cbm_free(CBM_MEM_CLASS_OTHER, order);
        cbm_free(CBM_MEM_CLASS_OTHER, seen);
        return CBM_STORE_ERR;
    }
    for (int i = 0; i < count; i++) {
        out[deps[i].source + 1]++;
        in[deps[i].target + 1]++;
    }
    for (int i = 0; i < n; i++) {
        out[i + 1] += out[i];
        in[i + 1] += in[i];
    }
    memcpy(cursor, out, ((size_t)n + 1) * sizeof(int));
    for (int i = 0; i < count; i++)
        forward[cursor[deps[i].source]++] = deps[i].target;
    memcpy(cursor, in, ((size_t)n + 1) * sizeof(int));
    for (int i = 0; i < count; i++)
        reverse[cursor[deps[i].target]++] = deps[i].source;
    memcpy(cursor, out, ((size_t)n + 1) * sizeof(int));
    int ordered = 0, rc = CBM_STORE_OK;
    for (int root = 0; root < n; root++) {
        if (ap_stopped(c)) {
            rc = CBM_STORE_ERR;
            goto done;
        }
        if (seen[root])
            continue;
        int depth = 0;
        stack[depth++] = root;
        seen[root] = 1;
        while (depth) {
            int v = stack[depth - 1];
            if (cursor[v] < out[v + 1]) {
                int next = forward[cursor[v]++];
                if (!seen[next]) {
                    seen[next] = 1;
                    stack[depth++] = next;
                }
            } else {
                order[ordered++] = v;
                depth--;
            }
            if ((ordered & 1023) == 0 && ap_stopped(c)) {
                rc = CBM_STORE_ERR;
                goto done;
            }
        }
    }
    memset(seen, 0, (size_t)n + 1);
    for (int i = ordered - 1; i >= 0; i--) {
        int root = order[i];
        if (seen[root])
            continue;
        int depth = 0, members = 0;
        stack[depth++] = root;
        seen[root] = 1;
        yyjson_mut_val *ids = yyjson_mut_arr(doc);
        bool visible = true;
        while (depth) {
            int v = stack[--depth];
            members++;
            if (c->components[v].displayed < 0)
                visible = false;
            char id[48];
            ap_component_id(c, v, id);
            yyjson_mut_arr_add_strcpy(doc, ids, id);
            for (int j = in[v]; j < in[v + 1]; j++) {
                int next = reverse[j];
                if (!seen[next]) {
                    seen[next] = 1;
                    stack[depth++] = next;
                }
            }
            if ((members & 255) == 0 && ap_stopped(c)) {
                rc = CBM_STORE_ERR;
                goto done;
            }
        }
        if (members > 1) {
            c->cycle_count++;
            if (visible) {
                yyjson_mut_val *cycle = yyjson_mut_obj(doc);
                yyjson_mut_obj_add_val(doc, cycle, "component_ids", ids);
                yyjson_mut_arr_add_val(cycles, cycle);
            } else
                c->omitted_cycles++;
        }
    }
done:
    cbm_free(CBM_MEM_CLASS_OTHER, out);
    cbm_free(CBM_MEM_CLASS_OTHER, in);
    cbm_free(CBM_MEM_CLASS_OTHER, forward);
    cbm_free(CBM_MEM_CLASS_OTHER, reverse);
    cbm_free(CBM_MEM_CLASS_OTHER, cursor);
    cbm_free(CBM_MEM_CLASS_OTHER, stack);
    cbm_free(CBM_MEM_CLASS_OTHER, order);
    cbm_free(CBM_MEM_CLASS_OTHER, seen);
    return rc;
}

/* One bounded BFS per selected entry. Select representative terminal witnesses
 * after traversal: emitting during BFS lets shallow utility leaves exhaust the
 * display budget before deeper queued branches are visited. The predecessor
 * tree stays linear in graph size; we never enumerate all possible paths. */
static int ap_paths(ap_context *c, yyjson_mut_doc *doc, yyjson_mut_val *paths, const int *entries,
                    int entry_count, bool *truncated) {
    int *offset = cbm_calloc(CBM_MEM_CLASS_OTHER, ((size_t)c->n + 2) * sizeof(int));
    int *adj = cbm_alloc(CBM_MEM_CLASS_OTHER, ((size_t)c->m + 1) * sizeof(int));
    int *cursor = cbm_calloc(CBM_MEM_CLASS_OTHER, ((size_t)c->n + 1) * sizeof(int));
    int *queue = cbm_alloc(CBM_MEM_CLASS_OTHER, ((size_t)c->n + 1) * sizeof(int));
    int *previous = cbm_alloc(CBM_MEM_CLASS_OTHER, ((size_t)c->n + 1) * sizeof(int));
    int *depth = cbm_calloc(CBM_MEM_CLASS_OTHER, ((size_t)c->n + 1) * sizeof(int));
    ap_path_candidate *candidates =
        cbm_alloc(CBM_MEM_CLASS_OTHER, ((size_t)c->n + 1) * sizeof(*candidates));
    if (!offset || !adj || !cursor || !queue || !previous || !depth || !candidates) {
        cbm_free(CBM_MEM_CLASS_OTHER, offset);
        cbm_free(CBM_MEM_CLASS_OTHER, adj);
        cbm_free(CBM_MEM_CLASS_OTHER, cursor);
        cbm_free(CBM_MEM_CLASS_OTHER, queue);
        cbm_free(CBM_MEM_CLASS_OTHER, previous);
        cbm_free(CBM_MEM_CLASS_OTHER, depth);
        cbm_free(CBM_MEM_CLASS_OTHER, candidates);
        return CBM_STORE_ERR;
    }
    for (int i = 0; i < c->m; i++)
        if (ap_execution(c->edges[i].type))
            offset[c->edges[i].source + 1]++;
    for (int i = 0; i < c->n; i++)
        offset[i + 1] += offset[i];
    memcpy(cursor, offset, ((size_t)c->n + 1) * sizeof(int));
    for (int i = 0; i < c->m; i++)
        if (ap_execution(c->edges[i].type))
            adj[cursor[c->edges[i].source]++] = i;
    int produced = 0, rc = CBM_STORE_OK;
    for (int ent = 0; ent < entry_count; ent++) {
        if (produced >= c->options.max_paths) {
            *truncated = true;
            break;
        }
        for (int i = 0; i < c->n; i++)
            previous[i] = -1;
        int entry = entries[ent], head = 0, tail = 0, candidate_count = 0;
        queue[tail++] = entry;
        previous[entry] = -2;
        depth[entry] = 0;
        /* Adjacency cursors are no longer needed; reuse them for first hops. */
        cursor[entry] = entry;
        while (head < tail) {
            if (ap_stopped(c)) {
                rc = CBM_STORE_ERR;
                goto done;
            }
            int node = queue[head++];
            bool extended = false;
            if (depth[node] < c->options.max_depth) {
                for (int j = offset[node]; j < offset[node + 1]; j++) {
                    int edge = adj[j], next = c->edges[edge].target;
                    if (previous[next] != -1)
                        continue;
                    previous[next] = edge;
                    depth[next] = depth[node] + 1;
                    cursor[next] = node == entry ? next : cursor[node];
                    queue[tail++] = next;
                    extended = true;
                }
            } else if (offset[node] != offset[node + 1])
                *truncated = true;
            if (extended || node == entry)
                continue;
            int crossings = 0, current = node;
            while (previous[current] >= 0) {
                int parent = c->edges[previous[current]].source;
                if (c->nodes[parent].component != c->nodes[current].component)
                    crossings++;
                current = parent;
            }
            candidates[candidate_count++] = (ap_path_candidate){.node = node,
                                                                .first_hop = cursor[node],
                                                                .depth = depth[node],
                                                                .crossings = crossings};
        }
        mori_architecture_sort_paths(candidates, candidate_count);
        CBM_MSAN_WRITTEN(candidates, (size_t)candidate_count * sizeof(*candidates));
        if (ap_stopped(c)) {
            rc = CBM_STORE_ERR;
            goto done;
        }
        /* Prefer different first-hop branches, then fill spare slots with
         * other deep witnesses. Depth storage can now track selected branches. */
        int selection[32];
        int selected = mori_architecture_select_paths(
            candidates, candidate_count, c->options.max_paths - produced, depth, c->n, selection);
        CBM_MSAN_WRITTEN(selection, (size_t)selected * sizeof(*selection));
        if (candidate_count > selected)
            *truncated = true;
        for (int selected_index = 0; selected_index < selected; selected_index++) {
            if (ap_stopped(c)) {
                rc = CBM_STORE_ERR;
                goto done;
            }
            int node = selection[selected_index];
            int chain[17], length = 0, current = node;
            while (previous[current] >= 0 && length < 16) {
                chain[length++] = previous[current];
                current = c->edges[previous[current]].source;
            }
            yyjson_mut_val *path = yyjson_mut_obj(doc), *nodes = yyjson_mut_arr(doc),
                           *edges = yyjson_mut_arr(doc);
            yyjson_mut_obj_add_sint(doc, path, "entrypoint_id", c->nodes[entry].id);
            yyjson_mut_arr_add_val(nodes, ap_symbol(doc, c, entry));
            for (int j = length - 1; j >= 0; j--) {
                ap_edge *edge = &c->edges[chain[j]];
                yyjson_mut_arr_add_val(nodes, ap_symbol(doc, c, edge->target));
                yyjson_mut_arr_add_val(edges, ap_path_edge(doc, c, chain[j]));
            }
            yyjson_mut_obj_add_val(doc, path, "nodes", nodes);
            yyjson_mut_obj_add_val(doc, path, "edges", edges);
            yyjson_mut_arr_add_val(paths, path);
            produced++;
        }
    }
done:
    cbm_free(CBM_MEM_CLASS_OTHER, offset);
    cbm_free(CBM_MEM_CLASS_OTHER, adj);
    cbm_free(CBM_MEM_CLASS_OTHER, cursor);
    cbm_free(CBM_MEM_CLASS_OTHER, queue);
    cbm_free(CBM_MEM_CLASS_OTHER, previous);
    cbm_free(CBM_MEM_CLASS_OTHER, depth);
    cbm_free(CBM_MEM_CLASS_OTHER, candidates);
    return rc;
}

static const char *ap_component_basis(const ap_context *c, int i) {
    const ap_component *p = &c->components[i];
    return p->atoms > 1                           ? "interaction_community"
           : c->nodes[p->representative].declared ? "declared_module"
                                                  : "unassigned";
}

static int ap_overview(ap_context *c, yyjson_mut_doc *doc, yyjson_mut_val *root) {
    yyjson_mut_val *overview = yyjson_mut_obj(doc), *groups = yyjson_mut_arr(doc),
                   *parts = yyjson_mut_arr(doc), *connections = yyjson_mut_arr(doc),
                   *totals = yyjson_mut_obj(doc), *limits = yyjson_mut_obj(doc);
    yyjson_mut_obj_add_val(doc, root, "overview", overview);
    yyjson_mut_obj_add_str(doc, overview, "grouping_basis", "common_source_directory_aggregate");
    yyjson_mut_obj_add_int(doc, overview, "grouping_depth", c->grouping_depth);
    yyjson_mut_obj_add_val(doc, overview, "groups", groups);
    yyjson_mut_obj_add_val(doc, overview, "components", parts);
    yyjson_mut_obj_add_val(doc, overview, "connections", connections);
    yyjson_mut_obj_add_val(doc, overview, "totals", totals);
    yyjson_mut_obj_add_val(doc, overview, "limits", limits);
    int connection_count = 0, shown = 0;
    int omitted_entries = 0;
    if (!c->limited) {
        for (int g = 0; g < c->group_count; g++) {
            ap_overview_group *group = &c->groups[g];
            yyjson_mut_val *value = yyjson_mut_obj(doc), *ids = yyjson_mut_arr(doc),
                           *reps = yyjson_mut_arr(doc);
            yyjson_mut_obj_add_strcpy(doc, value, "id", group->id);
            yyjson_mut_obj_add_strcpy(doc, value, "label", group->label);
            yyjson_mut_obj_add_str(doc, value, "role", group->test ? "test" : "non_test");
            yyjson_mut_obj_add_str(doc, value, "basis", "common_source_directory_aggregate");
            yyjson_mut_obj_add_int(doc, value, "component_count", group->components);
            yyjson_mut_obj_add_int(doc, value, "member_count", group->members);
            yyjson_mut_obj_add_int(doc, value, "file_count", group->files);
            int samples = 0;
            for (int i = 0; i < c->component_count; i++)
                if (c->components[i].overview_group == g) {
                    char id[48];
                    ap_component_id(c, i, id);
                    yyjson_mut_arr_add_strcpy(doc, ids, id);
                    if (samples++ < 1)
                        yyjson_mut_arr_add_val(reps,
                                               ap_symbol(doc, c, c->components[i].representative));
                }
            yyjson_mut_obj_add_val(doc, value, "component_ids", ids);
            yyjson_mut_obj_add_val(doc, value, "representatives", reps);
            yyjson_mut_obj_add_int(doc, value, "omitted_representatives",
                                   group->components - (group->components > 0 ? 1 : 0));
            yyjson_mut_arr_add_val(groups, value);
            if (ap_stopped(c))
                return CBM_STORE_ERR;
        }
        /* Groups above keep every component id and count; only the detailed
         * entries are bounded by the component ranking. */
        for (int i = 0; i < c->component_count; i++) {
            ap_component *part = &c->components[i];
            if (!part->listed) {
                omitted_entries++;
                continue;
            }
            yyjson_mut_val *value = yyjson_mut_obj(doc), *reps = yyjson_mut_arr(doc);
            char id[48];
            ap_component_id(c, i, id);
            yyjson_mut_obj_add_strcpy(doc, value, "id", id);
            yyjson_mut_obj_add_strcpy(doc, value, "label", c->nodes[part->representative].name);
            yyjson_mut_obj_add_strcpy(doc, value, "group_id", c->groups[part->overview_group].id);
            yyjson_mut_obj_add_str(doc, value, "basis", ap_component_basis(c, i));
            yyjson_mut_obj_add_str(doc, value, "role",
                                   c->nodes[part->root].test ? "test" : "non_test");
            yyjson_mut_obj_add_int(doc, value, "member_count", part->members);
            yyjson_mut_obj_add_int(doc, value, "file_count", part->files);
            yyjson_mut_arr_add_val(reps, ap_symbol(doc, c, part->representative));
            yyjson_mut_obj_add_val(doc, value, "representatives", reps);
            yyjson_mut_obj_add_int(doc, value, "omitted_members",
                                   part->members - 1);
            yyjson_mut_arr_add_val(parts, value);
            if ((i & 255) == 0 && ap_stopped(c))
                return CBM_STORE_ERR;
        }
        ap_dep *edges = cbm_alloc(CBM_MEM_CLASS_OTHER, ((size_t)c->m + 1) * sizeof(*edges));
        if (!edges)
            return CBM_STORE_ERR;
        int count = mori_architecture_dependencies(c->nodes, c->edges, c->m, c->components,
                                                    true, edges);
        CBM_MSAN_WRITTEN(edges, (size_t)count * sizeof(*edges));
        for (int i = 0; i < count;) {
            int end = i + 1;
            while (end < count && edges[end].source == edges[i].source &&
                   edges[end].target == edges[i].target && edges[end].type == edges[i].type)
                end++;
            connection_count++;
            if (shown < 4096) {
                yyjson_mut_val *value = yyjson_mut_obj(doc), *witnesses = yyjson_mut_arr(doc);
                yyjson_mut_obj_add_strcpy(doc, value, "source", c->groups[edges[i].source].id);
                yyjson_mut_obj_add_strcpy(doc, value, "target", c->groups[edges[i].target].id);
                yyjson_mut_obj_add_str(doc, value, "type", ap_types[edges[i].type]);
                yyjson_mut_obj_add_int(doc, value, "count", end - i);
                yyjson_mut_arr_add_val(witnesses, ap_witness(doc, c, edges[i].edge));
                yyjson_mut_obj_add_val(doc, value, "witnesses", witnesses);
                yyjson_mut_obj_add_int(doc, value, "omitted_witnesses", end - i - 1);
                yyjson_mut_arr_add_val(connections, value);
                shown++;
            }
            i = end;
            if (ap_stopped(c)) {
                cbm_free(CBM_MEM_CLASS_OTHER, edges);
                return CBM_STORE_ERR;
            }
        }
        cbm_free(CBM_MEM_CLASS_OTHER, edges);
    }
    yyjson_mut_obj_add_bool(doc, overview, "complete",
                            !c->limited && shown == connection_count && !omitted_entries);
    yyjson_mut_obj_add_int(doc, totals, "groups", c->limited ? 0 : c->group_count);
    yyjson_mut_obj_add_int(doc, totals, "components", c->limited ? 0 : c->component_count);
    yyjson_mut_obj_add_int(doc, totals, "accounted_nodes", c->limited ? 0 : c->accounted);
    yyjson_mut_obj_add_int(doc, totals, "connections", connection_count);
    yyjson_mut_obj_add_int(doc, limits, "omitted_connections", connection_count - shown);
    yyjson_mut_obj_add_int(doc, limits, "omitted_component_entries", omitted_entries);
    return CBM_STORE_OK;
}

/* SCCs here are on the displayed symbol corridor, never on aggregates. */
static int ap_symbol_cycles(ap_context *c, yyjson_mut_doc *doc, yyjson_mut_val *cycles,
                            const int *out, const int *in, const int *forward, const int *reverse,
                            const bool *nodes, const bool *edges) {
    int *stack = cbm_alloc(CBM_MEM_CLASS_OTHER, ((size_t)c->n + 1) * sizeof(int));
    int *cursor = cbm_alloc(CBM_MEM_CLASS_OTHER, ((size_t)c->n + 1) * sizeof(int));
    int *order = cbm_alloc(CBM_MEM_CLASS_OTHER, ((size_t)c->n + 1) * sizeof(int));
    bool *seen = cbm_calloc(CBM_MEM_CLASS_OTHER, ((size_t)c->n + 1) * sizeof(bool));
    if (!stack || !cursor || !order || !seen) {
        cbm_free(CBM_MEM_CLASS_OTHER, stack);
        cbm_free(CBM_MEM_CLASS_OTHER, cursor);
        cbm_free(CBM_MEM_CLASS_OTHER, order);
        cbm_free(CBM_MEM_CLASS_OTHER, seen);
        return CBM_STORE_ERR;
    }
    memcpy(cursor, out, ((size_t)c->n + 1) * sizeof(int));
    int count = 0, rc = CBM_STORE_OK;
    for (int start = 0; start < c->n; start++)
        if (nodes[start] && !seen[start]) {
            int top = 0;
            stack[top++] = start;
            seen[start] = true;
            while (top) {
                int node = stack[top - 1];
                bool pushed = false;
                while (cursor[node] < out[node + 1]) {
                    int e = forward[cursor[node]++], next = c->edges[e].target;
                    if (edges[e] && !seen[next]) {
                        seen[next] = true;
                        stack[top++] = next;
                        pushed = true;
                        break;
                    }
                }
                if (!pushed) {
                    order[count++] = node;
                    top--;
                }
                if (ap_stopped(c)) {
                    rc = CBM_STORE_ERR;
                    goto done;
                }
            }
        }
    memset(seen, 0, ((size_t)c->n + 1) * sizeof(bool));
    for (int i = count - 1; i >= 0; i--)
        if (!seen[order[i]]) {
            int top = 0, members = 0;
            bool self = false;
            stack[top++] = order[i];
            seen[order[i]] = true;
            yyjson_mut_val *ids = yyjson_mut_arr(doc);
            while (top) {
                int node = stack[--top];
                members++;
                yyjson_mut_arr_add_sint(doc, ids, c->nodes[node].id);
                for (int j = in[node]; j < in[node + 1]; j++) {
                    int e = reverse[j], next = c->edges[e].source;
                    if (!edges[e])
                        continue;
                    if (next == node)
                        self = true;
                    if (!seen[next]) {
                        seen[next] = true;
                        stack[top++] = next;
                    }
                }
                if (ap_stopped(c)) {
                    rc = CBM_STORE_ERR;
                    goto done;
                }
            }
            if (members > 1 || self) {
                yyjson_mut_val *cycle = yyjson_mut_obj(doc);
                yyjson_mut_obj_add_val(doc, cycle, "node_ids", ids);
                yyjson_mut_arr_add_val(cycles, cycle);
            }
        }
done:
    cbm_free(CBM_MEM_CLASS_OTHER, stack);
    cbm_free(CBM_MEM_CLASS_OTHER, cursor);
    cbm_free(CBM_MEM_CLASS_OTHER, order);
    cbm_free(CBM_MEM_CLASS_OTHER, seen);
    return rc;
}

static int ap_behavior(ap_context *c, yyjson_mut_doc *doc, yyjson_mut_val *root,
                       yyjson_mut_val *paths) {
    yyjson_mut_val *view = yyjson_mut_obj(doc), *targets = yyjson_mut_arr(doc),
                   *nodes = yyjson_mut_arr(doc), *edges = yyjson_mut_arr(doc),
                   *cycles = yyjson_mut_arr(doc), *limits = yyjson_mut_arr(doc),
                   *totals = yyjson_mut_obj(doc);
    yyjson_mut_obj_add_val(doc, root, "behavior", view);
    yyjson_mut_obj_add_str(doc, view, "mode", c->options.target_node_id ? "corridor" : "targets");
    yyjson_mut_obj_add_sint(doc, view, "source_id", c->options.entry_node_id);
    if (c->options.target_node_id)
        yyjson_mut_obj_add_sint(doc, view, "target_id", c->options.target_node_id);
    yyjson_mut_obj_add_int(doc, view, "max_hops", c->options.max_depth);
    yyjson_mut_obj_add_val(doc, view, "reachable_targets", targets);
    yyjson_mut_obj_add_val(doc, view, "nodes", nodes);
    yyjson_mut_obj_add_val(doc, view, "edges", edges);
    yyjson_mut_obj_add_val(doc, view, "cycles", cycles);
    yyjson_mut_obj_add_val(doc, view, "limits_hit", limits);
    yyjson_mut_obj_add_val(doc, view, "totals", totals);
    int entry = ap_find(c, c->options.entry_node_id),
        target = ap_find(c, c->options.target_node_id);
    bool valid = !c->limited && entry >= 0 && c->nodes[entry].component >= 0;
    if (!valid || (c->options.target_node_id && (target < 0 || c->nodes[target].component < 0))) {
        yyjson_mut_obj_add_bool(doc, view, "complete", false);
        yyjson_mut_obj_add_bool(doc, view, "corridor_complete", false);
        yyjson_mut_arr_add_str(doc, limits,
                               c->limited ? "analysis_budget"
                               : !valid   ? "select_source"
                                          : "target_not_found");
        yyjson_mut_obj_add_int(doc, totals, "reachable_nodes", 0);
        yyjson_mut_obj_add_int(doc, totals, "corridor_nodes", 0);
        yyjson_mut_obj_add_int(doc, totals, "corridor_edges", 0);
        return CBM_STORE_OK;
    }
    int *out = cbm_calloc(CBM_MEM_CLASS_OTHER, ((size_t)c->n + 2) * sizeof(int)),
        *in = cbm_calloc(CBM_MEM_CLASS_OTHER, ((size_t)c->n + 2) * sizeof(int));
    int *forward = cbm_alloc(CBM_MEM_CLASS_OTHER, ((size_t)c->m + 1) * sizeof(int)),
        *reverse = cbm_alloc(CBM_MEM_CLASS_OTHER, ((size_t)c->m + 1) * sizeof(int));
    int *queue = cbm_alloc(CBM_MEM_CLASS_OTHER, ((size_t)c->n + 1) * sizeof(int)),
        *cursor = cbm_alloc(CBM_MEM_CLASS_OTHER, ((size_t)c->n + 1) * sizeof(int));
    int *distance = cbm_alloc(CBM_MEM_CLASS_OTHER, ((size_t)c->n + 1) * sizeof(int)),
        *back = cbm_alloc(CBM_MEM_CLASS_OTHER, ((size_t)c->n + 1) * sizeof(int));
    int *previous = cbm_alloc(CBM_MEM_CLASS_OTHER, ((size_t)c->n + 1) * sizeof(int)),
        *next = cbm_alloc(CBM_MEM_CLASS_OTHER, ((size_t)c->n + 1) * sizeof(int));
    bool *keep_nodes = cbm_calloc(CBM_MEM_CLASS_OTHER, ((size_t)c->n + 1) * sizeof(bool)),
         *keep_edges = cbm_calloc(CBM_MEM_CLASS_OTHER, ((size_t)c->m + 1) * sizeof(bool));
    int rc = CBM_STORE_OK, reachable = 0, kept_nodes = 0, kept_edges = 0, target_count = 0;
    bool complete = true, corridor_complete = true;
    if (!out || !in || !forward || !reverse || !queue || !cursor || !distance || !back ||
        !previous || !next || !keep_nodes || !keep_edges) {
        rc = CBM_STORE_ERR;
        goto done;
    }
    for (int i = 0; i < c->n; i++)
        distance[i] = back[i] = previous[i] = next[i] = -1;
    for (int i = 0; i < c->m; i++)
        if (ap_execution(c->edges[i].type)) {
            out[c->edges[i].source + 1]++;
            in[c->edges[i].target + 1]++;
        }
    for (int i = 0; i < c->n; i++) {
        out[i + 1] += out[i];
        in[i + 1] += in[i];
    }
    memcpy(cursor, out, ((size_t)c->n + 1) * sizeof(int));
    for (int i = 0; i < c->m; i++)
        if (ap_execution(c->edges[i].type))
            forward[cursor[c->edges[i].source]++] = i;
    memcpy(cursor, in, ((size_t)c->n + 1) * sizeof(int));
    for (int i = 0; i < c->m; i++)
        if (ap_execution(c->edges[i].type))
            reverse[cursor[c->edges[i].target]++] = i;
    int head = 0, tail = 0;
    queue[tail++] = entry;
    distance[entry] = 0;
    while (head < tail) {
        int node = queue[head++];
        if (node != entry) {
            reachable++;
            if (target_count < c->options.max_targets || node == target) {
                yyjson_mut_val *symbol = ap_symbol(doc, c, node);
                yyjson_mut_obj_add_int(doc, symbol, "distance", distance[node]);
                yyjson_mut_arr_add_val(targets, symbol);
                target_count++;
            }
        }
        if (distance[node] < c->options.max_depth)
            for (int j = out[node]; j < out[node + 1]; j++) {
                int edge = forward[j], other = c->edges[edge].target;
                if (distance[other] < 0) {
                    distance[other] = distance[node] + 1;
                    previous[other] = edge;
                    queue[tail++] = other;
                }
            }
        if (ap_stopped(c)) {
            rc = CBM_STORE_ERR;
            goto done;
        }
    }
    if (reachable > target_count) {
        complete = false;
        yyjson_mut_arr_add_str(doc, limits, "target_catalog_budget");
    }
    if (c->options.target_node_id) {
        head = tail = 0;
        queue[tail++] = target;
        back[target] = 0;
        while (head < tail) {
            int node = queue[head++];
            if (back[node] < c->options.max_depth)
                for (int j = in[node]; j < in[node + 1]; j++) {
                    int edge = reverse[j], other = c->edges[edge].source;
                    if (back[other] < 0) {
                        back[other] = back[node] + 1;
                        next[other] = edge;
                        queue[tail++] = other;
                    }
                }
            if (ap_stopped(c)) {
                rc = CBM_STORE_ERR;
                goto done;
            }
        }
        for (int i = 0; i < c->n; i++)
            if (distance[i] >= 0 && back[i] >= 0 && distance[i] + back[i] <= c->options.max_depth) {
                keep_nodes[i] = true;
                kept_nodes++;
            }
        for (int i = 0; i < c->m; i++) {
            ap_edge *edge = &c->edges[i];
            if (ap_execution(edge->type) && distance[edge->source] >= 0 &&
                back[edge->target] >= 0 &&
                distance[edge->source] + 1 + back[edge->target] <= c->options.max_depth) {
                keep_edges[i] = true;
                kept_edges++;
            }
        }
        if (kept_nodes > c->options.max_corridor_nodes ||
            kept_edges > c->options.max_corridor_edges) {
            complete = corridor_complete = false;
            yyjson_mut_arr_add_str(doc, limits, "corridor_budget");
            /* Abstain instead of returning a broken graph with hidden joins. */
        } else {
            c->evidence_scope = AP_EVIDENCE_CORRIDOR;
            for (int i = 0; i < c->n; i++)
                if (keep_nodes[i])
                    yyjson_mut_arr_add_val(nodes, ap_symbol(doc, c, i));
            for (int i = 0; i < c->m; i++)
                if (keep_edges[i]) {
                    yyjson_mut_arr_add_val(edges, ap_path_edge(doc, c, i));
                    if (ap_stopped(c)) {
                        rc = CBM_STORE_ERR;
                        goto done;
                    }
                }
            if (ap_symbol_cycles(c, doc, cycles, out, in, forward, reverse, keep_nodes,
                                 keep_edges) != CBM_STORE_OK) {
                rc = CBM_STORE_ERR;
                goto done;
            }
            /* Each witness joins a shortest prefix, a real corridor edge, and
             * a shortest suffix. These bounded graph walks may revisit a node;
             * they never imply feasible branch conditions or runtime order. */
            c->evidence_scope = AP_EVIDENCE_PATH;
            int saved[32][16], lengths[32], produced = 0;
            for (int e = -1; e < c->m && produced < c->options.max_paths; e++) {
                if (e < 0 && distance[target] < 0)
                    continue;
                if (e >= 0 && !keep_edges[e])
                    continue;
                int chain[16], prefix[16], length = 0, before = 0;
                int node = e < 0 ? target : c->edges[e].source;
                while (previous[node] >= 0 && before < 16) {
                    prefix[before++] = previous[node];
                    node = c->edges[previous[node]].source;
                }
                for (int i = before - 1; i >= 0; i--)
                    chain[length++] = prefix[i];
                if (e >= 0) {
                    chain[length++] = e;
                    node = c->edges[e].target;
                    while (next[node] >= 0 && length < 16) {
                        chain[length++] = next[node];
                        node = c->edges[next[node]].target;
                    }
                }
                bool duplicate = false;
                for (int i = 0; i < produced; i++)
                    if (lengths[i] == length &&
                        !memcmp(saved[i], chain, (size_t)length * sizeof(int)))
                        duplicate = true;
                if (duplicate)
                    continue;
                memcpy(saved[produced], chain, (size_t)length * sizeof(int));
                lengths[produced++] = length;
                yyjson_mut_val *path = yyjson_mut_obj(doc), *pn = yyjson_mut_arr(doc),
                               *pe = yyjson_mut_arr(doc);
                yyjson_mut_obj_add_sint(doc, path, "entrypoint_id", c->nodes[entry].id);
                yyjson_mut_arr_add_val(pn, ap_symbol(doc, c, entry));
                for (int i = 0; i < length; i++) {
                    yyjson_mut_arr_add_val(pn, ap_symbol(doc, c, c->edges[chain[i]].target));
                    yyjson_mut_arr_add_val(pe, ap_path_edge(doc, c, chain[i]));
                }
                yyjson_mut_obj_add_val(doc, path, "nodes", pn);
                yyjson_mut_obj_add_val(doc, path, "edges", pe);
                yyjson_mut_arr_add_val(paths, path);
                if (ap_stopped(c)) {
                    rc = CBM_STORE_ERR;
                    goto done;
                }
            }
        }
        yyjson_mut_obj_add_bool(doc, view, "reachable", distance[target] >= 0);
    }
    yyjson_mut_obj_add_bool(doc, view, "complete", complete);
    yyjson_mut_obj_add_bool(doc, view, "corridor_complete", corridor_complete);
    yyjson_mut_obj_add_int(doc, totals, "reachable_nodes", reachable);
    yyjson_mut_obj_add_int(doc, totals, "omitted_targets", reachable - target_count);
    yyjson_mut_obj_add_int(doc, totals, "corridor_nodes", kept_nodes);
    yyjson_mut_obj_add_int(doc, totals, "corridor_edges", kept_edges);
done:
    c->evidence_scope = AP_EVIDENCE_CONTEXT;
    cbm_free(CBM_MEM_CLASS_OTHER, out);
    cbm_free(CBM_MEM_CLASS_OTHER, in);
    cbm_free(CBM_MEM_CLASS_OTHER, forward);
    cbm_free(CBM_MEM_CLASS_OTHER, reverse);
    cbm_free(CBM_MEM_CLASS_OTHER, queue);
    cbm_free(CBM_MEM_CLASS_OTHER, cursor);
    cbm_free(CBM_MEM_CLASS_OTHER, distance);
    cbm_free(CBM_MEM_CLASS_OTHER, back);
    cbm_free(CBM_MEM_CLASS_OTHER, previous);
    cbm_free(CBM_MEM_CLASS_OTHER, next);
    cbm_free(CBM_MEM_CLASS_OTHER, keep_nodes);
    cbm_free(CBM_MEM_CLASS_OTHER, keep_edges);
    return rc;
}

static int ap_render(ap_context *c, const char *project, char **out_json) {
    memset(c->evidence_bytes, 0, sizeof(c->evidence_bytes));
    c->evidence_scope = AP_EVIDENCE_CONTEXT;
    c->evidence_omitted = false;
    c->cycle_count = c->omitted_cycles = 0;
    ap_json_budget budget = {0};
    yyjson_alc allocator = {ap_json_malloc, ap_json_realloc, ap_json_free, &budget};
    yyjson_mut_doc *doc = yyjson_mut_doc_new(&allocator);
    if (!doc)
        return CBM_STORE_ERR;
    yyjson_mut_val *root = yyjson_mut_obj(doc);
    yyjson_mut_doc_set_root(doc, root);
    yyjson_mut_val *parts = yyjson_mut_arr(doc), *dependencies = yyjson_mut_arr(doc),
                   *cycles = yyjson_mut_arr(doc), *entries = yyjson_mut_arr(doc),
                   *paths = yyjson_mut_arr(doc), *warnings = yyjson_mut_arr(doc),
                   *totals = yyjson_mut_obj(doc), *limits = yyjson_mut_obj(doc);
    yyjson_mut_obj_add_int(doc, root, "schema_version", 1);
    yyjson_mut_obj_add_str(doc, root, "kind", "static_projection");
    yyjson_mut_obj_add_strcpy(doc, root, "project", project);
    yyjson_mut_obj_add_val(doc, root, "components", parts);
    yyjson_mut_obj_add_val(doc, root, "dependencies", dependencies);
    yyjson_mut_obj_add_val(doc, root, "cycles", cycles);
    yyjson_mut_obj_add_val(doc, root, "entrypoints", entries);
    yyjson_mut_obj_add_val(doc, root, "paths", paths);
    yyjson_mut_obj_add_val(doc, root, "warnings", warnings);
    yyjson_mut_obj_add_val(doc, root, "totals", totals);
    yyjson_mut_obj_add_val(doc, root, "limits", limits);
    int shown = 0, dependency_count = 0, dependency_shown = 0, entry_count = 0, entry_total = 0;
    int entry_indexes[AP_ENTRYPOINTS];
    bool paths_truncated = false;
    int rc = CBM_STORE_OK;
    ap_dep *deps = NULL;
    if (!c->limited) {
        shown = mori_architecture_rank(c->components, c->component_count, c->options.max_components);
        if (shown < 0 || ap_stopped(c)) {
            rc = CBM_STORE_ERR;
            goto done;
        }
        CBM_MSAN_WRITTEN(c->components, (size_t)c->component_count * sizeof(*c->components));
    }
    if (ap_overview(c, doc, root) != CBM_STORE_OK) {
        rc = CBM_STORE_ERR;
        goto done;
    }
    if (!c->limited) {
        for (int i = 0; i < c->component_count; i++) {
            ap_component *p = &c->components[i];
            if (p->displayed < 0)
                continue;
            yyjson_mut_val *part = yyjson_mut_obj(doc), *representatives = yyjson_mut_arr(doc);
            char id[48];
            ap_component_id(c, i, id);
            yyjson_mut_obj_add_strcpy(doc, part, "id", id);
            yyjson_mut_obj_add_strcpy(doc, part, "label", c->nodes[p->representative].name);
            yyjson_mut_obj_add_str(doc, part, "basis",
                                   p->atoms > 1                           ? "interaction_community"
                                   : c->nodes[p->representative].declared ? "declared_module"
                                                                          : "unassigned");
            yyjson_mut_obj_add_int(doc, part, "member_count", p->members);
            yyjson_mut_obj_add_int(doc, part, "file_count", p->files);
            yyjson_mut_obj_add_int(doc, part, "atom_count", p->atoms);
            bool test = c->nodes[p->root].test;
            yyjson_mut_obj_add_str(doc, part, "role", test ? "test" : "non_test");
            yyjson_mut_obj_add_str(doc, part, "role_basis",
                                   !test                             ? "no_test_evidence"
                                   : c->nodes[p->root].test_property ? "indexed_is_test"
                                                                     : "test_path_convention");
            yyjson_mut_arr_add_val(representatives, ap_symbol(doc, c, p->representative));
            yyjson_mut_obj_add_val(doc, part, "representatives", representatives);
            yyjson_mut_obj_add_int(doc, part, "omitted_members",
                                   p->members - 1);
            yyjson_mut_arr_add_val(parts, part);
        }
        deps = cbm_alloc(CBM_MEM_CLASS_OTHER, ((size_t)c->m + 1) * sizeof(*deps));
        if (!deps) {
            rc = CBM_STORE_ERR;
            goto done;
        }
        int count = mori_architecture_dependencies(c->nodes, c->edges, c->m, c->components,
                                                    false, deps);
        CBM_MSAN_WRITTEN(deps, (size_t)count * sizeof(*deps));
        for (int i = 0; i < count;) {
            int end = i + 1;
            while (end < count && deps[end].source == deps[i].source &&
                   deps[end].target == deps[i].target && deps[end].type == deps[i].type)
                end++;
            dependency_count++;
            if (dependency_shown < c->options.max_dependencies &&
                c->components[deps[i].source].displayed >= 0 &&
                c->components[deps[i].target].displayed >= 0) {
                yyjson_mut_val *dep = yyjson_mut_obj(doc), *witnesses = yyjson_mut_arr(doc);
                char a[48], b[48];
                ap_component_id(c, deps[i].source, a);
                ap_component_id(c, deps[i].target, b);
                yyjson_mut_obj_add_strcpy(doc, dep, "source", a);
                yyjson_mut_obj_add_strcpy(doc, dep, "target", b);
                yyjson_mut_obj_add_str(doc, dep, "type", ap_types[deps[i].type]);
                yyjson_mut_obj_add_int(doc, dep, "count", end - i);
                yyjson_mut_arr_add_val(witnesses, ap_witness(doc, c, deps[i].edge));
                yyjson_mut_obj_add_val(doc, dep, "witnesses", witnesses);
                yyjson_mut_obj_add_int(doc, dep, "omitted_witnesses", end - i - 1);
                yyjson_mut_arr_add_val(dependencies, dep);
                dependency_shown++;
            }
            i = end;
            if (ap_stopped(c)) {
                rc = CBM_STORE_ERR;
                goto done;
            }
        }
        if (ap_cycles(c, doc, cycles, deps, count) != CBM_STORE_OK) {
            rc = CBM_STORE_ERR;
            goto done;
        }
        /* Indexed entry points first, then route handlers, each in id order:
         * paths take their slots in this order, so many handlers cannot crowd
         * main out of the default paths. */
        enum { AP_ENTRY_INDEXED, AP_ENTRY_HANDLER, AP_ENTRY_PASSES };
        for (int pass = AP_ENTRY_INDEXED; pass < AP_ENTRY_PASSES; pass++) {
            for (int i = 0; i < c->n; i++) {
                ap_node *node = &c->nodes[i];
                if (node->component < 0 ||
                    !(pass == AP_ENTRY_HANDLER ? node->handler : node->entry)) {
                    continue;
                }
                entry_total++;
                if (entry_count < AP_ENTRYPOINTS) {
                    entry_indexes[entry_count++] = i;
                    c->evidence_scope = node->id == c->options.entry_node_id ? AP_EVIDENCE_PATH
                                                                             : AP_EVIDENCE_CONTEXT;
                    yyjson_mut_arr_add_val(entries, ap_symbol(doc, c, i));
                    c->evidence_scope = AP_EVIDENCE_CONTEXT;
                }
            }
        }
        c->evidence_scope = AP_EVIDENCE_PATH;
        if (c->options.target_node_id) {
            /* The target query produces exact endpoint witnesses below. */
        } else if (c->options.entry_node_id) {
            int selected = ap_find(c, c->options.entry_node_id);
            if (selected >= 0 && c->nodes[selected].component >= 0) {
                if (ap_paths(c, doc, paths, &selected, 1, &paths_truncated) != CBM_STORE_OK) {
                    rc = CBM_STORE_ERR;
                    goto done;
                }
            } else
                yyjson_mut_arr_add_str(
                    doc, warnings,
                    "The selected entry point is absent from this indexed generation.");
        } else if (ap_paths(c, doc, paths, entry_indexes, entry_count, &paths_truncated) !=
                   CBM_STORE_OK) {
            rc = CBM_STORE_ERR;
            goto done;
        }
        c->evidence_scope = AP_EVIDENCE_CONTEXT;
    }
    if (ap_behavior(c, doc, root, paths) != CBM_STORE_OK) {
        rc = CBM_STORE_ERR;
        goto done;
    }
    yyjson_mut_obj_add_str(doc, root, "status", c->limited ? "limited" : "ready");
    bool complete =
        !c->limited && shown == c->component_count && dependency_shown == dependency_count;
    yyjson_mut_obj_add_bool(doc, root, "complete", complete);
    yyjson_mut_obj_add_sint(doc, totals, "nodes", c->total_nodes);
    yyjson_mut_obj_add_sint(doc, totals, "files", c->total_files);
    yyjson_mut_obj_add_sint(doc, totals, "edges", c->total_edges);
    yyjson_mut_obj_add_int(doc, totals, "accounted_nodes", c->limited ? 0 : c->accounted);
    yyjson_mut_obj_add_int(doc, totals, "structural_nodes", c->limited ? 0 : c->structural);
    yyjson_mut_obj_add_int(doc, totals, "components", c->limited ? 0 : c->component_count);
    yyjson_mut_obj_add_int(doc, totals, "dependencies", dependency_count);
    yyjson_mut_obj_add_int(doc, totals, "entrypoints", entry_total);
    yyjson_mut_obj_add_int(doc, totals, "cycles", c->limited ? 0 : c->cycle_count);
    yyjson_mut_obj_add_int(doc, totals, "relationship_edges",
                           c->limited ? 0 : c->relationship_edges);
    yyjson_mut_obj_add_int(doc, totals, "unmodeled_edges", c->limited ? 0 : c->unmodeled_edges);
    yyjson_mut_obj_add_int(doc, limits, "node_budget", c->options.max_nodes);
    yyjson_mut_obj_add_int(doc, limits, "edge_budget", c->options.max_edges);
    yyjson_mut_obj_add_int(doc, limits, "omitted_components",
                           c->limited ? 0 : c->component_count - shown);
    yyjson_mut_obj_add_int(doc, limits, "omitted_dependencies",
                           dependency_count - dependency_shown);
    yyjson_mut_obj_add_int(doc, limits, "omitted_entrypoints", entry_total - entry_count);
    yyjson_mut_obj_add_int(doc, limits, "omitted_cycles", c->limited ? 0 : c->omitted_cycles);
    yyjson_mut_obj_add_bool(doc, limits, "paths_truncated", paths_truncated);
    yyjson_mut_obj_add_int(doc, limits, "path_depth", c->options.max_depth);
    yyjson_mut_obj_add_uint(doc, root, "elapsed_ms", cbm_now_ms() - c->started);
    yyjson_mut_arr_add_str(
        doc, warnings,
        "Component candidates are inferred from indexed declarations and interactions; they do not "
        "establish intended responsibilities or deployment boundaries.");
    yyjson_mut_arr_add_str(
        doc, warnings,
        "Paths are bounded static witnesses, not observed executions. Callback references are not "
        "invocation evidence. Index coverage gaps remain possible.");
    if (c->unmodeled_edges && !c->limited)
        yyjson_mut_arr_add_str(doc, warnings,
                               "Some indexed edge types are outside this projection's relationship "
                               "vocabulary; see unmodeled_edges. Similarity and change-history "
                               "edges are not structural interactions.");
    if (c->limited)
        yyjson_mut_arr_add_str(doc, warnings, c->limited);
    if (!complete && !c->limited)
        yyjson_mut_arr_add_str(doc, warnings,
                               "The displayed projection is capped; totals include omitted "
                               "components and dependencies.");
    if (c->omitted_cycles && !c->limited)
        yyjson_mut_arr_add_str(
            doc, warnings,
            "Dependency cycles involving omitted components are not drawn; see omitted_cycles.");
    if (c->options.include_behavior_evidence) {
        bool evidence_limited =
            c->evidence_omitted || c->metadata_budget.failed || c->evidence_suppressed;
        yyjson_mut_obj_add_bool(doc, limits, "behavior_evidence_budget_hit", evidence_limited);
        if (c->evidence_suppressed)
            yyjson_mut_arr_add_str(doc, warnings,
                                   "Optional behavior evidence was omitted entirely to keep the "
                                   "base graph within the response memory budget.");
        else if (evidence_limited)
            yyjson_mut_arr_add_str(
                doc, warnings,
                "Optional behavior evidence was omitted from some symbols or calls after reaching "
                "its memory budget; absent fields remain unknown.");
    }
    if (budget.failed)
        rc = CBM_STORE_SCAN_LIMIT;
    else {
        *out_json =
            yyjson_mut_write_opts(doc, YYJSON_WRITE_ALLOW_INVALID_UNICODE, &allocator, NULL, NULL);
        if (!*out_json)
            rc = budget.failed ? CBM_STORE_SCAN_LIMIT : CBM_STORE_ERR;
    }
done:
    cbm_free(CBM_MEM_CLASS_OTHER, deps);
    yyjson_mut_doc_free(doc);
    return rc;
}

static int ap_project_snapshot(void *context) {
    ap_context *c = context;
    const char *project = c->project;
    char **out_json = c->out_json;
    int rc = ap_counts(c, project);
    if (rc == CBM_STORE_OK && !c->limited)
        rc = ap_load(c, project);
    if (rc == CBM_STORE_OK && !c->limited)
        rc = ap_partition_graph(c);
    if (c->expired) {
        c->limited = "Analysis reached its deadline; no partial architecture was inferred.";
        rc = CBM_STORE_OK;
    }
    if (c->limited && !c->cancelled)
        rc = CBM_STORE_OK;
    if (rc == CBM_STORE_OK && !c->cancelled)
        rc = ap_render(c, project, out_json);
    if (rc == CBM_STORE_SCAN_LIMIT && c->options.include_behavior_evidence && !c->limited &&
        !c->expired && !c->cancelled && !*out_json) {
        /* Reuse the loaded graph. Optional evidence must not turn a renderable
         * base projection into an empty architecture or trigger another scan. */
        c->evidence_suppressed = true;
        rc = ap_render(c, project, out_json);
    }
    if ((rc == CBM_STORE_SCAN_LIMIT || c->expired) && !c->cancelled && !*out_json) {
        c->limited = c->expired
                        ? "Analysis reached its deadline; no partial architecture was inferred."
                        : "The architecture response exceeded its memory budget; narrow the "
                          "requested projection.";
        rc = ap_render(c, project, out_json);
    }
    sqlite3_finalize(c->provenance);
    sqlite3_finalize(c->symbol_metadata);
    return rc;
}

int cbm_store_architecture_projection(cbm_store_t *store, const char *project,
                                      const cbm_architecture_projection_options_t *options,
                                      char **out_json) {
    if (!store || !project || !project[0] || !out_json)
        return CBM_STORE_ERR;
    *out_json = NULL;
    ap_context c = {
        .db = cbm_store_get_db(store), .project = project, .out_json = out_json,
        .started = cbm_now_ms(),
    };
    if (options)
        c.options = *options;
    if (c.options.entry_node_id < 0 || c.options.target_node_id < 0 ||
        (c.options.target_node_id && !c.options.entry_node_id))
        return CBM_STORE_ERR;
    c.options.max_nodes = ap_bound(c.options.max_nodes, 100000, 250000);
    c.options.max_edges = ap_bound(c.options.max_edges, 500000, 1000000);
    c.options.max_components = ap_bound(c.options.max_components, 256, 512);
    c.options.max_dependencies = ap_bound(c.options.max_dependencies, 2048, 4096);
    c.options.max_paths = ap_bound(c.options.max_paths, 12, 32);
    c.options.max_depth = ap_bound(c.options.max_depth, 10, 16);
    c.options.max_targets = ap_bound(c.options.max_targets, 256, 1024);
    c.options.max_corridor_nodes = ap_bound(c.options.max_corridor_nodes, 1024, 4096);
    c.options.max_corridor_edges = ap_bound(c.options.max_corridor_edges, 4096, 16384);
    if (!c.options.deadline_ms)
        c.options.deadline_ms = c.started + 5000;
    if (ap_stopped(&c))
        return c.cancelled ? CBM_STORE_CANCELLED : CBM_STORE_SCAN_LIMIT;
    int rc = mori_architecture_snapshot(c.db, &c, ap_stopped, ap_project_snapshot);
    for (int i = 0; i < c.n; i++) {
        cbm_free(CBM_MEM_CLASS_OTHER, c.nodes[i].name);
        cbm_free(CBM_MEM_CLASS_OTHER, c.nodes[i].qn);
        cbm_free(CBM_MEM_CLASS_OTHER, c.nodes[i].file);
        cbm_free(CBM_MEM_CLASS_OTHER, c.nodes[i].label);
        if (c.symbol_properties)
            yyjson_doc_free(c.symbol_properties[i]);
    }
    cbm_free(CBM_MEM_CLASS_OTHER, c.symbol_properties);
    cbm_free(CBM_MEM_CLASS_OTHER, c.symbol_properties_loaded);
    for (int i = 0; i < c.group_count; i++) {
        cbm_free(CBM_MEM_CLASS_OTHER, (void *)c.groups[i].id);
        cbm_free(CBM_MEM_CLASS_OTHER, (void *)c.groups[i].label);
    }
    cbm_free(CBM_MEM_CLASS_OTHER, c.nodes);
    cbm_free(CBM_MEM_CLASS_OTHER, c.edges);
    cbm_free(CBM_MEM_CLASS_OTHER, c.components);
    if (c.cancelled) {
        cbm_free(CBM_MEM_CLASS_OTHER, *out_json);
        *out_json = NULL;
        return CBM_STORE_CANCELLED;
    }
    return rc;
}
