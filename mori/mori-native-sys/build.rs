//! Builds and links the native Mori archive when `link-native` is enabled.
//!
//! Steps: check that the pinned grammars are installed, run
//! `make -f Makefile.cbm libmori-c` in `mori/`, then link the archive whole
//! plus the system libraries the C binary links. The script never reaches
//! the network: `just --justfile mori/justfile grammars` installs the
//! grammars, and this script only compares `mori/.grammars.stamp` with
//! `mori/grammars.lock.json` and the installed tree. `Makefile.cbm` stays the
//! source of truth for C flags.
//!
//! Environment:
//! - `MORI_C_BUILD_DIR`: make `BUILD_DIR`, relative to `mori/` or absolute
//!   (default `build/mori-rs`, shared by every cargo profile so the grammar
//!   objects compile once).
//! - `MORI_MAKE_JOBS`: make parallelism (default 4).
//! - `MORI_WITH_UI=1`: link the graph UI (make `LIBMORI_WITH_UI=1`, which
//!   builds `graph-ui` with npm).
//! - `CC`, `CXX`, `CFLAGS_EXTRA`, `TEST_SEAMS`: passed to make as
//!   `scripts/build.sh` would.
//!
//! Build flavours that need their own link flags (`MEMWASTE*`,
//! `TEST_COVERAGE`) are pinned off on the make command line: those flags
//! live in the Makefile's `LDFLAGS`, which never reach the Rust link.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(feature = "link-native")]
    {
        println!("cargo:rerun-if-changed=build_native.rs");
        native::build();
    }
}

#[cfg(feature = "link-native")]
#[path = "build_native.rs"]
mod native;
