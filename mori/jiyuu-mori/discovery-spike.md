# Rust Directory Walk

On Unix, a linked `mori` build can use `MORI_IMPL_DISCOVERY=rust` for the
ordinary pipeline walk. Set it when starting the daemon or use a fresh
`CBM_RUNTIME_DIR` so the index worker inherits it. Both initial discovery and
rediscovery at a stored depth use the selector. Set `CBM_LOG_LEVEL=debug` and
`CBM_PROFILE=1` to keep the worker log. Its `pipeline.discovery_backend`
event records the phase, mode, actual backend, and reason.

Rust reads each directory and owns the pending directory stack. It visits
children in reverse enumeration order after scanning their parent. Native
code prepares the ignore rules, classifies entries, and builds the result
arrays, including native mtimes and the ordered ignore sample. C is the
default. It handles pipeline walks with aggregate resource limits, fresh
semantic-manifest checks, and frozen builds that need a walk. A frozen build
with its own file list reports `frozen_list` as the backend.

Set `MORI_IMPL_ADMISSION=rust` on the linked Unix executable to use Rust
directory iteration for the bounded count before MCP automatic indexing.
The daemon inherits the setting at startup. C still applies the same ignore,
language, file count, deadline, and resource checks. The
`discover.admission_backend` debug event records which implementation ran.

## Independent Prototype Comparison

The linked test module compares a Rust walk with C function
`cbm_discover_ex2`. Both use Mori's native language classifier, built-in
filters, and ignore matcher. The Rust code handles the directory walk,
ignore precedence, and tracked-path exceptions. The module compiles only in
Unix test builds with feature `c-backend`.

## Run the Fixtures

From the Jiyuu root, install the pinned grammar files and run the linked tests.

```sh
just --justfile mori/justfile grammars
cargo test --locked -p jiyuu-mori --features c-backend --lib discovery_spike
```

The fixtures compare full, moderate, and fast discovery. They exercise
nested `.gitignore` files, `.cbmignore` negation, `info/exclude`, tracked
files in ignored directories, a size limit, symlinks, shebang detection, and
unsupported extensions. The enclosing-repository fixture indexes a package
subdirectory. It checks ancestor rules against their original directories,
deeper negations, the repository's `info/exclude`, and tracked files.
The user excludes fixture checks `~/.gitconfig`, the XDG Git config, and
the XDG `git/ignore` default in order, in an isolated process.
The Sphinx fixture checks custom reStructuredText suffixes inherited from
`conf.py`, nested overrides, and the 64 KiB config read limit.

## Compare and Measure a Repository

Build once and run the test executable directly so elapsed time and peak RSS
exclude the build. Use an unchanged checkout for both walks. The commands
below use GNU `time` on Linux.

```sh
discovery_test="$(
  cargo test --release --locked -p jiyuu-mori --features c-backend \
    --lib --no-run --message-format=json |
  jq -er 'select(.reason == "compiler-artifact" and .profile.test
    and .target.name == "jiyuu_mori" and .executable != null) | .executable'
)"
export MORI_DISCOVERY_ROOT=/absolute/path/to/repository
export MORI_DISCOVERY_MODE=full
"$discovery_test" discovery_spike::corpus --exact --ignored --nocapture
for backend in c rust; do
  MORI_DISCOVERY_BACKEND="$backend" /usr/bin/time -v \
    "$discovery_test" discovery_spike::corpus --exact --ignored --nocapture
done
```

Use `moderate` or `fast` to change the discovery mode. Set
`MORI_DISCOVERY_SHOW_DIFF=1` to print up to ten examples per side in the
comparison output. The comparison exits unsuccessfully on a difference.
It checks relative paths, language IDs, sizes, excluded subtrees, and the
total and retained count of ignored paths. For at most 2,000 ignored paths
per walk it also compares the ignored paths and reasons. Beyond that cap,
the native result retains a traversal-order sample.

Set `MORI_DISCOVERY_BACKEND=pipeline-compare` to check the production walk
against C. This compares the results in order, including absolute paths,
native mtimes, and the retained ignore sample even when it hits the cap.
