# Displays available recipes by running `just -l`.
setup:
  #!/usr/bin/env bash
  just -l

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
    (cd mdtoc; bun install)
  )

alias i := install

# Format with prettier
fmt:
  bun run prettier --write .
