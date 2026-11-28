// Compile vendored tree-sitter runtime as a single compilation unit.
// Source: tree-sitter v0.26.0 (DeusData fork)
//
// lib.c internally #includes all other runtime .c files, so we only
// need this one entry point. The runtime headers are at vendored/ts_runtime/src/.
#include "vendored/ts_runtime/src/lib.c"
#include "native_tree_ffi.h"

/* Explicit pointers let instrumented C mark the exact node it will read.
 * A by-value TSNode may be copied indirectly by the caller on AArch64. */
const char *cbm_ffi_ts_node_type(const TSNode *node) {
    cbm_ffi_ts_node_written(node);
    return ts_node_type(*node);
}

cbm_ts_node_byte_range cbm_ffi_ts_node_byte_range(const TSNode *node) {
    cbm_ffi_ts_node_written(node);
    return (cbm_ts_node_byte_range){ts_node_start_byte(*node), ts_node_end_byte(*node)};
}

uint32_t cbm_ffi_ts_node_child_count(const TSNode *node) {
    cbm_ffi_ts_node_written(node);
    return ts_node_child_count(*node);
}

TSNode cbm_ffi_ts_node_child(const TSNode *node, uint32_t index) {
    cbm_ffi_ts_node_written(node);
    return ts_node_child(*node, index);
}

bool cbm_ffi_ts_node_is_named(const TSNode *node) {
    cbm_ffi_ts_node_written(node);
    return ts_node_is_named(*node);
}

uint32_t cbm_ffi_ts_node_named_child_count(const TSNode *node) {
    cbm_ffi_ts_node_written(node);
    return ts_node_named_child_count(*node);
}

TSNode cbm_ffi_ts_node_parent(const TSNode *node) {
    cbm_ffi_ts_node_written(node);
    return ts_node_parent(*node);
}

bool cbm_ffi_ts_node_eq(const TSNode *left, const TSNode *right) {
    cbm_ffi_ts_node_written(left);
    cbm_ffi_ts_node_written(right);
    return ts_node_eq(*left, *right);
}

TSNode cbm_ffi_ts_node_child_by_field_name(const TSNode *node, const char *name, uint32_t length) {
    cbm_ffi_ts_node_written(node);
    return ts_node_child_by_field_name(*node, name, length);
}

TSTreeCursor cbm_ffi_ts_tree_cursor_new(const TSNode *node) {
    cbm_ffi_ts_node_written(node);
    return ts_tree_cursor_new(*node);
}

static void cbm_ffi_ts_cursor_written(const TSTreeCursor *cursor) {
    (void)cursor;
#if __has_feature(memory_sanitizer)
    /* Mark the live fields of the actual cursor layout, leaving padding alone. */
    const TreeCursor *inner = (const TreeCursor *)cursor;
    CBM_MSAN_WRITTEN(&inner->tree, sizeof(inner->tree));
    CBM_MSAN_WRITTEN(&inner->stack.contents, sizeof(inner->stack.contents));
    CBM_MSAN_WRITTEN(&inner->stack.size, sizeof(inner->stack.size));
    CBM_MSAN_WRITTEN(&inner->stack.capacity, sizeof(inner->stack.capacity));
    CBM_MSAN_WRITTEN(&inner->root_alias_symbol, sizeof(inner->root_alias_symbol));
#endif
}

void cbm_ffi_ts_tree_cursor_reset_to(TSTreeCursor *cursor, const TSTreeCursor *source) {
    cbm_ffi_ts_cursor_written(cursor);
    cbm_ffi_ts_cursor_written(source);
    ts_tree_cursor_reset_to(cursor, source);
}

void cbm_ffi_ts_tree_cursor_delete(TSTreeCursor *cursor) {
    cbm_ffi_ts_cursor_written(cursor);
    ts_tree_cursor_delete(cursor);
}

TSNode cbm_ffi_ts_tree_cursor_current_node(const TSTreeCursor *cursor) {
    cbm_ffi_ts_cursor_written(cursor);
    return ts_tree_cursor_current_node(cursor);
}

const char *cbm_ffi_ts_tree_cursor_current_field_name(const TSTreeCursor *cursor) {
    cbm_ffi_ts_cursor_written(cursor);
    return ts_tree_cursor_current_field_name(cursor);
}

bool cbm_ffi_ts_tree_cursor_goto_first_child(TSTreeCursor *cursor) {
    cbm_ffi_ts_cursor_written(cursor);
    return ts_tree_cursor_goto_first_child(cursor);
}

bool cbm_ffi_ts_tree_cursor_goto_next_sibling(TSTreeCursor *cursor) {
    cbm_ffi_ts_cursor_written(cursor);
    return ts_tree_cursor_goto_next_sibling(cursor);
}

bool cbm_ffi_ts_tree_cursor_goto_parent(TSTreeCursor *cursor) {
    cbm_ffi_ts_cursor_written(cursor);
    return ts_tree_cursor_goto_parent(cursor);
}
