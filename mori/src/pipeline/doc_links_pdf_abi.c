/* Adapter from the native resolver table to the Rust PDF resolver. */
#include "pipeline/doc_links.h"
#include "doclink.h"
#include "foundation/mem_core.h"

typedef struct {
    int64_t id;
    int start_line;
    const char *label;
    const char *name;
    const char *qualified_name;
    const char *path;
    const char *properties;
} pdf_node_view_t;

typedef struct {
    const cbm_gbuf_node_t **nodes;
    int count;
} pdf_node_list_t;

typedef struct {
    pdf_node_list_t (*by_name)(const cbm_gbuf_t *graph, const char *name);
    pdf_node_list_t (*by_label)(const cbm_gbuf_t *graph, const char *label);
    void (*visit)(const cbm_gbuf_t *graph, cbm_gbuf_node_visitor_fn visit, void *userdata);
    pdf_node_view_t (*describe)(const cbm_gbuf_node_t *node);
} pdf_graph_ops_t;

typedef struct {
    const char *raw;
    int syntax;
    bool join;
} pdf_link_t;

typedef struct {
    const cbm_gbuf_node_t *target;
    bool exact;
    int reason;
} pdf_outcome_t;

typedef pdf_link_t (*pdf_link_at_fn)(const CBMDocLink *links, int index);

void *mori_pdf_links_build(const char *project, const cbm_gbuf_t *graph,
                           const pdf_graph_ops_t *ops);
void mori_pdf_links_destroy(void *index);
void *mori_pdf_links_prepare(const void *index, const cbm_gbuf_t *graph, const pdf_graph_ops_t *ops,
                             const CBMDocLink *links, int count, pdf_link_at_fn link_at);
void mori_pdf_links_end(void *state);
pdf_outcome_t mori_pdf_links_resolve(const void *index, void *state, const cbm_gbuf_t *graph,
                                     const pdf_graph_ops_t *ops, pdf_link_t link, int position);

enum {
    PDF_PATH = 1,
    PDF_FILE = 2,
    PDF_QN = 3,
    PDF_NAME = 4,
};

enum {
    PDF_LOCAL = 0,
    PDF_MISSING = 1,
    PDF_AMBIGUOUS = 2,
    PDF_TEST_ONLY = 3,
};

static pdf_node_list_t by_name(const cbm_gbuf_t *graph, const char *name) {
    pdf_node_list_t out = {0};
    (void)cbm_gbuf_find_by_name(graph, name, &out.nodes, &out.count);
    return out;
}

static pdf_node_list_t by_label(const cbm_gbuf_t *graph, const char *label) {
    pdf_node_list_t out = {0};
    (void)cbm_gbuf_find_by_label(graph, label, &out.nodes, &out.count);
    return out;
}

static pdf_node_view_t describe(const cbm_gbuf_node_t *node) {
    return (pdf_node_view_t){
        .id = node->id,
        .start_line = node->start_line,
        .label = node->label,
        .name = node->name,
        .qualified_name = node->qualified_name,
        .path = node->file_path,
        .properties = node->properties_json,
    };
}

static const pdf_graph_ops_t PDF_GRAPH_OPS = {
    .by_name = by_name,
    .by_label = by_label,
    .visit = cbm_gbuf_foreach_node,
    .describe = describe,
};

static pdf_link_t link_at(const CBMDocLink *links, int position) {
    const CBMDocLink *link = &links[position];
    int syntax = 0;
    switch (link->syntax) {
    case CBM_DOCLINK_PDF_PATH:
        syntax = PDF_PATH;
        break;
    case CBM_DOCLINK_PDF_FILE:
        syntax = PDF_FILE;
        break;
    case CBM_DOCLINK_PDF_QN:
        syntax = PDF_QN;
        break;
    case CBM_DOCLINK_PDF_NAME:
        syntax = PDF_NAME;
        break;
    }
    return (pdf_link_t){link->raw, syntax, (link->flags & CBM_DOCLINK_FLAG_JOIN) != 0};
}

static void *pdf_build(const cbm_doclink_build_in_t *in) {
    if (in->file_count == 0 || !in->ctx || !in->ctx->project_name) {
        return NULL;
    }
    return mori_pdf_links_build(in->ctx->project_name, in->graph, &PDF_GRAPH_OPS);
}

typedef struct {
    const CBMDocLink *links;
    void *rust;
} pdf_file_state_t;

static void *pdf_prepare(const void *index, int run_file, const CBMDocLink *links, int count,
                         const cbm_gbuf_t *graph) {
    (void)run_file;
    pdf_file_state_t *state = cbm_alloc(CBM_MEM_CLASS_OTHER, sizeof(*state));
    if (state) {
        state->links = links;
        state->rust = mori_pdf_links_prepare(index, graph, &PDF_GRAPH_OPS, links, count, link_at);
    }
    return state;
}

static void pdf_end(void *opaque) {
    pdf_file_state_t *state = opaque;
    if (state) {
        mori_pdf_links_end(state->rust);
        cbm_free(CBM_MEM_CLASS_OTHER, state);
    }
}

static void pdf_resolve(const void *index, void *opaque, int run_file, const CBMDocLink *link,
                        const cbm_gbuf_t *graph, cbm_doclink_outcome_t *out) {
    (void)run_file;
    pdf_file_state_t *state = opaque;
    /* The resolver table requires link to point into the prepared array. */
    int position = state ? (int)(link - state->links) : -1;
    pdf_outcome_t result = mori_pdf_links_resolve(index, state ? state->rust : NULL, graph,
                                                  &PDF_GRAPH_OPS, link_at(link, 0), position);
    if (result.target) {
        out->kind = CBM_DOCLINK_EDGE;
        out->target = result.target;
        out->exact = result.exact;
        return;
    }
    if (result.reason == PDF_LOCAL) {
        out->kind = CBM_DOCLINK_LOCAL;
        return;
    }
    out->kind = CBM_DOCLINK_UNRESOLVED;
    switch (result.reason) {
    case PDF_AMBIGUOUS:
        out->reason = CBM_DOCLINK_REASON_AMBIGUOUS;
        break;
    case PDF_TEST_ONLY:
        out->reason = CBM_DOCLINK_REASON_TEST_ONLY;
        break;
    case PDF_MISSING:
    default:
        out->reason = CBM_DOCLINK_REASON_MISSING;
        break;
    }
}

const cbm_doclink_resolver_t cbm_doclink_pdf_resolver = {
    .langs = {CBM_LANG_PDF},
    .lang_count = 1,
    .build = pdf_build,
    .destroy = mori_pdf_links_destroy,
    .file_prepare = pdf_prepare,
    .file_end = pdf_end,
    .resolve = pdf_resolve,
    .via = "pdf",
};
