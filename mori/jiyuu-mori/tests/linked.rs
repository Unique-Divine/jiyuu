//! Runs `mori` and the C `codebase-memory-mcp` from the same build directory
//! and checks that they behave the same. Needs feature `c-backend` and the
//! C reference binary (`MORI_C_REFERENCE=1`, set by `just check-linked`), so
//! a pure-Rust `cargo test` compiles this file to nothing.
#![cfg(feature = "c-backend")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

const MORI: &str = env!("CARGO_BIN_EXE_mori");

fn c_binary() -> PathBuf {
    // build.rs sets MORI_C_BUILD_DIR whenever c-backend is on.
    let bin = Path::new(env!("MORI_C_BUILD_DIR")).join("codebase-memory-mcp");
    assert!(
        bin.is_file(),
        "{} is missing; build with MORI_C_REFERENCE=1 (just check-linked)",
        bin.display()
    );
    bin
}

/// A short, mode-700 scratch directory: long runtime paths overflow the
/// Unix socket path limit.
struct Scratch(PathBuf);

impl Scratch {
    fn new(side: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = PathBuf::from(format!(
            "/tmp/mori-t{}-{n}-{side}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        for sub in ["cache", "rt"] {
            let path = dir.join(sub);
            fs::create_dir_all(&path).expect("create scratch dir");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                .expect("chmod scratch dir");
        }
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Runs `bin` with a minimal environment pointed at its own scratch caches.
fn run(bin: &Path, side: &str, args: &[&str]) -> Output {
    let scratch = Scratch::new(side);
    Command::new(bin)
        .args(args)
        .current_dir("/")
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", std::env::var_os("HOME").unwrap_or_default())
        .env("CBM_CACHE_DIR", scratch.0.join("cache"))
        .env("CBM_RUNTIME_DIR", scratch.0.join("rt"))
        .output()
        .expect("run binary")
}

fn assert_same(args: &[&str]) {
    let c = run(&c_binary(), "c", args);
    let rs = run(Path::new(MORI), "rs", args);
    let show = |o: &Output| {
        format!(
            "status {:?}\nstdout {}\nstderr {}",
            o.status.code(),
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        )
    };
    assert_eq!(
        (c.status.code(), &c.stdout, &c.stderr),
        (rs.status.code(), &rs.stdout, &rs.stderr),
        "{args:?} differs\n--- C\n{}\n--- mori\n{}",
        show(&c),
        show(&rs)
    );
}

#[test]
fn version_matches_c() {
    assert_same(&["--version"]);
}

#[test]
fn list_projects_matches_c() {
    assert_same(&["cli", "list_projects"]);
}

#[test]
fn unknown_tool_error_matches_c() {
    assert_same(&["cli", "no_such_tool"]);
}
