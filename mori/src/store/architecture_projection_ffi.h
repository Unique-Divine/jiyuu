#ifndef CBM_ARCHITECTURE_PROJECTION_FFI_H
#define CBM_ARCHITECTURE_PROJECTION_FFI_H

#include "store/architecture_projection.h"
#include <sqlite3.h>

/* Private ABI with src/store/architecture_projection.rs. The C reader
 * owns the rows and strings. Rust fills in grouping and display selection
 * before the same request renders the payload. */
typedef struct {
    int64_t id;
    char *name, *qn, *file, *label;
    int line, end, component, degree;
    bool entry, handler, route, callable, structural, declared, test, test_property;
} ap_node;

typedef struct {
    int64_t id;
    int source, target, type;
} ap_edge;

typedef struct {
    int root, members, files, atoms, representative, degree, displayed;
    int overview_group;
    bool listed;
} ap_component;

typedef struct {
    const char *id, *label;
    bool test;
    int components, members, files;
} ap_overview_group;

typedef struct {
    ap_node *nodes;
    const ap_edge *edges;
    ap_component *components;
    int node_count, edge_count, component_count, accounted, grouping_depth, relationship_edges;
    size_t text_bytes, text_budget;
} ap_partition;

typedef struct {
    int source, target, type, edge;
} ap_dep;

typedef struct {
    int node, first_hop, depth, crossings;
} ap_path_candidate;

typedef bool (*ap_stop_fn)(void *);
/* String lengths include the terminating NUL. */
typedef bool (*ap_group_fn)(void *, const ap_overview_group *, size_t id_bytes, size_t label_bytes);
typedef int (*ap_snapshot_fn)(void *);

void mori_architecture_scratch_add(size_t bytes);
void mori_architecture_scratch_remove(size_t bytes);
void mori_architecture_directory_written(const unsigned char *directory, size_t length);
int mori_architecture_snapshot(sqlite3 *db, void *context, ap_stop_fn stop, ap_snapshot_fn run);
int mori_architecture_partition(ap_partition *input, void *context, ap_stop_fn stop,
                                ap_group_fn store_group);
int mori_architecture_rank(ap_component *components, int count, int limit);
int mori_architecture_dependencies(const ap_node *nodes, const ap_edge *edges, int edge_count,
                                   const ap_component *components, bool overview, ap_dep *out);
void mori_architecture_sort_paths(ap_path_candidate *candidates, int count);
int mori_architecture_select_paths(const ap_path_candidate *candidates, int count, int limit,
                                   int *first_hops, int node_count, int *out);

#endif
