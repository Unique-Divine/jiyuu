# Rust Directory Walk

The C entry point `cbm_discover_ex2` calls the Rust function
`mori_discover_collect`, which owns the full result until native setup and the
walk succeed. The C entry point `cbm_discover_count_bounded` calls the Rust
function `mori_discover_count_bounded`, which uses the same walker without
collecting rows. It returns the count, status, and any resource diagnostic.
Mori and the candidate C test runners link both paths through the
`native-engine-abi` feature. The standalone reference build is pinned to
`8bdd8fc20327fcfea4d920138fef77a3b00ca1ef`.

Rust scans a directory before visiting its children in reverse enumeration
order. It decides which directories to descend into, applies ignore
precedence and tracked-path exceptions, and checks the bounded count,
aggregate file and byte limits, and monotonic deadline. C provides the
ignore rules and tracked paths and performs native metadata and language
probes. Rust collects raw path bytes, excluded subtrees, and the capped
ignore sample. The C destructors free the published arrays. The Windows
walker uses the existing UTF-8 Win32 enumeration helpers. Path capacity,
symlink and reparse-point handling, and native mtimes follow the existing
C metadata probes.

Ordinary and frozen semantic manifests use `mori_manifest_build` to own,
deduplicate, and sort the rows. C supplies the Git and config digests,
hashes stable file generations, and enumerates repository controls and
path aliases. Frozen manifest assembly and sorting poll the pipeline
checkpoint. The manifest destructor continues to free the ABI rows.

Initial discovery, fresh manifest validation, and stored-depth
rediscovery use the same walker, including when resource limits apply.
A frozen build with an explicit classified file list uses that list
for discovery and reconciliation, including when it is empty. A frozen
build without a list walks the source under its captured language
configuration.

With `CBM_LOG_LEVEL=debug` and `CBM_PROFILE=1`, the worker log records
the phase and backend in event `pipeline.discovery_backend`. MCP
auto-index admission records its backend in event
`discover.admission_backend`.

## Fixtures

From the Jiyuu root, with the pinned grammars installed, run the linked
discovery tests.

```sh
cargo test --locked -p jiyuu-mori --features c-backend --lib discovery_spike
cargo test --locked -p jiyuu-mori --features c-backend --test discovery_walk
```

The library fixtures call the C ABI and exercise raw paths, native mtimes,
the capped ignore sample, tracked paths, and count and resource refusals.
The linked integration fixture uses an isolated runtime and cache to check
initial, fresh, and stored-depth walks under a file limit. The native frozen
pipeline suite covers an explicit empty list.

The ignored library test `discovery_spike::corpus` compares the production
walker with the earlier Rust prototype on a repository selected by
`MORI_DISCOVERY_ROOT`. Set `MORI_DISCOVERY_MODE` to `full`, `moderate`
or `fast`. Set `MORI_DISCOVERY_BACKEND` to `production` or `prototype`
to run one walker. The default `compare` mode checks selected files,
excluded subtrees, and ignored totals. It checks the complete ignore sample
when neither walk exceeds the cap.
