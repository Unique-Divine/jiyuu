# CLI Parity Fixture

A small synthetic repository that `scripts/cli-parity.ts` indexes with both
`mori` and `codebase-memory-mcp`. It has Python, Go and TypeScript files with
calls inside and across files, so search, trace and architecture queries have
something to return. Keep it small: both binaries index it on every CI run.
