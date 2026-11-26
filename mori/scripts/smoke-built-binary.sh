#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 || $1 != /* || ! -f $1 || ! -x $1 ]]; then
    printf 'Usage: %s /absolute/path/to/executable\n' "$0" >&2
    exit 2
fi

binary=$1
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
scratch="$(mktemp -d /tmp/mori-native.XXXXXX)"
candidate=(env -i "PATH=$PATH" "HOME=$scratch/home"
    "CBM_RUNTIME_DIR=$scratch/rt" "CBM_CACHE_DIR=$scratch/cache" "$binary")
cleanup() {
    "${candidate[@]}" daemon stop >/dev/null 2>&1 || true
    rm -rf "$scratch"
}
trap cleanup EXIT

mkdir "$scratch/home" "$scratch/rt" "$scratch/cache"
chmod 700 "$scratch/home" "$scratch/rt" "$scratch/cache"
"${candidate[@]}" cli --quiet index_repository --repo_path "$root/tests/fixtures/parity-repo" |
    jq -e '.status == "indexed"'

symbols="$(nm --defined-only "$binary")"
if ! grep -Eq '[[:space:]][Tt] mori_fts_write$' <<< "$symbols"; then
    printf 'Missing mori_fts_write in %s\n' "$binary" >&2
    exit 1
fi
if grep -Eq '[[:space:]][Tt] cbm_text_set_.*_hook_for_testing$' <<< "$symbols"; then
    printf 'Unexpected managed-text fixture hooks in %s\n' "$binary" >&2
    exit 1
fi
