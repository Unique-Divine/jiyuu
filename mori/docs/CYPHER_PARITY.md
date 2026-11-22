# Cypher Port Acceptance

The first Rust evaluator runs in the linked test suite against the same SQLite
cache as the native executable. The MCP tool `query_graph` continues to run
the C evaluator and response encoder.

## Query Forms

| Source | Query forms | Acceptance fixture |
| --- | --- | --- |
| README, Cypher Subset and GraphQL Across Repos | Single directed typed hop, node label and inline literal property, conjunctive `WHERE` equality, identifier or property projection, `LIMIT`. The resolver query uses two directed hops and returns edge property `x.callers`. | `graphql_relink.rs` checks resolver and caller identities, then compares materialized columns and rows against Rust. |
| Graph UI `provider/cypher.ts` | `importEdges`, `classCallTargets`, `raises`, `throwsRelation`, `envReads`, `typeRefs` use a directed typed hop and literal filters/projections. | `graphql_relink.rs` checks forward and reverse traversal, terminal seeding, row ordering, duplicate projections, and zero limits. |
| Graph UI `provider/cypher.ts` | `callsOut` and `callsIn` order by an edge property. `enclosingByLabel` also compares numeric line bounds. File and hotspot reads can be node-only. Batch builders can emit OR-chained equality. | Keep on C. Builder tests and `src/test-support/rpc-recordings.ts` cover the query shapes and UI results. |
| Public and CI replays in `mori/parity/calls-*.jsonl` | Node-only reads, `ORDER BY`, `count`, `type`, aliases, untyped relationships, invalid syntax, and MCP paging. The broader agent replay inventory also records `WITH`, `STARTS WITH`, `labels`, `NOT`, and `CONTAINS`. | Keep on C. Use the replay corpus and `mori/tests/test_cypher.c` when extending the evaluator. |
| Remaining README clauses and patterns | `OPTIONAL MATCH`, repeated or comma-separated patterns, variable lengths, undirected hops, alternation, `WITH`, `DISTINCT`, `ORDER BY`, `SKIP`, `UNWIND`, `UNION`, `CASE`, functions, aggregation, and non-equality predicates. | Keep on C. Extend native and linked tests at each change in scope. |

## Boundaries

The Rust evaluator classifies the entire query before opening storage.
The first slice accepts plain quoted strings, at most one anonymous node,
and explicit limits up to 100,000. Escape sequences, inline regex-like
patterns, repeated aliases, and virtual degree properties stay on C.
Supported execution uses the provider's stored adjacency, preserves the
native scan and expansion order, and preserves projection duplicates.
A supported query that fails to read the cache returns a storage error.
Pagination, cursor digests, output budgets, tree rendering, and the
`missed` graph remain MCP work outside this evaluator.

The native engine caps derived property cells at 511 bytes. The resolver test
checks structured caller identities below that cap. Its long-map case checks
the existing clipped value. Any migration to full caller maps needs an
explicit parity decision and a way to page or render large values.
