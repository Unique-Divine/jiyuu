# Package Map Parity

The Rust package map parses package manifests, walks the repository for
manifests outside the discovery list, and merges per-worker entries. The
native indexer calls functions `cbm_pkgmap_try_parse`,
`cbm_pkgmap_scan_repo`, and `cbm_pkgmap_scan_swift_targets` for those
operations. The Rust module reuses the SwiftPM target scanner and retains its
manifest path and declaration order.

## Focused Comparison

Run `python3 -I mori/parity/swift-package-map/check.py` on Linux with a C
compiler, Rust, and the pinned reference commit available locally. The script
links the reference against the pinned C package-map source and the candidate
against the Rust package-map module. It compares the project name, manifest
counts, ordered entries and provenance, direct file-list lookup, selected
map values, and the exact `IMPORTS` edge endpoints and properties. It also
checks Go module prefixes, Composer targets and misses, and Rust crate scope.

The fixture covers all eleven manifest types, duplicate Composer entries,
default and custom Swift target paths, a skipped directory, and a symlink.
The general walk yields 23 entries from 14 manifests. The Swift-only walk
yields 12 entries, including four package boundary rows, from four manifests.
The sequential edge writer emits one `IMPORTS` edge from
`App/Sources/App/App.swift` to `Core/Library` with `{"local_name":"Core"}`.

## Linked Verification

Run `just mori-rs-check-linked` with the pinned grammars installed. In the
native runner, use suites `edge_imports` and `pipeline` for cancellation,
package precedence, and the Swift cross-package and manifest-edit cases.
