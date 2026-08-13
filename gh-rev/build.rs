use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

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
