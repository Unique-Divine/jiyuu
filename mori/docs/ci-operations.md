# Mori CI Operations

## Choose a Run

Workflow [`mori-release.yml`](../../.github/workflows/mori-release.yml)
builds and tests the Mori packages.

| Run | Targets | Output |
| --- | --- | --- |
| Branch dry run | Linux x64, Linux arm64, Apple Silicon | Workflow artifact `mori-preview-<version>-<sha>` with archives and checksums |
| Manual reference prewarm on `main` | Linux x64, Linux arm64, Apple Silicon | Frozen C reference caches |
| Daily check | All four release targets | Build, download, smoke, and CLI parity results |
| Manual daily check on `main` | All four release targets | Build and parity results, with issue reporting disabled |
| Tagged release or tagged dry run | All four release targets | Archives and checksums for the exact tag commit |

GitHub schedules the daily check for 04:23 UTC on the commit it selects
from `main`. Daily checks use the same grammar and pinned C reference
caches as previews and releases. The build verifies restored inputs and
fetches or rebuilds them when the cache has no valid entry. Each target
still builds Mori and runs CLI parity. Tagged runs check Intel macOS on
the tag's commit before creating a draft release.

Use the branch ref for a preview. Use `main` for a cache prewarm or a
manual run of the daily checks.

```sh
gh workflow run mori-release.yml --repo Unique-Divine/jiyuu --ref <branch> -f dry_run=true
gh workflow run mori-release.yml --repo Unique-Divine/jiyuu --ref main -f dry_run=true -f prewarm_oracle=true
gh workflow run mori-release.yml --repo Unique-Divine/jiyuu --ref main -f dry_run=true -f daily_check=true
```

For a new stack's branch preview gate, require the three per-target
`mori-build-<target>` artifacts and combined artifact
`mori-preview-<version>-<sha>` from a successful run on the exact stack head.

## Investigate a Daily Failure

The reporter files issues titled `Mori daily package check fails in <stage>`
with labels `Z-mori`, `A-tech-investment`, and `S-triage`. An issue records
the full commit SHA, attempt, and failing job. It includes a short
diagnostic when the logs match a known failure. Download file
`daily-package-failures.json` from the workflow's evidence artifact for the
job records and issue links. The artifact is retained for 30 days.

Matching failures on the same commit add a comment to the existing issue.
A recognized failure on a later commit can reuse an open issue. When logs
do not identify a known failure, the reporter groups by commit, target,
and stage. Keep raw output in Actions logs and use the issue to record
reproduction, diagnosis, and the fix.

1. Open the linked job and read the first meaningful diagnostic in the
   failed step. When a job timed out, check the last completed step and the
   last lines of output. Use `--log` if `--log-failed` returns nothing.
2. Rerun the failed jobs on the same commit. Save both attempt links in the
   issue so a passing retry does not erase the original failure.
3. Check the failure against the categories below. If a source change is
   needed, reproduce against the recorded SHA before testing the fix.

```sh
gh run view <run-id> --repo Unique-Divine/jiyuu --attempt <attempt> --job <job-id> --log-failed
gh run rerun <run-id> --repo Unique-Divine/jiyuu --failed
```

| Evidence | Next check |
| --- | --- |
| Runner setup, DNS, or download timeout | Check which tool or download failed and whether the same step failed on other targets. Compare the retry's runner image and tool versions. |
| Missing archive or checksum mismatch | Check the input URL and pin against the relevant source. A passing retry can point to a transient download problem. |
| Compiler, linker, or signing error | Reproduce the failing target with the compiler and SDK versions from the job log. Check recent build and native interface changes. |
| Smoke, replay, or CLI parity mismatch | Inspect the named fixture and expected output. Use the same candidate and frozen C reference as the failing run. |

For linked CLI checks in a local checkout, run
`just --justfile jiyuu-mori/justfile parity` from `mori/`. For release helper
tests, run `bun test scripts/release.test.ts` from the same directory.

Link the fix PR from the issue. Include the rerun or focused test that
verifies the correction. If a same-SHA rerun passes and the evidence points
to an infrastructure failure, record the result before closing the issue.
