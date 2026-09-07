# Sync durable branch reviews with pull requests

This change makes `gh-rev` treat a branch and its published pull request as one durable review history, with explicit revision authority and crash-safe ledger adoption.

- Closes https://github.com/Unique-Divine/jiyuu/issues/30
- Closes https://github.com/Unique-Divine/jiyuu/issues/31
- Advances https://github.com/Unique-Divine/jiyuu/issues/28

## Rationale

The prototype exposed `--pr` selectors without a supported discovery path, could resolve ambiguous Git names, and allowed `open` and `next` to describe different review heads. Branch and PR records also owned separate mutable target state, so publishing a branch could split its review history.

The ledger now keeps Markdown revisions authoritative while using `state.toml` as a recoverable index. Branch selectors intentionally use exact local refs; PR selectors use GitHub’s immutable head SHA. Adoption joins both selectors to one directory and one allocation sequence without weakening those authority boundaries.

## Key Changes

1. Add targeted PR discovery and bulk `sync`, with exact repository/branch matching and branch-to-PR aliases.
1. Merge existing branch and PR histories deterministically while preserving review evidence, context, and attachments.
1. Persist `open` snapshots so `next` rejects moved or authority-changing review ranges.
1. Keep GitHub network access outside the ledger lock and recover recognized interrupted adoptions without guessing at ambiguous artifacts.
1. Add the durable `gh-rev` agent workflow and retire copy-based public-skill synchronization in favor of repository-backed runtime links.

## 1 - Review identity and authority

```text
branch selector -> refs/heads/<branch> commit -> local review authority
PR selector     -> GitHub PR head SHA        -> published review authority
branch + PR     -> one pr-<number> ledger    -> one rev-N sequence
```

Exact ref resolution prevents a same-named tag from becoming the review head. Once a PR is adopted, branch and PR status filters resolve the same findings, while diagnostics retain local, remote-tracking, and GitHub head SHAs. A diverged local branch requires explicit `--head local`; PR mode never silently includes unpublished commits.

## 2 - Adoption and interrupted operations

```text
snapshot identity under lock
  -> fetch GitHub metadata without lock
  -> reacquire and reread current state
  -> stage branch/PR history merge
  -> atomically persist state
  -> remove scoped backups
```

Network latency no longer blocks other ledger writers. Lock acquisition is bounded and reports active contention rather than bypassing serialization.

If adoption is interrupted, the next synchronization uses persisted state and guarded backup shapes to discard abandoned staging, roll back an unpersisted replacement, or finish cleanup after persistence. Unknown, malformed, or ambiguous artifacts are preserved and reported. Recovery remains idempotent across crashes during recovery itself.

## 3 - Durable review workflow

The new `gh-rev` skill separates review quality from ledger mechanics: `gh-pr-review` owns findings and recommendations, while `gh-rev` owns target selection, revision allocation, and the exact `- [ ] rev:` / `- [x] rev:` lifecycle.

Dedicated implementation guidance treats a reviewer's concern as evidence to verify and its proposed solution as non-authoritative. It defines user-controlled modes, safe automatic-fix boundaries, and an independent read-only verification pass for non-obvious changes without turning that guardrail into a durable review submission unless requested.

Public skills now live directly under `jiyuu/ai-skills`, with private skills supplying the runtime union. The obsolete push, pull, and diff copy scripts are removed so runtime edits no longer drift from their repository source.
