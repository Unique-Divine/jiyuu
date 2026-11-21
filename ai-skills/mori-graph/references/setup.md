# Setup: Install, Register, and Index

## Install From a Release

Mori ships as `mori/v*` releases of https://github.com/Unique-Divine/jiyuu,
with archives for `linux-x64`, `linux-arm64`, and `darwin-arm64`, a
`checksums.txt`, and build provenance attestations. Install with mise's
`github:` backend, which picks the archive for the platform:

```toml
# ~/.config/mise/config.toml
[tools]
"github:Unique-Divine/jiyuu" = { version = "0.13.0", version_prefix = "mori/v" }
```

```sh
mise lock --platform linux-arm64,linux-x64,macos-arm64   # with lockfile = true
mise install
codebase-memory-mcp --version   # codebase-memory-mcp 0.13.0
```

The archive holds one binary, `codebase-memory-mcp`, which mise exposes
through its shim. To upgrade, bump `version`, relock, install, then restart
the background daemon, which keeps the old binary until it restarts:

```sh
codebase-memory-mcp daemon stop
```

Remove any hand-built `codebase-memory-mcp` earlier on `PATH` (for example in
`~/.local/bin`), or it keeps shadowing the release.

## Register the MCP Server

Register the server once per client under the key `mori-graph`, with the
binary path and no arguments. Use a path that survives upgrades, such as
the mise shim (`~/.local/share/mise/shims/codebase-memory-mcp`) or a link to
it:

```sh
claude mcp add --scope user mori-graph -- ~/.local/share/mise/shims/codebase-memory-mcp
codex mcp add mori-graph -- ~/.local/share/mise/shims/codebase-memory-mcp
```

The key, not the binary, names the tools (`mcp__mori-graph__search_graph`),
so rename it in hook matchers, permission rules, and agent profiles that
list tools. Claude Code hooks that run `codebase-memory-mcp hook-augment`
keep working through the same binary path.

Don't run `codebase-memory-mcp install` or `update`: they rewrite this
skill, register the server as `codebase-memory-mcp`, and fetch a build
without Mori's GraphQL linking.

## Build From Source

For development, build from `mori/` in a Jiyuu checkout; see
[tool-source.md](tool-source.md). Run the dev binary from `build/c/` with a
separate cache rather than installing it over the release.

## Index a Repo

```sh
codebase-memory-mcp cli --quiet index_repository --repo_path ~/some-repo
```

- **Exclude data and build output** with a `.cbmignore` (gitignore syntax) at
  the repo root. JSON keys alone can outnumber code nodes ten to one. In repos
  you don't own, keep the file out of git with
  `echo /.cbmignore >> "$(git rev-parse --git-path info/exclude)"`.
- **Map extra extensions** in `~/.config/codebase-memory-mcp/config.json`, for
  example `{"extra_extensions": {".graphqls": "graphql"}}` on builds that
  don't map it already.
- **Retry a busy repo.** `aborted_previous_preserved` means files changed
  while indexing; the old index keeps serving. Run it again.
- **Never test with "gql" in a repo's path.** GraphQL calls are recognized by
  library names inside qualified names, and a path segment `gql` makes every
  call in the repo look like a GraphQL call.

## Link Repos

Cross-repo links are a separate pass, run per source project:

```sh
codebase-memory-mcp cli --quiet index_repository \
  '{"repo_path":"/path/to/client-repo","mode":"cross-repo-intelligence","target_projects":["server-project"]}'
```

- **Reindex, then relink.** A full reindex drops the project's cross-repo
  links. Rerun linking for it and for every project that links to it.
- **One run per project, every target in it.** A link run replaces that
  project's earlier links, so a client calling two servers lists both in
  `target_projects` in a single run.
- **Name the target** when two servers share field or route names, so a
  client links only to the server it calls. Use `["*"]` only when names are
  unique across projects.
- `index_status` shows `cross_repo.status` and the edge count for each
  project.
