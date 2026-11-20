---
name: lexicon-domain-lang
description: Keep a project's domain language consistent with LEXICON.md files (the lexicon skill). Use whenever you write about a project or name one of its concepts, and when the user asks what to call something, debates terminology, uses a vague or overloaded word, or mentions a lexicon, glossary, domain language, or ubiquitous language. Covers reading the lexicons that apply to a path, using their terms, challenging conflicting words, and recording a term when it is settled.
---

# Lexicon: Domain Language

A project works better when the user, the agents, the docs, and the code use
the same word for the same idea. A `LEXICON.md` file is that agreement,
written down: what to call things in one part of a project, and what not to
call them. This is the "ubiquitous language" of domain-driven design.

Read [format.md](references/format.md) before creating or editing a
`LEXICON.md`.

## Which Lexicons Apply

A `LEXICON.md` covers its directory and everything below it, like
`AGENTS.md`.

1. Read every `LEXICON.md` from the repository root down to the directory you
   are working in. A child adds to its parents; where it marks a term
   `_Overrides_`, the child wins.
2. When the task crosses into another context, follow the `Related` links at
   the top of the file and read those too.
3. If no `LEXICON.md` exists, carry on. Don't announce its absence or create
   an empty one.

```sh
# Lexicons that apply to the current directory, outermost first.
d=$PWD; root=$(git rev-parse --show-toplevel)
while :; do [ -f "$d/LEXICON.md" ] && echo "$d/LEXICON.md"
  [ "$d" = "$root" ] && break; d=$(dirname "$d"); done | tac
```

## Using the Language

- Name concepts with the lexicon's terms everywhere: replies, docs, commit
  messages, issues, test names, and new identifiers in code.
- Never use a word a lexicon lists under `_Avoid_` or `Never Say`. If the
  user uses one, answer in the lexicon's term and say which word you mean.
- If you need a concept the lexicon lacks, either the project doesn't think in
  those terms (reconsider the wording) or there is a gap (raise it, below).

## Sharpening the Language

Do this while designing, drilling a spec, or reviewing, whenever wording
causes friction.

- **Challenge a conflict.** "The lexicon defines *full rebuild* as re-reading
  every file. You seem to mean index depth `full`. Which is it?"
- **Split an overloaded word.** When one word covers two ideas, propose one
  precise term for each.
- **Probe with scenarios.** Test a definition against concrete edge cases
  until its boundary is clear.
- **Check the code.** If the code does something other than the stated
  meaning, say so. The lexicon describes the project as it is or as decided,
  never a guess.

## Recording a Term

- Record a term the moment the user settles it. Don't batch terms for later.
- Edit the `LEXICON.md` nearest the code the term belongs to. Create the file
  only when there is a first term to write.
- Ask before changing or removing an existing definition; other people and
  agents rely on it.
- Propose terms freely, but the user decides. Mark a term you coined as a
  proposal when you ask.
- After a rename, offer to update the docs and identifiers that used the old
  word.

## What Does Not Go in a Lexicon

- **Decisions and their reasons.** Record those where the repository records
  decisions (an epic, an ADR, a spec) and link to them from the term with
  `_See_`.
- **Implementation detail.** No line numbers, algorithms, or file layouts in
  a definition. A `_See_` link can point to them.
- **General programming words.** Only terms with a meaning specific to this
  project.
- **Private wording in a public repository.** Internal positioning rules and
  links to private repositories belong in a private repository's lexicon.

## Attributions

Adapted from the `domain-modeling` skill in
[mattpocock/skills](https://github.com/mattpocock/skills) (MIT), which keeps
a `GLOSSARY.md` per context. This skill renames the file, scopes it by
directory, folds the context map into each file's links, and leaves decision
records to each repository's own convention.
