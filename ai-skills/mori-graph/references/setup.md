# Setup: Install, Register, and Index

## Install From a Release

Mori ships as `mori/v*` releases of https://github.com/Unique-Divine/jiyuu.
The published C release `mori/v0.13.1` packages `codebase-memory-mcp`
for Linux x64 and arm64 and macOS arm64. Install it with mise's `github:`
backend.

```toml
# ~/.config/mise/config.toml
[tools]
"github:Unique-Divine/jiyuu" = { version = "0.13.1", version_prefix = "mori/v" }
```

```sh
mise install
codebase-memory-mcp --version  # codebase-memory-mcp 0.13.1
```

For a published release with a `mori-<version>-<target>.tar.gz` archive,
pin that version and use its `mori` executable. With `lockfile = true`,
lock the platforms you use before installing.

To upgrade, bump `version`, relock, install, then restart the background
daemon, which keeps the old binary until it restarts:

```sh
mori daemon stop
```

Remove any hand-built `mori` or `codebase-memory-mcp` earlier on `PATH`
(for example in `~/.local/bin`), or it keeps shadowing the release.

Without mise, download the archive from the release page, check it against
`checksums.txt`, and unpack `mori` onto `PATH`. On macOS, a browser download
needs `xattr -d com.apple.quarantine mori` before Gatekeeper runs it; see
[Install](https://github.com/Unique-Divine/jiyuu/blob/main/mori/README.md#install) in
Mori's README.

## Register the MCP Server

Register the server once per client under the key `mori-graph`, with the
binary path and no arguments. Use a path that survives upgrades, such as
the mise shim (`~/.local/share/mise/shims/mori`) or a link to it:

```sh
claude mcp add --scope user mori-graph -- ~/.local/share/mise/shims/mori
codex mcp add mori-graph -- ~/.local/share/mise/shims/mori
```

The key, not the binary, names the tools (`mcp__mori-graph__search_graph`),
so rename it in hook matchers, permission rules, and agent profiles that
list tools. Claude Code hooks that run `mori hook-augment` work through the
same binary path; [Hooks and Tool Profiles](https://github.com/Unique-Divine/jiyuu/blob/main/mori/README.md#hooks-and-tool-profiles)
in Mori's README lists the hook entries and the `--tool-profile` option.

For 0.13.1, use the `codebase-memory-mcp` executable or shim in the
registration commands. Its inherited installer and updater contact the
upstream project's releases.

When switching an existing `mori-graph` entry, change its command path
and keep its arguments and environment. The `mori` shim stays the same
across Mori upgrades. If the shim is unavailable, register the executable
from `mise which mori` and update its path on upgrades. On rollback across
the rename, restore the previous `codebase-memory-mcp` command and hook paths.

In Claude Code, update the graph-tool `PostToolUse` matcher to
`Read|mcp__.*(mori-graph|codebase-memory-mcp)__.*` with the Mori hook.
Otherwise the legacy matcher misses `mori-graph` calls that reset the
search reminder. Keep the original matcher for rollback.

## Build From Source

For development, build from `mori/` in a Jiyuu checkout; see
[tool-source.md](tool-source.md). Run the dev binary from `build/c/` with a
separate cache rather than installing it over the release.

## Index a Repo

```sh
mori cli --quiet index_repository --repo_path ~/some-repo
```

- **Exclude data and build output** with a `.cbmignore` (gitignore syntax) at
  the repo root. JSON keys alone can outnumber code nodes ten to one. In repos
  you don't own, keep the file out of git with
  `echo /.cbmignore >> "$(git rev-parse --git-path info/exclude)"`.
- **Map extra extensions** in `~/.config/codebase-memory-mcp/config.json`
  (the path keeps its imported name), for example
  `{"extra_extensions": {".graphqls": "graphql"}}` on builds that don't map it
  already.
- **Retry a busy repo.** `aborted_previous_preserved` means files changed
  while indexing; the old index keeps serving. Run it again.
- **Never test with "gql" in a repo's path.** GraphQL calls are recognized by
  library names inside qualified names, and a path segment `gql` makes every
  call in the repo look like a GraphQL call.

## Link Repos

Cross-repo links are a separate pass, run per source project:

```sh
mori cli --quiet index_repository \
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
