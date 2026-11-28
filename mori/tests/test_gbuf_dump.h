/*
 * test_gbuf_dump.h -- a graph buffer as sorted text lines, for tests that
 * compare two buffers (a C pass and its Rust port run on the same fixture).
 */
#ifndef TEST_GBUF_DUMP_H
#define TEST_GBUF_DUMP_H

#include "graph_buffer/graph_buffer.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* One graph buffer as sorted lines: every node with its fields and
 * properties, every edge by its endpoints' qualified names with its
 * properties. */
typedef struct {
    const cbm_gbuf_t *gb;
    char **lines;
    int count;
    int cap;
} tg_dump_t;

static inline void tg_dump_push(tg_dump_t *d, const char *a, const char *b, const char *c,
                                const char *e) {
    if (d->count == d->cap) {
        d->cap = d->cap ? d->cap * 2 : 64;
        d->lines = realloc(d->lines, (size_t)d->cap * sizeof(char *));
    }
    size_t n = strlen(a) + strlen(b) + strlen(c) + strlen(e) + 4;
    char *line = malloc(n);
    snprintf(line, n, "%s|%s|%s|%s", a, b, c, e);
    d->lines[d->count++] = line;
}

static inline void tg_dump_node(const cbm_gbuf_node_t *n, void *ud) {
    char where[1024];
    snprintf(where, sizeof(where), "%s %s %s:%d-%d", n->qualified_name, n->name ? n->name : "?",
             n->file_path ? n->file_path : "?", n->start_line, n->end_line);
    tg_dump_push(ud, "N", n->label ? n->label : "?", where,
                 n->properties_json ? n->properties_json : "(null)");
}

static inline void tg_dump_edge(const cbm_gbuf_edge_t *e, void *ud) {
    tg_dump_t *d = ud;
    const cbm_gbuf_node_t *s = cbm_gbuf_find_by_id(d->gb, e->source_id);
    const cbm_gbuf_node_t *t = cbm_gbuf_find_by_id(d->gb, e->target_id);
    char kind[256];
    snprintf(kind, sizeof(kind), "E %s %s", e->type, t ? t->qualified_name : "?");
    tg_dump_push(d, kind, s ? s->qualified_name : "?", "", e->properties_json);
}

static inline int tg_cmp_line(const void *a, const void *b) {
    return strcmp(*(char *const *)a, *(char *const *)b);
}

static inline char *tg_dump(const cbm_gbuf_t *gb) {
    tg_dump_t d = {.gb = gb};
    cbm_gbuf_foreach_node(gb, tg_dump_node, &d);
    cbm_gbuf_foreach_edge(gb, tg_dump_edge, &d);
    qsort(d.lines, (size_t)d.count, sizeof(char *), tg_cmp_line);
    size_t len = 1;
    for (int i = 0; i < d.count; i++) {
        len += strlen(d.lines[i]) + 1;
    }
    char *out = malloc(len);
    out[0] = '\0';
    size_t pos = 0;
    for (int i = 0; i < d.count; i++) {
        size_t l = strlen(d.lines[i]);
        memcpy(out + pos, d.lines[i], l);
        pos += l;
        out[pos++] = '\n';
        out[pos] = '\0';
        free(d.lines[i]);
    }
    free(d.lines);
    return out;
}

/* Prints both dumps when they differ. 0 when equal. */
static inline int tg_dumps_differ(const cbm_gbuf_t *c_gb, const cbm_gbuf_t *rust_gb,
                                  const char *what) {
    char *c = tg_dump(c_gb);
    char *rust = tg_dump(rust_gb);
    int rc = strcmp(c, rust) != 0;
    if (rc) {
        printf("  %s: C and Rust graphs differ\n--- C\n%s--- Rust\n%s", what, c, rust);
    }
    free(c);
    free(rust);
    return rc;
}

#endif /* TEST_GBUF_DUMP_H */
