use std::env;
use std::process::Command;

fn main() {
    println!(
        "cargo:rustc-env=GH_REV_BUILD_VERSION={}",
        env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_string())
    );

    let commit = git_output(&["rev-parse", "HEAD"])
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=GH_REV_BUILD_COMMIT={commit}");

    let dirty = git_output(&["status", "--porcelain"])
        .is_some_and(|status| !status.trim().is_empty());
    println!("cargo:rustc-env=GH_REV_BUILD_DIRTY={dirty}");
}

fn git_output(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8(output.stdout).ok()?.trim().to_owned())
}
