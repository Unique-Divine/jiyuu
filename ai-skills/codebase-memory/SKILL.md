---
name: codebase-memory
description: Use Codebase Memory MCP, a local code graph, for structural and cross-repo questions; set up and index repos; link GraphQL clients to gqlgen resolvers across repos; build and run the tool from a fork. Triggers on who calls this, what does X call, trace a call chain, impact analysis, which client uses this API field or resolver, unused or deprecated API fields, cross-repo links, dead code, graph query syntax, Cypher examples, index a repo, .cbmignore, reindex, cross-repo intelligence, and working on the codebase-memory-mcp source.
---

# Codebase Memory

Codebase Memory MCP indexes repositories into a local graph (SQLite) and
answers structural questions through MCP tools: who calls a function, what a
change affects, which document section explains a symbol, and how services in
different repos call each other. A graph query costs far fewer tokens than
reading files, but it's a lead, not proof: confirm material claims in source.

This setup runs the tool from a fork,
https://github.com/Unique-Divine/codebase-memory-mcp, which adds GraphQL
cross-repo linking (a GraphQL client operation links to the gqlgen resolver
that answers it). The upstream project is
https://github.com/DeusData/codebase-memory-mcp. Read
[setup.md](references/setup.md) to build, install, and index;
[graphql.md](references/graphql.md) for API questions across repos;
[tool-source.md](references/tool-source.md) to work on the tool itself.

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

Install and upgrade through the fork build in [setup.md](references/setup.md),
not `codebase-memory-mcp install` or `update`. The installer rewrites this
skill and installs the upstream release, which lacks the fork's GraphQL
linking.
