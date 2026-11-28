#ifndef CBM_DEFINITION_PROPS_H
#define CBM_DEFINITION_PROPS_H

#include "cbm.h"
#include "foundation/mem_core.h"
#include "graph_buffer/graph_buffer.h"
#include "simhash/minhash.h"

/* File-result strings and arrays remain borrowed during serialization. */
typedef struct {
    const char *label;
    const char *call_selector;
    const char *docstring;
    const char *variants;
    const char *variant_key;
    const char *signature;
    const char *return_type;
    const char *parent_class;
    const char *route_path;
    const char *route_method;
    const char *client_route_path;
    const char *http_client;
    const char *http_base_url;
    const char *structural_profile;
    const char *body_tokens;
    const char *const *decorators;
    const char *const *base_classes;
    const char *const *param_names;
    const char *const *param_types;
    const char *const *extra_props;
    const uint32_t *fingerprint;
    uint64_t param_default_mask;
    int complexity;
    int cognitive;
    int loop_count;
    int loop_depth;
    int param_count;
    int max_access_depth;
    int linear_scan_in_loop;
    int alloc_in_loop;
    int lines;
    int test_role;
    uint8_t selector_param_count;
    bool has_signature;
    bool is_recursive;
    bool recursion_in_loop;
    bool unguarded_recursion;
    bool is_exported;
    bool is_test;
    bool is_entry_point;
} cbm_definition_props_input_t;

typedef char *(*cbm_props_alloc_fn)(size_t);
char *cbm_rust_definition_props(const cbm_definition_props_input_t *input, char *stack,
                                size_t stack_size, cbm_props_alloc_fn allocate);
char *cbm_rust_file_doc_props(const char *old, const char *doc, cbm_props_alloc_fn allocate);

static inline char *cbm_props_allocate(size_t size) {
    return cbm_alloc(CBM_MEM_CLASS_GBUF_STRING, size);
}

static inline char *cbm_definition_props(const CBMDefinition *def, char *stack, size_t stack_size) {
    const cbm_definition_props_input_t input = {
        .label = def->label,
        .call_selector = def->call_selector,
        .docstring = def->docstring,
        .variants = def->variants,
        .variant_key = def->variant_key,
        .signature = def->signature,
        .return_type = def->return_type,
        .parent_class = def->parent_class,
        .route_path = def->route_path,
        .route_method = def->route_method,
        .client_route_path = def->client_route_path,
        .http_client = def->http_client,
        .http_base_url = def->http_base_url,
        .structural_profile = def->structural_profile,
        .body_tokens = def->body_tokens,
        .decorators = def->decorators,
        .base_classes = def->base_classes,
        .param_names = def->param_names,
        .param_types = def->param_types,
        .extra_props = def->extra_props,
        .fingerprint = def->fingerprint_k == CBM_MINHASH_K ? def->fingerprint : NULL,
        .param_default_mask = def->param_default_mask,
        .complexity = def->complexity,
        .cognitive = def->cognitive,
        .loop_count = def->loop_count,
        .loop_depth = def->loop_depth,
        .param_count = def->param_count,
        .max_access_depth = def->max_access_depth,
        .linear_scan_in_loop = def->linear_scan_in_loop,
        .alloc_in_loop = def->alloc_in_loop,
        .lines = def->lines,
        .test_role = def->test_role,
        .selector_param_count = def->selector_param_count,
        .has_signature = def->qn_sig_off != 0,
        .is_recursive = def->is_recursive,
        .recursion_in_loop = def->recursion_in_loop,
        .unguarded_recursion = def->unguarded_recursion,
        .is_exported = def->is_exported,
        .is_test = def->is_test,
        .is_entry_point = def->is_entry_point,
    };
    return cbm_rust_definition_props(&input, stack, stack_size, cbm_props_allocate);
}

static inline void cbm_add_file_doc(const cbm_gbuf_node_t *file, const char *doc) {
    if (!file || !doc || !doc[0]) {
        return;
    }
    char *props = cbm_rust_file_doc_props(file->properties_json, doc, cbm_props_allocate);
    if (props) {
        (void)cbm_gbuf_node_set_properties_json((cbm_gbuf_node_t *)file, props);
        cbm_free(CBM_MEM_CLASS_GBUF_STRING, props);
    }
}

#endif
