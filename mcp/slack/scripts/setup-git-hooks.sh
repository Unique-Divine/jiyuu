#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

[[ -d .githooks ]] || {
  echo "Missing .githooks directory." >&2
  exit 1
}

git config core.hooksPath .githooks
find .githooks -maxdepth 1 -type f -exec chmod +x {} +

echo "Configured git hooks path: .githooks"
