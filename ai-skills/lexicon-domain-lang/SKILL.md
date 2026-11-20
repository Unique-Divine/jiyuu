---
name: lexicon-domain-lang
description: The lexicon skill. Keep a project's domain language in LEXICON.md files. Use when the user asks for the lexicon skill, asks what to call something or which word is right, wants terminology defined or cleaned up, or mentions a lexicon, glossary, domain language, ubiquitous language, or LEXICON.md. Covers which lexicons apply to a path, using their terms, challenging conflicting words, and recording a term once it is settled.
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

1. Start from the files the task touches, not only the directory the session
   started in. For each, read every `LEXICON.md` from the repository root down
   to that file's directory. A child adds to its ancestors; where it marks a
   term `_Overrides_`, the child's meaning holds inside its directory.
2. In a Git submodule, the superproject's lexicons above the submodule apply
   too (`git rev-parse --show-superproject-working-tree`).
3. When the task crosses into another context, follow the `Related` links at
   the top of the file and read those too.
4. Two sibling lexicons may define the same word differently. Each meaning
   holds in its own directory. When discussing both, qualify the word with
   its context ("Mori's Route", "the gateway's route").
5. Outside Git, a project's boundary is not marked. Read the lexicons in the
   ancestor directories that belong to the project and ignore any above it.
6. If no `LEXICON.md` exists, carry on. Don't announce its absence.

```sh
# Lexicons that may apply to the current directory, outermost first.
# Inside Git: climbs to the repository root, and through submodules into the
# superproject. Outside Git: climbs to $HOME or the filesystem root, so treat
# the output as candidates and drop any above the project (rule 5).
# Works in bash and zsh.
d=$(pwd -P); out=
while :; do
  [ -f "$d/LEXICON.md" ] && out="$d/LEXICON.md
$out"
  top=$(git -C "$d" rev-parse --show-toplevel 2>/dev/null)
  if [ -n "$top" ]; then
    if [ "$d" = "$top" ] &&
       [ -z "$(git -C "$d" rev-parse --show-superproject-working-tree 2>/dev/null)" ]
    then break; fi
  elif [ "$d" = "$HOME" ]; then break
  fi
  [ "$d" = / ] && break
  d=$(dirname "$d")
done
printf %s "$out"
```

## Using the Language

- In your own wording, name concepts with the lexicon's terms: replies, docs,
  commit messages, issues, test names, and new identifiers in code.
- Don't use a word a lexicon lists under `_Avoid_` or `Never Say` for the
  concept it rules out. The rule is about that concept, not the letters:
  quote text exactly as written, keep existing identifiers, API names, and
  flags as they are, and use the word freely where it means something else.
- If the user uses an avoided word, answer in the lexicon's term and say once
  which word you mean. Raise it as a question only when the difference could
  change the outcome. Don't correct wording repeatedly.
- If the user asks for a specific word, use it, and say so if it conflicts
  with a lexicon so they can decide whether to change the lexicon.
- If you need a concept the lexicon lacks, either the project doesn't think in
  those terms (reconsider the wording) or there is a gap (raise it, below).

## Sharpening the Language

Do this while designing, drilling a spec, or reviewing, whenever wording
causes friction.

- **Challenge a conflict.** "The lexicon uses *full rebuild* for an update
  path. You seem to mean index depth `full`. Which is it?"
- **Split an overloaded word.** When one word covers two ideas, propose one
  precise term for each.
- **Probe with scenarios.** Test a definition against concrete edge cases
  until its boundary is clear.
- **Check the code.** If the code does something other than the stated
  meaning, say so. The lexicon describes the project as it is or as decided,
  never a guess.

## Recording a Term

- A term is settled when the user agrees to it, not when you propose it.
  Propose terms freely and mark the ones you coined.
- Once the user settles a term, write it to the `LEXICON.md` nearest the code
  it belongs to, then, not in a batch later. The user's agreement to the term
  is the go-ahead for that entry.
- Ask first before creating a new `LEXICON.md`, and before changing or
  removing an existing definition; other people and agents rely on it.
- In read-only work (a review, plan mode, an investigation), write nothing.
  List the entries you would add.
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
[mattpocock/skills](https://github.com/mattpocock/skills), copyright (c) 2026
Matt Pocock, used under the MIT License; the notice is in
[THIRD_PARTY.md](THIRD_PARTY.md). That skill keeps a `GLOSSARY.md` per
context. This one renames the file, scopes it by directory, folds the context
map into each file's links, and leaves decision records to each repository's
own convention.
