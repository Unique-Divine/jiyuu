use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Make variables read from the environment that change the C objects.
const MAKE_ENV_VARS: &[&str] = &["CC", "CXX", "CFLAGS_EXTRA", "TEST_SEAMS"];

/// Make variables pinned on the command line, which overrides the
/// environment: each one also needs link flags the archive cannot carry.
const PINNED_MAKE_VARS: &[&str] = &[
    "MEMWASTE=0",
    "MEMWASTE_ACCESS=0",
    "MEMWASTE_PROFILE=0",
    "TEST_COVERAGE=0",
];

pub fn build() {
    println!("cargo:rerun-if-env-changed=MORI_C_BUILD_DIR");
    println!("cargo:rerun-if-env-changed=MORI_MAKE_JOBS");
    println!("cargo:rerun-if-env-changed=MORI_WITH_UI");
    if let Err(message) = build_and_link() {
        panic!("mori-native-sys build.rs: {message}");
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

    check_grammars(&mori_dir)?;

    let with_ui = env::var("MORI_WITH_UI").as_deref() == Ok("1");
    if with_ui {
        for path in [
            "graph-ui/src",
            "graph-ui/index.html",
            "graph-ui/package.json",
            "graph-ui/package-lock.json",
            "graph-ui/vite.config.ts",
            "graph-ui/tsconfig.json",
            "graph-ui/tsconfig.node.json",
        ] {
            let path = mori_dir.join(path);
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
    run_make(&mori_dir, &build_dir_arg, &jobs, &target_os, with_ui)?;

    let archive = build_dir.join("libmori-c.a");
    if !archive.is_file() {
        return Err(format!("make did not produce {}", archive.display()));
    }
    // A deleted or rebuilt archive (for example after `make clean-c`)
    // re-runs this script.
    println!("cargo:rerun-if-changed={}", archive.display());
    println!("cargo::metadata=build_dir={}", build_dir.display());
    emit_link_lines(&build_dir, &target_os);
    Ok(())
}

const GRAMMARS_HINT: &str = "run `just --justfile mori/justfile grammars` \
 (or `just mori-build`) to install them";

/// Checks the installed grammars against their stamp without hashing: the
/// stamp names the archive in `grammars.lock.json`, and every stamped file
/// has the size and mtime recorded at install (the same quick check
/// `scripts/grammars-fetch.ts` runs before it re-verifies anything).
fn check_grammars(mori_dir: &Path) -> Result<(), String> {
    let read_json = |name: &str| -> Result<serde_json::Value, String> {
        let path = mori_dir.join(name);
        let text = std::fs::read_to_string(&path).map_err(|e| {
            format!(
                "grammars missing ({}: {e}); {GRAMMARS_HINT}",
                path.display()
            )
        })?;
        serde_json::from_str(&text).map_err(|e| {
            format!("grammars stale ({}: {e}); {GRAMMARS_HINT}", path.display())
        })
    };
    let lock = read_json("grammars.lock.json")?;
    let stamp = read_json(".grammars.stamp")?;
    let stale = |why: String| format!("grammars stale ({why}); {GRAMMARS_HINT}");
    let want = lock["sha256"]
        .as_str()
        .ok_or_else(|| stale("grammars.lock.json has no sha256".into()))?;
    if stamp["archiveSha256"].as_str() != Some(want) {
        return Err(stale("installed from a different archive".into()));
    }
    let tree = mori_dir.join(
        lock["path"]
            .as_str()
            .unwrap_or("internal/cbm/vendored/grammars"),
    );
    let files = stamp["files"]
        .as_object()
        .filter(|files| !files.is_empty())
        .ok_or_else(|| stale("stamp lists no files".into()))?;
    for (rel, stat) in files {
        let recorded = (stat[0].as_u64(), stat[1].as_f64());
        let (Some(size), Some(mtime_ms)) = recorded else {
            return Err(stale(format!("bad stamp entry for {rel}")));
        };
        let meta = std::fs::symlink_metadata(tree.join(rel))
            .map_err(|_| stale(format!("missing: {rel}")))?;
        let seen_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs_f64() * 1000.0);
        let same_mtime = seen_ms.is_some_and(|ms| (ms - mtime_ms).abs() < 1.0);
        if !meta.is_file() || meta.len() != size || !same_mtime {
            return Err(stale(format!("changed since install: {rel}")));
        }
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
    with_ui: bool,
) -> Result<(), String> {
    let (default_cc, default_cxx) = default_compilers(target_os);
    let cc = env::var("CC").unwrap_or_else(|_| default_cc.into());
    let cxx = env::var("CXX").unwrap_or_else(|_| default_cxx.into());
    let mut cmd = Command::new("make");
    cmd.current_dir(mori_dir)
        .args(["-f", "Makefile.cbm"])
        .arg(format!("-j{jobs}"))
        .arg("libmori-c")
        .arg(format!("BUILD_DIR={build_dir_arg}"))
        .arg(format!("CC={cc}"))
        .arg(format!("CXX={cxx}"))
        .args(PINNED_MAKE_VARS)
        .arg(format!("LIBMORI_WITH_UI={}", u8::from(with_ui)))
        // Cargo's jobserver and an outer make must not change -j.
        .env_remove("MAKEFLAGS")
        .env_remove("MFLAGS")
        .env_remove("MAKELEVEL");
    // Cargo locks each profile separately, so `cargo build` and
    // `cargo build --release` (or clippy beside test) can reach this point
    // together. They share BUILD_DIR; one make at a time keeps them from
    // racing on the same objects and archive.
    let build_dir = mori_dir.join(build_dir_arg);
    std::fs::create_dir_all(&build_dir)
        .map_err(|e| format!("could not create {}: {e}", build_dir.display()))?;
    let lock_path = build_dir.join(".libmori-c.lock");
    let lock = std::fs::File::create(&lock_path)
        .map_err(|e| format!("could not open {}: {e}", lock_path.display()))?;
    lock.lock()
        .map_err(|e| format!("could not lock {}: {e}", lock_path.display()))?;
    let output = cmd
        .output()
        .map_err(|e| format!("could not run make in mori/: {e}"))?;
    drop(lock);
    // Stdout of a build script is cargo's channel; keep make's output on
    // stderr (shown with `cargo build -vv` or on failure).
    eprint!("{}", String::from_utf8_lossy(&output.stdout));
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    if !output.status.success() {
        return Err(format!(
            "`make -f Makefile.cbm libmori-c` failed ({}); see the output above",
            output.status
        ));
    }
    Ok(())
}

/// Links the selected archive whole along with the libraries from
/// `LDFLAGS` in `Makefile.cbm` (`-lm -lstdc++ -lpthread -lz`).
///
/// With `-C target-feature=+crt-static` (the static Linux release, like
/// `STATIC=1` for the C binary), zlib and libstdc++ link as archives too: a
/// `dylib` library would make the linker pick the shared object and produce
/// a dynamic binary. libm and libpthread come with the static libc that
/// Rust's std links under crt-static.
fn emit_link_lines(build_dir: &Path, target_os: &str) {
    let crt_static = env::var("CARGO_CFG_TARGET_FEATURE")
        .is_ok_and(|features| features.split(',').any(|f| f == "crt-static"));
    println!("cargo:rustc-link-search=native={}", build_dir.display());
    println!("cargo:rustc-link-lib=static:+whole-archive=mori-c");
    if crt_static && target_os == "linux" {
        // Resolve system archives at the final native link. Bundling them in
        // the Rust rlib would require teaching rustc the platform's GCC paths.
        println!("cargo:rustc-link-lib=static:-bundle=z");
        println!("cargo:rustc-link-lib=static:-bundle=stdc++");
    } else {
        println!("cargo:rustc-link-lib=dylib=z");
        println!("cargo:rustc-link-lib=dylib=m");
        println!("cargo:rustc-link-lib=dylib=pthread");
        if target_os == "macos" {
            println!("cargo:rustc-link-lib=dylib=c++");
        } else {
            println!("cargo:rustc-link-lib=dylib=stdc++");
        }
    }
}
