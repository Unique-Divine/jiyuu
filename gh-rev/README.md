# gh-rev

`gh-rev` maintains a persistent local Markdown ledger for branch and pull
request reviews. Ledger state lives under `~/gh/` by default, outside reviewed
repositories.

## Development

List the supported commands and run all checks:

```bash
just --list
just check
```

Command `just check` runs formatting verification, Clippy with warnings denied,
and all Rust tests.

## Build provenance

Command `gh-rev --version` prints package and Git provenance as JSON:

```json
{
  "version": "0.1.0",
  "commit": "<git-sha>",
  "dirty": false
}
```

Field `dirty` is `null` when Git metadata was unavailable at build time. The
dirty check is scoped to this `gh-rev` package rather than unrelated changes in
the surrounding `jiyuu` repository.

## Install or update

Install the verified release binary at the path used by this workstation:

```bash
just check
just install-local
```

Override the destination when needed:

```bash
just install-local "$HOME/bin/gh-rev"
```

The install recipe builds with the lockfile, replaces the destination only
after a successful build, and prints the installed binary's provenance.
