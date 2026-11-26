/* Route selection shared by the socket adapter and transport-independent tests. */
#ifndef CBM_UI_HTTP_ROUTES_H
#define CBM_UI_HTTP_ROUTES_H

#include <stdbool.h>
#include <string.h>

typedef enum {
    UI_ROUTE_ASSET,
    UI_ROUTE_PREFLIGHT,
    UI_ROUTE_HOME,
    UI_ROUTE_CONFIG,
    UI_ROUTE_READINESS,
    UI_ROUTE_RPC,
    UI_ROUTE_LAYOUT,
    UI_ROUTE_XREPO_SUMMARY,
    UI_ROUTE_XREPO_LINKS,
    UI_ROUTE_TREE,
    UI_ROUTE_SYMBOL_HISTORY,
    UI_ROUTE_WHO,
    UI_ROUTE_SYMBOL,
    UI_ROUTE_METRICS,
    UI_ROUTE_WHY,
    UI_ROUTE_HANDOUT,
    UI_ROUTE_BLAST,
    UI_ROUTE_BRIDGES,
    UI_ROUTE_SCENT,
    UI_ROUTE_TRACE,
    UI_ROUTE_REPOSITORY_MAP,
    UI_ROUTE_IMPACT_ANALYSIS,
    UI_ROUTE_IMPACT,
    UI_ROUTE_FLOWS,
    UI_ROUTE_FLOW,
    UI_ROUTE_REPO_INFO,
    UI_ROUTE_INDEX,
    UI_ROUTE_INDEX_STATUS,
    UI_ROUTE_UI_CONFIG,
    UI_ROUTE_DELETE_PROJECT,
    UI_ROUTE_BROWSE,
    UI_ROUTE_ADR_GET,
    UI_ROUTE_ADR_SAVE,
    UI_ROUTE_PROJECT_HEALTH,
    UI_ROUTE_PROCESSES,
    UI_ROUTE_AGENT_EVENTS,
    UI_ROUTE_LOGS,
    UI_ROUTE_UI_LOG_POST,
    UI_ROUTE_UI_LOG_GET,
    UI_ROUTE_NOT_FOUND,
} cbm_ui_route_t;

static inline cbm_ui_route_t cbm_ui_route_select(const char *method, const char *path,
                                                 bool read_only) {
    /* Prefix matching preserves the daemon's existing aliases. Standalone
     * allows only exact paths. More specific overlapping entries come first. */
    static const struct {
        const char *method;
        const char *path;
        cbm_ui_route_t route;
        bool standalone;
        bool daemon_prefix;
    } routes[] = {
        {"GET", "/api/config", UI_ROUTE_CONFIG, false, false},
        {"POST", "/api/config", UI_ROUTE_CONFIG, false, false},
        {"GET", "/__cbm/ui-readiness", UI_ROUTE_READINESS, false, false},
        {"POST", "/rpc", UI_ROUTE_RPC, true, false},
        {"GET", "/api/layout", UI_ROUTE_LAYOUT, true, true},
        {"GET", "/api/xrepo/summary", UI_ROUTE_XREPO_SUMMARY, true, false},
        {"GET", "/api/xrepo/links", UI_ROUTE_XREPO_LINKS, true, false},
        {"GET", "/api/tree", UI_ROUTE_TREE, true, true},
        {"GET", "/api/symbol-history", UI_ROUTE_SYMBOL_HISTORY, false, true},
        {"GET", "/api/who", UI_ROUTE_WHO, false, true},
        {"GET", "/api/symbol", UI_ROUTE_SYMBOL, false, true},
        {"GET", "/api/metrics", UI_ROUTE_METRICS, false, true},
        {"GET", "/api/why", UI_ROUTE_WHY, false, true},
        {"GET", "/api/handout", UI_ROUTE_HANDOUT, false, true},
        {"GET", "/api/blast", UI_ROUTE_BLAST, false, true},
        {"GET", "/api/bridges", UI_ROUTE_BRIDGES, false, true},
        {"GET", "/api/scent", UI_ROUTE_SCENT, false, true},
        {"GET", "/api/trace", UI_ROUTE_TRACE, true, true},
        {"GET", "/api/repository-map", UI_ROUTE_REPOSITORY_MAP, true, false},
        {"GET", "/api/impact-analysis", UI_ROUTE_IMPACT_ANALYSIS, true, false},
        {"GET", "/api/impact", UI_ROUTE_IMPACT, false, true},
        {"GET", "/api/flows", UI_ROUTE_FLOWS, true, true},
        {"GET", "/api/flow", UI_ROUTE_FLOW, true, false},
        {"GET", "/api/repo-info", UI_ROUTE_REPO_INFO, true, true},
        {"POST", "/api/index", UI_ROUTE_INDEX, false, false},
        {"GET", "/api/index-status", UI_ROUTE_INDEX_STATUS, true, false},
        {"GET", "/api/ui-config", UI_ROUTE_UI_CONFIG, false, false},
        {"DELETE", "/api/project", UI_ROUTE_DELETE_PROJECT, false, true},
        {"GET", "/api/browse", UI_ROUTE_BROWSE, false, true},
        {"GET", "/api/adr", UI_ROUTE_ADR_GET, true, true},
        {"POST", "/api/adr", UI_ROUTE_ADR_SAVE, false, false},
        {"GET", "/api/project-health", UI_ROUTE_PROJECT_HEALTH, true, true},
        {"GET", "/api/processes", UI_ROUTE_PROCESSES, false, false},
        {"GET", "/api/agent-events", UI_ROUTE_AGENT_EVENTS, false, false},
        {"POST", "/api/agent-events", UI_ROUTE_AGENT_EVENTS, false, false},
        {"GET", "/api/logs", UI_ROUTE_LOGS, false, true},
        {"POST", "/api/ui-log", UI_ROUTE_UI_LOG_POST, false, false},
        {"GET", "/api/ui-log", UI_ROUTE_UI_LOG_GET, false, true},
    };
    if (strcmp(method, "OPTIONS") == 0)
        return UI_ROUTE_PREFLIGHT;

    for (size_t i = 0; i < sizeof(routes) / sizeof(routes[0]); i++) {
        if ((read_only && !routes[i].standalone) || strcmp(method, routes[i].method) != 0)
            continue;
        const char *pattern = routes[i].path;
        const bool matches = !read_only && routes[i].daemon_prefix
                                 ? strncmp(path, pattern, strlen(pattern)) == 0
                                 : strcmp(path, pattern) == 0;
        if (matches)
            return routes[i].route;
    }

    if (read_only && strcmp(method, "GET") != 0)
        return UI_ROUTE_NOT_FOUND;
    return strcmp(path, "/") == 0 ? UI_ROUTE_HOME : UI_ROUTE_ASSET;
}

#endif
