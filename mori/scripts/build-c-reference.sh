#!/usr/bin/env bash
# Build the standalone C executable for the Rust-linked test suite.
set -euo pipefail

if [[ $# -ne 1 || $1 != /* ]]; then
    printf 'Usage: %s /absolute/path/to/codebase-memory-mcp\n' "$0" >&2
    exit 2
fi
output=$1
reference_sha=8bdd8fc20327fcfea4d920138fef77a3b00ca1ef
reference_dir="$(mktemp -d "${TMPDIR:-/tmp}/mori-c-reference.XXXXXX")"
trap 'rm -rf "$reference_dir"' EXIT

git -C "$reference_dir" init --quiet
git -C "$reference_dir" fetch --quiet --depth=1 --no-tags \
    https://github.com/Unique-Divine/jiyuu.git "$reference_sha"
git -C "$reference_dir" checkout --quiet --detach FETCH_HEAD
test "$(git -C "$reference_dir" rev-parse HEAD)" = "$reference_sha"
printf 'C reference source %s\n' "$reference_sha"

cd "$reference_dir/mori"
bun scripts/grammars-fetch.ts
case "$(uname -s)" in
    Darwin) default_cc=cc; default_cxx=c++ ;;
    *) default_cc=gcc; default_cxx=g++ ;;
esac
target=cbm
if [[ ${MORI_WITH_UI:-0} == 1 ]]; then
    target=cbm-with-ui
fi
make -f Makefile.cbm -j"${MORI_MAKE_JOBS:-4}" "$target" \
    BUILD_DIR=build/reference CC="${CC:-$default_cc}" CXX="${CXX:-$default_cxx}" \
    TEST_SEAMS="${TEST_SEAMS:-0}" \
    MEMWASTE=0 MEMWASTE_ACCESS=0 MEMWASTE_PROFILE=0 TEST_COVERAGE=0
mkdir -p "$(dirname "$output")"
cp build/reference/codebase-memory-mcp "$output"
