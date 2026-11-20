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
  suites reads them. Release and setup scripts that do are left unchanged.
- Removed 13 upstream contract scripts that check DeusData's release
  machinery, their steps in `scripts/test.sh`, and the fixture
  `tests/fixtures/select-lanes/` (see
  [Excluded Contract Checks](#excluded-contract-checks)). `scripts/test.sh`
  gains a comment above Step 0 that points here.
- Added by Jiyuu: `README.md`, `IMPORT.md`, `NOTICE`, `justfile`,
  `grammars.lock.json`, and the Bun scripts `scripts/grammars-fetch.ts`,
  `scripts/grammars-lib.ts`, `scripts/grammars-pack.ts`,
  `scripts/grammars.test.ts` and `scripts/tsconfig.json`.

## GraphQL Linking

GraphQL cross-repo linking was replayed from the fork's linear branch
`ud/gql-linear` onto the import with
`git format-patch | git am -3 --directory=mori`. Each commit keeps its
original author and message. The next replay starts from tip `9266f4c1`.

| Range | Commits | Contents |
| --- | --- | --- |
| `cdfdf264..dd23b155` | 30 | Server lane `ud/gql-server-routes`: schema Routes and gqlgen resolvers |
| `dd23b155..9266f4c1` | 14 | Client lane `ud/gql-client-fields` (tip `9c8df01b`) rebased onto the server lane: client field-path edges |

The range applied without conflicts. It touches no path excluded below and
no file this import modifies.

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
- Effect here: the contract checks that read the workflows are removed; see
  [Excluded Contract Checks](#excluded-contract-checks).
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

## Excluded Contract Checks

Step 0 of `scripts/test.sh` ran 45 static contract scripts. Thirteen of them
check DeusData's release machinery and read paths this import excludes, so
they failed, and with `set -e` the first one (Step 0e) stopped every later
step and suite. They are removed with their steps; the step letters in
`scripts/test.sh` skip where they were. Nine failed from the start of the
import; four (Steps 0o, 0p, 0x2 and 0y3) failed once `install.sh` and
`server.json` were removed.

Each entry gives the upstream file, its Step 0 label, what it guarded, and
the paths it reads. To restore one, run its restore command from the Jiyuu
root, restore the paths it reads (see [Excluded Paths](#excluded-paths)),
then add its two lines back to Step 0 of `scripts/test.sh`, copied from the
[upstream `scripts/test.sh`](https://github.com/DeusData/codebase-memory-mcp/blob/cdfdf26483fa56f4f7d9d85e7c4c3924133b458b/scripts/test.sh).

- [`tests/test_windows_bundle_contract.sh`](https://github.com/DeusData/codebase-memory-mcp/blob/cdfdf26483fa56f4f7d9d85e7c4c3924133b458b/tests/test_windows_bundle_contract.sh),
  Step 0e, "Windows launcher bundle contract"
  - Guarded: Windows releases ship one binary with no launcher stub, and
    `install.ps1` renames the running binary out of the way before installing
    the new one.
  - Reads: `install.ps1`, `pkg/npm/install.js`
  - Restore:

    ```bash
    git -C ~/codebase-memory-mcp archive cdfdf264 \
      tests/test_windows_bundle_contract.sh | tar -x -C mori
    ```

- [`tests/test_security_gate_fail_closed.sh`](https://github.com/DeusData/codebase-memory-mcp/blob/cdfdf26483fa56f4f7d9d85e7c4c3924133b458b/tests/test_security_gate_fail_closed.sh),
  Step 0e3, "security gate fail-closed contract"
  - Guarded: The CodeQL alert gate in upstream's workflows fails the build when
    it cannot read the alert API, instead of reporting zero alerts.
  - Reads: `.github/workflows/codeql.yml`
  - Restore:

    ```bash
    git -C ~/codebase-memory-mcp archive cdfdf264 \
      tests/test_security_gate_fail_closed.sh | tar -x -C mori
    ```

- [`tests/test_smoke_fixture_contract.sh`](https://github.com/DeusData/codebase-memory-mcp/blob/cdfdf26483fa56f4f7d9d85e7c4c3924133b458b/tests/test_smoke_fixture_contract.sh),
  Step 0h, "smoke release-fixture contract"
  - Guarded: The local and PR release-fixture smoke covers the paths it depends
    on, as decided by the PR lane selector.
  - Reads: `.github/workflows/pr.yml`, `install.sh`
  - Restore:

    ```bash
    git -C ~/codebase-memory-mcp archive cdfdf264 \
      tests/test_smoke_fixture_contract.sh | tar -x -C mori
    ```

- [`tests/test_venue_parity_contract.sh`](https://github.com/DeusData/codebase-memory-mcp/blob/cdfdf26483fa56f4f7d9d85e7c4c3924133b458b/tests/test_venue_parity_contract.sh),
  Step 0j, "venue parity contract (one harness, every venue)"
  - Guarded: Every `run:` step in upstream's workflows only provisions machines
    or calls a canonical leg script, so CI cannot grow its own copy of the test
    harness.
  - Reads: `.github/workflows/release.yml` and the other workflows
  - Restore:

    ```bash
    git -C ~/codebase-memory-mcp archive cdfdf264 \
      tests/test_venue_parity_contract.sh | tar -x -C mori
    ```

- [`tests/test_mcpb_bundle_contract.sh`](https://github.com/DeusData/codebase-memory-mcp/blob/cdfdf26483fa56f4f7d9d85e7c4c3924133b458b/tests/test_mcpb_bundle_contract.sh),
  Step 0o, "MCPB bundle contract (#1246)"
  - Guarded: `scripts/package-release.sh` builds MCPB bundles with the right
    members, manifest, and executable bit for each target that gets one.
  - Reads: `install.sh` (copied by `scripts/package-release.sh`)
  - Restore:

    ```bash
    git -C ~/codebase-memory-mcp archive cdfdf264 \
      tests/test_mcpb_bundle_contract.sh | tar -x -C mori
    ```

- [`tests/test_mcpb_registry_entries_contract.sh`](https://github.com/DeusData/codebase-memory-mcp/blob/cdfdf26483fa56f4f7d9d85e7c4c3924133b458b/tests/test_mcpb_registry_entries_contract.sh),
  Step 0p, "MCPB registry entries contract (#1246)"
  - Guarded: `scripts/ci/gen-mcpb-registry-entries.sh` turns `checksums.txt`
    into the MCP Registry bundle entries in `server.json` with correct hashes.
  - Reads: `server.json`
  - Restore:

    ```bash
    git -C ~/codebase-memory-mcp archive cdfdf264 \
      tests/test_mcpb_registry_entries_contract.sh | tar -x -C mori
    ```

- [`tests/test_release_gate_chain_contract.sh`](https://github.com/DeusData/codebase-memory-mcp/blob/cdfdf26483fa56f4f7d9d85e7c4c3924133b458b/tests/test_release_gate_chain_contract.sh),
  Step 0s, "release gate-chain ordering contract"
  - Guarded: In upstream's release workflow, skipping an optional phase does
    not skip the phases after it, and the release draft requires smoke and soak
    to succeed.
  - Reads: `.github/workflows/release.yml`, `_build.yml`, `dry-run.yml`,
    `_soak.yml`
  - Restore:

    ```bash
    git -C ~/codebase-memory-mcp archive cdfdf264 \
      tests/test_release_gate_chain_contract.sh | tar -x -C mori
    ```

- [`tests/test_version_metadata_contract.sh`](https://github.com/DeusData/codebase-memory-mcp/blob/cdfdf26483fa56f4f7d9d85e7c4c3924133b458b/tests/test_version_metadata_contract.sh),
  Step 0x, "packaging version-metadata contract"
  - Guarded: Every packaging file that states a version (`server.json`, npm,
    PyPI, Chocolatey, Go) states a released version, and registry-facing files
    state the newest.
  - Reads: `server.json`, `pkg/npm/package.json`, `pkg/pypi/pyproject.toml`
  - Restore:

    ```bash
    git -C ~/codebase-memory-mcp archive cdfdf264 \
      tests/test_version_metadata_contract.sh | tar -x -C mori
    ```

- [`tests/test_install_android_guard.sh`](https://github.com/DeusData/codebase-memory-mcp/blob/cdfdf26483fa56f4f7d9d85e7c4c3924133b458b/tests/test_install_android_guard.sh),
  Step 0x2, "installer Android/Termux guard contract (#1202)"
  - Guarded: `install.sh` refuses Android/Termux before downloading anything.
  - Reads: `install.sh`
  - Restore:

    ```bash
    git -C ~/codebase-memory-mcp archive cdfdf264 \
      tests/test_install_android_guard.sh | tar -x -C mori
    ```

- [`tests/test_setup_scripts_contract.sh`](https://github.com/DeusData/codebase-memory-mcp/blob/cdfdf26483fa56f4f7d9d85e7c4c3924133b458b/tests/test_setup_scripts_contract.sh),
  Step 0y3, "setup scripts install through the installers' verified path"
  - Guarded: `scripts/setup.sh` and `scripts/setup-windows.ps1` hand the
    download to the installers, which verify the checksum before installing.
  - Reads: `install.sh`, `install.ps1`
  - Restore:

    ```bash
    git -C ~/codebase-memory-mcp archive cdfdf264 \
      tests/test_setup_scripts_contract.sh | tar -x -C mori
    ```

- [`tests/test_select_lanes.sh`](https://github.com/DeusData/codebase-memory-mcp/blob/cdfdf26483fa56f4f7d9d85e7c4c3924133b458b/tests/test_select_lanes.sh),
  Step 0z, "PR lane selector contract"
  - Guarded: `scripts/ci/select-lanes.sh` picks the right PR CI lanes for a
    change set and falls back to every lane when unsure. Also restore
    `tests/fixtures/select-lanes/`.
  - Reads: `.github/codeql/codeql-config.yml`, read by
    `scripts/ci/select-lanes.sh`
  - Restore:

    ```bash
    git -C ~/codebase-memory-mcp archive cdfdf264 \
      tests/test_select_lanes.sh tests/fixtures/select-lanes | tar -x -C mori
    ```

- [`tests/test_codeql_retrace.sh`](https://github.com/DeusData/codebase-memory-mcp/blob/cdfdf26483fa56f4f7d9d85e7c4c3924133b458b/tests/test_codeql_retrace.sh),
  Step 0z1, "CodeQL retraces affected prebuilt objects"
  - Guarded: When CodeQL traces a build with prebuilt third-party objects,
    `scripts/ci/codeql-retrace-vendored.sh` recompiles exactly the objects a
    change affects.
  - Reads: `.github/codeql/codeql-config.yml`
  - Restore:

    ```bash
    git -C ~/codebase-memory-mcp archive cdfdf264 \
      tests/test_codeql_retrace.sh | tar -x -C mori
    ```

- [`tests/test_lane_wiring_contract.sh`](https://github.com/DeusData/codebase-memory-mcp/blob/cdfdf26483fa56f4f7d9d85e7c4c3924133b458b/tests/test_lane_wiring_contract.sh),
  Step 0z4, "lane wiring (PRs select, dry run and release run all)"
  - Guarded: Upstream's workflows honor the lane selection on PRs and run every
    lane on dry runs and releases.
  - Reads: `.github/workflows/dry-run.yml`, `pr.yml`, `release.yml`
  - Restore:

    ```bash
    git -C ~/codebase-memory-mcp archive cdfdf264 \
      tests/test_lane_wiring_contract.sh | tar -x -C mori
    ```

`test_codeql_retrace` and `test_release_gate_chain_contract` are candidates
for Mori's own CI. Neither has a trivial equivalent: Jiyuu has no CodeQL job
or CodeQL config for Mori, and no Mori release workflow with optional phases.
Port them when those exist.

Upstream files that only mention a removed check are unchanged, to keep
upstream catch-ups clean: comments in `tests/test_main.c` and
`tests/test_shell_line_endings.sh`, `scripts/README.md`,
`scripts/ci/README.md`, `test-infrastructure/README.md`, and the history
fixture `scripts/test-impact/fixtures/2026-09-filesets.tsv` (file lists of
past upstream PRs, replayed by Step 0z2 without opening the files).
