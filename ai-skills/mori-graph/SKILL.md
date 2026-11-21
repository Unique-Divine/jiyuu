---
name: mori-graph
description: Code graph for cross-repo context. Use Mori, a local code graph and code memory served as MCP server mori-graph (Tree-sitter parsing, LSP-style call resolution), for structural and cross-repo questions; install it from Mori releases with mise, index repos, and link GraphQL clients to gqlgen resolvers across repos. Triggers on who calls this, what does X call, trace a call chain, impact analysis, which client uses this API field or resolver, unused or deprecated API fields, cross-repo links, dead code, graph query syntax, Cypher examples, index a repo, .cbmignore, reindex, cross-repo intelligence, installing or upgrading Mori, and working on the Mori source in jiyuu/mori.
---

# Mori Graph

Mori indexes repositories into a local graph (SQLite) and answers structural
questions through MCP tools: who calls a function, what a change affects,
which document section explains a symbol, and how services in different repos
call each other. It parses with Tree-sitter, resolves calls with typed,
LSP-style resolution where the language allows, and keeps the graph between
sessions as code memory. A graph query costs far fewer tokens than reading
files, but it's a lead, not proof: confirm material claims in source.

Mori lives in `mori/` of https://github.com/Unique-Divine/jiyuu and ships as
`mori/v*` releases. It carries GraphQL cross-repo linking (a GraphQL client
operation links to the gqlgen resolver that answers it). The MCP server key
is `mori-graph`, so the tools are `mcp__mori-graph__search_graph` and so on.
The binary is still named `codebase-memory-mcp` until the Rust `mori` binary
ships; commands below use that name. Read [setup.md](references/setup.md) to
install, register, and index; [graphql.md](references/graphql.md) for API
questions across repos; [tool-source.md](references/tool-source.md) to work
on Mori itself.

## Start of a Task

1. `list_projects`: pick the project whose `root_path` contains the working
   directory and pass its `name` as `project`, never a folder name.
2. `index_status` for that project. A stale or missing index answers wrong;
   reindex first (see [setup.md](references/setup.md)).
3. Choose an evidence tier (below), then query.

## Which Tool

| Question | Call |
| --- | --- |
| Who calls X? | `trace_path(direction="inbound")` |
| What does X call? | `trace_path(direction="outbound")` |
| Find a symbol | `search_graph(name_pattern=".*Name.*")`, then `get_code_snippet` |
| Exact text | `search_code` |
| Calls between services or repos | `query_graph` with Cypher on `HTTP_CALLS`, `GRAPHQL_CALLS`, `CROSS_*` |
| Effect of local changes | `detect_changes()` |
| Dead code | `search_graph(max_degree=0, exclude_entry_points=true)` |
| Repo overview | `get_architecture` |

`trace_path` needs exact names, so find them with `search_graph` first. Use
`direction="both"` when callers may live in another repo.

## Evidence Tiers

- **Scout:** a quick positive lookup. Results are provisional; never claim
  absence or completeness from it.
- **Verify (default):** targeted searches, the relevant trace directions,
  snippets for every material claim, all result pages.
- **Audit:** a bounded scope checked completely, both call directions, with
  remaining gaps stated.
- **Every tier:** call `check_index_coverage` with the evidence paths before
  relying on the graph. For a "nothing uses this" claim, also include the
  scope. Partial, skipped, or stale coverage means read the source.

## Results Are Paged

`search_graph` returns 50 per page and `query_graph` stops at an output
budget; both report `has_more`. Page until done before counting or claiming
absence. A short first page once looked like missing GraphQL links.

## Cypher Examples

```cypher
MATCH (a)-[r:HTTP_CALLS]->(b) RETURN a.name, b.name, r.url_path LIMIT 20
MATCH (a)-[r:GRAPHQL_CALLS]->(b) RETURN a.name, a.file_path, b.name LIMIT 50
MATCH (h)-[:HANDLES]->(r:Route) RETURN r.name, h.name LIMIT 20
```

`search_graph(relationship="HTTP_CALLS")` filters nodes by degree; to see the
edges themselves, use `query_graph`.

## Delegating

Query the graph in the parent first, then pass the child the project name,
index freshness, exact symbols and paths, findings, and open questions. A
child without the MCP tools works from that evidence and reads source; it
must not claim graph results it didn't get.

## Upgrades

Install and upgrade through mise and the `mori/v*` releases, as in
[setup.md](references/setup.md). Never run `codebase-memory-mcp install` or
`update`: they rewrite this skill, register the server under the old key,
and fetch a build without Mori's GraphQL linking.
