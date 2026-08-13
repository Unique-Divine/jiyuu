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
4. Put reviewer context in this order: the PR title and synopsis, `## Rationale`,
   `## Key Changes`, then any additional behavior-oriented sections. Keep
   `Rationale` and `Key Changes` unnumbered. Number only the additional topic
   headings as `## 1 - ...`, `## 2 - ...`, and so forth. The numbering improves
   scanning across distinct topics; it does not imply implementation order or
   runtime sequence.
5. For changes with non-obvious behavior, cross-system effects, accounting,
   migrations, or several related fixes, use the additional sections to explain
   the relevant logic. Name them for the subject—for example
   `## 1 - Partial-close behavior`, `## 2 - Borrowing settlement`, or
   `## 3 - Historical compatibility`—rather than using implementation-oriented
   headings.
6. For migrations, accounting changes, data-contract cutovers, or asynchronous
   authority/fallback behavior, lead the relevant additional section with a
   compact ASCII flow or field map. Use prose afterward only for the rationale
   the flow cannot convey. Do not use Mermaid.
7. Do not add a dedicated `Validation`, `Testing`, or similarly named section,
   and do not enumerate routine test, lint, build, formatting, or check runs.
   Mention test strategy or coverage only when it materially changes or creates
   a reviewer-relevant risk; place that context in the most relevant
   behavior-oriented section.
8. Keep the writing concise and non-repetitive, but do not omit the reasoning
   that a reviewer needs to validate the design.
9. Save the final output to a markdown file named `gh-pr.md`.

## Output format

Use this structure:

````markdown
# <Short PR title>

<One short synopsis paragraph explaining the change and why it exists.>

- Closes <gh-issue-link> (If applicable, not all PRs pertain to issues)

## Rationale

<What was inconsistent, missing, or unsafe? Why is this event/data/model
boundary the right fix? Which related semantics deliberately do not change?>

## Key Changes

1. <Important change with rationale>
1. <Important change with rationale>

## 1 - <Behavior or logic topic>

```text
old source / behavior  -> replacement
authority source       -> fallback when unavailable
```

<Explain the important behavior, invariants, compatibility boundary, and
decision-making context. When useful, lead with a compact ASCII field map,
lifecycle, or state flow.>

## 2 - <Another behavior or logic topic>

<Add numbered topic-specific sections only when the PR has more than one
distinct behavior reviewers need to understand. The numbers aid scanning; they
do not describe implementation or runtime sequence.>

````

## Style

- Prefer clarity over cleverness.
- Do not repeat the same point in multiple sections.
- Lead with the outcome, then give reviewers the rationale and logic they need
  to assess correctness.
- Explain non-obvious tradeoffs explicitly. Examples: a new field is canonical
  but a legacy fallback remains for historical data; a total intentionally
  excludes a separate accounting component; or an operation preserves a
  state-transition event while moving fee details to a normal receipt.
- Always place `## Key Changes` immediately after `## Rationale`.
- Use numbered lists for the `## Key Changes` section, writing each item with
  `1.` so the markdown source is easy to reorder.
- Keep the description easy to scan with short paragraphs and direct headings.
  Prefer behavior-oriented sections over a chronological implementation diary.
  Keep `Rationale` and `Key Changes` unnumbered, and number only subsequent
  topic headings as `## 1 - ...`, `## 2 - ...`, and so forth. This numbering is
  for scanning, not sequencing; use only as many sections as the distinct fixes
  need.
- For multi-source, lifecycle, migration, or fallback behavior, prefer compact
  ASCII diagrams and review invariants over prose-only explanations. Do not use
  Mermaid.
- Exclude routine validation logs. Do not create dedicated validation or
  testing sections, and mention test strategy or coverage only when it is a
  meaningful change or reviewer risk.
