---
name: gh-ci-nektos-act
description: Run selected GitHub Actions workflows and jobs locally with nektos/act, including Docker preflight, event and job discovery, runner-image selection, secrets, durable logs, and failure triage. Use only when explicitly invoked to test CI locally with act.
compatibility: Requires nektos/act and a working Docker daemon; GitHub CLI is optional for jobs that use gh.
disable-model-invocation: true
---

# GitHub CI with nektos/act

Run the smallest relevant GitHub Actions job against the current local
workspace. Preserve logs and distinguish failures in the code from differences
between `act` and GitHub-hosted runners.

## Command shape

```text
act [<event>] [options]
```

With no event, `act` defaults to `push`. Prefer an explicit event because
workflow eligibility and expression values depend on it.

Useful discovery commands:

```bash
act --help
act -l
act workflow_dispatch -l
act -l -W .github/workflows/tests.yaml
act pull_request -l -W .github/workflows/tests.yaml
```

Useful execution commands:

```bash
act                         # default push event
act pull_request            # all eligible pull_request jobs
act pull_request -j test    # one job ID
act pull_request -j test -n # dry run
act pull_request -j test -v # verbose act diagnostics
```

`-j` takes the job ID under `jobs:`, not its display `name`.

## Workflow

### 1. Preflight

Work from the repository root.

1. Check `act --version` and `act --help`.
2. Confirm Docker is usable with `docker info`.
   - If Docker is unavailable, report that `act` requires a Docker daemon.
   - Do not silently install Docker or start a desktop application.
3. Check for an existing run before launching another one. Duplicate runs can
   waste resources and conflict on cache or artifact-server ports.
4. Identify the workflow, event, and exact job ID with `act -l`, an
   event-specific list command, and `-W` when the workflow is known.
5. Read the selected job before running it. Record:
   - supported triggers and any event-dependent conditions;
   - `runs-on`, matrix values, services, and container settings;
   - `${{ secrets.* }}`, `${{ vars.* }}`, and required environment variables;
   - commands that use `gh`, private registries, cloud credentials, or tokens;
   - artifact upload/download steps;
   - browser, architecture, or hosted-runner assumptions.

Use `actionlint` separately when validating workflow syntax. `act -n` is useful
for checking selection and expansion, but it is not a pure static check: it may
still require Docker, images, and action downloads.

### 2. Choose the runner image

For `runs-on: ubuntu-latest`, default to:

```bash
-P ubuntu-latest=ghcr.io/catthehacker/ubuntu:full-latest
```

The full image is much closer to GitHub's preinstalled toolset than the slim
`act-latest` image. It fixed cases where `actions/setup-node` with
`cache: yarn` expected Yarn to already be on `PATH`. It is still an
approximation, not GitHub's exact hosted image.

The image is large (tens of GB). Warn before the first pull when disk or network
cost matters. Once it is present, `--pull=false` avoids pulling it on every run.

Docker normally selects the native image manifest. When architecture matters,
compare `uname -m` / Docker server architecture with the image manifest. Do not
force `linux/amd64` on ARM unless the job truly needs amd64 and the user accepts
emulation cost.

### 3. Supply secrets and environment safely

Never print token values.

`-s` / `--secret` supplies an Actions secret:

```bash
export NPM_AUTH_TOKEN="..."  # obtain securely; do not echo
act ... -s NPM_AUTH_TOKEN
```

Before using bare `-s NAME`, verify `NAME` is exported and non-empty:

```bash
: "${NPM_AUTH_TOKEN:?export NPM_AUTH_TOKEN before running act}"
```

If it is unset, `act -s NAME` prompts for a value. That fails in a
non-interactive agent shell with an `inappropriate ioctl` error.

Supported but less safe forms:

```bash
act ... -s NPM_AUTH_TOKEN=ghp_xxx
act ... --secret-file .secrets
```

Avoid inline values because shell history and process inspection can expose
them. If using `.secrets`, keep it untracked, restrict its permissions, and
never show its contents in logs.

Secrets and ordinary environment variables are different:

- Use `-s NAME` for `${{ secrets.NAME }}`.
- Use `--env NAME` for a process environment variable read by a command.
- A `GITHUB_TOKEN` secret is not the same as an exported `GH_TOKEN`. If a step
  directly runs `gh`, export `GH_TOKEN` and pass `--env GH_TOKEN` unless the
  workflow already maps the token into that environment variable.
- Supply `-s GITHUB_TOKEN` too only when the workflow reads
  `${{ secrets.GITHUB_TOKEN }}`.

### 4. Preserve logs without hiding failures

Always save combined stdout/stderr with `tee`, and enable `pipefail`. Without
`pipefail`, the shell reports `tee`'s success even when `act` failed. This can
turn a fatal startup error into a misleading exit status 0.

Canonical pattern:

```bash
set -o pipefail

event=pull_request
job=webapp-test
workflow=.github/workflows/tests.yaml
log="${TMPDIR:-/tmp}/act-${job}-$(date -u +%Y%m%dT%H%M%SZ).log"

: "${NPM_AUTH_TOKEN:?export NPM_AUTH_TOKEN before running act}"

act "$event" \
  -j "$job" \
  -W "$workflow" \
  -s NPM_AUTH_TOKEN \
  -P ubuntu-latest=ghcr.io/catthehacker/ubuntu:full-latest \
  --pull=false \
  --rm \
  2>&1 | tee "$log"
status=$?

printf 'act exit=%s log=%s\n' "$status" "$log"
exit "$status"
```

Do not add `--pull=false` until the image exists locally.

### 5. Add optional features only when needed

- `--container-options "--shm-size=2g"` can help Chromium-based jobs.
- Enable the artifact server only when artifact actions matter. Give each run a
  unique artifact directory and ensure its port is unused; a stale server on
  the default port can prevent `act` from starting.
- Use `-e event.json` when the workflow needs realistic pull request fields,
  changed refs, labels, inputs, or other webhook payload data. The event name
  alone does not reproduce a full GitHub payload.
- Remember that `act` runs the local workspace, including relevant uncommitted
  changes. State that explicitly in the result.

## Failure triage

After a run:

1. Trust the `act` exit code captured with `pipefail`.
2. Identify the first failed workflow step and quote the smallest useful log
   excerpt.
3. Classify the failure:
   - **code/test failure**: the job reached its intended command and that command
     failed;
   - **workflow/config failure**: expressions, job selection, or missing inputs;
   - **local runner mismatch**: missing preinstalled tool, architecture issue,
     unsupported service, absent GitHub token context, or image limitation;
   - **infrastructure failure**: Docker, image pull, network, disk, cache, or
     artifact-server conflict.
4. Fix application code only when the user requested implementation. Do not
   mutate the workflow merely to accommodate `act` without discussing whether
   the same change is desirable in real CI.

Prefer real GitHub CI when browser/E2E setup, hosted-only services, privileged
features, architecture-sensitive native dependencies, or repeated fidelity
workarounds make the local run slower or less trustworthy than the hosted job.

## Report

Keep the result concise:

- event, workflow path, and job ID;
- runner image and local architecture when relevant;
- exact command shape with secret values omitted;
- exit status and first failed step, or success summary;
- log path and artifact path if enabled;
- whether the result reflects code behavior or an `act`/hosted-runner mismatch.
