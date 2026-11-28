/* Standalone test: cc -std=c11 -Wall -Wextra -Werror http_routes.c -o /tmp/ui-routes */
#include "../http_routes.h"

#include <stdio.h>

int main(void) {
    static const struct {
        const char *method;
        const char *path;
        cbm_ui_route_t daemon;
        cbm_ui_route_t standalone;
    } cases[] = {
        {"GET", "/", UI_ROUTE_HOME, UI_ROUTE_HOME},
        {"POST", "/", UI_ROUTE_HOME, UI_ROUTE_NOT_FOUND},
        {"OPTIONS", "/api/index", UI_ROUTE_PREFLIGHT, UI_ROUTE_PREFLIGHT},
        {"POST", "/rpc", UI_ROUTE_RPC, UI_ROUTE_RPC},
        {"POST", "/rpc/", UI_ROUTE_ASSET, UI_ROUTE_NOT_FOUND},
        {"POST", "/api/index", UI_ROUTE_INDEX, UI_ROUTE_NOT_FOUND},
        {"GET", "/api/index-status", UI_ROUTE_INDEX_STATUS, UI_ROUTE_INDEX_STATUS},
        {"POST", "/api/adr", UI_ROUTE_ADR_SAVE, UI_ROUTE_NOT_FOUND},
        {"GET", "/api/adr", UI_ROUTE_ADR_GET, UI_ROUTE_ADR_GET},
        {"DELETE", "/api/project-health", UI_ROUTE_DELETE_PROJECT, UI_ROUTE_NOT_FOUND},
        {"GET", "/api/project-health", UI_ROUTE_PROJECT_HEALTH, UI_ROUTE_PROJECT_HEALTH},
        {"GET", "/api/config", UI_ROUTE_CONFIG, UI_ROUTE_ASSET},
        {"POST", "/api/config", UI_ROUTE_CONFIG, UI_ROUTE_NOT_FOUND},
        {"GET", "/api/ui-config", UI_ROUTE_UI_CONFIG, UI_ROUTE_ASSET},
        {"GET", "/api/logs", UI_ROUTE_LOGS, UI_ROUTE_ASSET},
        {"POST", "/api/ui-log", UI_ROUTE_UI_LOG_POST, UI_ROUTE_NOT_FOUND},
        {"POST", "/api/agent-events", UI_ROUTE_AGENT_EVENTS, UI_ROUTE_NOT_FOUND},
        {"GET", "/__cbm/ui-readiness", UI_ROUTE_READINESS, UI_ROUTE_ASSET},
        {"GET", "/api/impact-analysis", UI_ROUTE_IMPACT_ANALYSIS, UI_ROUTE_IMPACT_ANALYSIS},
        {"GET", "/api/impact-analysis/", UI_ROUTE_IMPACT, UI_ROUTE_ASSET},
        {"GET", "/api/symbol-history/old", UI_ROUTE_SYMBOL_HISTORY, UI_ROUTE_ASSET},
        {"GET", "/api/tree/legacy", UI_ROUTE_TREE, UI_ROUTE_ASSET},
        {"GET", "/api%2Ftree", UI_ROUTE_ASSET, UI_ROUTE_ASSET},
        {"GET", "/api/xrepo/summary", UI_ROUTE_XREPO_SUMMARY, UI_ROUTE_XREPO_SUMMARY},
        {"GET", "/api/xrepo/links", UI_ROUTE_XREPO_LINKS, UI_ROUTE_XREPO_LINKS},
        {"GET", "/api/xrepo/summary/", UI_ROUTE_ASSET, UI_ROUTE_ASSET},
        {"GET", "/api/xrepo/links/legacy", UI_ROUTE_ASSET, UI_ROUTE_ASSET},
        {"HEAD", "/api/xrepo/summary", UI_ROUTE_ASSET, UI_ROUTE_NOT_FOUND},
        {"POST", "/api/xrepo/links", UI_ROUTE_ASSET, UI_ROUTE_NOT_FOUND},
        {"DELETE", "/api/xrepo/links", UI_ROUTE_ASSET, UI_ROUTE_NOT_FOUND},
        {"OPTIONS", "/api/xrepo/links", UI_ROUTE_PREFLIGHT, UI_ROUTE_PREFLIGHT},
        {"GET", "/api/tree", UI_ROUTE_TREE, UI_ROUTE_TREE},
        {"GET", "/api/flow", UI_ROUTE_FLOW, UI_ROUTE_FLOW},
        {"GET", "/api/flows", UI_ROUTE_FLOWS, UI_ROUTE_FLOWS},
        {"GET", "/api/flows/legacy", UI_ROUTE_FLOWS, UI_ROUTE_ASSET},
        {"GET", "/missing", UI_ROUTE_ASSET, UI_ROUTE_ASSET},
        {"HEAD", "/assets/graph.js", UI_ROUTE_ASSET, UI_ROUTE_NOT_FOUND},
    };

    for (size_t i = 0; i < sizeof(cases) / sizeof(cases[0]); i++) {
        if (cbm_ui_route_select(cases[i].method, cases[i].path, false) != cases[i].daemon ||
            cbm_ui_route_select(cases[i].method, cases[i].path, true) != cases[i].standalone) {
            fprintf(stderr, "route mismatch for %s %s\n", cases[i].method, cases[i].path);
            return 1;
        }
    }
    printf("%zu UI route cases passed in both modes\n", sizeof(cases) / sizeof(cases[0]));
    return 0;
}
