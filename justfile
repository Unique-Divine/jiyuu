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

# Run all Rust workspace tests with the committed dependency resolution.
rust-test:
  cargo test --workspace --locked

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

# Install the maintained MCP servers.
mcp-install:
  just --justfile mcp/discord/justfile install
  just --justfile mcp/slack/justfile install
  just --justfile mcp/telegram/justfile install

# Run the MCP regression suites.
mcp-test:
  just --justfile mcp/discord/justfile test
  just --justfile mcp/slack/justfile test
  just --justfile mcp/telegram/justfile test

# Write each MCP server's site-manifest.json; with --check, fail when one is stale.
mcp-manifest *ARGS:
  bun --no-env-file run mcp/scripts/site-manifest.ts {{ARGS}}

# Test the MCP site manifest generator against fixture servers.
mcp-manifest-test:
  bun test mcp/scripts

# Check the MCP source trees.
mcp-check:
  just --justfile mcp/discord/justfile check
  just --justfile mcp/slack/justfile check
  just --justfile mcp/telegram/justfile check

# Build the Mori (codebase-memory-mcp) binary, fetching pinned grammars first.
mori-build *ARGS:
  just --justfile mori/justfile grammars {{ARGS}}
  just --justfile mori/justfile build

# Run the Mori grammar and extraction suites.
mori-test *ARGS:
  just --justfile mori/justfile grammars {{ARGS}}
  just --justfile mori/justfile test

# Run the Mori script tests, build and targeted suites.
mori-check *ARGS:
  just --justfile mori/justfile grammars {{ARGS}}
  just --justfile mori/justfile check

# Check the jiyuu-mori crate in pure Rust (format, Clippy, tests).
mori-rs-check:
  just --justfile mori/jiyuu-mori/justfile check

# Build mori with the C program linked and compare it with the C binary.
mori-rs-check-linked:
  just --justfile mori/jiyuu-mori/justfile check-linked

# Build the release mori binary with the C program linked.
mori-rs-build:
  just --justfile mori/jiyuu-mori/justfile build

# Diff two Mori builds, L0-L4 (default: C vs C on the public corpus).
mori-parity *ARGS:
  just --justfile mori/justfile parity {{ARGS}}

# Mori parity on generated fixture repos (needs `just mori-build`).
mori-parity-ci *ARGS:
  just --justfile mori/justfile parity-ci {{ARGS}}
