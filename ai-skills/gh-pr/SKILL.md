---
name: gh-pr
description: Writes clear pull request descriptions from the current branch diff against a base branch. Defaults to `main`, but supports `dev` or any other user-specified comparison branch. Use when the user asks for a PR description, pull request summary, or a markdown write-up for changes against a base branch.
---

# GH PR

## Purpose

Create a clear, logical, and concise pull request description for the current
branch diff against a base branch.

## Instructions

When using this skill:

1. Determine the comparison branch first:
   - Use the branch the user specifies when they name one.
   - Otherwise, default to `main`.
   - If the correct base branch is unclear, ask the user before drafting the
     pull request description.
   - If the repo clearly uses another default integration branch, such as
     `dev`, use that instead of assuming `main`.
2. Write a pull request description that explains the rationale behind the
   changes, not just the file-by-file edits.
3. Keep the writing concise and non-repetitive.
4. Save the final output to a markdown file named `pull-request.md`.

## Output format

Use this structure:

```markdown
# <Short PR title>

<One short synopsis paragraph explaining the change and why it exists.>

## Key Changes

- <Important change with rationale>
- <Important change with rationale>

## Appendix

<Optional extra context, notes, follow-ups, or test details. Omit this section
if it does not add value.>
```

## Style

- Prefer clarity over cleverness.
- Do not repeat the same point in multiple sections.
- Explain rationale wherever it is useful.
- Keep the description easy to scan.
