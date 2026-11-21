# Working on Mori Itself

Mori's source is `mori/` in the Jiyuu repository. The Rust binary links the
C graph engine. The graph UI is written in TypeScript. Run `just grammars`
in `mori/` to fetch the pinned Tree-sitter grammars. `mori/README.md` covers
the build, tests, and releases. https://github.com/Unique-Divine/jiyuu

## Understand It With Itself

Index the checkout as its own project, excluding the grammars:

```sh
cd ~/jiyuu/mori   # your Jiyuu checkout
echo "/internal/cbm/vendored/" >> .cbmignore
echo /mori/.cbmignore >> "$(git rev-parse --git-path info/exclude)"
mori cli --quiet index_repository --repo_path "$PWD"
```

Then ask the graph: "who calls `find_route_handler`", "trace
`emit_graphql_edge`".

## Where GraphQL Support Lives

| File | Role |
| --- | --- |
| `src/discover/language.c` | File extensions per language (`.graphqls`) |
| `internal/cbm/extract_defs.c` | Schema types, fields, return types, `@deprecated` |
| `internal/cbm/extract_calls.c` | Operation text from call arguments and constants |
| `internal/cbm/service_patterns.c` | Client library names; operation parser and field paths |
| `src/pipeline/pass_route_nodes.c` | Server Route discovery and C graph adapter |
| `jiyuu-mori/src/graphql_ffi.rs`, `gqlgen.rs`, `route_writer.rs` | GraphQL Route planning, resolver selection, and metadata |
| `src/pipeline/pass_parallel.c`, `pass_calls.c` | Client `GRAPHQL_CALLS` edges (large and small repos) |
| `jiyuu-mori/src/graphql_link.rs` | Match GraphQL operations to indexed providers |
| `src/pipeline/pass_cross_repo.c` | Cross-repo persistence and C graph adapter |
| `src/pipeline/pipeline_incremental.c` | When an edit forces a full rebuild |

## Build and Test

Run these from the Jiyuu root:

```sh
just --justfile mori/justfile grammars
just mori-rs-build                    # target/release/mori
just mori-rs-check-linked             # Rust tests with the C engine linked
just --justfile mori/justfile test-graphql-rust
```

- GraphQL client edges in tests need a fixture of more than 50 files to
  exercise the parallel path, and a smaller one for the sequential path.
- Test a dev binary against real repos with a separate cache, so the live
  index and daemon are untouched:
  `CBM_CACHE_DIR=/tmp/mori-dev CBM_RUNTIME_DIR=/tmp/mori-dev-rt target/release/mori cli ...`
