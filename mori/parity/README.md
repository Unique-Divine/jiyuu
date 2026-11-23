# Mori GraphQL Parity

Use the parity harness to compare a published C release and the `mori`
candidate in separate caches and daemon runtimes. Choose a new, short
scratch path for each run. From the repository's `mori/` directory, run

```sh
just parity --bin-a "$c_reference" --bin-b "$mori_candidate" \
  --profile --work /tmp/mori-graphql-parity/work \
  --rt-prefix /tmp/mori-graphql-parity/rt
```

Use `--graphql-a c` or `--graphql-b rust` when replaying an older linked
build that still supports the `MORI_IMPL_GRAPHQL` selector. The flags also
retain native worker logs for their respective sides and record the requests
under `graphql_modes_requested` in `report.json`. The runner supplies these
settings to each subprocess explicitly.

On a repository with GraphQL server Routes, inspect the matching index
worker log at `<work>/b/cache/logs/.worker-log-*` for the
`msg=pass.route_nodes.graphql` event with `planner=rust`, `writer=rust`, and a
positive Route count. If the reference build emits the same diagnostic,
check side A for `planner=c`.

The polyglot fixture includes the GraphQL Route `Query.fixtureItem` and its
gqlgen resolver. The release workflow checks the worker marker and the
published `HANDLES` edge. Verify client and cross-repo links with fixtures
that exercise those stages.


## Worker Path Parity

Compare one worker with the default worker count using the same Mori
binary on both sides.

```sh
just parity-ci --path-parity --bin-a "$mori" --bin-b "$mori"
```

The fixture exceeds the 50-file threshold and includes cross-file Go calls
and a GraphQL Route. On a host with at least two default workers, the run
requires the sequential and parallel path markers in the worker logs. It
compares the normalized graph with the usual allow-lists. Use
`just parity --path-parity` with the same binary flags to compare a pinned
corpus. The report records each repository's observed worker path and file
count per side. A corpus may include small repositories that take the
sequential path on both sides. It must also contain at least one
repository where side B takes the parallel path. Both commands skip
tool-call replay.

## Verify Rust Execution for Selected Cypher Calls

Tool-call replay compares each saved request's result across both builds.
Mori can return the correct result through C fallback with the Rust
query implementation enabled. To also require Rust execution on side B,
pass `--require-rust-query CALL_ID` for each saved `query_graph` request
that the Rust build must handle. Use the stable `call:...` ID from an L4
report or compute it with `calls.call_id(tool, args)` on the saved request
before resolving its placeholders.

This flag enables the Rust query implementation and debug logging in the
side B replay daemon. The runner looks for exactly one
`query_graph.evaluator` debug event with `backend=rust` appended during
each selected request, including any extra pages the comparison needs.
Missing or ambiguous evidence fails the run even when both replies match.
An output allow-list cannot waive this check. The dispatch diagnostic
records the call ID and event count.
