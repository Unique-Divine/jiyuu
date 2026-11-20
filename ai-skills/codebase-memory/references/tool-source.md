# Working on the Tool Itself

The tool is C (about half a million lines of its own, plus tests and a
TypeScript UI); almost all of its 40-million-line checkout is generated
Tree-sitter grammars under `internal/cbm/vendored/`, which nobody edits.

## Understand It With Itself

Index the checkout as its own project, excluding the grammars:

```sh
echo "/internal/cbm/vendored/" >> ~/codebase-memory-mcp/.cbmignore
echo /.cbmignore >> "$(git -C ~/codebase-memory-mcp rev-parse --git-path info/exclude)"
codebase-memory-mcp cli --quiet index_repository --repo_path ~/codebase-memory-mcp
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

```sh
scripts/test.sh --suites "edge_types_probe lang_contract cross_repo extraction"
scripts/test.sh            # full suite, about 10,500 tests with sanitizers
scripts/lint.sh
```

- GraphQL client edges in tests need a fixture of more than 50 files to
  exercise the parallel path, and a smaller one for the sequential path.
- Test a dev binary against real repos with a separate cache, so the live
  index and daemon are untouched:
  `CBM_CACHE_DIR=/tmp/cbm-dev CBM_RUNTIME_DIR=/tmp/cbm-dev-rt build/c/codebase-memory-mcp cli ...`

## Contributing Upstream

Read `CONTRIBUTING.md`: open an issue before a new pass, keep each PR small
and to one issue, sign every commit (`git commit -s`), show a test that fails
without the change, and disclose AI help.
