/*
 * xrepo.h — Cross-repo views over several project databases.
 *
 * Reads the CROSS_* edges the cross-repo pass stores in each project's
 * database and aggregates them across every project index in a cache
 * directory. Read-only: each database opens as a query store, and no
 * write or exclusive lock is taken, so indexing is never blocked.
 *
 * The pass writes each link twice, once into the caller's database and
 * once (with a "callers" object, or as the listener's edge for channels)
 * into the provider's. Only the caller-side copy is counted, so a link
 * appears once, directed from the repo that uses to the repo that is used.
 *
 * Returned JSON is charged to OTHER; release it with
 * cbm_free(CBM_MEM_CLASS_OTHER, json).
 */
#ifndef CBM_UI_XREPO_H
#define CBM_UI_XREPO_H

#include "foundation/mem_core.h"

#include <stdbool.h>

/* Default and maximum page sizes for cbm_xrepo_links_json. */
#define CBM_XREPO_LINKS_DEFAULT_LIMIT 100
#define CBM_XREPO_LINKS_MAX_LIMIT 1000

/* GET /api/xrepo/summary: every project in cache_dir with node and edge
 * counts, and caller-side cross-repo links aggregated per (from, to, type).
 * NULL when cache_dir cannot be read or on allocation failure. */
char *cbm_xrepo_summary_json(const char *cache_dir);

/* GET /api/xrepo/links: the individual caller-side links stored in the
 * `from` project, optionally narrowed to one target project and one edge
 * type, ordered by type, target, file and line, and paged. NULL `to` or
 * `type` (or "") means any. *status receives the HTTP status: 200, 400 for
 * an invalid name or type, 404 when `from` has no index, 500 on failure. */
char *cbm_xrepo_links_json(const char *cache_dir, const char *from, const char *to,
                           const char *type, int limit, int offset, int *status);

/* True when type names a stored cross-repo edge type ("CROSS_" then
 * upper-case letters and underscores). */
bool cbm_xrepo_is_cross_type(const char *type);

#endif /* CBM_UI_XREPO_H */
