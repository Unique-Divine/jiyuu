# LEXICON.md Format

## Template

```md
# Lexicon: Ordering

Ordering receives customer orders and tracks them until delivery.

- Parent: [Shop](../LEXICON.md)
- Children: [Returns](returns/LEXICON.md)
- Related: [Billing](../billing/LEXICON.md): shares **Customer**; Billing
  invoices the Orders that Ordering marks delivered.

## Terms

**Order**:
A customer's request to buy one or more items.
_Avoid_: purchase, transaction

**Delivered**:
The carrier confirmed handover to the customer.
_Avoid_: shipped, completed
_See_: [delivery states](docs/delivery.md)

## Never Say

- "account" for a person: say **Customer** or **User**; they differ.
```

## Parts

- **Title and description.** `# Lexicon: <Context Name>`, then one or two
  sentences on what this part of the project is.
- **Links.** Up to three lines, each present only when it has a target:
  - `Parent`: the nearest lexicon in an ancestor directory, however many
    levels up.
  - `Children`: lexicons in subdirectories.
  - `Related`: lexicons of other contexts, each with the shared term and one
    line on how the two contexts relate.
- **Terms.** One entry per concept:
  - `**Term**:` then one or two sentences saying what the thing is.
  - `_Avoid_:` the other words people use for it. Optional.
  - `_See_:` links to the specs, ADRs, or docs that explain or decide it,
    each with a few words on what it holds. Optional.
  - `_Overrides_:` a link to the parent lexicon, when this context gives a
    parent's term a different meaning. Optional and rare.
- **Never Say.** Words ruled out that have no single replacement, each with
  what to say and, where it helps, why.

## Rules

- **One word per idea.** Pick the best word; list the rest under `_Avoid_`.
- **Define what it is, not how it works.** One or two sentences.
- **Project terms only.** A general programming word belongs only if the
  project gives it a specific meaning.
- **No implementation detail.** No line numbers, signatures, or algorithms.
  Put those behind a `_See_` link.
- **Group when clusters appear.** Use `###` subheadings under `## Terms` once
  a file passes about ten terms. A short file stays flat.
- **Sort for the reader.** Order terms so each builds on the ones above it;
  alphabetical order is a fallback.
- **Keep it short.** A lexicon is read at the start of many tasks. Remove a
  term that no longer causes confusion and has no synonym to rule out.

## Scope and Links

- A `LEXICON.md` applies to its directory and below. A child inherits every
  ancestor term and repeats none, except a term it marks `_Overrides_`.
- The same word may mean different things in two contexts. State the
  difference in `Related` on both sides.
- Inside a repository, link with relative paths. Across repositories, link
  with the hosting URL so the link works in any checkout.
- A public repository's lexicon links only to public material.

## Where Files Go

- One at the repository root when the project has one shared language.
- One in a subdirectory when that part has words of its own, such as a tool
  in a monorepo or a module with a distinct domain.
- Create each lazily, when its first term is settled. One exception: a file
  may exist with links and no terms when it is the parent that ties child
  lexicons together.
