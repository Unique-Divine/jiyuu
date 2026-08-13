---
name: just
description: >-
  Prefer the nearest relevant `justfile` as the default command surface for
  build, test, lint, format, run, deploy, and repo automation workflows. Use
  when a repo or subproject has a `justfile`, or when the user mentions `just`,
  `justfile`, recipes, or Makefile-to-just migration.
---

# Just

Use this skill when a repo may expose commands through `justfile`.

Treat `just` as the canonical task interface when it exists. Prefer existing
recipes over reconstructing the underlying `go`, `npm`, `cargo`, `docker`,
`make`, or shell commands by hand.

## When To Use This Skill

Use it when:

- the repo or a nearby subproject has a `justfile`
- the user asks how to build, test, lint, run, deploy, or inspect commands
- the user mentions `just`, `justfile`, recipes, or recipe arguments
- the repo has both `Makefile` and `justfile` and you need to understand which
  command surface is intended
- the user wants to migrate from `make` to `just`

## Default Workflow

1. Find the nearest relevant `justfile`. Do not assume repo root.
2. Run `just --list` in that directory to discover the supported UX.
3. If a recipe is unclear, inspect it with `just --show <recipe>`.
4. Use the recipe directly unless the user explicitly wants the raw command.
5. Fall back to ad hoc commands only when no suitable recipe exists.

Common recipe names to check first: `setup`, `install`, `build`, `test`, `lint`,
`fmt`, `dev`, `run`, `preview`, `deploy`.

## Agent Guardrails

- Prefer the nearest task-specific `justfile` over a broader root-level one.
- If both `Makefile` and `justfile` exist, inspect both before suggesting
  migration or replacement.
- Do not bypass an existing recipe unless it is missing, clearly wrong for the
  task, or the user asks for the underlying command.
- Do not assume `.env` exists, and do not print secrets just because a
  `justfile` uses `dotenv-load` or exported variables.
- Treat destructive or confirmation-gated recipes cautiously.
- Test-like shebang recipes (`test`, `test-*`, etc.) should use `#!/usr/bin/env bash` and `set -euo pipefail` so failures propagate instead of being masked by later commands.
- Recipe doc comments must be **exactly one** `# ...` line immediately above
  the recipe name. `just` / `just --list` use only that adjacent comment as the
  recipe description; a second `#` line replaces or truncates the intended
  preview text.

## Common Failure Modes

### Wrong Working Directory

Nested `justfile`s are common. If commands look wrong, verify you are running in
the correct subproject. When needed, invoke a specific file directly with
`just --justfile path/to/justfile <recipe>` instead of guessing.

### Sparse Or Misleading `--list` Output

Some repos split recipes across imported files or modules. If `just --list`
looks too small, inspect the `justfile` for `import` or `mod` statements before
assuming recipes are missing.

### Hidden Helper Recipes

`[private]` recipes do not appear in `just --list` or `just --summary`, so they
do not pollute help output. They are still valid recipe entry points and can be
invoked directly with `just <private-recipe>` when an internal workflow, CI job,
or explicit user request needs them. Check `just --show <recipe>`, use
`just --dry-run <recipe>`, or inspect the file if a public recipe depends on
helpers you cannot see in the listing.

### Env Or Shell Drift

Behavior can change based on settings such as:

- `set dotenv-load`
- `set export`
- `set shell := [...]`
- `set positional-arguments`

If a recipe works differently in CI, in a subdirectory, or in the terminal,
check these settings before changing the command.

### Multi-line Recipe Doc Comments

`just` treats only the single `#` comment immediately above a recipe as its
documentation for `just --list` and similar previews. If you write two or more
comment lines above a recipe, the intended summary is dropped or replaced and
the preview shows incomplete or misleading help text.

Wrong:

```just
# Install root workspace tooling + webapp deps, then restore extension caches
# (`just install`; CI: `just install CI=true` requires GH_TOKEN).
install CI="false":
```

Right:

```just
# Install webapp deps and restore extension caches (`just install`; CI: `just install CI=true` requires GH_TOKEN).
install CI="false":
```

After editing recipe comments, run `just --list` (or plain `just` when that
lists recipes) and confirm each public recipe shows the full intended
one-line description.

### Confirmation Gates

Recipes marked with `[confirm]` may block or fail in non-interactive contexts.
Treat them as intentional safety rails, especially for `destroy`, `clean`, or
production deploy flows.

## Quick Command Reference

- `just --list` list public recipes with descriptions
- `just --show <recipe>` show the resolved recipe body
- `just --summary` list recipe names only
- `just --dry-run <recipe>` print commands without executing them, including
  private recipes
- `just --dump` print the full justfile
- `just --evaluate` inspect variable evaluation
- `just <recipe> [args...]` run a recipe
- `just --justfile path/to/justfile <recipe>` target a specific justfile

## Notes

- Only **one** `#` comment line immediately above a recipe becomes its
  description in `just --list` / recipe previews. Extra comment lines above the
  recipe break or replace that documentation; keep the doc as a single line.
- Each recipe line runs in a separate shell unless the recipe is a shebang
  script.
- Large repos may use `just` as a command hub over other toolchains rather than
  exposing the toolchain directly.

## References

- [references/make-migration.md](references/make-migration.md)
- [references/justfile-patterns.md](references/justfile-patterns.md)
