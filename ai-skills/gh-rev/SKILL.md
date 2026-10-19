---
name: gh-rev
description: Use the gh-rev CLI to maintain a persistent Markdown review ledger for local branches and pull requests. Trigger when the user names gh-rev, asks to save or submit a durable review, inspect or implement existing findings, continue an independent review pass, or adopt branch history into a PR. Use the gh-pr-review skill for review content and judgment. Do not use this skill for a one-off review without a durable ledger.
---

# gh-rev

Command `gh-rev` is a CLI tool that records Markdown review revisions outside
source repositories, normally under `~/gh`. This skill explains how to use that
tool to select targets, read history, allocate revisions, and resolve findings.

## Compose with the gh-pr-review skill

Read and use the `gh-pr-review` skill whenever evaluating code or filling in a
review. It supplies the review criteria, investigation standards, evidence,
actionable findings, and recommendation. Apply it to the exact review range
selected by the CLI.

Record that review in the template allocated by `gh-rev next`. Keep the ledger
sections and exact finding markers described below; use the `gh-pr-review`
skill to determine their content rather than copying its standalone output
format over the ledger template. The CLI manages review records; it does not
perform the code review.

## Choose the workflow

Read [references/cli.md](references/cli.md) for CLI discovery, installation,
command purposes, and target selection. Then read the task reference:

- "Review my current branch with gh-rev; it is not a PR yet." Read
  [references/review-local-branch.md](references/review-local-branch.md).
- "Review the published PR for this branch" or "Review PR #1153." Read
  [references/review-pull-request.md](references/review-pull-request.md).
- "Are there unresolved reviews?", "What feedback remains?", or "Resolve this
  finding after verification." Read
  [references/work-revisions.md](references/work-revisions.md).
- "Implement the submitted reviews," "Address the review feedback," or
  "Automatically fix clear findings while implementing this slice." Read
  [references/implement-review-feedback.md](references/implement-review-feedback.md).
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

Use the installed CLI's help as the command contract. If it lacks a required
capability, report that limit rather than substituting a different review
target. See the CLI reference for discovery and update commands.

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
- [ ] rev: P1: Explain the concrete problem, impact, location, and resolution.
```

Resolve that same line in its originating revision after verification:

```markdown
- [x] rev: P1: Explain the concrete problem, impact, location, and resolution.
  - Resolved: Describe the fix or why no change was appropriate, with evidence.
```

The scanner recognizes only exact `- [ ] rev:` and `- [x] rev:` prefixes.
Findings have two states: unresolved or resolved. Do not invent stale,
awaiting-review, rejected, or similar intermediate states. A well-supported
disagreement may be resolved with a note explaining the decision.

Keep `## Scope`, `## Summary`, `## Findings`, and `## Recommendation`. An
approval still gets a complete revision, but has no placeholder finding and no
`rev:` checkbox. Use the recommendation vocabulary from the `gh-pr-review`
skill.

## Publishing boundary

Writing the revision submits it to the local ledger. Do not post a GitHub PR
comment or review unless the user explicitly asks. When publishing is asked
for, package the ledger findings into the requested GitHub surface without
changing which local findings are resolved.
