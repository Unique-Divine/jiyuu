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
positive routes count. If the reference build emits the same diagnostic,
check side A for `planner=c`.
The generated parity fixtures have no GraphQL server Routes. To confirm
planner execution, use a native GraphQL fixture or select a corpus repo
with server Routes. The request in the parity report records the settings
only. The Route marker covers the server planner. Establish execution of
client extraction or cross-repo link stages from their own evidence.
