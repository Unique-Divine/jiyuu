/*
 * pass_route_nodes.c — Create Route nodes for HTTP_CALLS/ASYNC_CALLS edges.
 *
 * After parallel resolve merges edges into the main gbuf, HTTP_CALLS and
 * ASYNC_CALLS edges have url_path/method/broker in properties but point to
 * the library function (e.g., requests.get). This pass:
 *
 *   1. Scans all HTTP_CALLS/ASYNC_CALLS edges
 *   2. Extracts url_path from edge properties
 *   3. Creates Route nodes with deterministic QNs (__route__METHOD__/path)
 *   4. Re-targets edges from library function → Route node
 *
 * Route nodes are the rendezvous point for cross-service communication:
 *   Service A: checkout() → HTTP_CALLS → Route("POST /api/orders")
 *   Service B: create_order() → HANDLES → Route("POST /api/orders")
 */
#include "foundation/constants.h"
#include "foundation/str_util.h" // cbm_json_escape
#include <yyjson/yyjson.h>       // validate assembled DATA_FLOWS props

enum {
    RN_MAX_SCHEME = 12,
    RN_MIN_SCHEME = 3,
    RN_STRIP_PASSES = 2,
    RN_SCHEME_SKIP = 3,
    RN_ARGS_SKIP = 7,
    RN_PROPS_MARGIN = 100,
    RN_CALLEE_SKIP = 14, /* strlen('"callee_args":[') - 1 */
};

#define SLEN(s) (sizeof(s) - 1)
#include "pipeline/pipeline_internal.h"
#include <stdint.h>
#include "graph_buffer/graph_buffer.h"
#include "foundation/log.h"
#include "service_patterns.h"     /* cbm_service_pattern_is_http_route_literal */
#include "helpers.h"              /* cbm_is_test_file */
#include "discover/discover.h"    /* cbm_language_for_filename */
#include "cbm.h"                  /* CBM_LANG_GO, CBM_LANG_GRAPHQL */
#include "foundation/arena.h"     /* CBMArena */
#include "foundation/compat_fs.h" /* cbm_fopen */
#include "foundation/mem_core.h"  /* cbm_alloc */
#include "foundation/yaml.h"      /* cbm_yaml_parse */

#include <ctype.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* Extract a JSON string value by key from properties.
 * Returns pointer into buf (caller provides buffer). NULL if not found. */
static const char *json_extract(const char *json, const char *key, char *buf, int bufsz) {
    if (!json || !key) {
        return NULL;
    }
    /* Build "key":" pattern */
    char pattern[CBM_SZ_128];
    snprintf(pattern, sizeof(pattern), "\"%s\":\"", key);
    const char *start = strstr(json, pattern);
    if (!start) {
        return NULL;
    }
    start += strlen(pattern);
    const char *end = strchr(start, '"');
    if (!end || end == start) {
        return NULL;
    }
    int len = (int)(end - start);
    if (len >= bufsz) {
        len = bufsz - SKIP_ONE;
    }
    memcpy(buf, start, (size_t)len);
    buf[len] = '\0';
    return buf;
}

/* Visitor context for edge scanning */
typedef struct {
    cbm_gbuf_t *gb;
    int created;
} route_ctx_t;

static void route_edge_visitor(const cbm_gbuf_edge_t *edge, void *userdata) {
    route_ctx_t *ctx = (route_ctx_t *)userdata;

    /* Only process HTTP_CALLS and ASYNC_CALLS */
    if (strcmp(edge->type, "HTTP_CALLS") != 0 && strcmp(edge->type, "ASYNC_CALLS") != 0) {
        return;
    }
    /* A script's request (pass_shell.c) names its host and already ends at
     * its Route, which keeps the host for one outside the repository: a
     * path-only Route made from its url_path would meet a handler it never
     * calls. A code client's edge names none and refreshes its Route here. */
    char host_buf[CBM_SZ_256];
    if (json_extract(edge->properties_json, "host", host_buf, sizeof(host_buf))) {
        const cbm_gbuf_node_t *target = cbm_gbuf_find_by_id(ctx->gb, edge->target_id);
        if (target && target->label && strcmp(target->label, "Route") == 0) {
            return;
        }
    }

    /* Extract url_path from properties */
    char url_buf[CBM_SZ_512];
    const char *url = json_extract(edge->properties_json, "url_path", url_buf, sizeof(url_buf));
    if (!url || !url[0]) {
        return;
    }
    char callee_buf[CBM_SZ_256];
    const char *callee =
        json_extract(edge->properties_json, "callee", callee_buf, sizeof(callee_buf));
    if (strcmp(edge->type, "HTTP_CALLS") == 0 &&
        !cbm_service_pattern_is_http_route_literal(url, callee)) {
        return;
    }

    /* Extract method or broker */
    char method_buf[CBM_SZ_16];
    char broker_buf[CBM_SZ_64];
    const char *method =
        json_extract(edge->properties_json, "method", method_buf, sizeof(method_buf));
    const char *broker =
        json_extract(edge->properties_json, "broker", broker_buf, sizeof(broker_buf));

    /* Build Route QN */
    char route_qn[CBM_ROUTE_QN_SIZE];
    if (strcmp(edge->type, "HTTP_CALLS") == 0) {
        char cpath[CBM_SZ_256];
        snprintf(route_qn, sizeof(route_qn), "__route__%s__%s", method ? method : "ANY",
                 cbm_route_canon_path(url, cpath, sizeof(cpath)));
    } else {
        snprintf(route_qn, sizeof(route_qn), "__route__%s__%s", broker ? broker : "async", url);
    }

    /* Build properties for Route node */
    char route_props[CBM_SZ_256];
    if (method) {
        snprintf(route_props, sizeof(route_props), "{\"method\":\"%s\"}", method);
    } else if (broker) {
        snprintf(route_props, sizeof(route_props), "{\"broker\":\"%s\"}", broker);
    } else {
        snprintf(route_props, sizeof(route_props), "{}");
    }

    /* Create or find Route node (deduped by QN) */
    cbm_gbuf_upsert_node(ctx->gb, "Route", url, route_qn, "", 0, 0, route_props);
    ctx->created++;

    /* Note: we do NOT re-target the edge here because modifying edges during
     * iteration is unsafe. The edge stays pointing to the library function.
     * The URL-in-args detection in pass_parallel will create Route→handler HANDLES
     * edge separately. The caller→Route edge is created by pass_calls for
     * the sequential path; for the parallel path, the caller→library edge
     * with url_path in properties is sufficient for query_graph to find
     * the Route via: caller → HTTP_CALLS(url_path="/api/x") + Route("/api/x"). */
}

/* Extract URL path from full URL: "https://host/path/" → "/path/" */
static const char *url_path(const char *url) {
    if (!url) {
        return NULL;
    }
    const char *scheme_end = strstr(url, "://");
    if (!scheme_end) {
        return url; /* Already a path */
    }
    const char *path = strchr(scheme_end + RN_SCHEME_SKIP, '/');
    return path ? path : "/";
}

/* Route path `needle` inside URL path `hay`, ending where a segment does:
 * `/items` is in `/api/items` and `/items/42`; `/a` is not in `/acidjunk`
 * (fastapi: a `/a` prefix route matched every github.com/a... URL). */
static bool path_has_route(const char *hay, const char *needle) {
    size_t n = strlen(needle);
    if (n == 0) {
        return false;
    }
    for (const char *p = strstr(hay, needle); p; p = strstr(p + SKIP_ONE, needle)) {
        char c = p[n];
        bool start = needle[0] == '/' || p == hay || p[-SKIP_ONE] == '/';
        bool end = needle[n - SKIP_ONE] == '/' || c == '\0' || c == '/' || c == '?' || c == '#';
        if (start && end) {
            return true;
        }
    }
    return false;
}

/* A route prefix's segments (`items`, `api/v1`) as whole segments of a file
 * path: `app/routers/items.py`, `items/views.py`; `a` is not in
 * `docs_src/app_testing/main.py`. */
static bool file_has_segments(const char *path, const char *segs) {
    size_t n = strlen(segs);
    if (n == 0) {
        return false;
    }
    for (const char *p = strstr(path, segs); p; p = strstr(p + SKIP_ONE, segs)) {
        char c = p[n];
        bool start = p == path || p[-SKIP_ONE] == '/';
        bool end = segs[n - SKIP_ONE] == '/' || c == '\0' || c == '/' || c == '.';
        if (start && end) {
            return true;
        }
    }
    return false;
}

/* Check if a trailing dash-segment is a hash/project-number (strippable).
 * Returns true if the segment is short alphanumeric but not a meaningful word. */
static bool is_hash_segment(const char *seg, size_t slen) {
    if (slen > RN_MAX_SCHEME || slen == 0) {
        return false;
    }
    int has_letter = 0;
    for (size_t si = 0; si < slen; si++) {
        if (!((seg[si] >= '0' && seg[si] <= '9') || (seg[si] >= 'a' && seg[si] <= 'z'))) {
            return false;
        }
        if (seg[si] >= 'a' && seg[si] <= 'z') {
            has_letter = SKIP_ONE;
        }
    }
    /* Don't strip meaningful words (>=3 chars with letters like "api", "endpoint") */
    return !(has_letter && slen >= RN_MIN_SCHEME);
}

/* Strip up to 2 trailing hash/project-number segments from a Cloud Run hostname. */
static void strip_cloud_run_suffixes(char *hostname) {
    for (int strip = 0; strip < RN_STRIP_PASSES; strip++) {
        char *last_dash = strrchr(hostname, '-');
        if (!last_dash) {
            break;
        }
        if (!is_hash_segment(last_dash + SKIP_ONE, strlen(last_dash + SKIP_ONE))) {
            break;
        }
        *last_dash = '\0';
    }
}

/* Extract service name from Cloud Run URL hostname.
 * "my-svc-ab12cd34ef-uc.a.run.app/path" → "my-svc" */
static const char *extract_service_name(const char *url, char *buf, int bufsz) {
    if (!url) {
        return NULL;
    }
    const char *scheme_end = strstr(url, "://");
    if (!scheme_end) {
        return NULL;
    }
    const char *host_start = scheme_end + RN_SCHEME_SKIP;
    const char *end = host_start;
    while (*end && *end != '.' && *end != '/') {
        end++;
    }
    int hlen = (int)(end - host_start);
    if (hlen <= 0 || hlen >= bufsz) {
        return NULL;
    }

    char tmp[CBM_SZ_256];
    if (hlen >= (int)sizeof(tmp)) {
        return NULL;
    }
    memcpy(tmp, host_start, (size_t)hlen);
    tmp[hlen] = '\0';

    strip_cloud_run_suffixes(tmp);

    snprintf(buf, (size_t)bufsz, "%s", tmp);
    return buf;
}

/* Phase 2: Match infra Route URLs to handler Route nodes by URL path + service name. */
/* Check if a Route QN indicates an infra/async/broker Route (not a handler). */
static bool is_broker_route(const char *qn) {
    static const char *const prefixes[] = {
        "__route__infra__", "__route__pubsub__",          "__route__cloud_tasks__",
        "__route__async__", "__route__cloud_scheduler__", "__route__kafka__",
        "__route__sqs__",   "__route__ingress__"};
    if (!qn) {
        return false;
    }
    for (int i = 0; i < (int)(sizeof(prefixes) / sizeof(prefixes[0])); i++) {
        if (strstr(qn, prefixes[i]) == qn) {
            return true;
        }
    }
    return false;
}

/* Try to match a single infra Route to a handler Route and create HANDLES bridge.
 * Returns 1 if matched, 0 otherwise. */
/* Total order over candidates, so "which handler" never depends on the order
 * nodes happened to land in the graph buffer. Qualified names are unique per
 * node, so this is a tie-break that always decides; a NULL name sorts last and
 * therefore only wins when nothing else matched. */
static bool infra_candidate_is_better(const cbm_gbuf_node_t *candidate,
                                      const cbm_gbuf_node_t *best) {
    if (!best) {
        return true;
    }
    if (!candidate->qualified_name) {
        return false;
    }
    if (!best->qualified_name) {
        return true;
    }
    return strcmp(candidate->qualified_name, best->qualified_name) < 0;
}

static int match_one_infra_route(cbm_gbuf_t *gb, const cbm_gbuf_node_t *infra,
                                 const char *infra_path, const char *svc_name,
                                 const cbm_gbuf_node_t **all_routes, int route_count) {
    /* The FIRST match in array order used to win, and that array is in graph
     * buffer insertion order — parallel worker interleaving. Two runs of the
     * same corpus linked the same infra route to different handlers, so the
     * HANDLES edge set moved between runs (kubernetes, 2026-09-18: 229 vs 242
     * HANDLES with an identical node set). Collect the best candidate under a
     * total order instead, and match on it. */
    const cbm_gbuf_node_t *best = NULL;
    for (int j = 0; j < route_count; j++) {
        const cbm_gbuf_node_t *handler_route = all_routes[j];
        if (is_broker_route(handler_route->qualified_name)) {
            continue;
        }
        int file_matches = (handler_route->file_path != NULL &&
                            strstr(handler_route->file_path, svc_name) != NULL);
        int is_prefix_route =
            (handler_route->qualified_name != NULL &&
             strncmp(handler_route->qualified_name, "__route__ANY__", SLEN("__route__ANY__")) == 0);
        if (!file_matches && !is_prefix_route) {
            continue;
        }
        const char *handler_name = handler_route->name;
        if (!handler_name) {
            continue;
        }
        const char *handler_path = strchr(handler_name, '/');
        if (!handler_path) {
            continue;
        }

        int path_match =
            (strlen(handler_path) > SKIP_ONE && path_has_route(infra_path, handler_path)) ||
            (strlen(infra_path) > SKIP_ONE && path_has_route(handler_path, infra_path));
        /* a root route stands for a URL's service only when its file is that
         * service's; any prefix or ANY route at `/` matched every URL (a docs
         * site's chat link took the root handlers) */
        int root_svc_match = file_matches && strcmp(handler_path, "/") == 0;
        if (path_match || root_svc_match) {
            if (infra_candidate_is_better(handler_route, best)) {
                best = handler_route;
            }
        }
    }
    if (!best) {
        return 0;
    }
    const cbm_gbuf_edge_t **fn_handles = NULL;
    int fn_hcount = 0;
    cbm_gbuf_find_edges_by_target_type(gb, best->id, "HANDLES", &fn_handles, &fn_hcount);
    for (int fh = 0; fh < fn_hcount; fh++) {
        cbm_gbuf_insert_edge(gb, fn_handles[fh]->source_id, infra->id, "HANDLES",
                             "{\"source\":\"infra_match\"}");
    }
    return SKIP_ONE;
}

/* Phase 2: Match infra Route URLs to handler Route nodes by URL path + service name. */
static void match_infra_routes(cbm_gbuf_t *gb) {
    const cbm_gbuf_node_t **all_routes = NULL;
    int route_count = 0;
    if (cbm_gbuf_find_by_label(gb, "Route", &all_routes, &route_count) != 0 || route_count == 0) {
        return;
    }

    int matched = 0;

    for (int i = 0; i < route_count; i++) {
        const cbm_gbuf_node_t *infra = all_routes[i];
        if (!infra->qualified_name ||
            strncmp(infra->qualified_name, "__route__infra__", SLEN("__route__infra__")) != 0) {
            continue;
        }
        if (!infra->name || !strstr(infra->name, "://")) {
            continue;
        }

        const char *infra_path = url_path(infra->name);
        char svc_buf[CBM_SZ_128];
        const char *svc_name = extract_service_name(infra->name, svc_buf, sizeof(svc_buf));
        if (!infra_path || !svc_name) {
            continue;
        }

        matched += match_one_infra_route(gb, infra, infra_path, svc_name, all_routes, route_count);
    }

    if (matched > 0) {
        char buf[CBM_SZ_16];
        snprintf(buf, sizeof(buf), "%d", matched);
        cbm_log_info("pass.route_match", "infra_matched", buf);
    }
}

/* A client interface method's declared request (Retrofit): an HTTP_CALLS edge
 * to the Route, with the url_path/method properties a call site's edge carries,
 * so the endpoint is matched to its server like any other outbound call. */
static void emit_client_route_call(cbm_gbuf_t *gb, int64_t func_id, const char *path,
                                   const char *method) {
    const char *rm = method ? method : "ANY";
    char route_qn[CBM_ROUTE_QN_SIZE];
    char cpath[CBM_SZ_256];
    snprintf(route_qn, sizeof(route_qn), "__route__%s__%s", rm,
             cbm_route_canon_path(path, cpath, sizeof(cpath)));
    char esc_m[CBM_SZ_128];
    cbm_json_escape(esc_m, sizeof(esc_m), rm);
    char rprops[CBM_SZ_256];
    (void)snprintf(rprops, sizeof(rprops), "{\"method\":\"%s\"}", esc_m);
    int64_t route_id = cbm_gbuf_upsert_node(gb, "Route", path, route_qn, "", 0, 0, rprops);
    char esc_p[CBM_SZ_512];
    cbm_json_escape(esc_p, sizeof(esc_p), path);
    char eprops[CBM_SZ_1K];
    (void)snprintf(eprops, sizeof(eprops),
                   "{\"callee\":\"%s\",\"url_path\":\"%s\",\"method\":\"%s\"}", esc_m, esc_p,
                   esc_m);
    cbm_gbuf_insert_edge(gb, func_id, route_id, "HTTP_CALLS", eprops);
}

void cbm_pipeline_emit_def_route(cbm_gbuf_t *gb, int64_t func_id, const CBMDefinition *def,
                                 const char *rel) {
    if (def->client_route_path && def->client_route_path[0]) {
        emit_client_route_call(gb, func_id, def->client_route_path, def->route_method);
        return;
    }
    if (!def->route_path || def->route_path[0] == '\0') {
        return;
    }
    const char *rm = def->route_method ? def->route_method : "ANY";
    char route_qn[CBM_ROUTE_QN_SIZE];
    char cpath[CBM_SZ_256];
    snprintf(route_qn, sizeof(route_qn), "__route__%s__%s", rm,
             cbm_route_canon_path(def->route_path, cpath, sizeof(cpath)));
    char rprops[CBM_SZ_256];
    char esc_m[CBM_SZ_128];
    cbm_json_escape(esc_m, sizeof(esc_m), rm);
    (void)snprintf(rprops, sizeof(rprops), "{\"method\":\"%s\",\"source\":\"decorator\"}", esc_m);
    int64_t route_id = cbm_gbuf_upsert_node(gb, "Route", def->route_path, route_qn,
                                            def->file_path ? def->file_path : rel, 0, 0, rprops);
    char hprops[CBM_SZ_1K]; /* must exceed escaped value + wrapper or snprintf cuts the
                               closing brace */
    char esc_h[CBM_SZ_512];
    cbm_json_escape(esc_h, sizeof(esc_h), def->qualified_name);
    (void)snprintf(hprops, sizeof(hprops), "{\"handler\":\"%s\"}", esc_h);
    cbm_gbuf_insert_edge(gb, func_id, route_id, "HANDLES", hprops);
}

/* Phase 2a: Ensure all functions with route_path properties have Route+HANDLES edges.
 * During incremental indexing, only changed files get Route nodes from extraction.
 * This pass scans ALL Function/Method nodes and creates missing Route+HANDLES. */
/* Extract the string property `key` of a parsed properties object into buf,
 * with its escapes decoded. Returns true if found: a non-empty string with no
 * NUL that fits buf. Nothing is copied otherwise. */
static bool extract_json_prop(yyjson_val *root, const char *key, char *buf, size_t bufsz) {
    yyjson_val *val = yyjson_obj_get(root, key);
    const char *str = yyjson_get_str(val);
    size_t len = yyjson_get_len(val);
    if (!str || len == 0 || len >= bufsz || memchr(str, '\0', len)) {
        return false;
    }
    memcpy(buf, str, len + SKIP_ONE);
    return true;
}

/* Process a single Function/Method node: create Route+HANDLES if it has route_path.
 * Returns 1 if a new HANDLES edge was created, 0 otherwise. */
static int ensure_one_decorator_route(cbm_gbuf_t *gb, const cbm_gbuf_node_t *func) {
    /* Most nodes carry no route: skip them before parsing their properties. */
    if (!func->properties_json || !strstr(func->properties_json, "\"route_path\"")) {
        return 0;
    }

    /* Properties hold source text as its bytes, which need not be UTF-8. */
    yyjson_doc *doc = yyjson_read(func->properties_json, strlen(func->properties_json),
                                  YYJSON_READ_ALLOW_INVALID_UNICODE);
    yyjson_val *root = yyjson_doc_get_root(doc);
    char path[CBM_SZ_256];
    char method[CBM_SZ_16] = "ANY";
    bool routed = extract_json_prop(root, "route_path", path, sizeof(path)) && path[0] == '/';
    if (routed) {
        extract_json_prop(root, "route_method", method, sizeof(method));
    }
    yyjson_doc_free(doc);
    if (!routed) {
        return 0;
    }

    char route_qn[CBM_ROUTE_QN_SIZE];
    char cpath[CBM_SZ_256];
    snprintf(route_qn, sizeof(route_qn), "__route__%s__%s", method,
             cbm_route_canon_path(path, cpath, sizeof(cpath)));
    const cbm_gbuf_node_t *existing = cbm_gbuf_find_by_qn(gb, route_qn);

    char rprops[CBM_SZ_256];
    char esc_m[CBM_SZ_128];
    cbm_json_escape(esc_m, sizeof(esc_m), method);
    (void)snprintf(rprops, sizeof(rprops), "{\"method\":\"%s\",\"source\":\"decorator\"}", esc_m);
    int64_t route_id = cbm_gbuf_upsert_node(gb, "Route", path, route_qn,
                                            func->file_path ? func->file_path : "", 0, 0, rprops);

    if (existing) {
        const cbm_gbuf_edge_t **existing_handles = NULL;
        int eh_count = 0;
        cbm_gbuf_find_edges_by_target_type(gb, route_id, "HANDLES", &existing_handles, &eh_count);
        for (int eh = 0; eh < eh_count; eh++) {
            if (existing_handles[eh]->source_id == func->id) {
                return 0;
            }
        }
    }

    /* The same properties cbm_pipeline_emit_def_route gives the edge. */
    char hprops[CBM_SZ_1K]; /* must exceed escaped value + wrapper or snprintf cuts the
                               closing brace */
    char esc_h[CBM_SZ_512];
    cbm_json_escape(esc_h, sizeof(esc_h), func->qualified_name ? func->qualified_name : "");
    (void)snprintf(hprops, sizeof(hprops), "{\"handler\":\"%s\"}", esc_h);
    cbm_gbuf_insert_edge(gb, func->id, route_id, "HANDLES", hprops);
    return SKIP_ONE;
}

/* Phase 2a: Ensure all functions with route_path properties have Route+HANDLES edges. */
static void ensure_decorator_routes(cbm_gbuf_t *gb) {
    /* "Module" is here for Blazor: a .razor component's class is implicit, so
     * its @page route is carried by the file's Module def. Extraction's own
     * insert_def_into_gbuf is label-agnostic and creates the Route either way —
     * this backstop is what runs on an INCREMENTAL re-index, so leaving Module
     * out would make a component's Route appear on a full index and vanish the
     * next time that one file changed.
     * Bound comes from the array, not the unrelated RN_STRIP_PASSES it used to
     * borrow, so adding a label cannot silently skip it. */
    const char *labels[] = {"Function", "Method", "Module"};
    const int label_count = (int)(sizeof(labels) / sizeof(labels[0]));
    int created = 0;

    for (int li = 0; li < label_count; li++) {
        const cbm_gbuf_node_t **nodes = NULL;
        int count = 0;
        if (cbm_gbuf_find_by_label(gb, labels[li], &nodes, &count) != 0) {
            continue;
        }
        for (int i = 0; i < count; i++) {
            created += ensure_one_decorator_route(gb, nodes[i]);
        }
    }

    if (created > 0) {
        char buf[CBM_SZ_16];
        snprintf(buf, sizeof(buf), "%d", created);
        cbm_log_info("pass.ensure_decorator_routes", "created", buf);
    }
}

/* Phase 2b: Connect prefix Routes to decorator handler Functions.
 * For each prefix Route (__route__ANY__/path), find the CALLS edge leading to it
 * (from the registering file), derive the service directory, then find decorator
 * Routes in that directory tree and create HANDLES from their handler Functions
 * to the prefix Route. This bridges include_router → decorator → handler. */
/* Bridge decorator handler Functions to a prefix Route. Returns number connected. */
static int bridge_funcs_to_prefix(cbm_gbuf_t *gb, const cbm_gbuf_node_t *prefix_route,
                                  const char *registrar_path, int dir_len,
                                  const char *prefix_segs) {
    const cbm_gbuf_node_t **funcs = NULL;
    int func_count = 0;
    cbm_gbuf_find_by_label(gb, "Function", &funcs, &func_count);

    int connected = 0;
    for (int fi = 0; fi < func_count; fi++) {
        const cbm_gbuf_node_t *func = funcs[fi];
        if (!func->file_path) {
            continue;
        }
        if (strncmp(func->file_path, registrar_path, (size_t)dir_len) != 0) {
            continue;
        }
        if (!func->properties_json || !strstr(func->properties_json, "\"route_path\"")) {
            continue;
        }
        if (prefix_segs && prefix_segs[0] && !file_has_segments(func->file_path, prefix_segs)) {
            continue;
        }
        cbm_gbuf_insert_edge(gb, func->id, prefix_route->id, "HANDLES",
                             "{\"source\":\"prefix_decorator_bridge\"}");
        connected++;
    }
    return connected;
}

/* Phase 2b: Connect prefix Routes to decorator handler Functions. */
static void connect_prefix_to_decorators(cbm_gbuf_t *gb) {
    const cbm_gbuf_node_t **routes = NULL;
    int route_count = 0;
    if (cbm_gbuf_find_by_label(gb, "Route", &routes, &route_count) != 0) {
        return;
    }

    int connected = 0;

    for (int ri = 0; ri < route_count; ri++) {
        const cbm_gbuf_node_t *prefix_route = routes[ri];
        if (!prefix_route->qualified_name ||
            strncmp(prefix_route->qualified_name, "__route__ANY__/", SLEN("__route__ANY__/")) !=
                0) {
            continue;
        }

        const char *prefix_path = prefix_route->name;
        const char *prefix_segs =
            (prefix_path && prefix_path[0] == '/') ? prefix_path + SKIP_ONE : prefix_path;
        /* A root mount (`app.mount("/", ...)`) names no part of a handler's
         * path, so nothing tells its handlers from the rest of the directory:
         * bridging it linked every decorated function there (fastapi: 702
         * handlers to `/`, then to every infra URL through it). */
        if (!prefix_segs || !prefix_segs[0]) {
            continue;
        }

        const cbm_gbuf_edge_t **calls_in = NULL;
        int calls_count = 0;
        cbm_gbuf_find_edges_by_target_type(gb, prefix_route->id, "CALLS", &calls_in, &calls_count);

        /* Every registrar's directory, not the first CALLS edge's: their
         * order is the workers' merge order, so the first one moved between
         * runs of the same corpus. Edges dedup, so the union is order-free. */
        for (int ci = 0; ci < calls_count; ci++) {
            const cbm_gbuf_node_t *registrar = cbm_gbuf_find_by_id(gb, calls_in[ci]->source_id);
            if (!registrar || !registrar->file_path) {
                continue;
            }
            const char *last_slash = strrchr(registrar->file_path, '/');
            if (!last_slash) {
                continue;
            }
            int dir_len = (int)(last_slash - registrar->file_path) + SKIP_ONE;
            connected += bridge_funcs_to_prefix(gb, prefix_route, registrar->file_path, dir_len,
                                                prefix_segs);
        }
    }

    if (connected > 0) {
        char buf[CBM_SZ_16];
        snprintf(buf, sizeof(buf), "%d", connected);
        cbm_log_info("pass.prefix_bridge", "connected", buf);
    }
}

/* Phase 3: Create DATA_FLOWS edges by linking callers through Route to handlers.
 * For each HTTP_CALLS/ASYNC_CALLS edge (caller → Route), find the HANDLES edge
 * (handler → Route) and create DATA_FLOWS (caller → handler) with route context. */
/* Check if a direct CALLS edge already exists between two nodes */
static int has_direct_call(const cbm_gbuf_t *gb, int64_t source, int64_t target) {
    const cbm_gbuf_edge_t **edges = NULL;
    int count = 0;
    cbm_gbuf_find_edges_by_source_type(gb, source, "CALLS", &edges, &count);
    for (int i = 0; i < count; i++) {
        if (edges[i]->target_id == target) {
            return SKIP_ONE;
        }
    }
    return 0;
}

/* Extract param_names from a node's properties_json.
 * Returns a comma-separated string in buf, or empty string. */
static void extract_param_names(const cbm_gbuf_node_t *node, char *buf, int bufsize) {
    buf[0] = '\0';
    if (!node || !node->properties_json) {
        return;
    }
    const char *p = strstr(node->properties_json, "\"param_names\":");
    if (!p) {
        return;
    }
    p = strchr(p, '[');
    if (!p) {
        return;
    }
    p++; /* skip '[' */
    const char *end = strchr(p, ']');
    if (!end || end <= p) {
        return;
    }
    int len = (int)(end - p);
    if (len >= bufsize) {
        len = bufsize - SKIP_ONE;
    }
    memcpy(buf, p, (size_t)len);
    buf[len] = '\0';
}

/* Extract the "args" JSON fragment from an edge's properties.
 * Returns pointer into the properties string (not copied). */
static const char *find_args_in_props(const char *props) {
    if (!props) {
        return NULL;
    }
    const char *p = strstr(props, "\"args\":[");
    if (!p) {
        return NULL;
    }
    return p + RN_ARGS_SKIP; /* skip "args":[ , points to first { or ] */
}

/* Append handler_params and caller_args to DATA_FLOWS props. Closes with '}'. */
static void finish_data_flow_props(char *props, size_t propsz, size_t pos,
                                   const char *handler_params, const char *args_json) {
    if (handler_params[0]) {
        int w = snprintf(props + pos, propsz - pos, ",\"handler_params\":[%s]", handler_params);
        if (w > 0) {
            pos += (size_t)w;
        }
    }
    if (args_json) {
        int w = snprintf(props + pos, propsz - pos, ",\"caller_args\":[%.*s", 400, args_json);
        if (w > 0) {
            pos += (size_t)w;
            char *close = strchr(props + (pos - (size_t)w) + RN_CALLEE_SKIP, ']');
            if (close && close < props + propsz - PAIR_LEN) {
                pos = (size_t)(close - props) + SKIP_ONE;
            }
        }
    }
    if (pos < propsz - SKIP_ONE) {
        props[pos] = '}';
        props[pos + SKIP_ONE] = '\0';
    }
}

static bool http_call_edge_has_valid_route(const cbm_gbuf_edge_t *edge) {
    char url_buf[CBM_SZ_512];
    const char *url = json_extract(edge->properties_json, "url_path", url_buf, sizeof(url_buf));
    char callee_buf[CBM_SZ_256];
    const char *callee =
        json_extract(edge->properties_json, "callee", callee_buf, sizeof(callee_buf));
    return cbm_service_pattern_is_http_route_literal(url, callee);
}

/* try_create_data_flow's refusal for a pair across the test-file boundary. */
enum { RN_FLOW_TEST_SCOPED = -2 };

/* Is this node defined in a test file, by the extraction's own per-language rule? */
static bool node_in_test_file(const cbm_gbuf_node_t *n) {
    if (!n || !n->file_path || !n->file_path[0]) {
        return false;
    }
    const char *slash = strrchr(n->file_path, '/');
    const char *base = slash ? slash + SKIP_ONE : n->file_path;
    return cbm_is_test_file(n->file_path, cbm_language_for_filename(base));
}

/* R1 + R2 (user decision, 2026-10-18): a handler defined in a test file pairs only with
 * callers in that same file, and a caller in a test file pairs only with handlers in that same
 * file. A test's request to the application, a production caller matched to a test's mock
 * server, and two tests' private mock servers pairing across files are not data flows of the
 * application. The HTTP_CALLS / HANDLES edges themselves stay; only the pairing is scoped. */
static bool data_flow_crosses_test_scope(const cbm_gbuf_t *gb, int64_t caller_id,
                                         int64_t handler_id) {
    const cbm_gbuf_node_t *caller = cbm_gbuf_find_by_id(gb, caller_id);
    const cbm_gbuf_node_t *handler = cbm_gbuf_find_by_id(gb, handler_id);
    if (!caller || !handler) {
        return false;
    }
    const char *cf = caller->file_path ? caller->file_path : "";
    const char *hf = handler->file_path ? handler->file_path : "";
    if (strcmp(cf, hf) == 0) {
        return false;
    }
    return node_in_test_file(caller) || node_in_test_file(handler);
}

/* Try to create a DATA_FLOWS edge between caller and handler via a route.
 * Returns: 1=created, 0=skipped (self/duplicate), -1=skipped (has direct call),
 * RN_FLOW_TEST_SCOPED=skipped (the pair crosses the test-file boundary, R1 + R2). */
static int try_create_data_flow(cbm_gbuf_t *gb, int64_t caller_id, int64_t handler_id,
                                const cbm_gbuf_node_t *route, const char *edge_type,
                                const char *caller_props, bool via_infra) {
    if (caller_id == handler_id) {
        return 0;
    }
    if (data_flow_crosses_test_scope(gb, caller_id, handler_id)) {
        return RN_FLOW_TEST_SCOPED;
    }
    if (has_direct_call(gb, caller_id, handler_id)) {
        return CBM_NOT_FOUND;
    }

    const char *args_json = find_args_in_props(caller_props);
    const cbm_gbuf_node_t *handler_node = cbm_gbuf_find_by_id(gb, handler_id);
    char handler_params[CBM_SZ_512];
    extract_param_names(handler_node, handler_params, sizeof(handler_params));

    /* Route names/QNs are sliced source text (URL strings, decorator args) and
     * can contain quotes — escape them or the edge properties JSON is
     * malformed (aborts json_extract consumers incl. integrity_check). */
    char esc_rname[CBM_SZ_256];
    char esc_rqn[CBM_SZ_512];
    cbm_json_escape(esc_rname, sizeof(esc_rname), route->name ? route->name : "");
    cbm_json_escape(esc_rqn, sizeof(esc_rqn), route->qualified_name ? route->qualified_name : "");
    char props[CBM_SZ_2K];
    int n;
    if (via_infra) {
        n = snprintf(props, sizeof(props),
                     "{\"via\":\"%s\",\"route\":\"%s\",\"edge_type\":\"%s\",\"via_infra\":true",
                     esc_rname, esc_rqn, edge_type);
    } else {
        n = snprintf(props, sizeof(props), "{\"via\":\"%s\",\"route\":\"%s\",\"edge_type\":\"%s\"",
                     esc_rname, esc_rqn, edge_type);
    }

    if (n > 0 && (size_t)n < sizeof(props) - RN_PROPS_MARGIN) {
        finish_data_flow_props(props, sizeof(props), (size_t)n, handler_params, args_json);
    }
    /* handler_params/args_json are sliced text fragments; truncation or stray
     * quotes can leave the assembled JSON malformed, which aborts every
     * json_extract consumer downstream. Validate and fall back to the minimal
     * envelope rather than persisting invalid properties. */
    {
        yyjson_doc *vdoc = yyjson_read(props, strlen(props), 0);
        if (!vdoc) {
            snprintf(props, sizeof(props), "{\"via\":\"%s\",\"edge_type\":\"%s\"}", esc_rname,
                     edge_type);
        } else {
            yyjson_doc_free(vdoc);
        }
    }
    cbm_gbuf_insert_edge(gb, caller_id, handler_id, "DATA_FLOWS", props);
    return SKIP_ONE;
}

/* One route's DATA_FLOWS context: its direct handlers (handler -[HANDLES]-> route) and its
 * INFRA_MAPS hops (route -[INFRA_MAPS]-> endpoint, whose handlers are reached via the endpoint).
 * The edge arrays are the graph buffer's own index arrays. Inserting DATA_FLOWS never appends to
 * a HANDLES, INFRA_MAPS, HTTP_CALLS or ASYNC_CALLS array, so they stay valid while flows are added
 * (only edge deletion rebuilds the indexes). */
typedef struct {
    cbm_gbuf_t *gb;
    const cbm_gbuf_node_t *route;
    const cbm_gbuf_edge_t **handles;
    int handles_count;
    const cbm_gbuf_edge_t **infra;
    int infra_count;
    int flows;
    int skipped;
    int test_scoped;
} route_flows_t;

typedef struct {
    int flows;
    int skipped;     /* the caller already CALLS the handler */
    int test_scoped; /* the pair crosses the test-file boundary (R1 + R2) */
} data_flow_stats_t;

static void route_flow_add(route_flows_t *rf, const cbm_gbuf_edge_t *caller, int64_t handler_id,
                           const char *edge_type, bool via_infra) {
    int rc = try_create_data_flow(rf->gb, caller->source_id, handler_id, rf->route, edge_type,
                                  caller->properties_json, via_infra);
    if (rc > 0) {
        rf->flows++;
    } else if (rc == RN_FLOW_TEST_SCOPED) {
        rf->test_scoped++;
    } else if (rc < 0) {
        rf->skipped++;
    }
}

/* DATA_FLOWS from one caller edge to every handler of the route, direct and via infra. */
static void caller_data_flows(route_flows_t *rf, const cbm_gbuf_edge_t *caller,
                              const char *edge_type) {
    for (int hi = 0; hi < rf->handles_count; hi++) {
        route_flow_add(rf, caller, rf->handles[hi]->source_id, edge_type, false);
    }
    for (int ie = 0; ie < rf->infra_count; ie++) {
        const cbm_gbuf_edge_t **ep_handles = NULL;
        int ep_hcount = 0;
        cbm_gbuf_find_edges_by_target_type(rf->gb, rf->infra[ie]->target_id, "HANDLES", &ep_handles,
                                           &ep_hcount);
        for (int eh = 0; eh < ep_hcount; eh++) {
            route_flow_add(rf, caller, ep_handles[eh]->source_id, edge_type, true);
        }
    }
}

/* Create DATA_FLOWS between EVERY caller (HTTP_CALLS + ASYNC_CALLS) and EVERY handler of one
 * route node.
 *
 * Callers were copied into a 64-slot buffer and infra handlers into a 32-slot one, each keeping the
 * first entries in edge-insertion order -- i.e. in parallel worker interleaving. Which DATA_FLOWS
 * existed therefore changed between two indexes of the same tree with the same binary
 * (thingsboard, 2026-10-17), and every caller past the 64th lost its flow outright. The flow set
 * is now the full caller x handler product. A (caller, handler) pair reached twice -- through two
 * routes, or directly and via infra -- keeps the order-free property winner of
 * cbm_gbuf_insert_edge. */
static void create_route_data_flows(cbm_gbuf_t *gb, const cbm_gbuf_node_t *route,
                                    data_flow_stats_t *st) {
    route_flows_t rf = {.gb = gb, .route = route};
    cbm_gbuf_find_edges_by_target_type(gb, route->id, "HANDLES", &rf.handles, &rf.handles_count);
    cbm_gbuf_find_edges_by_source_type(gb, route->id, "INFRA_MAPS", &rf.infra, &rf.infra_count);

    const cbm_gbuf_edge_t **http_edges = NULL;
    int http_count = 0;
    cbm_gbuf_find_edges_by_target_type(gb, route->id, "HTTP_CALLS", &http_edges, &http_count);
    for (int i = 0; i < http_count; i++) {
        if (http_call_edge_has_valid_route(http_edges[i])) {
            caller_data_flows(&rf, http_edges[i], "HTTP_CALLS");
        }
    }
    const cbm_gbuf_edge_t **async_edges = NULL;
    int async_count = 0;
    cbm_gbuf_find_edges_by_target_type(gb, route->id, "ASYNC_CALLS", &async_edges, &async_count);
    for (int i = 0; i < async_count; i++) {
        caller_data_flows(&rf, async_edges[i], "ASYNC_CALLS");
    }
    st->flows += rf.flows;
    st->skipped += rf.skipped;
    st->test_scoped += rf.test_scoped;
}

/* Find the enclosing proto service for a function by file + line range. */
static const cbm_gbuf_node_t *find_enclosing_service(const cbm_gbuf_node_t *fn,
                                                     const cbm_gbuf_node_t **services,
                                                     int svc_count) {
    for (int s = 0; s < svc_count; s++) {
        if (strcmp(fn->file_path, services[s]->file_path) != 0) {
            continue;
        }
        if (fn->start_line >= services[s]->start_line && fn->end_line <= services[s]->end_line) {
            return services[s];
        }
    }
    return NULL;
}

/* Phase 4: Create __grpc__ Route nodes from protobuf service definitions. */
static void create_grpc_routes(cbm_gbuf_t *gb) {
    const cbm_gbuf_node_t **classes = NULL;
    int class_count = 0;
    if (cbm_gbuf_find_by_label(gb, "Class", &classes, &class_count) != 0 || class_count == 0) {
        return;
    }

    /* EVERY proto service, not a fixed-size prefix of them.
     *
     * This array held 64 and the loop stopped at that many, so WHICH services
     * were considered depended on the order Class nodes happened to land in the
     * graph buffer — i.e. on parallel worker interleaving. Two indexes of the
     * same corpus with the same binary produced different __grpc__ Routes
     * (kubernetes, 2026-09-18: 5 Routes present in one run and absent from the
     * other, 4 the other way, plus their HANDLES edges). It was a silent
     * quality cut as well: every service past the 64th got no Routes at all,
     * and a repository with more than 64 services is not unusual. */
    int svc_total = 0;
    for (int i = 0; i < class_count; i++) {
        if (classes[i]->file_path && strstr(classes[i]->file_path, ".proto")) {
            svc_total++;
        }
    }
    if (svc_total == 0) {
        return;
    }
    const cbm_gbuf_node_t **services =
        cbm_alloc(CBM_MEM_CLASS_OTHER, (size_t)svc_total * sizeof(*services));
    if (!services) {
        return; /* the borrowed `classes` array belongs to the graph buffer */
    }
    int svc_count = 0;
    for (int i = 0; i < class_count && svc_count < svc_total; i++) {
        if (classes[i]->file_path && strstr(classes[i]->file_path, ".proto")) {
            services[svc_count++] = classes[i];
        }
    }

    const cbm_gbuf_node_t **funcs = NULL;
    int func_count = 0;
    if (cbm_gbuf_find_by_label(gb, "Function", &funcs, &func_count) != 0 || func_count == 0) {
        cbm_free(CBM_MEM_CLASS_OTHER, services);
        return;
    }

    int grpc_routes = 0;
    for (int f = 0; f < func_count; f++) {
        const cbm_gbuf_node_t *fn = funcs[f];
        if (!fn->file_path || !strstr(fn->file_path, ".proto") || !fn->name) {
            continue;
        }
        const cbm_gbuf_node_t *svc = find_enclosing_service(fn, services, svc_count);
        if (!svc) {
            continue;
        }
        char route_qn[CBM_ROUTE_QN_SIZE];
        snprintf(route_qn, sizeof(route_qn), "__grpc__%s/%s", svc->name, fn->name);

        char props[CBM_SZ_128];
        snprintf(props, sizeof(props), "{\"source\":\"proto\",\"service\":\"%s\"}", svc->name);

        int64_t route_id = cbm_gbuf_upsert_node(gb, "Route", fn->name, route_qn, fn->file_path,
                                                fn->start_line, fn->end_line, props);
        cbm_gbuf_insert_edge(gb, fn->id, route_id, "HANDLES", "{\"via\":\"proto_rpc\"}");
        grpc_routes++;
    }
    cbm_free(CBM_MEM_CLASS_OTHER, services);
    if (grpc_routes > 0) {
        char buf[CBM_SZ_16];
        char svc_buf[CBM_SZ_16];
        snprintf(buf, sizeof(buf), "%d", grpc_routes);
        snprintf(svc_buf, sizeof(svc_buf), "%d", svc_count);
        cbm_log_info("pass.route_nodes.grpc", "routes", buf, "services", svc_buf);
    }
}

/* ── gqlgen configuration (gqlgen.yml) ──────────────────────────────
 *
 * A gqlgen server is defined by its config file: the schema files it loads
 * (`schema:` globs, relative to the config's directory) and where its
 * resolvers live (`resolver:`; `layout: follow-schema` puts them in `dir`,
 * the default single-file layout in `filename`). Only those keys are read,
 * line by line; anything else in the YAML is ignored. */

enum {
    GQLGEN_MAX_CONFIG_BYTES = CBM_SZ_256 * CBM_SZ_1K,
    GQLGEN_MAX_SCHEMA_GLOBS = 64,
    GQLGEN_MAX_GLOB_SEGMENTS = 64,
};

static const char *const gqlgen_config_names[] = {
    "gqlgen.yml",
    "gqlgen.yaml",
    ".gqlgen.yml",
    ".gqlgen.yaml",
};

bool cbm_gqlgen_is_config_path(const char *rel_path) {
    if (!rel_path) {
        return false;
    }
    const char *slash = strrchr(rel_path, '/');
    const char *base = slash ? slash + SKIP_ONE : rel_path;
    for (size_t i = 0; i < sizeof(gqlgen_config_names) / sizeof(gqlgen_config_names[0]); i++) {
        if (strcmp(base, gqlgen_config_names[i]) == 0) {
            return true;
        }
    }
    return false;
}

static bool gql_word_char(char c) {
    return isalnum((unsigned char)c) || c == '_';
}

/* Skip a GraphQL string at text[i] ('"' or a '"""' block); returns the index
 * after it. */
static size_t gql_skip_string(const char *text, size_t len, size_t i) {
    if (i + PAIR_LEN < len && text[i + SKIP_ONE] == '"' && text[i + PAIR_LEN] == '"') {
        for (i += CBM_SZ_3; i < len; i++) {
            if (text[i] == '\\') {
                i++;
            } else if (i + PAIR_LEN < len && text[i] == '"' && text[i + SKIP_ONE] == '"' &&
                       text[i + PAIR_LEN] == '"') {
                return i + CBM_SZ_3;
            }
        }
        return len;
    }
    for (i++; i < len && text[i] != '"' && text[i] != '\n'; i++) {
        if (text[i] == '\\') {
            i++;
        }
    }
    return i < len ? i + SKIP_ONE : len;
}

bool cbm_graphql_text_defines_schema(const char *text, size_t len) {
    static const char *const kinds[] = {"type",  "interface", "input",     "enum",   "union",
                                        "scalar", "directive", "schema", "extend"};
    int depth = 0; /* braces and parentheses */
    for (size_t i = 0; text && i < len;) {
        char c = text[i];
        if (c == '#') {
            while (i < len && text[i] != '\n') {
                i++;
            }
        } else if (c == '"') {
            i = gql_skip_string(text, len, i);
        } else if (c == '{' || c == '(' || c == '[') {
            depth++;
            i++;
        } else if (c == '}' || c == ')' || c == ']') {
            depth -= depth > 0 ? SKIP_ONE : 0;
            i++;
        } else if (gql_word_char(c) || c == '$' || c == '@') {
            size_t start = i++;
            while (i < len && gql_word_char(text[i])) {
                i++;
            }
            for (size_t k = 0; depth == 0 && c != '$' && c != '@' && k < sizeof(kinds) / sizeof(kinds[0]); k++) {
                if (strlen(kinds[k]) == i - start && strncmp(text + start, kinds[k], i - start) == 0) {
                    return true;
                }
            }
        } else {
            i++;
        }
    }
    return false;
}

bool cbm_go_text_has_resolver_method(const char *text, size_t len) {
    static const char FUNC[] = "func";
    static const char SUFFIX[] = "Resolver";
    size_t fl = sizeof(FUNC) - SKIP_ONE;
    size_t sl = sizeof(SUFFIX) - SKIP_ONE;
    for (size_t i = 0; text && i + fl < len; i++) {
        if (strncmp(text + i, FUNC, fl) != 0 || (i > 0 && gql_word_char(text[i - SKIP_ONE])) ||
            gql_word_char(text[i + fl])) {
            continue;
        }
        size_t j = i + fl;
        while (j < len && (text[j] == ' ' || text[j] == '\t')) {
            j++;
        }
        if (j >= len || text[j] != '(') {
            continue;
        }
        size_t close = j;
        while (close < len && text[close] != ')' && text[close] != '\n') {
            close++;
        }
        size_t end = close;
        while (end > j && (text[end - SKIP_ONE] == ' ' || text[end - SKIP_ONE] == '\t')) {
            end--;
        }
        if (close < len && text[close] == ')' && end - j > sl &&
            strncmp(text + end - sl, SUFFIX, sl) == 0) {
            return true;
        }
    }
    return false;
}

static char *gqlgen_strndup(const char *s, size_t n) {
    char *out = cbm_alloc(CBM_MEM_CLASS_OTHER, n + SKIP_ONE);
    if (out) {
        memcpy(out, s, n);
        out[n] = '\0';
    }
    return out;
}

/* A YAML scalar with surrounding blanks, quotes and a trailing comment
 * removed; a heap copy, or NULL when empty. */
static char *gqlgen_scalar(const char *s, const char *end) {
    while (s < end && (*s == ' ' || *s == '\t')) {
        s++;
    }
    char quote = s < end && (*s == '"' || *s == '\'') ? *s : '\0';
    if (quote) {
        const char *close = memchr(s + SKIP_ONE, quote, (size_t)(end - s - SKIP_ONE));
        return close && close > s + SKIP_ONE
                   ? gqlgen_strndup(s + SKIP_ONE, (size_t)(close - s - SKIP_ONE))
                   : NULL;
    }
    const char *e = s;
    while (e < end && !(*e == '#' && (e == s || e[-SKIP_ONE] == ' ' || e[-SKIP_ONE] == '\t'))) {
        e++;
    }
    while (e > s && (e[-SKIP_ONE] == ' ' || e[-SKIP_ONE] == '\t')) {
        e--;
    }
    return e > s ? gqlgen_strndup(s, (size_t)(e - s)) : NULL;
}

static void gqlgen_add_glob(cbm_gqlgen_config_t *c, char *glob) {
    if (glob && c->schema_count < GQLGEN_MAX_SCHEMA_GLOBS) {
        c->schema_globs[c->schema_count++] = glob;
    } else {
        cbm_free(CBM_MEM_CLASS_OTHER, glob);
    }
}

/* `schema: [a, b]` or `schema: a`. */
static void gqlgen_inline_schema(cbm_gqlgen_config_t *c, const char *v, const char *end) {
    while (v < end && (*v == ' ' || *v == '\t')) {
        v++;
    }
    if (v >= end || *v == '#') {
        return;
    }
    if (*v != '[') {
        gqlgen_add_glob(c, gqlgen_scalar(v, end));
        return;
    }
    const char *close = memchr(v, ']', (size_t)(end - v));
    const char *stop = close ? close : end;
    for (const char *p = v + SKIP_ONE; p < stop;) {
        const char *comma = memchr(p, ',', (size_t)(stop - p));
        const char *item_end = comma ? comma : stop;
        gqlgen_add_glob(c, gqlgen_scalar(p, item_end));
        p = item_end + SKIP_ONE;
    }
}

static void gqlgen_set(char **slot, char *value) {
    cbm_free(CBM_MEM_CLASS_OTHER, *slot);
    *slot = value;
}

/* Normalize a repo-relative path in place: "." segments and empty ones
 * dropped, ".." removes the segment before it; a trailing '/' is kept.
 * False when ".." climbs above the repository root. */
static bool gqlgen_normalize(char *path) {
    size_t len = strlen(path);
    bool trailing = len > 0 && path[len - SKIP_ONE] == '/';
    size_t out = 0;
    for (size_t i = 0; i < len;) {
        size_t end = i;
        while (end < len && path[end] != '/') {
            end++;
        }
        size_t seg = end - i;
        if (seg == PAIR_LEN && path[i] == '.' && path[i + SKIP_ONE] == '.') {
            if (out == 0) {
                return false;
            }
            out--; /* the '/' after the previous segment */
            while (out > 0 && path[out - SKIP_ONE] != '/') {
                out--;
            }
        } else if (seg > 0 && !(seg == SKIP_ONE && path[i] == '.')) {
            memmove(path + out, path + i, seg);
            out += seg;
            path[out++] = '/';
        }
        i = end + SKIP_ONE;
    }
    if (out > 0 && !trailing) {
        out--;
    }
    path[out] = '\0';
    return true;
}

/* Join the config's directory and a path it names, normalized ("./",
 * "../"); NULL when it leaves the repository or on allocation failure. */
static char *gqlgen_join(const char *dir, const char *rel) {
    size_t dl = strlen(dir);
    size_t rl = strlen(rel);
    char *out = cbm_alloc(CBM_MEM_CLASS_OTHER, dl + rl + SKIP_ONE);
    if (out) {
        (void)snprintf(out, dl + rl + SKIP_ONE, "%s%s", dir, rel);
        if (!gqlgen_normalize(out)) {
            cbm_free(CBM_MEM_CLASS_OTHER, out);
            out = NULL;
        }
    }
    return out;
}

/* Resolve the config's paths against its directory: schema globs, and the
 * package directory of its resolvers ("" when it has none). */
static void gqlgen_resolve(cbm_gqlgen_config_t *c, const char *rel_path) {
    const char *slash = strrchr(rel_path, '/');
    char *dir = gqlgen_strndup(rel_path, slash ? (size_t)(slash - rel_path) + SKIP_ONE : 0);
    if (!dir) {
        return;
    }
    /* gqlgen's default when `schema` is omitted (codegen/config/config.go
     * DefaultConfig): schema.graphql beside the config. */
    if (!c->has_schema_key) {
        gqlgen_add_glob(c, gqlgen_strndup("schema.graphql", strlen("schema.graphql")));
    }
    /* A glob whose join fails is dropped, never left NULL in the list. */
    int kept = 0;
    for (int i = 0; i < c->schema_count; i++) {
        char *joined = gqlgen_join(dir, c->schema_globs[i]);
        cbm_free(CBM_MEM_CLASS_OTHER, c->schema_globs[i]);
        if (joined) {
            c->schema_globs[kept++] = joined;
        }
    }
    c->schema_count = kept;
    char *pkg = NULL;
    bool follow = c->resolver_layout && strcmp(c->resolver_layout, "follow-schema") == 0;
    if (c->has_resolver && follow && c->resolver_dir) {
        char *joined = gqlgen_join(dir, c->resolver_dir);
        size_t jl = joined ? strlen(joined) : 0;
        pkg = joined && jl > 0 && joined[jl - SKIP_ONE] != '/' ? gqlgen_join(joined, "/") : joined;
        if (pkg != joined) {
            cbm_free(CBM_MEM_CLASS_OTHER, joined);
        }
    } else if (c->has_resolver && !follow) {
        char *file = gqlgen_join(dir, c->resolver_filename ? c->resolver_filename : "resolver.go");
        const char *fs = file ? strrchr(file, '/') : NULL;
        pkg = file ? gqlgen_strndup(file, fs ? (size_t)(fs - file) + SKIP_ONE : 0) : NULL;
        cbm_free(CBM_MEM_CLASS_OTHER, file);
    }
    gqlgen_set(&c->resolver_pkg, pkg);
    cbm_free(CBM_MEM_CLASS_OTHER, dir);
}

/* A config value as a heap copy, quotes and trailing comment removed. */
static char *gqlgen_value(const cbm_yaml_node_t *y, const char *path) {
    const char *v = cbm_yaml_get_str(y, path);
    return v ? gqlgen_scalar(v, v + strlen(v)) : NULL;
}

/* Read the keys a config needs through the foundation YAML reader (block
 * maps and lists); a flow list `schema: [a, b]` is split here. */
static void gqlgen_read(cbm_gqlgen_config_t *c, const cbm_yaml_node_t *y) {
    const char *items[GQLGEN_MAX_SCHEMA_GLOBS];
    int n = cbm_yaml_get_str_list(y, "schema", items, GQLGEN_MAX_SCHEMA_GLOBS);
    for (int i = 0; i < n; i++) {
        gqlgen_add_glob(c, gqlgen_scalar(items[i], items[i] + strlen(items[i])));
    }
    const char *inline_schema = n == 0 ? cbm_yaml_get_str(y, "schema") : NULL;
    if (inline_schema) {
        gqlgen_inline_schema(c, inline_schema, inline_schema + strlen(inline_schema));
    }
    c->has_schema_key = cbm_yaml_has(y, "schema");
    c->has_resolver = cbm_yaml_has(y, "resolver");
    c->resolver_layout = gqlgen_value(y, "resolver.layout");
    c->resolver_dir = gqlgen_value(y, "resolver.dir");
    c->resolver_filename = gqlgen_value(y, "resolver.filename");
}

int cbm_gqlgen_config_load(const char *repo_path, const char *rel_path, cbm_gqlgen_config_t *out) {
    memset(out, 0, sizeof(*out));
    if (!repo_path || !rel_path) {
        return CBM_NOT_FOUND;
    }
    char path[CBM_SZ_4K];
    int n = snprintf(path, sizeof(path), "%s/%s", repo_path, rel_path);
    if (n <= 0 || (size_t)n >= sizeof(path)) {
        return CBM_NOT_FOUND;
    }
    FILE *f = cbm_fopen(path, "rb");
    if (!f) {
        return CBM_NOT_FOUND;
    }
    /* The parser takes the bytes read and their count; the terminator is
     * a guard, never part of the input. */
    char *buf = cbm_alloc(CBM_MEM_CLASS_OTHER, GQLGEN_MAX_CONFIG_BYTES + SKIP_ONE);
    size_t len = buf ? fread(buf, SKIP_ONE, GQLGEN_MAX_CONFIG_BYTES, f) : 0;
    if (buf) {
        buf[len] = '\0';
    }
    (void)fclose(f);
    cbm_yaml_node_t *y = buf ? cbm_yaml_parse(buf, (int)len) : NULL;
    cbm_free(CBM_MEM_CLASS_OTHER, buf);
    out->schema_globs =
        y ? cbm_alloc(CBM_MEM_CLASS_OTHER, GQLGEN_MAX_SCHEMA_GLOBS * sizeof(*out->schema_globs))
          : NULL;
    if (!out->schema_globs) {
        cbm_yaml_free(y);
        return CBM_NOT_FOUND;
    }
    gqlgen_read(out, y);
    cbm_yaml_free(y);
    gqlgen_resolve(out, rel_path);
    return 0;
}

void cbm_gqlgen_config_free(cbm_gqlgen_config_t *c) {
    if (!c) {
        return;
    }
    for (int i = 0; i < c->schema_count; i++) {
        cbm_free(CBM_MEM_CLASS_OTHER, c->schema_globs[i]);
    }
    cbm_free(CBM_MEM_CLASS_OTHER, c->schema_globs);
    cbm_free(CBM_MEM_CLASS_OTHER, c->resolver_layout);
    cbm_free(CBM_MEM_CLASS_OTHER, c->resolver_dir);
    cbm_free(CBM_MEM_CLASS_OTHER, c->resolver_filename);
    cbm_free(CBM_MEM_CLASS_OTHER, c->resolver_pkg);
    memset(c, 0, sizeof(*c));
}

/* Does one path segment match one glob segment (`*` and `?`, no '/')? */
static bool gqlgen_segment_match(const char *pat, size_t pl, const char *str, size_t sl) {
    size_t p = 0;
    size_t s = 0;
    size_t star = SIZE_MAX;
    size_t mark = 0;
    while (s < sl) {
        if (p < pl && (pat[p] == '?' || pat[p] == str[s])) {
            p++;
            s++;
        } else if (p < pl && pat[p] == '*') {
            star = p++;
            mark = s;
        } else if (star != SIZE_MAX) {
            p = star + SKIP_ONE;
            s = ++mark;
        } else {
            return false;
        }
    }
    while (p < pl && pat[p] == '*') {
        p++;
    }
    return p == pl;
}

typedef struct {
    const char *s;
    size_t len;
} gqlgen_seg_t;

static int gqlgen_split(const char *path, gqlgen_seg_t *segs, int max) {
    int n = 0;
    for (const char *p = path; *p && n < max;) {
        const char *slash = strchr(p, '/');
        size_t len = slash ? (size_t)(slash - p) : strlen(p);
        if (len > 0) {
            segs[n++] = (gqlgen_seg_t){p, len};
        }
        p += len + (slash ? SKIP_ONE : 0);
    }
    return n;
}

bool cbm_gqlgen_glob_match(const char *glob, const char *path) {
    gqlgen_seg_t pat[GQLGEN_MAX_GLOB_SEGMENTS];
    gqlgen_seg_t str[GQLGEN_MAX_GLOB_SEGMENTS];
    int pn = gqlgen_split(glob, pat, GQLGEN_MAX_GLOB_SEGMENTS);
    int sn = gqlgen_split(path, str, GQLGEN_MAX_GLOB_SEGMENTS);
    int p = 0;
    int s = 0;
    int star = CBM_NOT_FOUND; /* the last `**` segment, for backtracking */
    int mark = 0;
    while (s < sn) {
        bool globstar = p < pn && pat[p].len == PAIR_LEN && strncmp(pat[p].s, "**", PAIR_LEN) == 0;
        if (globstar) {
            star = p++;
            mark = s;
        } else if (p < pn && gqlgen_segment_match(pat[p].s, pat[p].len, str[s].s, str[s].len)) {
            p++;
            s++;
        } else if (star >= 0) {
            p = star + SKIP_ONE;
            s = ++mark;
        } else {
            return false;
        }
    }
    while (p < pn && pat[p].len == PAIR_LEN && strncmp(pat[p].s, "**", PAIR_LEN) == 0) {
        p++;
    }
    return p == pn;
}

/* ── Phase 4b: GraphQL schema Routes (field paths) ──────────────────
 *
 * A GraphQL server defines fields, not operations: a client's operation
 * selects field paths such as `perp { trades }`. One Route per field path,
 * `__graphql__<Op>.<field>` for each field of the query, mutation and
 * subscription root types, and `__graphql__<Op>.<field>.<child>` for each
 * field of that field's return type, one level down. <Op> is the operation
 * (Query, Mutation, Subscription) whatever the root type is named. Leaf
 * (scalar or enum) return types have no fields, so they get the root Route
 * only.
 *
 * Schema files belong to a server. A gqlgen config (gqlgen.yml) names its
 * server's schema files and resolver package: a `schema { query: X }`
 * definition anywhere in that set names its roots, `extend type T` fields
 * join that set's T, and only methods in that resolver package resolve its
 * fields. Schema files no config claims form one heuristic set: a `schema`
 * definition applies to the files under its parent directory, and a resolver
 * package (a directory with gqlgen `*.resolvers.go` files, or a single-file
 * `resolver.go`) resolves a schema file only when it is the file's directory,
 * below it, or its parent, and no config claims the package.
 *
 * Each Route gets exactly one HANDLES edge: from the one resolver method that
 * owns it (receiver `lcFirst(T)Resolver`, method named after the field
 * case-insensitively), else from the schema Field. Two owning methods, or two
 * schemas defining the same path with no method to tell them apart, mark the
 * Route `ambiguous` and link the first schema's Field. */

enum { GQL_OP_COUNT = 3, GQL_EMIT_INITIAL = 64, GQL_HEURISTIC = -1 };

static const char *const gql_ops[GQL_OP_COUNT] = {"Query", "Mutation", "Subscription"};
static const char *const gql_schema_keys[GQL_OP_COUNT] = {
    "graphql_schema_query",
    "graphql_schema_mutation",
    "graphql_schema_subscription",
};

/* A schema type. */
typedef struct {
    const cbm_gbuf_node_t *node;
    int group; /* index of the owning gqlgen config, or GQL_HEURISTIC */
} gql_class_t;

/* One schema field and the props its Routes carry. Strings live in the
 * pass arena, unbounded. */
typedef struct {
    const cbm_gbuf_node_t *node;
    const char *parent_qn;
    const char *extends; /* T of an `extend type T` block, or NULL */
    const char *return_type;
    const char *reason;
    bool deprecated;
    int group;
} gql_field_t;

/* One file's `schema { ... }` definition and `extend schema { ... }`
 * blocks: the root type name per operation. */
typedef struct {
    const char *file;
    const char *roots[GQL_OP_COUNT];
    int group;
    bool defined; /* holds a `schema` definition, not only extensions */
} gql_schema_t;

/* One candidate resolver method, keyed "<receiver>.<lower-case method>". */
typedef struct {
    const cbm_gbuf_node_t *node;
    const char *key;
    const char *dir; /* its Go package directory, with the trailing '/' */
} gql_resolver_t;

/* One field path a schema defines. */
typedef struct {
    const char *qn;
    const char *path;
    const char *type_name; /* the field's owner type (names the resolver) */
    const gql_field_t *field;
} gql_emit_t;

typedef struct {
    cbm_gbuf_t *gb;
    const char *repo_path;
    CBMArena arena;
    cbm_gqlgen_config_t *configs;
    int config_count;
    gql_class_t *classes;
    int class_count;
    gql_field_t *fields;
    int field_count;
    const gql_class_t **classes_by_name; /* classes sorted by name */
    const gql_field_t **fields_by_parent; /* fields sorted by parent_qn */
    gql_schema_t *schemas;
    int schema_count;
    gql_resolver_t *resolvers;
    int resolver_count;
    const char **heuristic_dirs; /* resolver packages no config claims */
    int heuristic_dir_count;
    gql_emit_t *emits;
    int emit_count;
    int emit_cap;
    int routes;
    int via_resolver;
    int via_field;
    int ambiguous;
    int too_long;
} gql_ctx_t;

static bool gql_is_schema_file(const char *path) {
    return path && path[0] && cbm_language_for_filename(path) == CBM_LANG_GRAPHQL;
}

static bool gql_is_go_file(const char *path) {
    return path && path[0] && cbm_language_for_filename(path) == CBM_LANG_GO;
}

static bool gql_ends_with(const char *s, const char *suffix) {
    size_t sl = strlen(s);
    size_t xl = strlen(suffix);
    return sl > xl && strcmp(s + sl - xl, suffix) == 0;
}

/* Length of the directory part of path, including its trailing '/'. */
static size_t gql_dir_len(const char *path) {
    const char *slash = strrchr(path, '/');
    return slash ? (size_t)(slash - path) + SKIP_ONE : 0;
}

/* Length of the longest common directory prefix of two file paths. */
static size_t gql_common_dir_len(const char *a, const char *b) {
    size_t common = 0;
    for (size_t i = 0; a[i] && a[i] == b[i]; i++) {
        if (a[i] == '/') {
            common = i + SKIP_ONE;
        }
    }
    return common;
}

/* Length of the parent of `path`'s directory, with its trailing '/'. */
static size_t gql_parent_dir_len(const char *path) {
    size_t dl = gql_dir_len(path);
    size_t i = dl > 0 ? dl - SKIP_ONE : 0; /* the slash closing the directory */
    while (i > 0 && path[i - SKIP_ONE] != '/') {
        i--;
    }
    return i;
}

/* The named type of a declared GraphQL type: `[PerpTrade!]!` -> `PerpTrade`. */
static const char *gql_base_type(CBMArena *a, const char *declared) {
    const char *p = declared ? declared : "";
    while (*p && !isalnum((unsigned char)*p) && *p != '_') {
        p++;
    }
    const char *start = p;
    while (*p && (isalnum((unsigned char)*p) || *p == '_')) {
        p++;
    }
    return cbm_arena_strndup(a, start, (size_t)(p - start));
}

/* The string property `key` of a parsed properties object, copied into the
 * arena whatever its length; NULL when absent or not a string. */
static const char *gql_prop(CBMArena *a, yyjson_val *root, const char *key) {
    yyjson_val *val = yyjson_obj_get(root, key);
    const char *str = yyjson_get_str(val);
    return str ? cbm_arena_strndup(a, str, yyjson_get_len(val)) : NULL;
}

/* The gqlgen config whose schema globs claim `path`, or GQL_HEURISTIC. */
static int gql_group_of(const gql_ctx_t *ctx, const char *path) {
    for (int i = 0; i < ctx->config_count; i++) {
        for (int g = 0; g < ctx->configs[i].schema_count; g++) {
            if (cbm_gqlgen_glob_match(ctx->configs[i].schema_globs[g], path)) {
                return i;
            }
        }
    }
    return GQL_HEURISTIC;
}

static int gql_str_cmp(const void *x, const void *y) {
    return strcmp(*(const char *const *)x, *(const char *const *)y);
}

/* Load every gqlgen config the index holds (sorted by path, so the first
 * config claiming a schema file is the same on every run). */
static void gql_load_configs(gql_ctx_t *ctx) {
    const cbm_gbuf_node_t **nodes = NULL;
    int count = 0;
    if (!ctx->repo_path || cbm_gbuf_find_by_label(ctx->gb, "File", &nodes, &count) != 0) {
        return;
    }
    const char **paths = cbm_arena_alloc(&ctx->arena, ((size_t)count + SKIP_ONE) * sizeof(*paths));
    int n = 0;
    for (int i = 0; paths && i < count; i++) {
        if (cbm_gqlgen_is_config_path(nodes[i]->file_path)) {
            paths[n++] = nodes[i]->file_path;
        }
    }
    if (n == 0) {
        return;
    }
    qsort(paths, (size_t)n, sizeof(*paths), gql_str_cmp);
    ctx->configs = cbm_arena_alloc(&ctx->arena, (size_t)n * sizeof(*ctx->configs));
    for (int i = 0; ctx->configs && i < n; i++) {
        if (cbm_gqlgen_config_load(ctx->repo_path, paths[i], &ctx->configs[ctx->config_count]) ==
            0) {
            ctx->config_count++;
        }
    }
}

static void gql_free_configs(gql_ctx_t *ctx) {
    for (int i = 0; i < ctx->config_count; i++) {
        cbm_gqlgen_config_free(&ctx->configs[i]);
    }
}

static int gql_collect_classes(gql_ctx_t *ctx) {
    const cbm_gbuf_node_t **nodes = NULL;
    int count = 0;
    if (cbm_gbuf_find_by_label(ctx->gb, "Class", &nodes, &count) != 0 || count == 0) {
        return 0;
    }
    ctx->classes = cbm_arena_alloc(&ctx->arena, (size_t)count * sizeof(*ctx->classes));
    if (!ctx->classes) {
        return CBM_NOT_FOUND;
    }
    for (int i = 0; i < count; i++) {
        const cbm_gbuf_node_t *c = nodes[i];
        if (gql_is_schema_file(c->file_path) && c->name && c->qualified_name) {
            ctx->classes[ctx->class_count++] = (gql_class_t){c, GQL_HEURISTIC};
        }
    }
    return ctx->class_count;
}

/* `schema { ... }` definitions, recorded on each schema file's Module node. */
static void gql_collect_schemas(gql_ctx_t *ctx) {
    const cbm_gbuf_node_t **nodes = NULL;
    int count = 0;
    if (cbm_gbuf_find_by_label(ctx->gb, "Module", &nodes, &count) != 0 || count == 0) {
        return;
    }
    for (int i = 0; i < count; i++) {
        const cbm_gbuf_node_t *m = nodes[i];
        if (!gql_is_schema_file(m->file_path) || !m->properties_json ||
            !strstr(m->properties_json, "\"graphql_schema_")) {
            continue;
        }
        if (!ctx->schemas) {
            ctx->schemas = cbm_arena_alloc(&ctx->arena, (size_t)count * sizeof(*ctx->schemas));
            if (!ctx->schemas) {
                return;
            }
        }
        gql_schema_t *sd = &ctx->schemas[ctx->schema_count++];
        sd->file = m->file_path;
        sd->group = gql_group_of(ctx, m->file_path);
        yyjson_doc *doc = yyjson_read(m->properties_json, strlen(m->properties_json),
                                      YYJSON_READ_ALLOW_INVALID_UNICODE);
        yyjson_val *root = yyjson_doc_get_root(doc);
        for (int op = 0; op < GQL_OP_COUNT; op++) {
            sd->roots[op] = gql_prop(&ctx->arena, root, gql_schema_keys[op]);
        }
        sd->defined = gql_prop(&ctx->arena, root, "graphql_schema_defined") != NULL;
        yyjson_doc_free(doc);
    }
}

/* Does schema file sd apply to class c? In a config's set, every one of the
 * set's files; in the heuristic set, those whose parent directory holds c. */
static bool gql_schema_applies(const gql_schema_t *sd, const gql_class_t *c) {
    if (sd->group != c->group) {
        return false;
    }
    if (c->group != GQL_HEURISTIC) {
        return true;
    }
    return strncmp(sd->file, c->node->file_path, gql_parent_dir_len(sd->file)) == 0;
}

/* The root type name of operation op for class c. The `schema` definitions
 * and `extend schema` blocks that apply to c merge: for each operation, the
 * first (by path) in a config's set, the nearest in the heuristic set.
 * Without any `schema` definition the default name stands for an operation
 * no extension names; with one, an unnamed operation has no root (NULL). */
static const char *gql_root_name(const gql_ctx_t *ctx, const gql_class_t *c, int op) {
    const gql_schema_t *best = NULL;
    size_t best_len = 0;
    bool defined = false;
    for (int i = 0; i < ctx->schema_count; i++) {
        const gql_schema_t *sd = &ctx->schemas[i];
        if (!gql_schema_applies(sd, c)) {
            continue;
        }
        defined = defined || sd->defined;
        if (!sd->roots[op]) {
            continue;
        }
        size_t len =
            c->group == GQL_HEURISTIC ? gql_common_dir_len(sd->file, c->node->file_path) : 0;
        if (!best || len > best_len || (len == best_len && strcmp(sd->file, best->file) < 0)) {
            best = sd;
            best_len = len;
        }
    }
    if (best) {
        return best->roots[op];
    }
    return defined ? NULL : gql_ops[op];
}

/* The operation class c is the root type of, or CBM_NOT_FOUND. */
static int gql_root_op(const gql_ctx_t *ctx, const gql_class_t *c) {
    for (int op = 0; op < GQL_OP_COUNT; op++) {
        const char *root = gql_root_name(ctx, c, op);
        if (root && strcmp(root, c->node->name) == 0) {
            return op;
        }
    }
    return CBM_NOT_FOUND;
}

static int gql_class_name_cmp(const void *x, const void *y) {
    const gql_class_t *a = *(const gql_class_t *const *)x;
    const gql_class_t *b = *(const gql_class_t *const *)y;
    return strcmp(a->node->name, b->node->name);
}

static int gql_field_parent_cmp(const void *x, const void *y) {
    const gql_field_t *a = *(const gql_field_t *const *)x;
    const gql_field_t *b = *(const gql_field_t *const *)y;
    return strcmp(a->parent_qn, b->parent_qn);
}

/* Sort pointers to the n items of size `size` at `items` with cmp; NULL on
 * allocation failure. */
static const void **gql_sorted_index(CBMArena *a, const void *items, int n, size_t size,
                                     int (*cmp)(const void *, const void *)) {
    const void **index = cbm_arena_alloc(a, ((size_t)n + SKIP_ONE) * sizeof(*index));
    for (int i = 0; index && i < n; i++) {
        index[i] = (const char *)items + ((size_t)i * size);
    }
    if (index && n > SKIP_ONE) {
        qsort(index, (size_t)n, sizeof(*index), cmp);
    }
    return index;
}

/* The type named `name` in set `group` nearest `near`, or NULL (ties:
 * smaller QN). */
static const gql_class_t *gql_find_class(const gql_ctx_t *ctx, const char *name, const char *near,
                                         int group) {
    const gql_class_t *best = NULL;
    size_t best_len = 0;
    int lo = 0;
    int hi = ctx->class_count;
    while (lo < hi) { /* the first class named `name` or after it */
        int mid = lo + ((hi - lo) / PAIR_LEN);
        if (strcmp(ctx->classes_by_name[mid]->node->name, name) < 0) {
            lo = mid + SKIP_ONE;
        } else {
            hi = mid;
        }
    }
    for (int i = lo; i < ctx->class_count && strcmp(ctx->classes_by_name[i]->node->name, name) == 0;
         i++) {
        const gql_class_t *c = ctx->classes_by_name[i];
        if (c->group != group) {
            continue;
        }
        size_t len = gql_common_dir_len(c->node->file_path, near);
        if (!best || len > best_len ||
            (len == best_len && strcmp(c->node->qualified_name, best->node->qualified_name) < 0)) {
            best = c;
            best_len = len;
        }
    }
    return best;
}

/* Read one Field node's props; false when it has no parent type. */
static bool gql_read_field(gql_ctx_t *ctx, const cbm_gbuf_node_t *n, gql_field_t *f) {
    memset(f, 0, sizeof(*f));
    yyjson_doc *doc = yyjson_read(n->properties_json, strlen(n->properties_json),
                                  YYJSON_READ_ALLOW_INVALID_UNICODE);
    yyjson_val *root = yyjson_doc_get_root(doc);
    f->node = n;
    f->parent_qn = gql_prop(&ctx->arena, root, "parent_class");
    f->extends = gql_prop(&ctx->arena, root, "graphql_extends");
    f->return_type = gql_prop(&ctx->arena, root, "return_type");
    const char *flag = gql_prop(&ctx->arena, root, "deprecated");
    f->deprecated = flag && strcmp(flag, "true") == 0;
    f->reason = f->deprecated ? gql_prop(&ctx->arena, root, "deprecation_reason") : NULL;
    f->group = gql_group_of(ctx, n->file_path);
    yyjson_doc_free(doc);
    return f->parent_qn != NULL;
}

static int gql_pending_cmp(const void *x, const void *y) {
    const gql_field_t *a = *(const gql_field_t *const *)x;
    const gql_field_t *b = *(const gql_field_t *const *)y;
    int c = strcmp(a->node->file_path, b->node->file_path);
    if (c == 0) {
        c = a->node->start_line - b->node->start_line;
    }
    return c != 0 ? c : strcmp(a->node->qualified_name, b->node->qualified_name);
}

/* Declare T for `extend type T` fields whose set declares no T (a
 * federation subgraph's `extend type Query`): the first extension (by
 * path, then line) stands for the type, named T, with its block's QN; the
 * set's other extensions of T join it. False on allocation failure. */
static bool gql_declare_extended(gql_ctx_t *ctx, gql_field_t **pending, int n) {
    if (n == 0) {
        return true;
    }
    qsort(pending, (size_t)n, sizeof(*pending), gql_pending_cmp);
    gql_class_t *grown =
        cbm_arena_alloc(&ctx->arena, ((size_t)ctx->class_count + (size_t)n) * sizeof(*grown));
    if (!grown) {
        return false;
    }
    if (ctx->class_count > 0) {
        memcpy(grown, ctx->classes, (size_t)ctx->class_count * sizeof(*grown));
    }
    ctx->classes = grown;
    for (int i = 0; i < n; i++) {
        gql_field_t *f = pending[i];
        const gql_class_t *owner = gql_find_class(ctx, f->extends, f->node->file_path, f->group);
        if (!owner) {
            cbm_gbuf_node_t *type = cbm_arena_alloc(&ctx->arena, sizeof(*type));
            if (!type) {
                return false;
            }
            memset(type, 0, sizeof(*type));
            type->label = (char *)"Class";
            type->name = (char *)f->extends;
            type->qualified_name = (char *)f->parent_qn;
            type->file_path = f->node->file_path;
            type->start_line = f->node->start_line;
            type->end_line = f->node->end_line;
            ctx->classes[ctx->class_count++] = (gql_class_t){type, f->group};
            ctx->classes_by_name = (const gql_class_t **)gql_sorted_index(
                &ctx->arena, ctx->classes, ctx->class_count, sizeof(*ctx->classes),
                gql_class_name_cmp);
            if (!ctx->classes_by_name) {
                return false;
            }
            owner = &ctx->classes[ctx->class_count - SKIP_ONE];
        }
        f->parent_qn = owner->node->qualified_name;
    }
    return true;
}

/* Collect every schema Field; an `extend type T` field is re-parented to its
 * set's nearest declared T, or declares T when its set has none. */
static int gql_collect_fields(gql_ctx_t *ctx) {
    const cbm_gbuf_node_t **nodes = NULL;
    int count = 0;
    if (cbm_gbuf_find_by_label(ctx->gb, "Field", &nodes, &count) != 0 || count == 0) {
        return 0;
    }
    ctx->fields = cbm_arena_alloc(&ctx->arena, (size_t)count * sizeof(*ctx->fields));
    gql_field_t **pending = cbm_arena_alloc(&ctx->arena, (size_t)count * sizeof(*pending));
    if (!ctx->fields || !pending) {
        return CBM_NOT_FOUND;
    }
    int pending_count = 0;
    for (int i = 0; i < count; i++) {
        const cbm_gbuf_node_t *n = nodes[i];
        if (!gql_is_schema_file(n->file_path) || !n->name || !n->properties_json) {
            continue;
        }
        gql_field_t *f = &ctx->fields[ctx->field_count];
        if (!gql_read_field(ctx, n, f)) {
            continue;
        }
        if (f->extends) {
            const gql_class_t *owner = gql_find_class(ctx, f->extends, n->file_path, f->group);
            if (owner) {
                f->parent_qn = owner->node->qualified_name;
            } else {
                pending[pending_count++] = f;
            }
        }
        ctx->field_count++;
    }
    return gql_declare_extended(ctx, pending, pending_count) ? ctx->field_count : CBM_NOT_FOUND;
}

/* The receiver type of a Go method: the last segment of its QN's parent,
 * `proj.graph.resolver.perpResolver.Trades` -> `perpResolver`. */
static const char *gql_receiver_type(CBMArena *a, const cbm_gbuf_node_t *m) {
    const char *qn = m->qualified_name;
    size_t qlen = strlen(qn);
    size_t nlen = strlen(m->name);
    if (qlen <= nlen + SKIP_ONE || qn[qlen - nlen - SKIP_ONE] != '.' ||
        strcmp(qn + qlen - nlen, m->name) != 0) {
        return NULL;
    }
    size_t end = qlen - nlen - SKIP_ONE;
    size_t start = end;
    while (start > 0 && qn[start - SKIP_ONE] != '.') {
        start--;
    }
    return cbm_arena_strndup(a, qn + start, end - start);
}

/* A Go method name as the resolver lookup compares it: lower case. */
static char *gql_lower_dup(CBMArena *a, const char *s) {
    char *out = cbm_arena_strdup(a, s);
    for (char *p = out; p && *p; p++) {
        *p = (char)tolower((unsigned char)*p);
    }
    return out;
}

/* A schema field name as its generated resolver method is named, in lower
 * case. gqlgen names the method with templates.ToGo, which splits words at
 * '_' and joins them capitalized (`user_count` -> `UserCount`, `user_id` ->
 * `UserID`), but keeps one '_' of a run between two digits (`score0_1` ->
 * `Score0_1`, distinct from `score01`). Leading and trailing '_' go. Folding
 * case covers ToGo's initialisms. */
static char *gql_field_method_dup(CBMArena *a, const char *field) {
    char *out = cbm_arena_strdup(a, field);
    char *w = out;
    for (const char *p = field; out && *p;) {
        if (*p != '_') {
            *w++ = (char)tolower((unsigned char)*p++);
            continue;
        }
        const char *run = p;
        while (*p == '_') {
            p++;
        }
        if (run > field && isdigit((unsigned char)run[-SKIP_ONE]) && isdigit((unsigned char)*p)) {
            *w++ = '_';
        }
    }
    if (w) {
        *w = '\0';
    }
    return out;
}

static int gql_resolver_cmp(const void *x, const void *y) {
    const gql_resolver_t *a = x;
    const gql_resolver_t *b = y;
    int c = strcmp(a->key, b->key);
    return c != 0 ? c : strcmp(a->node->qualified_name, b->node->qualified_name);
}

static bool gql_dir_listed(const char *const *dirs, int count, const char *dir) {
    for (int i = 0; i < count; i++) {
        if (strcmp(dirs[i], dir) == 0) {
            return true;
        }
    }
    return false;
}

static bool gql_is_config_package(const gql_ctx_t *ctx, const char *dir) {
    for (int i = 0; i < ctx->config_count; i++) {
        if (ctx->configs[i].resolver_pkg && strcmp(ctx->configs[i].resolver_pkg, dir) == 0) {
            return true;
        }
    }
    return false;
}

/* The resolver packages no config claims: directories holding a gqlgen
 * `*.resolvers.go` file (follow-schema layout) or a `resolver.go` with
 * resolver methods (single-file layout). */
static void gql_heuristic_dirs(gql_ctx_t *ctx, const cbm_gbuf_node_t **nodes, int count) {
    ctx->heuristic_dirs = cbm_arena_alloc(&ctx->arena, (size_t)count * sizeof(char *));
    for (int i = 0; ctx->heuristic_dirs && i < count; i++) {
        const char *fp = nodes[i]->file_path;
        const char *base = fp ? fp + gql_dir_len(fp) : "";
        if (!fp || (!gql_ends_with(fp, ".resolvers.go") && strcmp(base, "resolver.go") != 0)) {
            continue;
        }
        const char *dir = cbm_arena_strndup(&ctx->arena, fp, gql_dir_len(fp));
        if (dir && !gql_is_config_package(ctx, dir) &&
            !gql_dir_listed(ctx->heuristic_dirs, ctx->heuristic_dir_count, dir)) {
            ctx->heuristic_dirs[ctx->heuristic_dir_count++] = dir;
        }
    }
}

/* Collect the Go methods that can be gqlgen resolvers: receiver type ending
 * in "Resolver", in a configured or heuristic resolver package. Sorted by
 * key. */
static int gql_collect_resolvers(gql_ctx_t *ctx) {
    const cbm_gbuf_node_t **nodes = NULL;
    int count = 0;
    if (cbm_gbuf_find_by_label(ctx->gb, "Method", &nodes, &count) != 0 || count == 0) {
        return 0;
    }
    gql_heuristic_dirs(ctx, nodes, count);
    ctx->resolvers = cbm_arena_alloc(&ctx->arena, (size_t)count * sizeof(*ctx->resolvers));
    if (!ctx->resolvers) {
        return CBM_NOT_FOUND;
    }
    for (int i = 0; i < count; i++) {
        const cbm_gbuf_node_t *m = nodes[i];
        if (!m->name || !m->qualified_name || !gql_is_go_file(m->file_path)) {
            continue;
        }
        const char *dir = cbm_arena_strndup(&ctx->arena, m->file_path, gql_dir_len(m->file_path));
        if (!dir || (!gql_is_config_package(ctx, dir) &&
                     !gql_dir_listed(ctx->heuristic_dirs, ctx->heuristic_dir_count, dir))) {
            continue;
        }
        const char *recv = gql_receiver_type(&ctx->arena, m);
        if (!recv || !gql_ends_with(recv, "Resolver")) {
            continue;
        }
        const char *method = gql_lower_dup(&ctx->arena, m->name);
        const char *key = method ? cbm_arena_sprintf(&ctx->arena, "%s.%s", recv, method) : NULL;
        if (!key) {
            continue; /* out of memory: never publish an entry without its key */
        }
        ctx->resolvers[ctx->resolver_count++] = (gql_resolver_t){m, key, dir};
    }
    if (ctx->resolver_count > SKIP_ONE) {
        qsort(ctx->resolvers, (size_t)ctx->resolver_count, sizeof(*ctx->resolvers),
              gql_resolver_cmp);
    }
    return ctx->resolver_count;
}

/* The index of the first resolver whose key is `key`, or resolver_count. */
static int gql_resolver_lower_bound(const gql_ctx_t *ctx, const char *key) {
    int lo = 0;
    int hi = ctx->resolver_count;
    while (lo < hi) {
        int mid = lo + ((hi - lo) / PAIR_LEN);
        if (strcmp(ctx->resolvers[mid].key, key) < 0) {
            lo = mid + SKIP_ONE;
        } else {
            hi = mid;
        }
    }
    return lo;
}

/* The lookup key gqlgen's naming gives field `field` of type `type_name`:
 * `Perp`, `trades` -> "perpResolver.trades". */
static const char *gql_resolver_key(CBMArena *a, const char *type_name, const char *field) {
    const char *lower = gql_field_method_dup(a, field);
    char *key = lower ? cbm_arena_sprintf(a, "%sResolver.%s", type_name, lower) : NULL;
    if (key && key[0]) {
        key[0] = (char)tolower((unsigned char)key[0]);
    }
    return key;
}

/* Does resolver r belong to the server of schema field f? A configured
 * server owns exactly its resolver package; a heuristic package owns the
 * schema files in its own directory, below it, or one level below. */
static bool gql_owns(const gql_ctx_t *ctx, const gql_resolver_t *r, const gql_field_t *f) {
    if (f->group != GQL_HEURISTIC) {
        const char *pkg = ctx->configs[f->group].resolver_pkg;
        return pkg && strcmp(pkg, r->dir) == 0;
    }
    if (gql_is_config_package(ctx, r->dir)) {
        return false;
    }
    const char *file = f->node->file_path;
    size_t sl = gql_dir_len(file);
    size_t rl = strlen(r->dir);
    bool same_or_below = rl >= sl && strncmp(r->dir, file, sl) == 0;
    bool parent = rl == gql_parent_dir_len(file) && strncmp(r->dir, file, rl) == 0;
    return same_or_below || parent;
}

/* The fields whose parent is `parent_qn`: [*first, return). */
static int gql_fields_of(const gql_ctx_t *ctx, const char *parent_qn, int *first) {
    int lo = 0;
    int hi = ctx->field_count;
    while (lo < hi) {
        int mid = lo + ((hi - lo) / PAIR_LEN);
        if (strcmp(ctx->fields_by_parent[mid]->parent_qn, parent_qn) < 0) {
            lo = mid + SKIP_ONE;
        } else {
            hi = mid;
        }
    }
    int end = lo;
    while (end < ctx->field_count && strcmp(ctx->fields_by_parent[end]->parent_qn, parent_qn) == 0) {
        end++;
    }
    *first = lo;
    return end;
}

static void gql_add_emit(gql_ctx_t *ctx, const char *path, const char *type_name,
                         const gql_field_t *f) {
    if (!path) {
        return; /* out of memory building the path */
    }
    const char *qn = cbm_arena_sprintf(&ctx->arena, "__graphql__%s", path);
    if (!qn || strlen(qn) >= CBM_ROUTE_QN_SIZE) {
        ctx->too_long++; /* reported in the pass summary, never truncated */
        return;
    }
    if (ctx->emit_count == ctx->emit_cap) {
        int cap = ctx->emit_cap ? ctx->emit_cap * PAIR_LEN : GQL_EMIT_INITIAL;
        gql_emit_t *grown = cbm_arena_alloc(&ctx->arena, (size_t)cap * sizeof(*grown));
        if (!grown) {
            return;
        }
        if (ctx->emit_count > 0) {
            memcpy(grown, ctx->emits, (size_t)ctx->emit_count * sizeof(*grown));
        }
        ctx->emits = grown;
        ctx->emit_cap = cap;
    }
    ctx->emits[ctx->emit_count++] = (gql_emit_t){qn, path, type_name, f};
}

/* The field paths of one root type: each field, and each field of that
 * field's return type. */
static void gql_plan_root(gql_ctx_t *ctx, const gql_class_t *root, int op) {
    int first = 0;
    int end = gql_fields_of(ctx, root->node->qualified_name, &first);
    for (int i = first; i < end; i++) {
        const gql_field_t *rf = ctx->fields_by_parent[i];
        const char *path = cbm_arena_sprintf(&ctx->arena, "%s.%s", gql_ops[op], rf->node->name);
        if (!path) {
            continue;
        }
        gql_add_emit(ctx, path, root->node->name, rf);
        const char *base = gql_base_type(&ctx->arena, rf->return_type);
        const gql_class_t *type =
            base && base[0] ? gql_find_class(ctx, base, rf->node->file_path, root->group) : NULL;
        int child = 0;
        int child_end = type ? gql_fields_of(ctx, type->node->qualified_name, &child) : 0;
        for (int j = child; j < child_end; j++) {
            const gql_field_t *cf = ctx->fields_by_parent[j];
            const char *child_path = cbm_arena_sprintf(&ctx->arena, "%s.%s", path, cf->node->name);
            if (child_path) {
                gql_add_emit(ctx, child_path, type->node->name, cf);
            }
        }
    }
}

static int gql_emit_cmp(const void *x, const void *y) {
    const gql_emit_t *a = x;
    const gql_emit_t *b = y;
    int c = strcmp(a->qn, b->qn);
    if (c == 0) {
        c = strcmp(a->field->node->file_path, b->field->node->file_path);
    }
    if (c == 0) {
        c = a->field->node->start_line - b->field->node->start_line;
    }
    return c != 0 ? c : strcmp(a->field->node->qualified_name, b->field->node->qualified_name);
}

/* The decision for one Route: the emission whose field it shows, and the
 * resolver that handles it (NULL: the field does). */
typedef struct {
    const gql_emit_t *emit;
    const cbm_gbuf_node_t *resolver;
    bool ambiguous;
} gql_pick_t;

/* Pick among the emissions [first, first + n) of one field path: the one
 * resolver method that owns any of them wins; none leaves the Field; more
 * than one is ambiguous. */
static gql_pick_t gql_pick(gql_ctx_t *ctx, const gql_emit_t *first, int n) {
    gql_pick_t pick = {first, NULL, n > SKIP_ONE};
    for (int e = 0; e < n; e++) {
        const gql_emit_t *em = &first[e];
        const char *key = gql_resolver_key(&ctx->arena, em->type_name, em->field->node->name);
        for (int i = key ? gql_resolver_lower_bound(ctx, key) : ctx->resolver_count;
             i < ctx->resolver_count && strcmp(ctx->resolvers[i].key, key) == 0; i++) {
            const gql_resolver_t *r = &ctx->resolvers[i];
            if (!gql_owns(ctx, r, em->field) || r->node == pick.resolver) {
                continue;
            }
            if (pick.resolver) {
                return (gql_pick_t){first, NULL, true};
            }
            pick = (gql_pick_t){em, r->node, false};
        }
    }
    return pick;
}

static char *gql_route_props(const gql_field_t *f, bool ambiguous) {
    yyjson_mut_doc *doc = yyjson_mut_doc_new(NULL);
    if (!doc) {
        return NULL;
    }
    yyjson_mut_val *root = yyjson_mut_obj(doc);
    yyjson_mut_doc_set_root(doc, root);
    yyjson_mut_obj_add_str(doc, root, "source", "graphql_schema");
    yyjson_mut_obj_add_str(doc, root, "schema_type", f->return_type ? f->return_type : "");
    yyjson_mut_obj_add_bool(doc, root, "deprecated", f->deprecated);
    yyjson_mut_obj_add_str(doc, root, "deprecation_reason", f->reason ? f->reason : "");
    if (ambiguous) {
        yyjson_mut_obj_add_bool(doc, root, "ambiguous", true);
    }
    char *json = yyjson_mut_write(doc, YYJSON_WRITE_ALLOW_INVALID_UNICODE, NULL);
    yyjson_mut_doc_free(doc);
    return json;
}

/* Upsert one Route from its pick, with its one HANDLES edge. */
static void gql_emit_route(gql_ctx_t *ctx, gql_pick_t pick) {
    const gql_field_t *f = pick.emit->field;
    char *props = gql_route_props(f, pick.ambiguous);
    if (!props) {
        return;
    }
    const cbm_gbuf_node_t *fn = f->node;
    int64_t route_id = cbm_gbuf_upsert_node(ctx->gb, "Route", pick.emit->path, pick.emit->qn,
                                            fn->file_path, fn->start_line, fn->end_line, props);
    free(props);
    if (route_id == 0) {
        return;
    }
    ctx->routes++;
    ctx->ambiguous += pick.ambiguous ? SKIP_ONE : 0;
    if (pick.resolver) {
        cbm_gbuf_insert_edge(ctx->gb, pick.resolver->id, route_id, "HANDLES",
                             "{\"via\":\"gqlgen_resolver\"}");
        ctx->via_resolver++;
    } else {
        cbm_gbuf_insert_edge(ctx->gb, fn->id, route_id, "HANDLES",
                             "{\"via\":\"graphql_schema_field\"}");
        ctx->via_field++;
    }
}

static void gql_log_summary(const gql_ctx_t *ctx) {
    char b1[CBM_SZ_16];
    char b2[CBM_SZ_16];
    char b3[CBM_SZ_16];
    char b4[CBM_SZ_16];
    (void)snprintf(b1, sizeof(b1), "%d", ctx->routes);
    (void)snprintf(b2, sizeof(b2), "%d", ctx->via_resolver);
    (void)snprintf(b3, sizeof(b3), "%d", ctx->via_field);
    (void)snprintf(b4, sizeof(b4), "%d", ctx->ambiguous);
    cbm_log_info("pass.route_nodes.graphql", "routes", b1, "via_resolver", b2, "via_field", b3,
                 "ambiguous", b4);
    if (ctx->too_long > 0) {
        char b5[CBM_SZ_16];
        (void)snprintf(b5, sizeof(b5), "%d", ctx->too_long);
        cbm_log_warn("pass.route_nodes.graphql_skipped", "reason", "route_qn_too_long", "paths",
                     b5);
    }
}

static void gql_plan_and_emit(gql_ctx_t *ctx) {
    for (int i = 0; i < ctx->class_count; i++) {
        ctx->classes[i].group = gql_group_of(ctx, ctx->classes[i].node->file_path);
    }
    ctx->classes_by_name = (const gql_class_t **)gql_sorted_index(
        &ctx->arena, ctx->classes, ctx->class_count, sizeof(*ctx->classes), gql_class_name_cmp);
    if (!ctx->classes_by_name) {
        return;
    }
    gql_collect_schemas(ctx);
    if (gql_collect_fields(ctx) <= 0 || gql_collect_resolvers(ctx) < 0) {
        return;
    }
    ctx->fields_by_parent = (const gql_field_t **)gql_sorted_index(
        &ctx->arena, ctx->fields, ctx->field_count, sizeof(*ctx->fields), gql_field_parent_cmp);
    if (!ctx->fields_by_parent) {
        return;
    }
    for (int i = 0; i < ctx->class_count; i++) {
        int op = gql_root_op(ctx, &ctx->classes[i]);
        if (op >= 0) {
            gql_plan_root(ctx, &ctx->classes[i], op);
        }
    }
    if (ctx->emit_count > SKIP_ONE) {
        qsort(ctx->emits, (size_t)ctx->emit_count, sizeof(*ctx->emits), gql_emit_cmp);
    }
    for (int i = 0; i < ctx->emit_count;) {
        int n = SKIP_ONE;
        while (i + n < ctx->emit_count && strcmp(ctx->emits[i + n].qn, ctx->emits[i].qn) == 0) {
            n++;
        }
        gql_emit_route(ctx, gql_pick(ctx, &ctx->emits[i], n));
        i += n;
    }
}

/* Does the buffer hold a Field from a GraphQL file? */
static bool gql_has_schema_field(cbm_gbuf_t *gb) {
    const cbm_gbuf_node_t **nodes = NULL;
    int count = 0;
    if (cbm_gbuf_find_by_label(gb, "Field", &nodes, &count) != 0) {
        return false;
    }
    for (int i = 0; i < count; i++) {
        if (gql_is_schema_file(nodes[i]->file_path)) {
            return true;
        }
    }
    return false;
}

/* Phase 4b entry: __graphql__<Op>.<field>[.<child>] Routes from GraphQL
 * schema files, with HANDLES from gqlgen resolvers or the schema Field.
 * repo_path locates the gqlgen configs (NULL: none are read). */
void cbm_pipeline_create_graphql_routes(cbm_gbuf_t *gb, const char *repo_path) {
    if (!gb) {
        return;
    }
    gql_ctx_t ctx = {.gb = gb, .repo_path = repo_path};
    cbm_arena_init(&ctx.arena);
    /* Most repositories have no schema: stop before touching anything else.
     * A schema of extensions only (a federation subgraph) has no Class. */
    if (gql_collect_classes(&ctx) > 0 || (ctx.class_count == 0 && gql_has_schema_field(gb))) {
        gql_load_configs(&ctx);
        gql_plan_and_emit(&ctx);
        gql_free_configs(&ctx);
        if (ctx.routes > 0 || ctx.too_long > 0) {
            gql_log_summary(&ctx);
        }
    }
    cbm_arena_destroy(&ctx.arena);
}

/* Phase 3: Create DATA_FLOWS edges by linking callers through Route to handlers. */
static void create_data_flows(cbm_gbuf_t *gb) {
    const cbm_gbuf_node_t **routes = NULL;
    int route_count = 0;
    if (cbm_gbuf_find_by_label(gb, "Route", &routes, &route_count) != 0 || route_count == 0) {
        return;
    }

    data_flow_stats_t st = {0};

    for (int ri = 0; ri < route_count; ri++) {
        create_route_data_flows(gb, routes[ri], &st);
    }

    if (st.flows > 0 || st.skipped > 0 || st.test_scoped > 0) {
        char buf1[CBM_SZ_16];
        char buf2[CBM_SZ_16];
        char buf3[CBM_SZ_16];
        snprintf(buf1, sizeof(buf1), "%d", st.flows);
        snprintf(buf2, sizeof(buf2), "%d", st.skipped);
        snprintf(buf3, sizeof(buf3), "%d", st.test_scoped);
        cbm_log_info("pass.data_flows", "created", buf1, "skipped_has_call", buf2,
                     "skipped_test_scope", buf3);
    }
}

/* ── SvelteKit filesystem-based route extractor ──────────────────────
 *
 * SvelteKit derives REST endpoints and SSR loaders from the filesystem
 * layout under `src/routes/`. No `app.get(...)` style call exists for
 * pass_calls.c to pick up, so we walk File nodes and synthesise Route
 * nodes + HANDLES edges directly here.
 *
 * Recognised filenames (TypeScript and JavaScript variants):
 *
 *   +server.{ts,js}            → REST endpoints, exports GET/POST/...
 *   +page.server.{ts,js}       → SSR page server module: load + actions
 *   +layout.server.{ts,js}     → SSR layout server module: load
 *
 * Route path is derived from the directory chain under `routes/`:
 *
 *   apps/x/src/routes/foo/+server.ts        → /foo
 *   apps/x/src/routes/api/items/+server.ts  → /api/items
 *   apps/x/src/routes/(grp)/foo/+server.ts  → /foo            (group stripped)
 *   apps/x/src/routes/[slug]/+server.ts     → /:slug          (param rewrite)
 *   apps/x/src/routes/+server.ts            → /               (root)
 */

enum {
    SKR_PATH_BUF = 1024,
    SKR_NAME_BUF = 64,
};

/* Detect the SvelteKit kind of a file path. Returns 1 for +server,
 * 2 for +page.server, 3 for +layout.server; 0 if not a SvelteKit
 * server-side route file. */
static int sveltekit_file_kind(const char *file_path) {
    if (!file_path) {
        return 0;
    }
    /* The basename must match one of the three patterns. We also require
     * a "/routes/" segment somewhere in the path so we don't snag files
     * in unrelated directories that happen to be named "+server.ts". */
    if (!strstr(file_path, "/routes/")) {
        return 0;
    }
    const char *slash = strrchr(file_path, '/');
    const char *base = slash ? slash + 1 : file_path;
    if (strcmp(base, "+server.ts") == 0 || strcmp(base, "+server.js") == 0) {
        return 1;
    }
    if (strcmp(base, "+page.server.ts") == 0 || strcmp(base, "+page.server.js") == 0) {
        return 2;
    }
    if (strcmp(base, "+layout.server.ts") == 0 || strcmp(base, "+layout.server.js") == 0) {
        return 3;
    }
    return 0;
}

/* Compute the URL route path for a SvelteKit file path. Writes into
 * `out` (caller-provided buffer) and returns the buffer or NULL on
 * error. The leading "/" is always present; "/" is used for the root
 * route. Group segments wrapped in parentheses are stripped. Dynamic
 * params `[slug]` become `:slug`, and rest params `[...slug]` become
 * `*slug` so the result is recognisable as a path pattern. */
static const char *sveltekit_route_path(const char *file_path, char *out, int outsz) {
    if (!file_path || !out || outsz <= 1) {
        return NULL;
    }
    const char *routes_seg = strstr(file_path, "/routes/");
    if (!routes_seg) {
        return NULL;
    }
    /* Walk segment-by-segment between "/routes/" and the trailing "+...". */
    const char *p = routes_seg + strlen("/routes/");
    const char *last_slash = strrchr(file_path, '/');
    if (!last_slash || last_slash < p) {
        /* File is directly under /routes/ — root route. */
        out[0] = '/';
        out[1] = '\0';
        return out;
    }
    /* p points at first char after /routes/, last_slash points at the
     * slash before the +...filename. Iterate segments between them. */
    int pos = 0;
    out[pos] = '\0';
    while (p < last_slash) {
        const char *seg_end = strchr(p, '/');
        if (!seg_end || seg_end > last_slash) {
            seg_end = last_slash;
        }
        size_t seg_len = (size_t)(seg_end - p);
        if (seg_len > 0) {
            /* Group `(name)` — skip entirely. */
            if (p[0] == '(' && p[seg_len - 1] == ')') {
                p = seg_end + 1;
                continue;
            }
            /* Emit "/" then rewritten segment. */
            if (pos + 1 < outsz - 1) {
                out[pos++] = '/';
            }
            /* Dynamic param: `[slug]` → `:slug`, `[...slug]` → `*slug`. */
            if (seg_len >= 2 && p[0] == '[' && p[seg_len - 1] == ']') {
                const char *inner = p + 1;
                size_t inner_len = seg_len - 2;
                if (inner_len >= 3 && strncmp(inner, "...", 3) == 0) {
                    if (pos < outsz - 1) {
                        out[pos++] = '*';
                    }
                    inner += 3;
                    inner_len -= 3;
                } else {
                    if (pos < outsz - 1) {
                        out[pos++] = ':';
                    }
                }
                /* Strip an optional `=matcher` suffix after the param name. */
                size_t copy_len = inner_len;
                for (size_t i = 0; i < inner_len; i++) {
                    if (inner[i] == '=') {
                        copy_len = i;
                        break;
                    }
                }
                if ((int)copy_len > outsz - 1 - pos) {
                    copy_len = (size_t)(outsz - 1 - pos);
                }
                memcpy(out + pos, inner, copy_len);
                pos += (int)copy_len;
            } else {
                size_t copy_len = seg_len;
                if ((int)copy_len > outsz - 1 - pos) {
                    copy_len = (size_t)(outsz - 1 - pos);
                }
                memcpy(out + pos, p, copy_len);
                pos += (int)copy_len;
            }
            out[pos] = '\0';
        }
        p = seg_end + 1;
    }
    if (pos == 0) {
        out[pos++] = '/';
        out[pos] = '\0';
    }
    return out;
}

/* Returns the HTTP method string a function name maps to for a +server
 * file, or NULL if the function isn't a SvelteKit REST handler. The set
 * mirrors SvelteKit's documented HTTP verb exports. */
static const char *sveltekit_server_method(const char *name) {
    if (!name) {
        return NULL;
    }
    static const char *const verbs[] = {
        "GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "HEAD",
    };
    for (size_t i = 0; i < sizeof(verbs) / sizeof(verbs[0]); i++) {
        if (strcmp(name, verbs[i]) == 0) {
            return verbs[i];
        }
    }
    /* `fallback` catches any verb not explicitly exported. */
    if (strcmp(name, "fallback") == 0) {
        return "ANY";
    }
    return NULL;
}

typedef struct {
    cbm_gbuf_t *gb;
    int routes_created;
    int handles_created;
    int files_seen;
} sveltekit_ctx_t;

/* The properties of a SvelteKit handler's HANDLES edge: its QN, escaped as
 * every HANDLES edge's handler is. */
static void sveltekit_handles_props(char *out, size_t outsz, const cbm_gbuf_node_t *child,
                                    bool is_actions) {
    char esc_h[CBM_SZ_512];
    cbm_json_escape(esc_h, sizeof(esc_h),
                    child->qualified_name ? child->qualified_name : child->name);
    (void)snprintf(out, outsz, "{\"handler\":\"%s\",\"framework\":\"sveltekit\"%s}", esc_h,
                   is_actions ? ",\"via\":\"actions_object\"" : "");
}

/* Process one File node: if it matches a SvelteKit server-side file,
 * synthesise a Route node and HANDLES edges from any handler functions
 * (or actions Variable) defined in the file. */
static void sveltekit_file_visitor(const cbm_gbuf_node_t *node, void *userdata) {
    sveltekit_ctx_t *ctx = (sveltekit_ctx_t *)userdata;
    if (!node || !node->label || strcmp(node->label, "File") != 0) {
        return;
    }
    int kind = sveltekit_file_kind(node->file_path);
    if (kind == 0) {
        return;
    }
    ctx->files_seen++;

    char route_path[SKR_PATH_BUF];
    if (!sveltekit_route_path(node->file_path, route_path, sizeof(route_path))) {
        return;
    }

    /* Find every DEFINES edge from this file to a Function or Variable. */
    const cbm_gbuf_edge_t **edges = NULL;
    int edge_count = 0;
    if (cbm_gbuf_find_edges_by_source_type(ctx->gb, node->id, "DEFINES", &edges, &edge_count) !=
            0 ||
        edge_count == 0) {
        return;
    }

    for (int i = 0; i < edge_count; i++) {
        const cbm_gbuf_node_t *child = cbm_gbuf_find_by_id(ctx->gb, edges[i]->target_id);
        if (!child || !child->name || !child->label) {
            continue;
        }
        bool is_fn = strcmp(child->label, "Function") == 0;
        bool is_var = strcmp(child->label, "Variable") == 0;
        if (!is_fn && !is_var) {
            continue;
        }

        const char *method = NULL;
        bool is_actions = false;
        if (kind == 1) {
            method = sveltekit_server_method(child->name);
        } else if (kind == 2) {
            if (is_fn && strcmp(child->name, "load") == 0) {
                method = "LOAD";
            } else if (strcmp(child->name, "actions") == 0) {
                /* SvelteKit form actions: emit a single Route at this
                 * path with method=ACTIONS; finer-grained action names
                 * aren't exposed as top-level identifiers. */
                method = "ACTIONS";
                is_actions = is_var;
            }
        } else if (kind == 3) {
            if (is_fn && strcmp(child->name, "load") == 0) {
                method = "LOAD";
            }
        }
        if (!method) {
            continue;
        }

        char route_qn[CBM_ROUTE_QN_SIZE];
        char cpath[CBM_SZ_256];
        snprintf(route_qn, sizeof(route_qn), "__route__%s__%s", method,
                 cbm_route_canon_path(route_path, cpath, sizeof(cpath)));
        char route_props[CBM_SZ_256];
        snprintf(route_props, sizeof(route_props),
                 "{\"method\":\"%s\",\"framework\":\"sveltekit\"}", method);
        int64_t route_id =
            cbm_gbuf_upsert_node(ctx->gb, "Route", route_path, route_qn, "", 0, 0, route_props);
        if (route_id == 0) {
            continue;
        }
        ctx->routes_created++;

        char hprops[CBM_SZ_1K]; /* must exceed escaped value + wrapper or snprintf cuts the
                                   closing brace */
        sveltekit_handles_props(hprops, sizeof(hprops), child, is_actions);
        cbm_gbuf_insert_edge(ctx->gb, child->id, route_id, "HANDLES", hprops);
        ctx->handles_created++;
    }
}

/* Public entry point: scan all File nodes for SvelteKit server modules
 * and synthesise Route + HANDLES nodes from the filesystem convention. */
static void create_sveltekit_routes(cbm_gbuf_t *gb) {
    if (!gb) {
        return;
    }
    sveltekit_ctx_t ctx = {.gb = gb, .routes_created = 0, .handles_created = 0, .files_seen = 0};
    cbm_gbuf_foreach_node(gb, sveltekit_file_visitor, &ctx);
    if (ctx.files_seen > 0) {
        char b1[CBM_SZ_16];
        char b2[CBM_SZ_16];
        char b3[CBM_SZ_16];
        snprintf(b1, sizeof(b1), "%d", ctx.files_seen);
        snprintf(b2, sizeof(b2), "%d", ctx.routes_created);
        snprintf(b3, sizeof(b3), "%d", ctx.handles_created);
        cbm_log_info("pass.sveltekit_routes", "files", b1, "routes", b2, "handles", b3);
    }
}

void cbm_pipeline_create_route_nodes(cbm_gbuf_t *gb) {
    if (!gb) {
        return;
    }

    route_ctx_t ctx = {.gb = gb, .created = 0};
    cbm_gbuf_foreach_edge(gb, route_edge_visitor, &ctx);

    if (ctx.created > 0) {
        char buf[CBM_SZ_16];
        snprintf(buf, sizeof(buf), "%d", ctx.created);
        cbm_log_info("pass.route_nodes", "created", buf);
    }

    /* Phase 2a: ensure all functions with route_path have Route+HANDLES.
     * Handles incremental mode where unchanged files don't re-extract. */
    ensure_decorator_routes(gb);

    /* Phase 2b: connect prefix Routes to decorator handler Functions.
     * Must run BEFORE match_infra_routes so infra matching can find
     * HANDLES edges on prefix Routes for the bridge. */
    connect_prefix_to_decorators(gb);

    /* Phase 2b: match infra Routes to handler Routes by URL path */
    match_infra_routes(gb);

    /* Phase 3: create DATA_FLOWS edges through Routes */
    create_data_flows(gb);

    /* Phase 4: create gRPC Route nodes from protobuf service definitions.
     * Scans Class nodes from .proto files, follows DEFINES_METHOD edges
     * to find rpc methods, creates __grpc__ServiceName/MethodName Route nodes. */
    create_grpc_routes(gb);

    /* Phase 5: filesystem-based SvelteKit routes (+server / +page.server /
     * +layout.server) — no call-site equivalent for pass_calls.c to pick
     * up, so we walk File nodes directly here. */
    create_sveltekit_routes(gb);
}
