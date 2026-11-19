# Import Record

Mori is imported into Jiyuu from a snapshot of Codebase Memory MCP. This file
records the snapshot, what was left out, and every local change, so an agent
can restore any excluded path from this file alone.

| Field | Value |
| --- | --- |
| Source repository | <https://github.com/Unique-Divine/codebase-memory-mcp> |
| Original upstream | <https://github.com/DeusData/codebase-memory-mcp> |
| Source commit | `cdfdf26483fa56f4f7d9d85e7c4c3924133b458b` |
| Imported | 2026-11-19 |
| Upstream history imported | No |
| License | MIT ([LICENSE](LICENSE), unchanged) |

Links below point at the commit on DeusData's repository. The fork holds the
same commit, so `~/codebase-memory-mcp` (a clone of the fork) can restore any
path offline.

## Restoring an Excluded Path

Run from the Jiyuu root. `git archive` writes the path as it was at the
source commit, with no upstream history:

```bash
git -C ~/codebase-memory-mcp archive \
  cdfdf26483fa56f4f7d9d85e7c4c3924133b458b <path> | tar -x -C mori
```

Then drop the matching entry from this file, and re-add any local change this
file says was made because the path was missing.

## Local Modifications

- Imported as directory `mori/` of the Jiyuu monorepo. The project is being
  renamed Mori; the C binary keeps the name `codebase-memory-mcp`.
- The vendored Tree-sitter grammars are not committed. They are fetched at
  build time (see [Vendored Grammars](#vendored-grammars)).
- Upstream `README.md` is kept as `README.upstream.md`. `README.md` is Jiyuu's.
- `tests/test_cli.c` reads `README.upstream.md` for the supported-agent
  contract and drops its `pkg/npm/README.md` assertions (the 46-surface
  sentence and the per-agent list).
- `tests/test_language_count_contract.sh` checks one surface,
  `README.upstream.md`, instead of two (`README.md` and the Chocolatey nuspec
  under `pkg/`).
- `tests/test_vt_gate_policy_contract.sh` reads `README.upstream.md` in place
  of `README.md` in both of its README checks.
- `.gitignore` gains a Jiyuu block that ignores the fetched
  `internal/cbm/vendored/grammars/`, `.grammars.stamp` and `.grammars-tmp-*/`.
- Removed upstream distribution files Mori does not use: `flake.nix`,
  `flake.lock`, `server.json`, `glama.json`, `install.sh` and `install.ps1`
  (see [Excluded Paths](#excluded-paths)). Nothing in the build or the C test
  suites reads them. Scripts and contract tests that do are left unchanged
  and listed under [Known Test Gaps](#known-test-gaps).
- Added by Jiyuu: `README.md`, `IMPORT.md`, `NOTICE`, `justfile`,
  `grammars.lock.json`, and the Bun scripts `scripts/grammars-fetch.ts`,
  `scripts/grammars-lib.ts`, `scripts/grammars-pack.ts`,
  `scripts/grammars.test.ts` and `scripts/tsconfig.json`.

## Excluded Paths

### Vendored Grammars

- Upstream:
  [`internal/cbm/vendored/grammars/`](https://github.com/DeusData/codebase-memory-mcp/tree/cdfdf26483fa56f4f7d9d85e7c4c3924133b458b/internal/cbm/vendored/grammars)
- Upstream role: the generated parsers of 164 Tree-sitter grammars (about
  1.3 GB), each with its LICENSE, plus `MANIFEST.md` listing every grammar's
  source and license. The build compiles them into the binary.
- In Jiyuu: `scripts/grammars-fetch.ts` installs the directory from
  `grammars.tar.zst`, published as release
  [`mori/grammars/v1`](https://github.com/Unique-Divine/jiyuu/releases/tag/mori%2Fgrammars%2Fv1)
  and pinned in [grammars.lock.json](grammars.lock.json) (sha256
  `6bc58d5ae682284c434cb18779b554cfd3af7de6ab307ba24a67daea2ec114e8`). Every
  file is checked against `scripts/vendored-checksums.txt`.
- Restore: `just grammars` inside `mori/`. To commit the directory instead,
  restore it with `git archive` and remove the `/internal/cbm/vendored/grammars/`
  line from `.gitignore`.

### Generated Grammar Sources Under `tools/`

- Upstream:
  [`tools/`](https://github.com/DeusData/codebase-memory-mcp/tree/cdfdf26483fa56f4f7d9d85e7c4c3924133b458b/tools)
- Excluded globs: `tools/tree-sitter-*/**/src/parser.c`,
  `tools/tree-sitter-*/**/src/grammar.json`,
  `tools/tree-sitter-*/**/src/node-types.json` and
  `tools/tree-sitter-*/**/src/tree_sitter/` (about 161 MB).
- Upstream role: `tree-sitter generate` output for the ten grammars upstream
  maintains (arkts, chialisp, form, javascript, magma, pascal, sql, tsx, vba,
  vbnet). The same generated files are copied into
  `internal/cbm/vendored/grammars/`, which is what the build compiles.
- Kept: the hand-written sources (`grammar.js` and its modules, external
  scanners, patches, corpus tests, `package.json`, `tree-sitter.json` and
  `tools/tree-sitter-sql/LICENSE`).
- Restore: run `tree-sitter generate` in a grammar directory, or extract the
  generated files from the commit:

  ```bash
  git -C ~/codebase-memory-mcp archive \
    cdfdf26483fa56f4f7d9d85e7c4c3924133b458b tools | tar -x -C mori
  ```

### `.github/`

- Upstream:
  [`.github/`](https://github.com/DeusData/codebase-memory-mcp/tree/cdfdf26483fa56f4f7d9d85e7c4c3924133b458b/.github)
- Upstream role: DeusData's GitHub Actions (25 workflows: PR lanes, build,
  test, lint, security, smoke, soak, CodeQL, Scorecard, release, Pages),
  issue and PR templates, labeler and Dependabot config, CODEOWNERS.
- Effect here: contract tests that read the workflows fail; see
  [Known Test Gaps](#known-test-gaps).
- Restore: `git archive ... .github`. Workflows only run from the repository
  root, so they would need to move to Jiyuu's `.github/workflows/` and be
  rewritten for the `mori/` working directory.

### `pkg/` and `Formula`

- Upstream:
  [`pkg/`](https://github.com/DeusData/codebase-memory-mcp/tree/cdfdf26483fa56f4f7d9d85e7c4c3924133b458b/pkg)
  and
  [`Formula`](https://github.com/DeusData/codebase-memory-mcp/blob/cdfdf26483fa56f4f7d9d85e7c4c3924133b458b/Formula)
  (a symlink to `pkg/homebrew/Formula`).
- Upstream role: wrappers that publish DeusData's release binaries to package
  registries. Each downloads the matching GitHub release asset:
  - `pkg/npm/`: npm package `codebase-memory-mcp` (`install.js` fetches the
    binary on install, `bin.js` launches it), with Node tests.
  - `pkg/pypi/`: PyPI package `codebase-memory-mcp`, a Python launcher.
  - `pkg/go/`: a Go module whose `main.go` downloads and runs the binary.
  - `pkg/homebrew/`: Homebrew formula, exposed at the root as `Formula`.
  - `pkg/scoop/`: Scoop manifest for Windows.
  - `pkg/chocolatey/`: Chocolatey nuspec and install/uninstall scripts.
  - `pkg/winget/`: winget manifests for `DeusData.CodebaseMemoryMcp`.
  - `pkg/aur/`: Arch Linux `PKGBUILD` and `.SRCINFO`.
  - `pkg/glama/`: Dockerfile and `verify.sh` for Glama's directory checks.
- Restore: `git archive ... pkg Formula`, then rename packages and release
  URLs before publishing anything for Mori.

### Claude Code Plugin: `plugin/` and `.claude-plugin/`

- Upstream:
  [`plugin/`](https://github.com/DeusData/codebase-memory-mcp/tree/cdfdf26483fa56f4f7d9d85e7c4c3924133b458b/plugin)
  and
  [`.claude-plugin/`](https://github.com/DeusData/codebase-memory-mcp/tree/cdfdf26483fa56f4f7d9d85e7c4c3924133b458b/.claude-plugin)
- Upstream role: a Claude Code plugin (`plugin/.claude-plugin/plugin.json`,
  `plugin/.mcp.json` running `npx -y codebase-memory-mcp@0.12.1`) and the
  marketplace manifest `.claude-plugin/marketplace.json` that lists it.
- Restore: `git archive ... plugin .claude-plugin`; point `.mcp.json` at a
  Mori release before use.

### `flake.nix` and `flake.lock`

- Upstream:
  [`flake.nix`](https://github.com/DeusData/codebase-memory-mcp/blob/cdfdf26483fa56f4f7d9d85e7c4c3924133b458b/flake.nix)
  and
  [`flake.lock`](https://github.com/DeusData/codebase-memory-mcp/blob/cdfdf26483fa56f4f7d9d85e7c4c3924133b458b/flake.lock)
- Upstream role: a Nix flake that builds the binary and provides dev shells,
  including a macOS cross shell with the other architecture's zlib.
- Restore: `git archive ... flake.nix flake.lock`. A flake must sit at a Git
  repository root or be addressed as `path:./mori`, and the build needs the
  grammars fetched first.

### `server.json`

- Upstream:
  [`server.json`](https://github.com/DeusData/codebase-memory-mcp/blob/cdfdf26483fa56f4f7d9d85e7c4c3924133b458b/server.json)
- Upstream role: the official MCP Registry entry
  `io.github.DeusData/codebase-memory-mcp`, which installs through DeusData's
  npm and PyPI packages. `scripts/ci/gen-mcpb-registry-entries.sh` adds MCPB
  bundle entries to it at release time.
- Restore: `git archive ... server.json`, then rename the entry and its
  packages for Mori.

### `glama.json`

- Upstream:
  [`glama.json`](https://github.com/DeusData/codebase-memory-mcp/blob/cdfdf26483fa56f4f7d9d85e7c4c3924133b458b/glama.json)
- Upstream role: claims the [Glama](https://glama.ai) MCP directory listing
  for maintainer `DeusData`.
- Restore: `git archive ... glama.json` with Mori's maintainer.

### `install.sh` and `install.ps1`

- Upstream:
  [`install.sh`](https://github.com/DeusData/codebase-memory-mcp/blob/cdfdf26483fa56f4f7d9d85e7c4c3924133b458b/install.sh)
  and
  [`install.ps1`](https://github.com/DeusData/codebase-memory-mcp/blob/cdfdf26483fa56f4f7d9d85e7c4c3924133b458b/install.ps1)
- Upstream role: one-line installers (`curl ... | bash`, PowerShell) that
  download DeusData's GitHub release for the host, verify its checksum,
  install the binary and run agent setup. Release archives also ship the
  installer beside the binary, and `codebase-memory-mcp update` points at it.
- Restore: `git archive ... install.sh install.ps1`, then change `REPO` and
  the asset names to Mori's releases.
- Effect here: `scripts/package-release.sh` copies the installer into every
  release archive and stops without it; `scripts/smoke-local.sh`,
  `scripts/setup.sh` and `scripts/setup-windows.ps1` also expect it. None of
  them runs from a Jiyuu recipe. A Mori release needs either its own
  installers or a release layout without them. The binary's `update` command
  already handles an archive with no installer beside the binary.

## Known Test Gaps

`scripts/test.sh` with no arguments, and `--contracts-only`, stop at Step 0e
because `tests/test_windows_bundle_contract.sh` reads excluded files
(`install.ps1`, `pkg/npm/install.js`). With `set -e`, no later contract step,
suite or prod-binary guard runs. Run every C suite with `just test-all`
inside `mori/`.

Run one by one, 13 of the 45 Step 0 contract scripts fail on this tree, each
because it reads a path excluded above. Whether to remove, rewrite or guard
them is pending a decision; they are unchanged.

| Contract | Missing input |
| --- | --- |
| `tests/test_codeql_retrace.sh` | `.github/codeql/codeql-config.yml` |
| `tests/test_lane_wiring_contract.sh` | `.github/workflows/dry-run.yml` |
| `tests/test_release_gate_chain_contract.sh` | `.github/workflows/release.yml` |
| `tests/test_security_gate_fail_closed.sh` | `.github/workflows/codeql.yml` |
| `tests/test_select_lanes.sh` | `.github/codeql/codeql-config.yml`, read by `scripts/ci/select-lanes.sh` |
| `tests/test_smoke_fixture_contract.sh` | `.github/workflows/pr.yml`, `install.sh` |
| `tests/test_venue_parity_contract.sh` | `.github/workflows/release.yml` |
| `tests/test_version_metadata_contract.sh` | `server.json`, `pkg/npm/package.json`, `pkg/pypi/pyproject.toml` |
| `tests/test_windows_bundle_contract.sh` | `install.ps1`, `pkg/npm/install.js` |
| `tests/test_install_android_guard.sh` | `install.sh` |
| `tests/test_mcpb_bundle_contract.sh` | `install.sh` (through `scripts/package-release.sh`) |
| `tests/test_mcpb_registry_entries_contract.sh` | `server.json` |
| `tests/test_setup_scripts_contract.sh` | `install.sh` |

The first nine failed before the distribution files were removed; the last
four fail because of that removal.
