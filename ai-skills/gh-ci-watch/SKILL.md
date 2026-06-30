---
name: gh-ci-watch
description: >-
  Watch GitHub PR checks with gh, poll until green/failure/timeout, fetch
  failed GitHub Actions logs, save logs locally, summarize actionable CI
  failures, and apply trivial local fixes when safe. Use when the user asks to
  watch PR checks, wait for CI, inspect failed actions, fix obvious CI failures,
  or summarize gh pr checks results.
---

# GitHub CI Watch

Watch GitHub pull request CI until checks pass, fail with actionable logs, or
reach a timeout. Keep noisy command output out of the final answer. Save
detailed logs locally, then report the important failures clearly. If the logs
show a trivial, high-confidence local fix, make it and stage only the files
touched by that fix.

## Workflow

1. Identify the PR:
   - If the user gives a PR number or URL, use it.
   - Otherwise use the repository at the current working directory.
   - Infer the PR associated with the current branch using `gh pr view`.
   - If there is no PR for the current branch, stop and report that blocker.
   - Do not switch branches or repositories unless the user explicitly asks.

2. Create a local temporary log directory:
   - Use a deterministic base under `${TMPDIR:-/tmp}/<owner>__<repo>/`.
   - Prefer this shape:
     `${TMPDIR:-/tmp}/<owner>__<repo>/pr-<number>-checks/<utc-timestamp>/`
   - Sanitize `<owner>__<repo>` and branch fallback names to lowercase letters,
     numbers, dots, underscores, and hyphens.
   - Use `gh repo view --json nameWithOwner` and `gh pr view --json number`
     when available.
   - If no PR number is available yet, use `branch-<branch-slug>` instead of
     `pr-<number>-checks`.
   - Create subfiles such as `checks.txt`, `summary.md`, and
     `run-<id>-<job>.log` as useful.
   - Tell the user the final log directory path.

3. Check status:
   - Run `gh pr checks`.
   - If checks are pending, queued, in progress, or running, wait and poll
     again.
   - Default poll interval: 20 seconds.
   - Default timeout: 30 minutes unless the user specifies otherwise.

4. On failure:
   - Identify failed workflow runs and jobs.
   - Use `gh run view` and `gh run view --log` or job-specific logs when useful.
   - Save logs into the temp directory.
   - Extract the most relevant error snippets.
   - Report failed check name, workflow, job, conclusion, and likely cause.

5. Apply trivial local fixes when safe:
   - First run `git status --short` and keep track of pre-existing dirty files.
   - Only edit when the failure is narrow, local, and high-confidence, such as:
     formatting/lint errors with exact file locations, missing imports,
     typecheck errors with obvious symbol names, or a deterministic test failure
     caused by a small mistake in the PR's changed code.
   - Do not perform broad refactors, dependency changes, workflow edits, flaky
     test workarounds, or product behavior changes without asking first.
   - Do not overwrite or revert unrelated user changes.
   - After editing, run the smallest relevant local verification command if it
     is clear from the logs or repo conventions.
   - Stage only files changed by the fix with `git add <paths>`.
   - If a file was already dirty before the fix, do not stage it unless the
     entire file change is clearly part of this fix. Report the situation
     instead of risking staging unrelated work.
   - Never commit or push unless the user explicitly asks.
   - After staging, continue watching CI only if the user has already pushed the
     fix or explicitly asks you to push. Otherwise report the staged local fix
     and stop.

6. On success:
   - Report that all PR checks are green.
   - Include PR number, elapsed time, and final check summary.

## Constraints

- Stay read-only for remote services unless the user explicitly asks for a
  remote mutating action.
- Local code edits are allowed only for trivial, high-confidence CI fixes from
  the logs.
- Stage trivial local fixes with `git add <paths>` when safe.
- Do not push commits.
- Do not rerun, cancel, approve, merge, or mutate GitHub state unless the user
  explicitly asks.
- Prefer foreground monitoring. Do not start a background shell unless the user
  explicitly asks for background monitoring.
- If GitHub CLI auth is missing or `gh` fails, stop and report the blocker.
- If checks remain pending at timeout, report current status and log directory.

## Output Format

Return a concise final report:

- `Status`: green, failed, pending timeout, or blocked
- `PR`: number and URL if available
- `Elapsed`: approximate time watched
- `Summary`: one paragraph
- `Failed Checks`: only if failures exist
- `Local Fixes`: files edited and staged, only if local fixes were made
- `Logs`: local temp directory path
- `Next Step`: concrete recommendation
