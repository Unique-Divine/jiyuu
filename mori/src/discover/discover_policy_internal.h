#ifndef CBM_DISCOVER_POLICY_INTERNAL_H
#define CBM_DISCOVER_POLICY_INTERNAL_H
#include "discover/discover.h"

/* Pure legacy policy predicates; callers provide live terminated strings. */
bool cbm_discover_path_has_skip_suffix(const char *relative_path);
bool cbm_discover_is_safety_core_directory(const char *name);
bool cbm_discover_path_in_jvm_package(const char *relative_path);

/* Bounded discovery borrows native inputs for one synchronous Rust walk.
 * The native runner reports setup/traversal failure. Rust owns the result. */
typedef bool (*cbm_discover_bounded_walk_fn)(cbm_discover_walk_t *walk, cbm_discover_frame_t *root,
                                             void *state);
bool cbm_discover_run_bounded(const char *repo_path, const cbm_discover_opts_t *opts,
                              cbm_discover_bounded_walk_fn walk, void *state);
cbm_discover_status_t mori_discover_count_bounded(const char *repo_path,
                                                  const cbm_discover_opts_t *opts,
                                                  const cbm_discover_walk_limits_t *limits,
                                                  cbm_index_resource_violation_t *violation,
                                                  int *count_out);

#endif
