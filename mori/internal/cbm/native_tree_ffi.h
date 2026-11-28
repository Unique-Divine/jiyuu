#ifndef CBM_NATIVE_TREE_FFI_H
#define CBM_NATIVE_TREE_FFI_H

#include "../../src/foundation/sanitized.h"
#include "tree_sitter/api.h"

/* Rust may have moved a native node through uninstrumented storage. */
static inline void cbm_ffi_ts_node_written(const TSNode *node) {
    (void)node;
    CBM_MSAN_WRITTEN(&node->context, sizeof(node->context));
    CBM_MSAN_WRITTEN(&node->id, sizeof(node->id));
    CBM_MSAN_WRITTEN(&node->tree, sizeof(node->tree));
}

const char *cbm_ffi_ts_node_type(const TSNode *node);
typedef struct {
    uint32_t start;
    uint32_t end;
} cbm_ts_node_byte_range;
cbm_ts_node_byte_range cbm_ffi_ts_node_byte_range(const TSNode *node);
uint32_t cbm_ffi_ts_node_child_count(const TSNode *node);
TSNode cbm_ffi_ts_node_child(const TSNode *node, uint32_t index);
bool cbm_ffi_ts_node_is_named(const TSNode *node);
uint32_t cbm_ffi_ts_node_named_child_count(const TSNode *node);
TSNode cbm_ffi_ts_node_parent(const TSNode *node);
bool cbm_ffi_ts_node_eq(const TSNode *left, const TSNode *right);
TSNode cbm_ffi_ts_node_child_by_field_name(const TSNode *node, const char *name, uint32_t length);

TSTreeCursor cbm_ffi_ts_tree_cursor_new(const TSNode *node);
void cbm_ffi_ts_tree_cursor_reset_to(TSTreeCursor *cursor, const TSTreeCursor *source);
void cbm_ffi_ts_tree_cursor_delete(TSTreeCursor *cursor);
TSNode cbm_ffi_ts_tree_cursor_current_node(const TSTreeCursor *cursor);
const char *cbm_ffi_ts_tree_cursor_current_field_name(const TSTreeCursor *cursor);
bool cbm_ffi_ts_tree_cursor_goto_first_child(TSTreeCursor *cursor);
bool cbm_ffi_ts_tree_cursor_goto_next_sibling(TSTreeCursor *cursor);
bool cbm_ffi_ts_tree_cursor_goto_parent(TSTreeCursor *cursor);

#endif
