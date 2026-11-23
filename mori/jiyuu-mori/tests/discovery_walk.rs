//! Exercise discovery selection through the CLI, daemon, and index worker.
#![cfg(all(feature = "c-backend", unix))]

use rusqlite::{Connection, OpenFlags, types::Value};
use serde_json::Value as Json;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let output = Command::new("mktemp")
            .args(["-d", "/tmp/mori-d-XXXXXXXX"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let path = OsStr::from_bytes(output.stdout.strip_suffix(b"\n").unwrap());
        Self(PathBuf::from(path))
    }

    fn command(&self, backend: Option<&str>) -> Command {
        let side = self.0.join(backend.unwrap_or("default"));
        let mut dirs = fs::DirBuilder::new();
        dirs.recursive(true).mode(0o700);
        dirs.create(side.join("cache")).unwrap();
        dirs.create(side.join("rt")).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_mori"));
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &side)
            .env("XDG_CONFIG_HOME", side.join("config"))
            .env("CBM_CACHE_DIR", side.join("cache"))
            .env("CBM_RUNTIME_DIR", side.join("rt"))
            .env("CBM_LOG_LEVEL", "debug")
            .env("CBM_PROFILE", "1")
            .env("CBM_LOG_FORMAT", "json");
        if let Some(backend) = backend {
            command.env("MORI_IMPL_DISCOVERY", backend);
        }
        command
    }

    fn index(
        &self,
        backend: Option<&str>,
        mode: &str,
    ) -> (Connection, Vec<Json>, Json) {
        let run = self
            .command(backend)
            .args(["cli", "--verbose", "index_repository", "--repo_path"])
            .arg(self.0.join("repo"))
            .args(["--mode", mode])
            .output()
            .unwrap();
        let stopped = self
            .command(backend)
            .args(["daemon", "stop"])
            .output()
            .unwrap();
        assert!(stopped.status.success(), "{stopped:?}");
        assert!(run.status.success(), "{run:?}");
        let reply: Json = serde_json::from_slice(&run.stdout).unwrap();
        assert_eq!(reply["status"], "indexed", "{reply}");
        let project = reply["project"].as_str().unwrap();
        let cache = self.0.join(backend.unwrap_or("default")).join("cache");
        let mut logs = String::from_utf8_lossy(&run.stderr).into_owned();
        for entry in fs::read_dir(cache.join("logs")).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_file() {
                logs.push('\n');
                logs.push_str(&fs::read_to_string(entry.path()).unwrap());
            }
        }
        let events = logs
            .lines()
            .filter_map(|line| serde_json::from_str::<Json>(line).ok())
            .filter(|event| event["event"] == "pipeline.discovery_backend")
            .collect::<Vec<_>>();
        assert!(!events.is_empty(), "{logs}");
        let db = Connection::open_with_flags(
            cache.join(format!("{project}.db")),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        (db, events, reply)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn rows(db: &Connection, sql: &str) -> Vec<Vec<Value>> {
    let mut stmt = db.prepare(sql).unwrap();
    let columns = stmt.column_count();
    stmt.query_map([], |row| (0..columns).map(|col| row.get(col)).collect())
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn witness(events: &[Json], phase: &str, mode: &str, backend: &str) {
    assert!(
        events.iter().any(|event| {
            event["phase"] == phase
                && event["mode"] == mode
                && event["backend"] == backend
        }),
        "{events:?}"
    );
}

fn write(root: &Path, path: &str, source: &str) {
    fs::write(root.join(path), source).unwrap();
}

#[test]
fn rust_walk_runs_at_requested_and_stored_depths() {
    let scratch = Scratch::new();
    let repo = scratch.0.join("repo");
    fs::create_dir_all(repo.join("tools")).unwrap();
    fs::create_dir(repo.join("ignored")).unwrap();
    write(&repo, "main.go", "package main\nfunc main() {}\n");
    write(&repo, "tools/util.go", "package tools\nfunc Before() {}\n");
    write(&repo, "ignored/hidden.go", "package hidden\n");
    write(&repo, ".cbmignore", "/ignored/\nskip.go\n");
    write(&repo, "skip.go", "package main\nfunc Skip() {}\n");

    let (_, rust_logs, _) = scratch.index(Some("rust"), "full");
    witness(&rust_logs, "initial", "full", "rust");
    let (_, c_logs, _) = scratch.index(None, "full");
    witness(&c_logs, "initial", "full", "c");

    write(&repo, "tools/util.go", "package tools\nfunc After() {}\n");
    let (c, c_logs, c_reply) = scratch.index(None, "fast");
    let (rust, rust_logs, rust_reply) = scratch.index(Some("rust"), "fast");
    witness(&c_logs, "initial", "fast", "c");
    witness(&c_logs, "rediscover", "full", "c");
    witness(&rust_logs, "initial", "fast", "rust");
    witness(&rust_logs, "rediscover", "full", "rust");

    for field in ["excluded", "not_indexed_files", "adr_hint"] {
        assert!(!c_reply[field].is_null(), "{c_reply}");
        assert_eq!(c_reply[field], rust_reply[field], "{field}");
    }
    assert_eq!(rust_reply["excluded"]["count"], 2);
    assert_eq!(rust_reply["not_indexed_files_count"], 1);

    for sql in [
        "SELECT rel_path, sha256, size, mtime_ns FROM file_hashes ORDER BY rel_path",
        "SELECT rel_path, kind, detail FROM index_coverage ORDER BY rel_path, kind",
        "SELECT label, name, file_path FROM nodes ORDER BY label, name, file_path",
    ] {
        let expected = rows(&c, sql);
        assert!(!expected.is_empty(), "{sql}");
        assert_eq!(expected, rows(&rust, sql), "{sql}");
    }
    assert_eq!(
        rows(&rust, "SELECT name FROM nodes WHERE name = 'After'"),
        [vec![Value::Text("After".into())],]
    );
}

#[test]
fn discovery_resource_policy_keeps_the_native_walk() {
    let scratch = Scratch::new();
    let repo = scratch.0.join("repo");
    fs::create_dir(&repo).unwrap();
    write(&repo, "main.go", "package main\nfunc main() {}\n");
    let config = scratch
        .command(Some("rust"))
        .args(["config", "set", "index_max_files", "10"])
        .output()
        .unwrap();
    assert!(config.status.success(), "{config:?}");
    let (_, logs, _) = scratch.index(Some("rust"), "full");
    assert!(
        logs.iter().any(|event| {
            event["phase"] == "initial"
                && event["backend"] == "c"
                && event["reason"] == "resource_policy"
        }),
        "{logs:?}"
    );
}
