# Mori GraphQL Parity

Use the parity harness to compare the C and Rust GraphQL implementations in
separate caches and daemon runtimes. Choose a new, short scratch path for
each run. From the repository's `mori/` directory, run

```sh
just parity --bin-a "$c_reference" --bin-b "$mori_candidate" \
  --graphql-a c --graphql-b rust --work /tmp/mori-graphql-parity/work \
  --rt-prefix /tmp/mori-graphql-parity/rt
```

The two mode flags also work with the `parity-ci` recipe for generated
fixtures. The harness records the requested modes in `report.json` under
`graphql_modes_requested`. The flags set `MORI_IMPL_GRAPHQL` for those sides
and set `CBM_PROFILE=1` to retain their native index-worker logs. A mode set
only in the parent shell does not reach either subprocess.

On a repository with GraphQL server Routes, inspect the matching index
worker log at `<work>/b/cache/logs/.worker-log-*` for the
`msg=pass.route_nodes.graphql` event with `planner=rust` and a positive routes
count. This establishes that the Rust server Route planner ran. If the
reference build emits the same diagnostic, check side A for `planner=c`.
The generated parity fixtures have no GraphQL server Routes. To confirm
planner execution, use a native GraphQL fixture or select a corpus repo
with server Routes. The request in the parity report records the settings
only. The Route marker covers the server planner. Establish execution of
client extraction or cross-repo link stages from their own evidence.
