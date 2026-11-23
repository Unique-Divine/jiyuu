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
corpus. Both commands skip tool-call replay.
