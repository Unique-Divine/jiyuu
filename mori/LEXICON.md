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
Mori's own C code in this directory, released as `v0.13.x`.
_Avoid_: the fork

**Rust build**:
The `mori` binary, in which Rust takes over components one at a time and the
C is linked inside until the last phase.
_See_: [jiyuu-mori](jiyuu-mori): the crate

**Reference**:
The build whose output counts as correct in a comparison. Today that is the C
build.

### Index Runs

**Index run**:
One execution of the indexer on one repository.
_Avoid_: reindex, build

**Index depth**:
How much an index run computes: `full`, `moderate`, or `fast`.
_Avoid_: mode, a bare "full"

**Update path**:
How an index run reaches a current graph: no-op, closure repair, or full
rebuild.
_Avoid_: route, run kind

**No-op**:
The update path taken when nothing changed since the last index run; the graph
is left as is.

**Closure repair**:
The update path that re-reads only the changed files and the files that depend
on them, then patches the graph.
_Avoid_: incremental build, partial index

**Closure**:
The changed files plus the files that depend on them.

**Full rebuild**:
The update path that re-reads every file and replaces the graph.
_Avoid_: a bare "full", full index

**Incremental indexing**:
The indexer's habit of taking the closure repair path whenever it can.

**Decline**:
Closure repair refusing a set of changes it cannot patch safely, which turns
the index run into a full rebuild.
_Avoid_: fall back, bail out

**Decline rule**:
One condition that causes a decline, such as a new file or an added definition
name.
_Avoid_: the incremental rule

**GraphQL decline rule**:
The decline rule for GraphQL: a change to a schema file, a gqlgen config, or a
resolver file declines.

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
A graph node for an entry point that code in another place can call, such as
an HTTP path or a GraphQL field. Written with a capital.

**Field path**:
A GraphQL root type and field names, such as `Query.perp.trades`; the key a
client and a server share.

**Ambiguous Route**:
A field path that two servers in one project both provide, so a link cannot
name one handler.

### Resolve Paths

**Sequential path**:
How the indexer resolves calls in a repository of 50 files or fewer.

**Parallel path**:
How the indexer resolves calls in a larger repository, with worker threads.

**Path parity**:
Both resolve paths producing the same graph for the same input.

**Worker-count stability**:
The graph not depending on the number of worker threads.

### Linking Across Repositories

**Link run**:
One execution of the cross-repository pass. It is separate from an index run
and reads no source files.
_Avoid_: relink, cross-repo index

**Client project**:
The project whose code makes a call.

**Provider project**:
The project whose code answers a call.
_Avoid_: server project

**Client-side run**:
A link run started from the client project, naming its provider projects as
targets. The normal way to link.

**Provider-side run**:
A link run started from the provider project. It maintains only links that
already name that provider.
_Avoid_: server-side run

**Forward link**:
The edge stored in the client project that points at a provider's Route.

**Caller record**:
The entry stored in the provider project that lists who calls a Route.
_Avoid_: reverse edge

### Parity and the Port

**Port**:
Reimplementing a component in Rust with the same behavior, including its
update paths and decline rules.
_Avoid_: rewrite

**Improvement**:
A deliberate change in behavior, made after parity holds and named as one.

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
- "the fork": say **upstream**, the **C build**, or the **Rust build**.
