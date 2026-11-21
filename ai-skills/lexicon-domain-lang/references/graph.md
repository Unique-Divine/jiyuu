# Link Lexicon Terms to Code

Use [Mori Graph](../../mori-graph/SKILL.md) when the task calls for navigation
between terminology and implementation. During a migration, use the
Codebase Memory MCP tools if those are the tools the session exposes.

## Write the Reference

Give the canonical term a Markdown heading. Mori indexes the heading as
a `Section` node. Put the definition and any aliases under that heading.
Bold text is searchable as text. A term heading is addressable by name in
graph search.

When a concrete symbol implements or stores the concept, put a code span
of the form `path/to/file.ext::Symbol` under _See_. Use a path relative to
the lexicon and spell the symbol as it appears in source.

```md
### Order

A customer's request to buy one or more items.

_See_ Rust struct `src/orders.rs::Order`.
```

Use the source reference to explain the mapping. For example, two domain
concepts can share a storage type. Keep their separate definitions and
reference the shared type where it helps a reader find that representation.

## Verify the Graph

1. Find the project by repository root with tool `list_projects`. Check
   the index and use tool `check_index_coverage` for the lexicon and source
   files. Let the watcher catch up or refresh the index as described in the
   Mori skill if it still serves old content.
2. Use tool `search_graph` with the lexicon's repository-relative path as
   `file_pattern`. Verify that the canonical heading is a `Section`.
3. Use tool `query_graph` to follow its `MENTIONS` edge. Check the target
   path, name, and label. A file-level result establishes a file reference.
   A result with label `Struct`, `Enum`, or another symbol label identifies
   a particular definition. Query the same edge by target symbol to find
   which definition documents the code.

```cypher
MATCH (term:Section)-[:MENTIONS]->(symbol)
WHERE term.file_path = 'shop/LEXICON.md' AND term.name = 'Order'
RETURN term.name, symbol.name, labels(symbol), symbol.file_path
```

Ordinary Markdown links can resolve to a file node even when their URL has
a heading fragment. Prefer explicit symbol references where the symbol
matters. Verify nested references such as enum variants individually.

The graph records the reference as `MENTIONS`. State kinds, aliases, and
other domain relationships in the definition. Queryable subtype or alias
edges require indexer support for those relationships.
