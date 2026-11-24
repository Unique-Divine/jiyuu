//! Exercise the opt-in stage writer through the public index command.
#![cfg(all(feature = "c-backend", unix))]

use rusqlite::{Connection, OpenFlags, types::Value};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const MORI: &str = env!("CARGO_BIN_EXE_mori");

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
    bin: &Path,
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
        let mut command = Command::new(bin);
        command
            .current_dir("/")
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", std::env::var_os("HOME").unwrap_or_default())
            .env("CBM_CACHE_DIR", &cache)
            .env("CBM_RUNTIME_DIR", &runtime)
            .env("CBM_PROFILE", "1")
            .env("CBM_LOG_LEVEL", "info")
            .env("MORI_IMPL_GRAPH_WRITER", writer)
            .env("CBM_WORKERS", "2");
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
fn rust_writes_graph_records_and_vectors_during_full_rebuilds() {
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

    let bin = Path::new(MORI);
    let (reference, _) =
        index(bin, &repo, &scratch.0.join("control"), "fast", "c");
    let (rust_graph, logs) =
        index(bin, &repo, &scratch.0.join("candidate"), "fast", "rust");
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

    let (semantic_reference, _) = index(
        bin,
        &repo,
        &scratch.0.join("semantic-reference"),
        "full",
        "c",
    );
    let (semantic, logs) =
        index(bin, &repo, &scratch.0.join("semantic"), "full", "rust");
    assert!(
        logs.lines().any(|line| line.contains("gbuf.dump_writer")
            && line.contains("implementation=rust")),
        "{logs}"
    );
    assert!(
        logs.contains("msg=pipeline.full_build_driver implementation=rust"),
        "{logs}"
    );
    assert_published(&semantic);
    for query in [
        "SELECT node_id, project, vector FROM node_vectors ORDER BY node_id",
        "SELECT project, token, vector, idf FROM token_vectors ORDER BY project, token",
    ] {
        let expected = graph_rows(&semantic_reference, query);
        assert!(!expected.is_empty(), "{query}");
        assert_eq!(graph_rows(&semantic, query), expected, "{query}");
    }
}

#[test]
fn full_build_driver_preserves_the_extraction_boundary() {
    let scratch = Scratch::new();
    let repo = scratch.0.join("source");
    fs::create_dir(&repo).unwrap();
    for file in 0..50 {
        fs::write(
            repo.join(format!("part_{file}.py")),
            format!("def part_{file}():\n    return {file}\n"),
        )
        .unwrap();
    }

    let reference = std::env::var_os("MORI_C_REFERENCE_BIN").map(PathBuf::from);
    if let Some(bin) = &reference {
        assert!(
            bin.is_absolute() && bin.is_file(),
            "invalid C reference {bin:?}"
        );
    }
    for (files, mode) in [(50, "sequential"), (51, "parallel")] {
        if files == 51 {
            fs::write(
                repo.join("extra.py"),
                "from part_0 import part_0\ndef extra():\n    return part_0()\n",
            )
            .unwrap();
        }
        let (rust, rust_logs) = index(
            Path::new(MORI),
            &repo,
            &scratch.0.join(format!("rust-{files}")),
            "fast",
            "c",
        );
        let evidence = if files == 51 {
            "msg=pipeline.mode mode=parallel workers=2 files=51".to_owned()
        } else {
            format!("msg=pipeline.mode mode={mode} files={files}")
        };
        assert!(rust_logs.contains(&evidence), "{rust_logs}");
        assert!(rust_logs.contains(&format!("msg=pipeline.full_build_driver implementation=rust files={files}")), "{rust_logs}");
        assert_eq!(
            files,
            count(&rust, "SELECT count(*) FROM nodes WHERE label='Function'")
        );
        assert!(
            count(
                &rust,
                "SELECT CAST(v AS INTEGER) FROM store_meta WHERE k='mutation_gen'"
            ) > 0
        );
        let Some(bin) = &reference else {
            continue;
        };
        let (c, c_logs) = index(
            bin,
            &repo,
            &scratch.0.join(format!("c-{files}")),
            "fast",
            "c",
        );
        assert!(c_logs.contains(&evidence), "{c_logs}");
        assert!(
            !c_logs
                .contains("msg=pipeline.full_build_driver implementation=rust"),
            "{c_logs}"
        );
        for sql in [
            "SELECT label, name, qualified_name, file_path, start_line, end_line, properties FROM nodes ORDER BY qualified_name",
            "SELECT s.qualified_name, t.qualified_name, e.type, e.properties FROM edges e JOIN nodes s ON s.id=e.source_id JOIN nodes t ON t.id=e.target_id ORDER BY 1, 2, 3, 4",
            "SELECT rel_path, sha256, size, mtime_ns FROM file_hashes ORDER BY rel_path",
        ] {
            let expected = graph_rows(&c, sql);
            assert!(!expected.is_empty(), "{sql}");
            assert_eq!(expected, graph_rows(&rust, sql), "{sql}");
        }
    }
}
