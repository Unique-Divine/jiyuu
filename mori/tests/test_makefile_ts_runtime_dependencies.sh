#!/usr/bin/env bash
set -euo pipefail

# Regression guard for the ts_runtime unity build.  ts_runtime.c includes
# vendored/ts_runtime/src/lib.c, which in turn includes the rest of the runtime.
# A change below that wrapper must invalidate every build variant.

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

BUILD_DIR="$WORKDIR/build"
mkdir -p "$BUILD_DIR"

DEPENDENCIES=(
    internal/cbm/vendored/ts_runtime/src/stack.c
    internal/cbm/vendored/ts_runtime/src/stack.h
    internal/cbm/vendored/ts_runtime/src/unicode/utf8.h
    internal/cbm/vendored/ts_runtime/include/tree_sitter/api.h
)
TARGETS=(
    "$BUILD_DIR/ts_runtime.o"
    "$BUILD_DIR/tsan_ts_runtime.o"
    "$BUILD_DIR/prod_ts_runtime.o"
)

cd "$ROOT"

# The config stamps check their settings on every build. Exclude them from these
# probes so the exit code measures only file dependencies.
make_query=(make -f Makefile.cbm -q -o "$BUILD_DIR/.test-build-config"
            -o "$BUILD_DIR/.build-config" "BUILD_DIR=$BUILD_DIR")
for target in "${TARGETS[@]}"; do
    touch "$target"
    if ! "${make_query[@]}" "$target"; then
        echo "FAIL: $target should be current before changing any dependency"
        exit 1
    fi
done

for dependency in "${DEPENDENCIES[@]}"; do
    if [[ ! -f "$dependency" ]]; then
        echo "FAIL: expected tree-sitter runtime dependency is missing: $dependency"
        exit 1
    fi

    for target in "${TARGETS[@]}"; do
        # Make the object newer than its currently-declared prerequisites.  -W
        # then asks make whether changing the unity-included file would rebuild
        # it, without compiling or modifying the source tree.
        touch "$target"

        status=0
        "${make_query[@]}" -W "$dependency" "$target" || status=$?

        if [[ $status -eq 0 ]]; then
            echo "FAIL: $target would stay stale after changing $dependency"
            exit 1
        fi
        if [[ $status -ne 1 ]]; then
            echo "FAIL: make dependency probe failed for $target and $dependency (exit $status)"
            exit 1
        fi
    done
done

echo "PASS: all tree-sitter runtime unity objects track included sources and headers"
