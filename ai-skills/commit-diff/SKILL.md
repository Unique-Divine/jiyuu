---
name: commit-diff
description: >-
  Generate a clear, cogent, concise, and conventional-like commit message from
  the relevant git diff. Default is staged changes (what will be committed). Use
  when the user asks for a commit message, says "save/commit", or asks to
  summarize staged, working tree, or last commit changes.
---

# Commit Diff

## When to Apply

Use this skill when the user wants a commit message written from their `git diff`, or when they ask for a commit message for their changes without specifying
one.

## Instructions

### 0) Confirm what the user wants to summarize

Default to **what will be committed** (staged changes). If the user's phrasing is ambiguous, pick the most likely intent from the table below.

| User intent (trigger phrases) | Use this command | Notes |
|---|---|---|
| "commit message", "save", "ready to commit", "staged", "what I'm about to commit" (default) | `git diff --staged` | Exactly what `git commit` would include right now. |
| "latest changes", "since last commit", "working tree", "everything I changed" | `git diff HEAD` | Includes staged + unstaged changes vs last commit. |
| "what's in my last commit", "latest commit", "changes in commit" | `git show HEAD` | Shows the patch for the last commit (includes new files). |
| "compare to main", "PR diff", "branch delta", "since branching" | `git diff <base>...HEAD` | Merge-base diff (commits on this branch). If `<base>` not given, assume `main` but ask once to confirm. |

If `git diff --staged` is empty, check `git status`. If there are only unstaged changes, either ask the user to stage what they want committed or switch to `git diff HEAD` to summarize the whole working tree.

### 1) Untracked files gotcha (new files)

Diffs only include **tracked** content.

- If `git status` shows `?? some-file`, that file will not appear in `git diff --staged` or `git diff HEAD` until you stage it.
- If you want a diff that includes new files without fully staging them, use `git add -N <file>` (intent-to-add), then rerun the chosen diff.

### 2) Write the commit message (Conventional Commits)

Output **one subject line**, plus an optional body when it helps explain "why".

Subject format:

`type(scope): Imperative summary`

- **type**: prefer one of `feat`, `fix`, `docs`, `refactor`, `test`, `chore`, `build`, `ci`
- **scope**: Optional, short (e.g. `sai`, `indexer`, `scripts`, `docs`)
- **imperative** (subject): "Add", "Fix", "Remove", "Refactor" (not past tense)
- **length**: Keep subject <= 80 chars. No trailing period
- **breaking changes**: Add `!` after type/scope when clearly breaking (`feat!: …`, `feat(api)!: …`)

Body (optional):

- Blank line after subject
- 1–3 bullets focusing on **why** and any non-obvious behavior changes
- Be clear, cogent, concise.

### Examples

- Break "conventional commit" pattern slightly to use this preferred format,
where the summary is capitalized.

```
feat(sai): Add vault OI query helpers
```

```
fix(indexer): Query staking data via staking container

- Avoid deprecated root-level fields
- Clarify amount units (unibi) in output
```

```
chore(scripts): Align tx query output format
```
