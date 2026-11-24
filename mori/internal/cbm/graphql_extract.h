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

/* A missing child has present=false. The callback borrows the native tree. */
void cbm_graphql_set_field_extractor(cbm_graphql_field_spans_t (*extract)(TSNode));
/* Mutates an arena-owned copy, leaving byte offsets and line ends unchanged. */
void cbm_graphql_set_parse_masker(void (*mask)(char *, size_t));

#endif
