# Setup: Build, Install, and Index

## Why a Fork

The fork carries GraphQL cross-repo linking (see [graphql.md](graphql.md)),
offered upstream as pull requests. Until they merge, the fork's `ud/main`
branch is what runs: upstream `main` plus the GraphQL branches. Each branch
merged upstream shrinks the difference; once it's zero, go back to upstream
releases.

```sh
git clone git@github.com:Unique-Divine/codebase-memory-mcp.git ~/codebase-memory-mcp
cd ~/codebase-memory-mcp
git remote add upstream https://github.com/DeusData/codebase-memory-mcp.git
git switch ud/main
```

Keep the checkout beside your other repos, not inside one as a submodule. Its
working tree is large (generated parsers for hundreds of languages), and
anything that walks the parent repo would search, index, and sync it.

## Build and Install

```sh
scripts/build.sh --with-ui     # binary at build/c/codebase-memory-mcp
```

The UI step needs a JavaScript package manager; Bun works in place of npm.
Requirements: a C compiler, zlib, git.

Install over the existing binary, keeping the previous one for rollback:

```sh
cp ~/.local/bin/codebase-memory-mcp ~/.local/bin/codebase-memory-mcp.prev
install -m 0755 build/c/codebase-memory-mcp ~/.local/bin/codebase-memory-mcp
codebase-memory-mcp daemon stop; codebase-memory-mcp daemon start
```

Register the MCP server once per client (Claude: `~/.claude.json`; Codex:
`[mcp_servers]` in its config) with the binary path and no arguments. Don't
use `codebase-memory-mcp install` or `update` after that: they replace this
skill and install the upstream release.

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
