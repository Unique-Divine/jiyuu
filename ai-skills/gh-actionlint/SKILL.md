---
name: gh-actionlint
description: Validate GitHub Actions workflow YAML with actionlint when editing files under `.github/workflows/`, checking CI workflow syntax, diagnosing GitHub Actions expression or schema errors, or when the user asks to lint/check Actions workflows. Prefer `bunx github-actionlint` when `bun` is available and fall back to `npx --yes github-actionlint`; use this proactively after modifying GitHub Actions workflows.
---

# GH Actionlint

## Purpose

Use `actionlint` to catch GitHub Actions workflow mistakes before CI does:
YAML syntax errors, invalid workflow keys, expression mistakes, bad `needs`
references, invalid triggers, and other common Actions-specific issues.

Generic YAML linters are useful for indentation, but they do not understand
GitHub Actions semantics. Prefer this skill whenever the file is a GitHub
Actions workflow.

## When To Use

Use this skill when:

- The user asks to check, lint, validate, or debug GitHub Actions workflows.
- You modify files under `.github/workflows/`.
- CI fails with workflow parsing, expression, trigger, `needs`, matrix, or
  Actions schema errors.
- The user asks whether a workflow step shape is valid, such as an unnamed
  `run:` step.

## Workflow

1. Work from the repository root when possible.
2. Identify the workflow targets:
   - If the user named specific files, check those files.
   - Otherwise, check `.github/workflows/`.
3. Choose the runner:
   - If `bun` is available, use `bunx github-actionlint`.
   - Otherwise, if `npx` is available, use `npx --yes github-actionlint`.
   - If neither exists, tell the user that `bun` or `npx` is needed.
4. Run actionlint.
5. Report whether the workflows passed. If there are failures, quote the
   relevant actionlint messages and suggest the smallest fix.

## Commands

Check all workflows:

```bash
if command -v bun >/dev/null 2>&1; then
  bunx github-actionlint .github/workflows/
elif command -v npx >/dev/null 2>&1; then
  npx --yes github-actionlint .github/workflows/
else
  echo "need bun or npx on PATH" >&2
  exit 1
fi
```

Check specific workflow files:

```bash
if command -v bun >/dev/null 2>&1; then
  bunx github-actionlint .github/workflows/tests.yaml
elif command -v npx >/dev/null 2>&1; then
  npx --yes github-actionlint .github/workflows/tests.yaml
else
  echo "need bun or npx on PATH" >&2
  exit 1
fi
```

Use `-color -verbose` when extra diagnostics are helpful:

```bash
bunx github-actionlint -color -verbose .github/workflows/
```

## Reporting Style

Keep the result concise:

- Say which command ran.
- Say whether actionlint passed.
- For failures, include file path, line/column if provided, and the exact rule
  message.
- Distinguish Actions validation errors from unrelated shellcheck warnings when
  actionlint reports both.
- Avoid adding `github-actionlint` to `package.json` unless the user explicitly
  wants it pinned as a project dependency.

## Test Prompts

Use these prompts to sanity-check the skill behavior:

- "I changed `.github/workflows/tests.yaml`; can you check if the workflow YAML
  is valid?"
- "While you're editing the GitHub Action, please run the action linter before
  you finish."
- "CI says my workflow has an unexpected key under `push`; can you diagnose the
  Actions YAML?"
