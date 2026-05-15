---
name: writing-tech
description: Improve technical writing for specs, implementation notes, READMEs, engineering references, and agent-authored docs. Use when drafting, editing, reviewing, or polishing technical prose; when the user mentions technical writing, style, clarity, ambiguity, specs, docs, documentation, or Google developer docs; or when docs contain many code symbols, database names, API endpoints, GraphQL schema items, commands, or skills.
---

# Writing Tech

Use this skill to write and edit clear technical documentation for engineers and
agents. Prioritize project-specific clarity over generic style rules.

## Core stance

- Write for readers who navigate code, schemas, databases, APIs, commands,
  services, and operational systems.
- Make the entity type clear when referring to code-formatted symbols.
- Be concise, but don't let brevity create ambiguity.
- Treat these as guidelines, not rigid laws. Depart from them when doing so
  improves the document.

## Qualify code symbols with entity type

When referencing a named code, data, or tool symbol in prose, include a nominal
qualifier that identifies what kind of entity it is.

Backticks identify the exact symbol. The surrounding noun tells the reader what
the symbol is.

Prefer:

- table `stats_perp_by_user`
- database column `market_id`
- GraphQL field `volume`
- GraphQL type `PerpBorrowing`
- function `Delegation`
- agent skill `nibiru-cli-nibid`
- command `just fmt`
- REST endpoint `GET /dexpal/v1/stats`
- file `graphql/graph/perp.graphqls`
- service `StatsRefresher`

Avoid bare references when the entity type is not obvious:

- `stats_perp_by_user` refreshes
- `Delegation` handles this
- use `nibiru-cli-nibid`
- run `just fmt`

Better:

- table `stats_perp_by_user` refreshes
- function `Delegation` handles this
- use agent skill `nibiru-cli-nibid`
- run command `just fmt`

## How strict to be

- Prefer qualifiers on first use in a section, like defining a term or spelling
  out an abbreviation before using it freely.
- In specs, lean toward qualifying symbols more often than not.
- Relax the rule when repeated qualifiers make a paragraph noisy and the entity
  type is already clear.
- Use judgment. The goal is unambiguous technical prose, not mechanical
  repetition.
- Abbreviations are fine in drafts and working notes: `proc`, `fn`, `func`,
  `db column`, and `GQL field` are acceptable when they improve flow.
- If the user asks for a polished style or says not to use abbreviations, prefer
  fuller labels like SQL procedure, function, database column, and GraphQL field.

## Formatting conventions

- Use code font for exact code-related text: filenames, paths, commands,
  symbols, class names, method names, database objects, GraphQL objects, API
  fields, config keys, HTTP methods, and status codes.
- Use bold for UI elements and run-in labels, not as a substitute for code font.
- Use sentence case for headings.
- Use standard American spelling and punctuation.
- Use serial commas.
- Avoid underlining except for links.

## Prose conventions

- Prefer active voice. Make clear who or what performs the action.
- Put conditions, circumstances, or goals before instructions when that helps
  readers skip irrelevant content.
- Use present tense for current behavior.
- Avoid future-tense promises unless distinguishing a future action is necessary.
- Avoid time-sensitive words like now, new, currently, soon, and as of this
  writing in durable docs.
- Avoid excessive claims: best, simplest, fastest, never, always, ensure, and
  guarantee. Prefer verifiable language.
- Define domain jargon on first use when the reader might not already know it.
- Avoid anthropomorphism when literal wording is clearer.

## Editing checklist

When editing technical docs:

1. Identify ambiguous bare code symbols.
2. Add entity-type qualifiers where they improve clarity.
3. Check whether the first use in each section is clear enough for a reader who
   knows the domain but not this exact codepath.
4. Remove repeated qualifiers when they become noisy.
5. Verify that exact symbols use code font.
6. Tighten sentences that rely on vague pronouns such as it, this, that, or they.
7. Keep examples concrete and domain-specific.

## Reference

Long-form source reference:
`/home/realu/ki/boku/epics/writing-tech/SKILL.md`

Additional style resources:
`resources.md`
