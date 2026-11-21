# Lexicon: Mori

Mori indexes source repositories into a code graph that agents query, and
links code across repositories. It began as Codebase Memory MCP's C and is
moving to Rust.

- Parent: [Jiyuu](../LEXICON.md)

## Terms

### Builds

**Upstream**:
Codebase Memory MCP by DeusData, the project Mori came from.
_See_: [IMPORT.md](IMPORT.md): the imported commit and what was replayed on it

**C build**:
The program built from Mori's C code in this directory.

**Rust build**:
The `mori` binary built from crate `jiyuu-mori`.
_See_: [jiyuu-mori](jiyuu-mori): the crate and how it relates to the C

**Reference**:
The build whose output counts as correct in a comparison.

**Graph UI**:
The browser app the daemon serves from a binary built with the UI embedded,
with workspaces such as Explore, Galaxy, and Repos.
_Avoid_: Lens, CBM Lens (upstream's name, still shown in the app)

### Index Runs

**Index run**:
One execution of the indexer on one repository.
_Avoid_: reindex, build

**Index depth**:
How much an index run discovers and computes: `full`, `moderate`, or `fast`.
_Avoid_: a bare "full"
_See_: [pipeline.h](src/pipeline/pipeline.h): the three depths.
[README.md](README.md#index-and-query): the public `mode` parameter, which
also accepts a link run

**Update path**:
The way an index run reaches a current graph: no-op, closure repair, or full
rebuild.
_Avoid_: route, run kind

**No-op**:
The update path that leaves the graph as it is, because the stored generation
is current and nothing the index depends on changed.

**Closure repair**:
The update path that re-reads a chosen subset of files and patches the graph.
_Avoid_: incremental build, partial index
_See_: [pipeline_incremental.c](src/pipeline/pipeline_incremental.c): how the
subset is chosen

**Closure**:
The set of files a closure repair selects for reparsing.

**Full rebuild**:
The update path that re-reads every file in the index's scope and replaces
the graph.
_Avoid_: a bare "full", full index

**Incremental indexing**:
The policy of reusing the stored graph: a no-op when nothing relevant changed,
closure repair when the change can be patched.

**Decline**:
Closure repair refusing a set of changes, because it cannot patch them safely
or a full rebuild would cost less. A decline normally turns the index run into
a full rebuild.
_Avoid_: fall back, bail out

**Decline rule**:
One condition that causes a decline.
_Avoid_: the incremental rule

**GraphQL decline rule**:
The decline rule for changes that can alter GraphQL field Routes or their
handlers.

**Generation**:
One published version of a project's graph.

**Publish**:
Replacing the previous generation with a new one in a single step.

### Facts in the Graph

**File-local fact**:
A node or edge that depends on one file, such as a function.

**Derived fact**:
A node or edge that depends on several files together, such as a GraphQL field
Route and its handler.
_Avoid_: cross-file fact

**Route**:
A graph node for an entry point that other code can call, such as an HTTP path
or a GraphQL field. Written with a capital.

**Field path**:
A GraphQL operation kind (`Query`, `Mutation`, or `Subscription`) followed by
field names, such as `Query.perp.trades`. A client and a server share it as
the key of a link.

**Ambiguous Route**:
A Route with more than one candidate handler and no single winner.

### Resolve Paths

**Sequential path**:
The indexer resolving calls with one worker.

**Parallel path**:
The indexer resolving calls with several worker threads, used for large index
runs.
_See_: [pipeline.c](src/pipeline/pipeline.c): when each path is chosen

**Path parity**:
Both resolve paths producing the same graph for the same input.

### Linking Across Repositories

**Link run**:
One execution of the cross-repository pass. It is separate from an index run
and reads no source files.
_Avoid_: relink, cross-repo index

**Client project**:
The project whose code uses another project's entry point, by calling it or
sending to it.

**Provider project**:
The project whose code provides the entry point a client project uses.
_Avoid_: server project

**Client-side run**:
A link run started from the client project, naming its provider projects as
targets.

**Provider-side run**:
A link run started from the provider project.
_Avoid_: server-side run

**Forward link**:
The edge stored in the client project that records its use of a provider
project's entry point.

**Caller record**:
The entry stored in the provider project that lists whose code uses one of its
entry points.
_Avoid_: reverse edge

### Parity and the Port

**Port**:
Reimplementing a component in Rust with the same behavior, including its
update paths and decline rules.
_Avoid_: rewrite

**Behavior parity**:
Two builds producing the same graph and the same tool output for the same
repositories. "Parity" alone means this.

**Update-path parity**:
Two builds taking the same update path for the same change and ending with the
same graph.

**Noise floor**:
What differs between two index runs of the same build on the same input.

**Volatile key**:
A field in the noise floor, such as a timestamp, that comparisons ignore.

**Allow-list**:
The reviewed list of accepted differences between two builds, each with a
reason.

**Fixture case**:
One small repository with steps and the graph facts expected after them.

**Acceptance case**:
A known wrong behavior of the C build that the Rust build should fix, with its
failing input.

## Never Say

- "recompute": it reads as re-reading every file. For the narrow idea of
  rebuilding derived facts after an index run, say "rebuild derived facts".
- "route" for a choice between ways of running: say **update path**. **Route**
  is a graph node.
- "the fork": say **upstream**, the **C build**, or the **Rust build**,
  whichever is meant.
