//! Exercise the opt-in stage writer through the public index command.
#![cfg(all(feature = "c-backend", unix))]

use rusqlite::{Connection, OpenFlags, types::Value};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let created = Command::new("mktemp")
            .args(["-d", "/tmp/mori-stage-XXXXXXXX"])
            .output()
            .unwrap();
        assert!(created.status.success(), "{created:?}");
        Self(PathBuf::from(OsStr::from_bytes(
            created.stdout.strip_suffix(b"\n").unwrap(),
        )))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn index(
    repo: &Path,
    run_dir: &Path,
    depth: &str,
    writer: &str,
) -> (Connection, String) {
    let cache = run_dir.join("cache");
    let runtime = run_dir.join("rt");
    let mut dirs = fs::DirBuilder::new();
    dirs.recursive(true).mode(0o700);
    dirs.create(&cache).unwrap();
    dirs.create(&runtime).unwrap();
    let command = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mori"));
        command
            .current_dir("/")
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", std::env::var_os("HOME").unwrap_or_default())
            .env("CBM_CACHE_DIR", &cache)
            .env("CBM_RUNTIME_DIR", &runtime)
            .env("CBM_PROFILE", "1")
            .env("CBM_LOG_LEVEL", "info")
            .env("MORI_IMPL_GRAPH_WRITER", writer);
        command
    };
    let indexed = command()
        .args(["cli", "--quiet", "index_repository", "--repo_path"])
        .arg(repo)
        .args(["--mode", depth])
        .output()
        .unwrap();
    let stopped = command().args(["daemon", "stop"]).output().unwrap();
    assert!(stopped.status.success(), "{stopped:?}");
    assert!(indexed.status.success(), "{indexed:?}");
    let reply: serde_json::Value = serde_json::from_slice(&indexed.stdout)
        .unwrap_or_else(|error| panic!("index response: {error}: {indexed:?}"));
    assert_eq!(reply["status"], "indexed", "{reply}");
    let project = reply["project"].as_str().unwrap();
    let mut output = String::from_utf8_lossy(&indexed.stderr).into_owned();
    for entry in fs::read_dir(cache.join("logs")).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            output.push_str(&fs::read_to_string(entry.path()).unwrap());
        }
    }
    let db = Connection::open_with_flags(
        cache.join(format!("{project}.db")),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    (db, output)
}

fn graph_rows(db: &Connection, query: &str) -> Vec<Vec<Value>> {
    let mut stmt = db.prepare(query).unwrap();
    let columns = stmt.column_count();
    stmt.query_map([], |row| {
        (0..columns).map(|column| row.get(column)).collect()
    })
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

fn count(db: &Connection, query: &str) -> i64 {
    db.query_row(query, [], |row| row.get(0)).unwrap()
}

fn assert_published(db: &Connection) {
    let generation = count(
        db,
        "SELECT CAST(v AS INTEGER) FROM store_meta WHERE k = 'mutation_gen'",
    );
    assert!(generation > 0);
    assert!(count(db, "SELECT count(*) FROM file_hashes") > 0);
    assert!(
        count(
            db,
            "SELECT count(*) FROM nodes_fts WHERE nodes_fts MATCH 'parse' "
        ) > 0
    );
    let integrity: String = db
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .unwrap();
    assert_eq!(integrity, "ok");
}

#[test]
fn rust_writes_graph_only_rebuilds_and_c_writes_semantic_vectors() {
    let scratch = Scratch::new();
    let repo = scratch.0.join("source");
    fs::create_dir(&repo).unwrap();
    fs::write(
        repo.join("source.go"),
        r#"package source

// Parse normalizes a request into a stable token range.
func Parse(input string) string {
    return input
}

// Render formats a parsed request for output on the console.
func Render(input string) string {
    parsed := Parse(input)
    return parsed + "result"
}

// Inspect parses multiple requests, checking their output range.
func Inspect(input string) string {
    rendered := Render(input)
    return rendered
}
"#,
    )
    .unwrap();

    let (reference, _) = index(&repo, &scratch.0.join("control"), "fast", "c");
    let (rust_graph, logs) =
        index(&repo, &scratch.0.join("candidate"), "fast", "rust");
    assert!(
        logs.lines().any(|line| line.contains("gbuf.dump_writer")
            && line.contains("implementation=rust")),
        "{logs}"
    );
    assert_published(&rust_graph);
    for query in [
        "SELECT id, project, label, name, qualified_name, file_path, start_line, end_line, properties FROM nodes ORDER BY id",
        "SELECT id, project, source_id, target_id, type, properties FROM edges ORDER BY id",
    ] {
        assert_eq!(
            graph_rows(&reference, query),
            graph_rows(&rust_graph, query),
            "{query}"
        );
    }

    let (semantic, logs) =
        index(&repo, &scratch.0.join("semantic"), "full", "rust");
    assert!(
        logs.lines().any(|line| line.contains("gbuf.dump_writer")
            && line.contains("reason=semantic_vectors")),
        "{logs}"
    );
    assert_published(&semantic);
    assert!(count(&semantic, "SELECT count(*) FROM node_vectors") > 0);
}
