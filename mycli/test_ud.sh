#!/usr/bin/env bash
set -euo pipefail

# Load the CLI script
source ./ud.sh

pass() { echo "✅ $1"; }
fail() { echo "😞❌ $1"; exit 1; }

# Test: top-level help
output=$(ud help)
([[ "$output" == *"USAGE:"* ]] && pass "help command shows usage") || fail "help command failed"

# Test: rs test --cmd
output=$(ud rs test --cmd)
([[ "$output" == "cargo test" ]] && pass "rs test --cmd outputs correct command") || fail "rs test --cmd incorrect"

# Test: go test-short --cmd
output=$(ud go test-short --cmd)
([[ "$output" == *"go test ./..."* ]] && pass "go test-short --cmd shows test command") || fail "go test-short failed"

# Test: nibi cfg prod dispatches
output=$(ud nibi cfg prod)
([[ "$output" == *"nibid config"* ]] && pass "nibi cfg prod runs config function") || fail "nibi cfg prod failed"
