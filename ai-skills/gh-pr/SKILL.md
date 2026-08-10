---
name: gh-pr
description: Writes clear pull request descriptions from the current branch diff against a base branch. Defaults to `main`, but supports `dev` or any other user-specified comparison branch. Use when the user asks for a PR description, pull request summary, or a markdown write-up for changes against a base branch.
---

# GH PR

## Purpose

Create a clear, logical pull request description for the current branch diff
against a base branch. A good PR body explains the problem, the decisions that
shape the implementation, and the resulting behavior—not merely the changed
files.

## Instructions

When using this skill:

1. Determine the comparison branch first:
   - Use the branch the user specifies when they name one.
   - Otherwise, default to `main`.
   - If the correct base branch is unclear, ask the user before drafting the
     pull request description.
   - If the repo clearly uses another default integration branch, such as
     `dev`, use that instead of assuming `main`.
2. Read any linked issue, epic, journal, design, or acceptance criteria before
   drafting. Treat agreed decisions and accounting/event semantics in those
   documents as source material for the body.
3. Write a pull request description that explains the rationale behind the
   changes, not just the file-by-file edits. State what was broken or
   incomplete, why the chosen boundary or data contract is correct, and which
   existing semantics intentionally remain unchanged.
4. For changes with non-obvious behavior, cross-system effects, accounting,
   migrations, or several related fixes, include a dedicated `## Rationale`
   section and one or more descriptive sections that explain the relevant
   logic. Name these for the subject—for example `## Partial-close behavior`,
   `## Borrowing settlement`, or `## Historical compatibility`—rather than
   forcing an implementation-oriented heading.
5. For migrations, accounting changes, data-contract cutovers, or asynchronous
   authority/fallback behavior, lead the explanatory sections with a compact
   ASCII flow or field map. Use prose afterward only for the rationale the flow
   cannot convey. Do not use Mermaid.
6. Keep the writing concise and non-repetitive, but do not omit the reasoning
   that a reviewer needs to validate the design.
7. Save the final output to a markdown file named `gh-pr.md`.

## Output format

Use this structure:

```markdown
# <Short PR title>

<One short synopsis paragraph explaining the change and why it exists.>

- Closes <gh-issue-link> (If applicable, not all PRs pertain to issues)

## Rationale

<What was inconsistent, missing, or unsafe? Why is this event/data/model
boundary the right fix? Which related semantics deliberately do not change?>

## API cutover or behavior flow

```text
old source / behavior  -> replacement
authority source       -> fallback when unavailable
```

<Use a compact ASCII field map, lifecycle, or state flow when the change has
multiple data sources, transitions, or compatibility boundaries.>

## <Behavior or logic topic>

<Explain the important behavior, invariants, compatibility boundary, and
decision-making context. Add more topic-specific sections when the PR fixes
more than one distinct behavior.>

## Key Changes

1. <Important change with rationale>
1. <Important change with rationale>

```

## Style

- Prefer clarity over cleverness.
- Do not repeat the same point in multiple sections.
- Lead with the outcome, then give reviewers the rationale and logic they need
  to assess correctness.
- Explain non-obvious tradeoffs explicitly. Examples: a new field is canonical
  but a legacy fallback remains for historical data; a total intentionally
  excludes a separate accounting component; or an operation preserves a
  state-transition event while moving fee details to a normal receipt.
- Use numbered lists for the `## Key Changes` section, writing each item with
  `1.` so the markdown source is easy to reorder.
- Keep the description easy to scan with short paragraphs and direct headings.
  Prefer behavior-oriented sections over a chronological implementation diary;
  use as many sections as the distinct fixes need.
- For multi-source, lifecycle, migration, or fallback behavior, prefer compact
  ASCII diagrams and review invariants over prose-only explanations. Do not use
  Mermaid.
