# Justfile Patterns

Use this as a reference for common `justfile` patterns. Keep examples small and
non-prescriptive. Prefer the repo's existing recipes over copying templates.

## Core Settings

```just
set dotenv-load
set shell := ["bash", "-euco", "pipefail"]
set export
```

- `dotenv-load` explains implicit env loading from `.env`
- `shell` explains fail-fast behavior and shell-specific syntax
- `export` makes `just` variables available to recipe commands

## Default Help Pattern

```just
[private]
default:
    @just --list --unsorted
```

Use this when the repo wants `just` with no arguments to behave like help.

## Parameterized Recipe

```just
deploy env:
    ./scripts/deploy {{env}}
```

Use recipe arguments instead of hardcoding environment names or resources.

## Variadic Recipe

```just
test *args:
    uv run pytest {{args}}
```

Useful when the underlying tool already has a stable CLI and the recipe mostly
adds project defaults.

## Shebang Recipe

```just
check:
    #!/usr/bin/env bash
    set -euo pipefail
    for cmd in jq rg just; do
        command -v "$cmd" >/dev/null || { echo "Missing: $cmd"; exit 1; }
    done
```

Use shebang recipes when multiple lines must share shell state.

## Hidden Helper Recipe

```just
[private]
_ensure-tools:
    #!/usr/bin/env bash
    command -v terraform >/dev/null

plan: _ensure-tools
    terraform plan
```

Remember that `[private]` helpers will not appear in `just --list`.

## Confirmation Gate

```just
[confirm("Destroy all resources?")]
destroy:
    terraform destroy
```

Use for destructive operations. Agents should treat these as intentional safety
rails rather than friction to bypass.

## Grouped Recipes

```just
[group("ci")]
ci-test:
    cargo test

[group("deploy")]
deploy-prod:
    ./deploy production
```

Helpful when `just --list` doubles as the repo's command menu.

## Imports And Modules

```just
import "ci.just"
mod deploy "deploy.just"
```

- `import` pulls in recipes from another file
- `mod` namespaces recipes, for example `just deploy::prod`

If a recipe seems missing, inspect the main `justfile` for `import` or `mod`
before assuming the command surface is incomplete.

## Project-Type Hints

Use these as discovery hints, not templates:

- Go repos often expose `build`, `test`, `lint`, `fmt`, and release helpers.
- Python repos often expose `install`, `test`, `lint`, `fmt`, and `typecheck`.
- Infra repos often expose `init`, `plan`, `apply`, `destroy`, and validation.
- Docker-heavy repos often expose image build/push recipes plus local run flows.

For all of them, prefer `just --list` and `just --show` over guessing.
