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
    run_in(bin, &Scratch::new(side), args)
}

fn run_in(bin: &Path, scratch: &Scratch, args: &[&str]) -> Output {
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

/// A small repository written by the test: Python and Go, each with one
/// call. Both binaries index the same path, so project names match.
struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let dir = PathBuf::from(format!("/tmp/mori-fx{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let files = [
            (
                "py/app.py",
                "def helper(x):\n    return x + 1\n\n\ndef run():\n    \
                 return helper(2)\n",
            ),
            (
                "go/calc.go",
                "package calc\n\nfunc Add(a, b int) int {\n\treturn a + b\n\
                 }\n\nfunc Twice(a int) int {\n\treturn Add(a, a)\n}\n",
            ),
            ("go/go.mod", "module example.com/calc\n\ngo 1.22\n"),
        ];
        for (rel, text) in files {
            let path = dir.join(rel);
            fs::create_dir_all(path.parent().expect("parent"))
                .expect("create fixture dir");
            fs::write(&path, text).expect("write fixture file");
        }
        Fixture(dir)
    }

    fn project(&self) -> String {
        // Project names are the path with `/` turned into `-`.
        self.0
            .to_string_lossy()
            .trim_start_matches('/')
            .replace('/', "-")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The value of `"key":<number>` in the index result.
fn json_number(text: &str, key: &str) -> Option<u64> {
    let start = text.find(&format!("\"{key}\":"))? + key.len() + 3;
    let digits: String = text[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// Indexes the fixture with each binary, which goes through the temporary
/// daemon and the index worker that the binary re-executes as itself, then
/// compares queries over the two graphs.
#[test]
fn index_through_worker_reexec_matches_c() {
    let fixture = Fixture::new();
    let project = fixture.project();
    let repo = fixture.0.to_string_lossy().into_owned();
    let sides = [("c", c_binary()), ("rs", PathBuf::from(MORI))];
    let mut results = Vec::new();
    for (side, bin) in &sides {
        let scratch = Scratch::new(side);
        let index = run_in(
            bin,
            &scratch,
            &["cli", "--quiet", "index_repository", "--repo_path", &repo],
        );
        let text = String::from_utf8_lossy(&index.stdout).into_owned();
        assert!(
            index.status.success() && text.contains("\"status\":\"indexed\""),
            "{side} index failed: {text}\n{}",
            String::from_utf8_lossy(&index.stderr)
        );
        let counts = (json_number(&text, "nodes"), json_number(&text, "edges"));
        assert!(counts.0.is_some_and(|n| n > 0), "{side}: {text}");
        let queries: Vec<Output> = [
            vec!["cli", "get_graph_schema", "--project", &project],
            vec![
                "cli",
                "get_graph_schema",
                "--project",
                &project,
                "--format",
                "json",
                "--diagnostics",
                "full",
                "--limit",
                "4",
                "--offset",
                "1",
            ],
            vec![
                "cli",
                "get_graph_schema",
                "--project",
                &project,
                "--limit",
                "1",
                "--offset",
                "9999",
            ],
            vec![
                "cli",
                "search_graph",
                "--project",
                &project,
                "--name-pattern",
                "^(Add|Twice|helper|run)$",
                "--format",
                "json",
            ],
            vec![
                "cli",
                "trace_path",
                "--project",
                &project,
                "--function-name",
                "Add",
                "--direction",
                "inbound",
            ],
            vec![
                "cli",
                "trace_path",
                "--project",
                &project,
                "--function-name",
                "helper",
                "--direction",
                "inbound",
            ],
        ]
        .iter()
        .map(|args| run_in(bin, &scratch, args))
        .collect();
        results.push((counts, queries));
    }
    let (c, rs) = (&results[0], &results[1]);
    assert_eq!(c.0, rs.0, "node and edge counts differ");
    for (c_out, rs_out) in c.1.iter().zip(&rs.1) {
        assert!(c_out.status.success(), "{c_out:?}");
        assert_eq!(
            (&c_out.status.code(), &c_out.stdout, &c_out.stderr),
            (&rs_out.status.code(), &rs_out.stdout, &rs_out.stderr),
            "query output differs"
        );
    }
    let trace = String::from_utf8_lossy(&c.1[1].stdout);
    assert!(trace.contains(".go.Twice"), "Add caller missing: {trace}");
}
