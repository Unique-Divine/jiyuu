# Displays available recipes when running `just` or `just list`.
[private]
list:
  #!/usr/bin/env bash
  just --list

# Install bun for TypeScript and Rust stuff for scripts
install:
  #!/usr/bin/env bash
  echo "RUN: just install"
  ( 
    set -euo pipefail
    source scripts/bashlib.sh # deps: which_ok

    if ! which_ok bun ; then
      # Install bun
      curl -fsSL https://bun.sh/install | bash
    else
      log_success "bun is installed"
    fi 

    if ! which_ok cargo ; then
      # Install rustup 
      curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
      rustup component add clippy
    else
      log_success "cargo (Rust) is installed"
    fi

    bun install
    for project in mdtoc discord-nibiru ts-pkg/bash ts-pkg/jiyuu ts-pkg/tg-sai; do
      (cd "$project" && bun install)
    done
  )

alias i := install

# Format with prettier
fmt:
  bun run prettier --write .

# Check all TypeScript projects
typecheck:
  bun check

# Check both Python projects even when the first reports findings.
python-typecheck:
  #!/usr/bin/env bash
  set -euo pipefail
  status=0
  if ! just --justfile mcp/telegram/justfile typecheck; then
    status=1
  fi
  if ! just --justfile ts-pkg/tg-sai/justfile py-typecheck; then
    status=1
  fi
  exit "$status"

# Preview merged PR classification inheritance; add --run to apply it.
github-labels *ARGS:
  bun run scripts/github-labels/main.ts {{ARGS}}

# Test GitHub label planning, API decoding, and repository path mappings.
github-labels-test:
  bun test scripts/github-labels

# Install both maintained MCP servers.
mcp-install:
  just --justfile mcp/slack/justfile install
  just --justfile mcp/telegram/justfile install

# Run both MCP regression suites.
mcp-test:
  just --justfile mcp/slack/justfile test
  just --justfile mcp/telegram/justfile test

# Check both MCP source trees.
mcp-check:
  just --justfile mcp/slack/justfile check
  just --justfile mcp/telegram/justfile check
