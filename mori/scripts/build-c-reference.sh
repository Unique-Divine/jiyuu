#!/usr/bin/env bash
# Build the standalone C executable for the Rust-linked test suite.
set -euo pipefail

if [[ $# -ne 1 || $1 != /* ]]; then
    printf 'Usage: %s /absolute/path/to/codebase-memory-mcp\n' "$0" >&2
    exit 2
fi
output=$1
script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
cache_tool="$script_dir/c_reference_cache.py"
reference_sha="$(python3 "$cache_tool" pin)"
reference_dir="$(mktemp -d "${TMPDIR:-/tmp}/mori-c-reference.XXXXXX")"
trap 'rm -rf "$reference_dir"' EXIT

git -C "$reference_dir" init --quiet
for attempt in 1 2 3; do
    if git -C "$reference_dir" fetch --quiet --depth=1 --no-tags \
        https://github.com/Unique-Divine/jiyuu.git "$reference_sha"; then
        break
    fi
    if [[ $attempt == 3 ]]; then
        exit 1
    fi
    sleep "$((attempt * 5))"
done
git -C "$reference_dir" checkout --quiet --detach FETCH_HEAD
test "$(git -C "$reference_dir" rev-parse HEAD)" = "$reference_sha"
printf 'C reference source %s\n' "$reference_sha"

cd "$reference_dir/mori"
python3 "$cache_tool" verify-grammar grammars.lock.json
bun scripts/grammars-fetch.ts
case "$(uname -s)" in
    Darwin) default_cc=cc; default_cxx=c++ ;;
    *) default_cc=gcc; default_cxx=g++ ;;
esac
if [[ -n ${MORI_REFERENCE_VERSION:-} ]]; then
    export CFLAGS_EXTRA="-DCBM_VERSION=\"\\\"$MORI_REFERENCE_VERSION\\\"\""
fi
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
