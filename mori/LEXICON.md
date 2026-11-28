# Lexicon: Mori

Mori indexes source repositories into a code graph that agents query, and
links code across repositories. It began as Codebase Memory MCP's C and is
moving to Rust.

- Parent: [Jiyuu](../LEXICON.md)

## Builds

### Upstream

Codebase Memory MCP by DeusData, the project Mori came from.
_See_: [IMPORT.md](IMPORT.md): the imported commit and what was replayed on it

### C build

The program built from Mori's C code in this directory.

_See_ C entry point `src/main.c::main` and build target `cbm` in
[Makefile.cbm](Makefile.cbm).

### Rust build

The `mori` binary built from crate `jiyuu-mori`.

_See_ Rust entry point `src/main.rs::main`.

### Reference

The build whose output counts as correct in a comparison.

_See_ [Mori GraphQL Parity](parity/README.md), which passes the reference executable as flag `--bin-a`.

### Graph UI

The browser app the daemon serves from a binary with the UI embedded, such
as a release `mori`, with workspaces such as Explore, Galaxy, and Repos.
_Avoid_: Lens, CBM Lens (upstream's name)

_See_ React component `graph-ui/src/App.tsx::App`.

## Index Runs

### Index run

One execution of the indexer on one repository.
_Avoid_: reindex, build

_See_ function `src/pipeline/pipeline.c::cbm_pipeline_run`.

### Index depth

How much an index run discovers and computes: `full`, `moderate`, or `fast`.
_Avoid_: a bare "full"

_See_ C enum `src/pipeline/pipeline.h::cbm_index_mode_t` and the public parameter `mode` in [Index and Query](README.md#index-and-query).

### Update path

The way an index run reaches a current graph: no-op, closure repair, or full
rebuild.
_Avoid_: route, run kind

_See_ function `src/pipeline/pipeline.c::route_existing_stage` and its opt-in
Rust counterpart `src/mcp/index_route.rs::route_existing`.

### No-op

The update path that leaves the graph as it is, because the stored generation
is current and nothing the index depends on changed.

_See_ function `src/pipeline/pipeline.c::finish_live_noop`.

### Closure repair

The update path that re-reads a chosen subset of files and patches the graph.
_Avoid_: incremental build, partial index

_See_ function `src/pipeline/pipeline_incremental.c::run_closure_delta`.

### Closure

The set of files a closure repair selects for reparsing.

_See_ C struct `src/pipeline/pipeline_incremental.c::closure_plan_t`, whose field `files` holds the selected files.

### Full rebuild

The update path that re-reads every file in the index's scope and replaces
the graph.
_Avoid_: a bare "full", full index

_See_ function `src/pipeline/pipeline.c::run_full_build`.

### Incremental indexing

The policy of reusing the stored graph: a no-op when nothing relevant changed,
closure repair when the change can be patched.

_See_ function `src/pipeline/pipeline_incremental.c::cbm_pipeline_run_incremental`.

### Decline

Closure repair refusing a set of changes, because it cannot patch them safely
or a full rebuild would cost less. A decline normally turns the index run into
a full rebuild.
_Avoid_: fall back, bail out

_See_ function `src/pipeline/pipeline_incremental.c::closure_try_plan`.

### Decline rule

One condition that causes a decline.
_Avoid_: the incremental rule

_See_ function `src/pipeline/pipeline_incremental.c::closure_try_plan`, which evaluates the rules when planning a repair.

### GraphQL decline rule

The decline rule for changes that can alter GraphQL field Routes or their
handlers.

_See_ function `src/pipeline/pipeline_incremental.c::closure_graphql_decline`.

### Generation

One published version of a project's graph.

_See_ C struct `src/pipeline/pipeline_internal.h::cbm_pipeline_generation_t`, which describes the graph and metadata to publish.

### Publish

Replacing the previous generation with a new one in a single step.

_See_ function `src/pipeline/pipeline.c::cbm_pipeline_publish_generation`.

## Facts in the Graph

### File-local fact

A node or edge that depends on one file, such as a function.

_See_ function `src/pipeline/pipeline_delta.c::cbm_delta_purge`, which removes graph facts owned by files selected for repair.

### Derived fact

A node or edge that depends on several files together, such as a GraphQL field
Route and its handler.
_Avoid_: cross-file fact

_See_ function `src/pipeline/pipeline_delta.c::delta_edge_type_is_recomputed` for examples of edges rebuilt by the repair.

### Route

A graph node for an entry point that other code can call, such as an HTTP path
or a GraphQL field. Written with a capital.

_See_ Rust function `src/pipeline/graphql_ffi.rs::write_graphql_route` for GraphQL Routes.

### Field path

A GraphQL operation kind (`Query`, `Mutation`, or `Subscription`) followed by
field names, such as `Query.perp.trades`. A client and a server share it as
the key of a link.

_See_ function `internal/cbm/service_patterns.c::cbm_service_pattern_graphql_sig_parts`, which reads the operation kind and paths from a signature.

### Ambiguous Route

A Route with more than one candidate handler and no single winner.

_See_ Rust function `src/pipeline/route_writer.rs::route_properties`, which sets the route property `ambiguous`.

## Resolve Paths

### Sequential path

The indexer resolving calls with one worker.

_See_ function `src/pipeline/pipeline.c::run_sequential_pipeline`.

### Parallel path

The indexer resolving calls with several worker threads, used for large index
runs.

_See_ function `src/pipeline/pipeline.c::run_parallel_pipeline`.

### Path parity

Both resolve paths producing the same graph for the same input.

_See_ Python function `parity/parity.py::verify_worker_paths`, which checks that a parity run exercised the intended paths.

## Linking Across Repositories

### Link run

One execution of the cross-repository pass. It is separate from an index run
and reads no source files.
_Avoid_: relink, cross-repo index

_See_ function `src/pipeline/pass_cross_repo.c::cbm_cross_repo_match_cancellable`.

### Client project

The project whose code uses another project's entry point, by calling it or
sending to it.

_See_ parameter `src_project` of function `src/pipeline/pass_cross_repo.c::match_typed_routes`.

### Provider project

The project whose code provides the entry point a client project uses.
_Avoid_: server project

_See_ parameter `tgt_project` of function `src/pipeline/pass_cross_repo.c::match_typed_routes`.

### Client-side run

A link run started from the client project, naming its provider projects as
targets.

_See_ function `src/pipeline/pass_cross_repo.c::cr_match_non_http_outgoing` for matching a client's outgoing typed calls.

### Provider-side run

A link run started from the provider project.
_Avoid_: server-side run

_See_ function `src/pipeline/pass_cross_repo.c::cr_match_project_pair` for direction-specific matching.

### Forward link

The edge stored in the client project that records its use of a provider
project's entry point.

_See_ function `src/pipeline/pass_cross_repo.c::emit_cross_route_bidirectional`, which writes the client edge and the provider's caller record.

### Caller record

The entry stored in the provider project that lists whose code uses one of its
entry points.
_Avoid_: reverse edge

_See_ function `src/pipeline/pass_cross_repo.c::cr_reconcile_reverse`, which reconciles records in the provider graph.

## Parity and the Port

### Port

Reimplementing a component in Rust with the same behavior, including its
update paths and decline rules.
_Avoid_: rewrite

### Behavior parity

Two builds producing the same graph and the same tool output for the same
repositories. "Parity" alone means this.

_See_ Python function `parity/parity.py::compare_repo` for graph comparisons and function `parity/parity.py::replay` for tool output.

### Update-path parity

Two builds taking the same update path for the same change and ending with the
same graph.

### Noise floor

What differs between two index runs of the same build on the same input.

_See_ [Mori GraphQL Parity](parity/README.md) for running comparisons with separate caches and runtimes.

### Volatile key

A field in the noise floor, such as an elapsed duration, that comparisons ignore.

_See_ Python class `parity/calls.py::OutputNormalizer`, which removes elapsed-time fields from tool output before comparison.

### Allow-list

The reviewed list of accepted differences between two builds, each with a
reason.

_See_ Python class `parity/allowlist.py::Entry`, which represents one accepted difference.

### Fixture case

One small repository with steps and the graph facts expected after them.

_See_ Python function `parity/fixtures.py::write_all`, which creates the parity fixture repositories.

### Acceptance case

A known wrong behavior of the C build that the Rust build should fix, with its
failing input.

## Never Say

- "recompute": it reads as re-reading every file. For the narrow idea of
  rebuilding derived facts after an index run, say "rebuild derived facts".
- "route" for a choice between ways of running: say **update path**. **Route**
  is a graph node.
- "the fork": say **upstream**, the **C build**, or the **Rust build**,
  whichever is meant.
