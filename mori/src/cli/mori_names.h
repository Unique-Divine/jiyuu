/*
 * mori_names.h — User-facing names of Mori.
 *
 * The product is Mori, its command is `mori`, and agents reach it through the
 * MCP server key `mori-graph` (tools `mcp__mori-graph__<tool>`). Setups made
 * before the rename register the server as `codebase-memory-mcp`; code that
 * recognizes tool names accepts both keys, new key first.
 *
 * Internal identifiers, CBM_* environment variables, cache paths, and database
 * formats keep their codebase-memory names.
 */
#ifndef CBM_MORI_NAMES_H
#define CBM_MORI_NAMES_H

#define CBM_PRODUCT_NAME "Mori"
#define CBM_COMMAND_NAME "mori"

#define CBM_MCP_SERVER_KEY "mori-graph"
#define CBM_MCP_SERVER_KEY_LEGACY "codebase-memory-mcp"

/* Claude Code tool-name prefixes: mcp__<server>__<tool>. */
#define CBM_MCP_TOOL_PREFIX "mcp__" CBM_MCP_SERVER_KEY "__"
#define CBM_MCP_TOOL_PREFIX_LEGACY "mcp__" CBM_MCP_SERVER_KEY_LEGACY "__"

/* Server keys that name Mori's graph tools, in match order. */
#define CBM_MCP_SERVER_KEYS \
    { CBM_MCP_SERVER_KEY, CBM_MCP_SERVER_KEY_LEGACY }
#define CBM_MCP_SERVER_KEY_COUNT 2U

/* Where releases live and how to upgrade. */
#define CBM_RELEASES_URL "https://github.com/Unique-Divine/jiyuu/releases?q=mori%2Fv"
#define CBM_UPGRADE_HINT                                                                       \
    "Mori installs and upgrades through mise and the mori/v* releases of\n"                    \
    "Unique-Divine/jiyuu. Bump the version in ~/.config/mise/config.toml:\n\n"                 \
    "  \"github:Unique-Divine/jiyuu\" = { version = \"<x.y.z>\", version_prefix = \"mori/v\" " \
    "}\n\n"                                                                                    \
    "then run:\n\n"                                                                            \
    "  mise install\n"                                                                         \
    "  " CBM_COMMAND_NAME " daemon stop   # the daemon restarts on the new binary\n\n"         \
    "Releases: " CBM_RELEASES_URL "\n"

#endif /* CBM_MORI_NAMES_H */
