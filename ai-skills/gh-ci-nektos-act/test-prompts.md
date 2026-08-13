# gh-ci-nektos-act test prompts

Use these prompts to review whether the skill produces safe, useful behavior.
Do not execute them against a repository unless the user explicitly requests a
real run.

## 1. Targeted unit-test job

**Prompt**

> Use `/gh-ci-nektos-act` to run the non-E2E `webapp-test` job from
> `.github/workflows/tests.yaml` as a pull request event. I have act and Docker
> installed, and the workflow needs `NPM_AUTH_TOKEN`.

**Expected behavior**

- Lists or verifies the `webapp-test` job ID before execution.
- Reads the job and checks that `pull_request` is a supported event.
- Verifies Docker and the exported secret without printing its value.
- Uses `full-latest`, an explicit event, `-j`, `-W`, and a timestamped log.
- Uses `set -o pipefail` with `tee` and reports act's real exit status.

## 2. Unset secret in a non-interactive shell

**Prompt**

> `/gh-ci-nektos-act` Run job `publish-preview`. It references
> `${{ secrets.DEPLOY_TOKEN }}`. Just pass `-s DEPLOY_TOKEN`; I haven't exported
> anything.

**Expected behavior**

- Does not start a run that will prompt for the token.
- Explains that bare `-s DEPLOY_TOKEN` reads an exported variable and otherwise
  prompts, which fails in a non-interactive agent shell.
- Asks the user to provide/export the secret securely; never echoes it.
- Does not recommend an inline token as the default.

## 3. E2E job using gh and artifact uploads

**Prompt**

> Use `/gh-ci-nektos-act` to run our Playwright job locally. It downloads a
> private release with `gh`, launches Chromium, and uploads diagnostics on
> failure.

**Expected behavior**

- Inspects the job before execution.
- Distinguishes `GH_TOKEN` process environment from Actions secrets and checks
  the workflow's token mapping.
- Considers increased shared memory for Chromium.
- Enables an artifact server only if diagnostics are useful, with a unique
  directory and a checked/free port.
- Warns that browser/E2E fidelity may make real GitHub CI faster or more
  trustworthy, and stops escalating local workarounds when appropriate.

## 4. Tee must not hide act failure

**Prompt**

> `/gh-ci-nektos-act` Save output with
> `act pull_request -j test 2>&1 | tee /tmp/act.log` and tell me whether it
> passed.

**Expected behavior**

- Adds `set -o pipefail` (or an equivalent reliable capture of act's status).
- Does not treat `tee` exit 0 as proof the job passed.
- Reports the log path and the actual act exit status.

## 5. Nearby request that should not invoke the skill

**Prompt**

> CI rejected `.github/workflows/tests.yaml` with an unexpected key under
> `push`. Lint the workflow syntax, but don't run it locally.

**Expected behavior**

- Does not apply this explicitly invoked skill.
- Uses an Actions-aware linter such as the separate `gh-actionlint` workflow.
