# SwiftPM Target Scanner Cutover

`pass_pkgmap.c` calls the Rust export `cbm_rust_swift_scan_targets` from
`parse_package_swift`. The parser reaches that call through
`cbm_pkgmap_try_parse` for discovered manifests, through
`cbm_pkgmap_scan_repo` for repository walks, and through
`cbm_pkgmap_scan_swift_targets` for the Swift target index in `registry.c`.
The C adapter copies the target name and entry path and stamps the manifest
path. The existing C caller stamps the source path and declaration order.

## Linked Verification

The `swift_package_map` module in `mori/jiyuu-mori/src/lib.rs` supplies the
export under `native-engine-abi`. The Rust binary enables that feature
through `c-backend`. The native executable and test runner link the Rust
archive built with the same feature.

Run `just mori-rs-check-linked` with the pinned grammars installed. In the
native pipeline suite, run the `pkgmap_swift_*` cases,
`pipeline_swift_cross_package_import`, and
`pipeline_swift_custom_target_paths_and_manifest_edit_issue2061`.

## Focused Comparison

Run `python3 -I mori/parity/swift-package-map/check.py` on Linux with a C
compiler, Rust, and the pinned base commit available locally. The script
compiles the reference package-map source from that commit and links the
candidate against the Rust scanner. It compares the project name, manifest
counts, ordered entries and provenance, direct file-list lookup, and exact
`IMPORTS` edge endpoints and properties.

The fixture includes nested manifests, a duplicate target, custom and
default paths, skipped directories, a symlink, and a JavaScript manifest.
The general walk yields 9 entries from 5 manifests. The Swift-only walk
yields 12 entries, including 4 package boundary rows, from 4 manifests.
Mori's sequential edge writer takes seeded File and Folder nodes and import
facts. It emits one edge from `App/Sources/App/App.swift` to `Core/Library`
with `{"local_name":"Core"}`.
