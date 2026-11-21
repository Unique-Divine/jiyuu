# Mori (森)

Mori is a local code-intelligence server for coding agents. It indexes a
repository with Tree-sitter into a persistent knowledge graph and answers
structural questions over the Model Context Protocol (MCP) and a CLI.

Mori is [Codebase Memory MCP](https://github.com/DeusData/codebase-memory-mcp),
now maintained by Unique Divine in Jiyuu. The code is still the C
implementation, and the binary keeps the name `codebase-memory-mcp` for now. A
Rust port is planned as crate `jiyuu-mori` with command `mori`. Releases will
be tagged `mori/v*`.

Upstream's README, with usage, MCP tools and agent setup, is kept as
[README.upstream.md](README.upstream.md). [IMPORT.md](IMPORT.md) and
[NOTICE](NOTICE) record the source commit, exclusions and local changes.

## Build

Requirements: `gcc` (or `clang`), `make`, zlib development headers
(`zlib1g-dev` on Debian and Ubuntu), `zstd`, `tar`, `git` and
[Bun](https://bun.sh). The test scripts also use `bash` and `python3`.

```bash
just mori-build      # from the Jiyuu root
# or, inside mori/
just build
```

The binary is written to `build/c/codebase-memory-mcp`.

```bash
build/c/codebase-memory-mcp --version
```

## Grammar Archive

The vendored Tree-sitter grammars in `internal/cbm/vendored/grammars/` take
about 1.3 GB, so Jiyuu does not commit them. File
[grammars.lock.json](grammars.lock.json) pins a 22 MB `grammars.tar.zst` by
URL, sha256 and size, along with the source commit it was packed from.
`just grammars` runs [scripts/grammars-fetch.ts](scripts/grammars-fetch.ts),
which:

1. Reuses `${XDG_CACHE_HOME:-~/.cache}/jiyuu-mori/grammars/<sha256>.tar.zst`
   after checking its hash, or downloads the archive from the lock URL.
2. Checks the size and sha256 before unpacking anything.
3. Rejects absolute paths, `..` components, members outside the grammar
   directory, and any member that is not a regular file or directory
   (symlinks, hard links, devices).
4. Unpacks into a temporary directory and checks every file against the
   grammar lines of `scripts/vendored-checksums.txt` before moving it into
   place.
5. Writes `.grammars.stamp` with the archive hash and the size and mtime of
   every installed file. The next run compares the tree with the stamp and
   hashes `MANIFEST.md` and the license files, which takes under a second.
   If a file was added, removed or changed, it hashes the whole tree (about
   6 seconds with a warm page cache, longer cold) and reinstalls from the
   cached archive when that check fails.
   Pass `--force` to reinstall.

The archive is published as release
[`mori/grammars/v1`](https://github.com/Unique-Divine/jiyuu/releases/tag/mori%2Fgrammars%2Fv1).
To build offline, pass a local copy of the same archive; it must match the
lock's sha256:

```bash
just mori-build --from /path/to/grammars.tar.zst
```

[scripts/grammars-pack.ts](scripts/grammars-pack.ts) builds the archive from a
codebase-memory-mcp checkout. It runs `git archive` on the grammar directory
and compresses with fixed `zstd` settings, so the same commit gives the same
bytes:

```bash
bun scripts/grammars-pack.ts --repo ~/codebase-memory-mcp --commit <sha> \
  --out /tmp/grammars.tar.zst --update-lock --release-tag mori/grammars/v2
```

`--update-lock` rewrites the lock's `url`, `sha256`, `sizeBytes` and
`sourceCommit`. `--release-tag` puts the new tag into the lock's GitHub
release URL; `--url` sets the URL directly.

Ten grammars that upstream maintains itself (arkts, chialisp, form,
javascript, magma, pascal, sql, tsx, vba and vbnet) keep their hand-written
sources in `tools/tree-sitter-*/`: `grammar.js`, external scanners, patches
and corpus tests. The files `tree-sitter generate` writes from them
(`src/parser.c`, `src/grammar.json`, `src/node-types.json` and
`src/tree_sitter/`) are left out; the build uses the generated copies in the
grammar archive.

To change grammars, update `scripts/vendored-checksums.txt` and the grammar
sources in the source repository, pack at the new commit with
`--update-lock --release-tag <new tag>`, and publish the archive under that
tag.

## Test

```bash
just mori-test       # grammar and extraction suites, language-count and
                     # vendored-checksum checks
just mori-check      # script tests, build, then the same suites
```

Inside `mori/`, `just test "<suites>"` runs other suites and
`just scripts-test` runs the grammar script tests.

`scripts/test.sh` with no arguments runs the full leg: the static contract
checks (Step 0), a clean sanitizer build, every suite (194, over an hour),
and the production-binary checks. `scripts/test.sh --contracts-only` runs
Step 0 alone in a few minutes. Both need the grammars installed first
(`just grammars`).

```bash
just test-all        # inside mori/; same as: scripts/test.sh
```

Twelve upstream contract checks that test DeusData's release machinery are
removed; [IMPORT.md](IMPORT.md#excluded-contract-checks) lists them with
their restore commands.

When running the binary against a real repository during development, point
`CBM_CACHE_DIR` and `CBM_RUNTIME_DIR` at scratch directories to keep the
index away from `~/.cache/codebase-memory-mcp`.

## Release

File [VERSION](VERSION) holds Mori's version. Workflow
[mori-release.yml](../.github/workflows/mori-release.yml) passes it to
`scripts/build.sh --version`, which stamps it into the binary, so
`codebase-memory-mcp --version` prints it. Local builds without `--version`
print `dev`.

To release, bump `VERSION` in a PR, merge it, then tag the merged commit:

```bash
git tag -a mori/v0.13.0 -m "Mori 0.13.0"
git push origin mori/v0.13.0
```

The workflow fails unless the tag equals `mori/v` plus `VERSION`. It builds
three targets, smoke-tests `--version` and a tiny index on each, and packages
each with [scripts/package-release.sh](scripts/package-release.sh) as
`codebase-memory-mcp-<version>-<target>.tar.gz`:

| Target | Runner | Build |
| --- | --- | --- |
| `linux-x64` | `ubuntu-latest` | static (`STATIC=1`) |
| `linux-arm64` | `ubuntu-24.04-arm` | static (`STATIC=1`) |
| `darwin-arm64` | `macos-15` | dynamic, ad-hoc signed, macOS 14 or later |

Each archive holds the binary, `LICENSE`, `NOTICE` and
`THIRD_PARTY_NOTICES.md` (generated from `THIRD_PARTY.md` and the vendored
licenses). There is no installer; install from the release assets, for
example with mise's `github:` backend, which picks the archive by the
platform in its name:

```toml
[tools."github:Unique-Divine/jiyuu"]
version = "0.13.0"
version_prefix = "mori/v"
```

There is no Intel macOS build. The macOS binary links only system
libraries (`libSystem`, `libz`, `libc++`); static linking is Linux-only.

The macOS binary carries an ad-hoc signature, not a Developer ID signature,
and is not notarized. Downloads through mise, `curl` or `gh release download`
carry no quarantine attribute and run as is. If you download the archive in
a browser, macOS Gatekeeper refuses to open the binary because Apple can't
verify it; remove the quarantine attribute after unpacking:

```bash
xattr -d com.apple.quarantine codebase-memory-mcp
```

The release job attests build provenance for the archives and
`checksums.txt`, then creates a draft release with `checksums.txt` and
`release-metadata.json`. Publish it by hand, keeping Jiyuu's latest release
unchanged:

```bash
gh release edit mori/v0.13.0 --draft=false --latest=false
```

Rerunning the workflow refills a draft but never changes a published
release.

To build without releasing, run the workflow by hand on any branch; the
`dry_run` input defaults to true and uploads the archives and checksums as
workflow artifacts. [scripts/release.ts](scripts/release.ts) holds the tag
check, asset names, checksums and the draft step; test it with
`bun test scripts/release.test.ts`.

## Attributions

- [DeusData/codebase-memory-mcp](https://github.com/DeusData/codebase-memory-mcp),
  MIT License, Copyright (c) 2025 DeusData. Mori is imported from commit
  `cdfdf26483fa56f4f7d9d85e7c4c3924133b458b` through the fork
  [Unique-Divine/codebase-memory-mcp](https://github.com/Unique-Divine/codebase-memory-mcp).
  See [LICENSE](LICENSE) and [NOTICE](NOTICE).
- Vogel, Meyer-Eschenbach, Kohler, Grünewald, Balzer, "Codebase-Memory:
  Tree-Sitter-Based Knowledge Graphs for LLM Code Exploration via MCP",
  [arXiv:2603.27277](https://arxiv.org/abs/2603.27277).
- The Tree-sitter grammars in the grammar archive. Each grammar directory
  carries its own LICENSE, and `MANIFEST.md` in the archive lists every
  grammar's source and license.
- Other vendored libraries keep their licenses beside their sources; see
  [THIRD_PARTY.md](THIRD_PARTY.md).
