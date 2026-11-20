//! Builds and links the C program as `libmori-c.a` when feature `c-backend`
//! is on (Strategy A, phase P0 in the Rust port design).
//!
//! Steps: install the pinned grammars with `mori/scripts/grammars-fetch.ts`
//! (cached under `~/.cache/jiyuu-mori/grammars/`), run
//! `make -f Makefile.cbm libmori-c` in `mori/`, then link the archive whole
//! plus the system libraries the C binary links. `Makefile.cbm` stays the
//! source of truth for C flags, so the C inside `mori` is compiled exactly
//! like `codebase-memory-mcp`.
//!
//! Environment:
//! - `MORI_C_BUILD_DIR`: make `BUILD_DIR`, relative to `mori/` or absolute
//!   (default `build/mori-rs`, shared by every cargo profile so the grammar
//!   objects compile once).
//! - `MORI_MAKE_JOBS`: make parallelism (default 4).
//! - `MORI_C_REFERENCE=1`: also build `codebase-memory-mcp` in the same
//!   `BUILD_DIR`, for the comparison test in `tests/linked.rs`.
//! - `CC`, `CXX`, `CFLAGS_EXTRA`, `TEST_SEAMS`, `STATIC`: passed to make as
//!   `scripts/build.sh` would.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Make variables read from the environment that change the C objects.
const MAKE_ENV_VARS: &[&str] =
    &["CC", "CXX", "CFLAGS_EXTRA", "TEST_SEAMS", "STATIC"];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=MORI_C_BUILD_DIR");
    println!("cargo:rerun-if-env-changed=MORI_MAKE_JOBS");
    println!("cargo:rerun-if-env-changed=MORI_C_REFERENCE");
    if env::var_os("CARGO_FEATURE_C_BACKEND").is_none() {
        return;
    }
    if let Err(message) = build_and_link() {
        panic!("jiyuu-mori build.rs: {message}");
    }
}

fn build_and_link() -> Result<(), String> {
    let manifest_dir = PathBuf::from(
        env::var("CARGO_MANIFEST_DIR").map_err(|e| e.to_string())?,
    );
    let mori_dir = manifest_dir
        .parent()
        .ok_or("crate has no parent directory")?
        .to_path_buf();
    let build_dir_arg = env::var("MORI_C_BUILD_DIR")
        .unwrap_or_else(|_| "build/mori-rs".to_string());
    let build_dir = mori_dir.join(&build_dir_arg);
    let jobs = env::var("MORI_MAKE_JOBS").unwrap_or_else(|_| "4".into());
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    for path in ["src", "internal", "vendored", "Makefile.cbm"] {
        println!("cargo:rerun-if-changed={}", mori_dir.join(path).display());
    }
    for path in ["grammars.lock.json", ".grammars.stamp"] {
        println!("cargo:rerun-if-changed={}", mori_dir.join(path).display());
    }
    for var in MAKE_ENV_VARS {
        println!("cargo:rerun-if-env-changed={var}");
    }

    fetch_grammars(&mori_dir)?;

    let mut targets = vec!["libmori-c"];
    if env::var("MORI_C_REFERENCE").as_deref() == Ok("1") {
        targets.push("cbm");
    }
    run_make(&mori_dir, &build_dir_arg, &jobs, &target_os, &targets)?;

    let archive = build_dir.join("libmori-c.a");
    if !archive.is_file() {
        return Err(format!("make did not produce {}", archive.display()));
    }
    // A deleted or rebuilt archive (for example after `make clean-c`)
    // re-runs this script.
    println!("cargo:rerun-if-changed={}", archive.display());
    println!("cargo:rustc-env=MORI_C_BUILD_DIR={}", build_dir.display());
    emit_link_lines(&build_dir, &target_os);
    Ok(())
}

/// Installs the pinned grammars; a repeat run with an intact tree is a
/// quick stamp check.
fn fetch_grammars(mori_dir: &Path) -> Result<(), String> {
    let status = Command::new("bun")
        .arg("scripts/grammars-fetch.ts")
        .current_dir(mori_dir)
        .status()
        .map_err(|e| {
            format!("could not run `bun scripts/grammars-fetch.ts`: {e}")
        })?;
    if !status.success() {
        return Err(format!(
            "grammar fetch failed ({status}); run `just mori-build` or \
             `bun scripts/grammars-fetch.ts --from <archive>` in mori/"
        ));
    }
    Ok(())
}

/// Default compilers, as `mori/scripts/env.sh` picks them.
fn default_compilers(target_os: &str) -> (&'static str, &'static str) {
    if target_os == "macos" {
        ("cc", "c++")
    } else {
        ("gcc", "g++")
    }
}

fn run_make(
    mori_dir: &Path,
    build_dir_arg: &str,
    jobs: &str,
    target_os: &str,
    targets: &[&str],
) -> Result<(), String> {
    let (default_cc, default_cxx) = default_compilers(target_os);
    let cc = env::var("CC").unwrap_or_else(|_| default_cc.into());
    let cxx = env::var("CXX").unwrap_or_else(|_| default_cxx.into());
    let mut cmd = Command::new("make");
    cmd.current_dir(mori_dir)
        .args(["-f", "Makefile.cbm"])
        .arg(format!("-j{jobs}"))
        .args(targets)
        .arg(format!("BUILD_DIR={build_dir_arg}"))
        .arg(format!("CC={cc}"))
        .arg(format!("CXX={cxx}"))
        // Cargo's jobserver and an outer make must not change -j.
        .env_remove("MAKEFLAGS")
        .env_remove("MFLAGS")
        .env_remove("MAKELEVEL");
    let output = cmd
        .output()
        .map_err(|e| format!("could not run make in mori/: {e}"))?;
    // Stdout of a build script is cargo's channel; keep make's output on
    // stderr (shown with `cargo build -vv` or on failure).
    eprint!("{}", String::from_utf8_lossy(&output.stdout));
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    if !output.status.success() {
        return Err(format!(
            "`make -f Makefile.cbm {}` failed ({}); see the output above",
            targets.join(" "),
            output.status
        ));
    }
    Ok(())
}

/// Links the archive whole, so the binary holds the same objects as
/// `codebase-memory-mcp`, and the libraries from `LDFLAGS` in
/// `Makefile.cbm` (`-lm -lstdc++ -lpthread -lz`).
fn emit_link_lines(build_dir: &Path, target_os: &str) {
    println!("cargo:rustc-link-search=native={}", build_dir.display());
    println!("cargo:rustc-link-lib=static:+whole-archive=mori-c");
    println!("cargo:rustc-link-lib=dylib=z");
    println!("cargo:rustc-link-lib=dylib=m");
    println!("cargo:rustc-link-lib=dylib=pthread");
    if target_os == "macos" {
        println!("cargo:rustc-link-lib=dylib=c++");
    } else {
        println!("cargo:rustc-link-lib=dylib=stdc++");
    }
    if target_os == "linux" {
        // ELF_HARDENING_FLAGS in Makefile.cbm. Rust already links with
        // `-z relro -z now` and a non-executable stack; separate-code is the
        // one it does not set (not a default on aarch64 GNU ld).
        println!("cargo:rustc-link-arg-bins=-Wl,-z,separate-code");
    }
}
