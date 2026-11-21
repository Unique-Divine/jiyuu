# Working on Mori Itself

Mori's source is `mori/` in https://github.com/Unique-Divine/jiyuu. It is C
(about half a million lines of its own, plus tests and a TypeScript UI). The
Tree-sitter grammars under `internal/cbm/vendored/grammars/` are generated,
not committed: `just grammars` in `mori/` fetches a pinned archive, and
nobody edits them. `mori/README.md` covers the build, tests, and releases.

## Understand It With Itself

Index the checkout as its own project, excluding the grammars:

```sh
cd ~/jiyuu/mori   # your Jiyuu checkout
echo "/internal/cbm/vendored/" >> .cbmignore
echo /mori/.cbmignore >> "$(git rev-parse --git-path info/exclude)"
codebase-memory-mcp cli --quiet index_repository --repo_path "$PWD"
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
| `src/pipeline/pass_route_nodes.c` | Server field Routes and gqlgen `HANDLES` |
| `src/pipeline/pass_parallel.c`, `pass_calls.c` | Client `GRAPHQL_CALLS` edges (large and small repos) |
| `src/pipeline/pass_cross_repo.c` | Cross-repo matching (`match_typed_routes`, `find_route_handler`) |
| `src/pipeline/pipeline_incremental.c` | When an edit forces a full rebuild |

## Build and Test

Run these in `mori/`:

```sh
just build     # build/c/codebase-memory-mcp, after fetching the grammars
just test      # the grammar and extraction suites
just test-all  # every suite, about 10,500 tests with sanitizers (over an hour)
scripts/lint.sh
```

- GraphQL client edges in tests need a fixture of more than 50 files to
  exercise the parallel path, and a smaller one for the sequential path.
- Test a dev binary against real repos with a separate cache, so the live
  index and daemon are untouched:
  `CBM_CACHE_DIR=/tmp/cbm-dev CBM_RUNTIME_DIR=/tmp/cbm-dev-rt build/c/codebase-memory-mcp cli ...`
