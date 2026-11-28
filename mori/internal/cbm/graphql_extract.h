#ifndef CBM_GRAPHQL_EXTRACT_H
#define CBM_GRAPHQL_EXTRACT_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <tree_sitter/api.h>

typedef struct {
    uint32_t start;
    uint32_t end;
    bool present;
} cbm_graphql_span_t;

typedef struct {
    cbm_graphql_span_t name;
    cbm_graphql_span_t type;
} cbm_graphql_field_spans_t;

/* The Rust extractor borrows the native tree. A missing child has present=false. */
cbm_graphql_field_spans_t mori_graphql_extract_field_spans(TSNode node);
/* Mutates an arena-owned copy, leaving byte offsets and line ends unchanged. */
void cbm_graphql_set_parse_masker(void (*mask)(char *, size_t));

typedef void (*cbm_graphql_schema_emit_t)(void *, bool, cbm_graphql_field_spans_t);

/* Emit schema roots in source order. A schema definition sets the boolean
 * without emitting a root. The context and tree are valid only for this call. */
void cbm_graphql_set_schema_extractor(
    void (*extract)(TSNode, cbm_graphql_schema_emit_t, void *));

#endif
