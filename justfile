# Displays available recipes by running `just -l`.
setup:
  #!/usr/bin/env bash
  just -l

# Install bun for TypeScript and Rust stuff for scripts
install:
  #!/usr/bin/env bash

  # Install bun
  curl -fsSL https://bun.sh/install | bash 
  # Install rustup 
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  rustup component add clippy

# Format with prettier
fmt:
  bun run prettier --write .
