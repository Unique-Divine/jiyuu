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
[README.upstream.md](README.upstream.md). [IMPORT.json](IMPORT.json) and
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
`just scripts-test` runs the grammar script tests. `scripts/test.sh` with no
arguments runs upstream's full matrix. Some of its contract steps read the
upstream CI workflows and package wrappers, which this import excludes, and
fail for that reason.

When running the binary against a real repository during development, point
`CBM_CACHE_DIR` and `CBM_RUNTIME_DIR` at scratch directories to keep the
index away from `~/.cache/codebase-memory-mcp`.

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
