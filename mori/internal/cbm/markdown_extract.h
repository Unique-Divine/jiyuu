#ifndef CBM_MARKDOWN_EXTRACT_H
#define CBM_MARKDOWN_EXTRACT_H

#include "cbm.h"

/* All input and output views are borrowed for the duration of the call.
 * The result sink copies emitted text into the C result arena. */
typedef void (*cbm_markdown_section_sink)(void *ctx, const TSNode *heading, const char *name,
                                          size_t name_len, const char *segment, size_t segment_len,
                                          const char *body, size_t body_len);
typedef bool (*cbm_markdown_diagram_labels)(void *ctx, const TSNode *fence, const char **labels);

typedef struct {
    uint64_t collection_steps;
    uint64_t name_checks;
} cbm_markdown_work_t;

/* Charge Rust-owned scratch capacity to the C runtime's ARENA class. */
void cbm_markdown_scratch_change(size_t bytes, bool release, bool publish_peak);

cbm_markdown_work_t mori_markdown_extract_sections(TSNode root, const char *source,
                                                   size_t source_len, size_t body_cap, void *ctx,
                                                   cbm_markdown_section_sink emit,
                                                   cbm_markdown_diagram_labels diagram);

typedef struct {
    const char *qn;
    uint32_t line;
    bool shared_qn;
} cbm_markdown_section_owner_t;

/* Rows and QNs remain C-owned. Rust orders the rows and marks ambiguous QNs. */
void mori_markdown_sort_section_owners(cbm_markdown_section_owner_t *sections, size_t count);
const cbm_markdown_section_owner_t *mori_markdown_reference_owner(
    const cbm_markdown_section_owner_t *sections, size_t count, uint32_t line);

#endif
