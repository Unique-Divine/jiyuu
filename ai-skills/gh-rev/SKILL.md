---
name: gh-rev
description: Use gh-rev's persistent Markdown review ledger to review local branches or online pull requests, inspect prior revisions, work recorded findings, resolve findings with evidence, or continue an independent review pass. Trigger when the user names gh-rev, asks to save or submit a durable review, asks whether a branch or PR has reviews, wants existing review feedback implemented, or needs branch review history adopted into a PR. Pair with gh-pr-review for the substance and quality of a code review; do not use this skill for a generic one-off review when no durable ledger workflow is wanted.
---

# gh-rev

Use `gh-rev` as the durable workflow and identity layer around code review.
It records Markdown revisions outside source repositories, normally under
`~/gh`. Use `gh-pr-review` alongside it whenever judging code: that skill owns
review depth, categories, evidence, and the final recommendation. This skill
owns target selection, history, revision allocation, and finding lifecycle.

## Choose the workflow

Read only the reference matching the user's task:

- "Review my current branch with gh-rev; it is not a PR yet." Read
  [references/review-local-branch.md](references/review-local-branch.md).
- "Review the published PR for this branch" or "Review PR #1153." Read
  [references/review-pull-request.md](references/review-pull-request.md).
- "Read the revisions and fix their feedback," "Are there unresolved
  reviews?" or "Resolve the requested changes." Read
  [references/work-revisions.md](references/work-revisions.md).
- For another independent review pass, use the applicable local-branch or PR
  reference. A new pass receives a new revision; addressing an existing pass
  does not.

If a local branch is associated with a PR, select the authority that matches
the user's intent:

- Review what is published on GitHub with `--pr`.
- Review unpublished local commits with `--branch ... --head local` when that
  option is supported.

Do not silently substitute one for the other when their heads diverge.

## Start with capability and context

Run `command -v gh-rev` and inspect `gh-rev --help` before choosing commands.
The installed binary can lag the source documentation. Use `sync`, `--pr`, and
`--head local` only when help advertises them. Prefer the current command
contract when present; follow the compatibility behavior in the relevant
reference otherwise.

Before creating a new revision:

1. Read the repository's agent instructions and the issue, PR description, or
   user request that explains the intended change.
2. Use `gh-rev open ...` to select the review snapshot and read its returned
   `context_path` and every prior review path. `open` records a pending snapshot
   for `next`; do not use it merely to inspect or resolve existing findings.
3. Preserve useful human context. Use `gh-rev context ...` only when the user
   asks to create or update durable context.

For existing findings, use `gh-rev status` or `gh-rev status todo`; read all
`rev-*.md` files and `context.md` beneath the returned `target_path`.

## Ledger conventions

`gh-rev next ...` exclusively allocates the next `rev-N.md` template. Allocate
only after the review range and findings are ready to record, then fill the
generated template without replacing its YAML evidence.

Record each actionable finding on a line beginning in column zero:

```markdown
- [ ] rev: P1 — Explain the concrete problem, impact, location, and resolution.
```

Resolve that same line in its originating revision after verification:

```markdown
- [x] rev: P1 — Explain the concrete problem, impact, location, and resolution.
  - Resolved: Describe the fix or why no change was appropriate, with evidence.
```

The scanner recognizes only exact `- [ ] rev:` and `- [x] rev:` prefixes.
Findings have two states: unresolved or resolved. Do not invent stale,
awaiting-review, rejected, or similar intermediate states. A well-supported
disagreement may be resolved with a note explaining the decision.

Keep `## Scope`, `## Summary`, `## Findings`, and `## Recommendation`. An
approval still gets a complete revision, but has no placeholder finding and no
`rev:` checkbox. Use the recommendation vocabulary from `gh-pr-review`.

## Publishing boundary

Writing the revision submits it to the local ledger. Do not post a GitHub PR
comment or review unless the user explicitly asks. When publishing is asked
for, package the ledger findings into the requested GitHub surface without
changing which local findings are resolved.
