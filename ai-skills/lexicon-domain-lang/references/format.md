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

### Order

A customer's request to buy one or more items.
_See_ Rust struct `src/orders.rs::Order`.

### Delivered

The carrier confirmed handover to the customer.
_Avoid_ Shipped, completed

_See_ [Delivery states](docs/delivery.md)

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
- **Terms.** Give each canonical concept its own heading under `## Terms`.
  Follow it with one or two sentences defining the concept.
  - `_Also called_` lists genuine aliases. Optional.
  - `_Avoid_` lists familiar wording the project discourages. Optional.
  - `_See_` links to specs, ADRs, docs, or source symbols. For source
    references that resolve in the code graph, see [Graph Links](graph.md).
  - `_Overrides_:` a link to the parent lexicon, when this context gives a
    parent's term a different meaning. Optional and rare.
- **Never Say.** Words ruled out that have no single replacement, each with
  what to say and, where it helps, why.

## Rules

- **One heading per idea.** Put aliases in the entry body. Give two distinct
  concepts separate headings even when one is a kind of the other.
- **Define what it is, not how it works.** One or two sentences.
- **Project terms only.** A general programming word belongs only if the
  project gives it a specific meaning.
- **No implementation detail.** No line numbers, signatures, or algorithms.
  Put those behind a `_See_` link.
- **Use H3 for terms in a flat lexicon.** If groups help readers scan a longer
  lexicon, use H3 for groups and H4 for individual terms.
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
