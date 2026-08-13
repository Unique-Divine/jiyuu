//! Embeds package and Git provenance in `gh-rev` at build time.
//!
//! Review state is long-lived, while installed binaries can lag the source
//! checkout. The CLI therefore exposes its package version, commit, and local
//! package dirtiness so operators can identify the executable that made a
//! ledger change. The build script also asks Cargo to rebuild when relevant
//! source or Git-reference inputs change, avoiding stale provenance after a
//! branch move or package edit.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Emits the provenance values that `gh-rev --version` returns at runtime.
///
/// Git failures are intentionally non-fatal: crates can build from exported
/// source archives, in which case the runtime command reports unknown metadata
/// rather than preventing use of the review ledger.
fn main() {
    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR")
            .expect("CARGO_MANIFEST_DIR must be set by Cargo"),
    );
    println!(
        "cargo:rustc-env=GH_REV_BUILD_VERSION={}",
        env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_string())
    );

    for path in [
        "Cargo.toml",
        "Cargo.lock",
        "build.rs",
        "src",
        "README.md",
        "justfile",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
    watch_git_path(&manifest_dir, "HEAD");
    watch_git_path(&manifest_dir, "index");
    watch_git_path(&manifest_dir, "packed-refs");
    if let Some(reference) =
        git_output(&manifest_dir, &["symbolic-ref", "-q", "HEAD"])
    {
        watch_git_path(&manifest_dir, &reference);
    }

    let commit = git_output(&manifest_dir, &["rev-parse", "HEAD"])
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=GH_REV_BUILD_COMMIT={commit}");

    let dirty = git_output(
        &manifest_dir,
        &[
            "status",
            "--porcelain",
            "--untracked-files=normal",
            "--",
            ".",
        ],
    )
    .map(|status| (!status.trim().is_empty()).to_string())
    .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=GH_REV_BUILD_DIRTY={dirty}");
}

/// Tells Cargo to rerun this script when a Git metadata path changes.
///
/// Git may return a relative path for a normal checkout or an absolute path for
/// a worktree, so the path is normalized before passing it to Cargo.
fn watch_git_path(manifest_dir: &Path, git_path: &str) {
    let Some(path) =
        git_output(manifest_dir, &["rev-parse", "--git-path", git_path])
    else {
        return;
    };
    let path = PathBuf::from(path);
    let path = if path.is_absolute() {
        path
    } else {
        manifest_dir.join(path)
    };
    println!("cargo:rerun-if-changed={}", path.display());
}

/// Runs Git from the package directory and returns trimmed stdout on success.
///
/// A missing Git executable, non-Git source archive, or failed Git query is
/// represented as `None` so callers can make provenance optional.
fn git_output(worktree: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(worktree)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8(output.stdout).ok()?.trim().to_owned())
}
