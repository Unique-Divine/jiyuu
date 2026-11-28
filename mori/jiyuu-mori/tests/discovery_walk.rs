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

    fn command(&self) -> Command {
        let side = &self.0;
        let mut dirs = fs::DirBuilder::new();
        dirs.recursive(true).mode(0o700);
        dirs.create(side.join("cache")).unwrap();
        dirs.create(side.join("rt")).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_mori"));
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", side)
            .env("XDG_CONFIG_HOME", side.join("config"))
            .env("CBM_CACHE_DIR", side.join("cache"))
            .env("CBM_RUNTIME_DIR", side.join("rt"))
            .env("CBM_LOG_LEVEL", "debug")
            .env("CBM_PROFILE", "1")
            .env("CBM_LOG_FORMAT", "json");
        command
    }

    fn index(&self, mode: &str) -> (Connection, Vec<Json>, Json) {
        let cache = self.0.join("cache");
        let logs_path = cache.join("logs");
        if logs_path.exists() {
            fs::remove_dir_all(&logs_path).unwrap();
        }
        let run = self
            .command()
            .args(["cli", "--verbose", "index_repository", "--repo_path"])
            .arg(self.0.join("repo"))
            .args(["--mode", mode])
            .output()
            .unwrap();
        let stopped = self.command().args(["daemon", "stop"]).output().unwrap();
        assert!(stopped.status.success(), "{stopped:?}");
        assert!(run.status.success(), "{run:?}");
        let reply: Json = serde_json::from_slice(&run.stdout).unwrap();
        assert_eq!(reply["status"], "indexed", "{reply}");
        let project = reply["project"].as_str().unwrap();
        let mut logs = String::from_utf8_lossy(&run.stderr).into_owned();
        for entry in fs::read_dir(logs_path).unwrap() {
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

fn witness(events: &[Json], phase: &str, mode: Option<&str>) {
    assert!(
        events.iter().any(|event| {
            event["phase"] == phase
                && mode.is_none_or(|mode| event["mode"] == mode)
                && event["backend"] == "rust"
        }),
        "{events:?}"
    );
}

fn write(root: &Path, path: &str, source: &str) {
    fs::write(root.join(path), source).unwrap();
}

#[test]
fn rust_walk_runs_at_requested_and_stored_depths_with_resource_policy() {
    let scratch = Scratch::new();
    let repo = scratch.0.join("repo");
    fs::create_dir_all(repo.join("tools")).unwrap();
    fs::create_dir(repo.join("ignored")).unwrap();
    write(&repo, "main.go", "package main\nfunc main() {}\n");
    write(&repo, "tools/util.go", "package tools\nfunc Before() {}\n");
    write(&repo, "ignored/hidden.go", "package hidden\n");
    write(&repo, ".cbmignore", "/ignored/\nskip.go\n");
    write(&repo, "skip.go", "package main\nfunc Skip() {}\n");

    let config = scratch
        .command()
        .args(["config", "set", "index_max_files", "10"])
        .output()
        .unwrap();
    assert!(config.status.success(), "{config:?}");

    let (_, first_logs, _) = scratch.index("full");
    witness(&first_logs, "initial", Some("full"));
    witness(&first_logs, "fresh_manifest", None);

    write(&repo, "tools/util.go", "package tools\nfunc After() {}\n");
    let (rust, logs, reply) = scratch.index("fast");
    witness(&logs, "initial", Some("fast"));
    witness(&logs, "rediscover", Some("full"));
    witness(&logs, "fresh_manifest", None);
    assert_eq!(reply["excluded"]["count"], 2);
    assert_eq!(reply["not_indexed_files_count"], 1);
    assert!(!reply["adr_hint"].is_null(), "{reply}");

    for sql in [
        "SELECT rel_path, sha256, size, mtime_ns FROM file_hashes WHERE rel_path = 'tools/util.go'",
        "SELECT rel_path, kind, detail FROM index_coverage",
    ] {
        assert!(!rows(&rust, sql).is_empty(), "{sql}");
    }
    assert_eq!(
        rows(&rust, "SELECT name FROM nodes WHERE name = 'After'"),
        [vec![Value::Text("After".into())],]
    );
}

#[test]
fn mcp_auto_index_uses_the_rust_bounded_walker() {
    use std::io::Write;
    use std::process::Stdio;
    use std::thread;
    use std::time::{Duration, Instant};

    for (limit, expected_event) in [
        ("1", "daemon.autoindex.skipped"),
        ("2", "daemon.index.result"),
    ] {
        let scratch = Scratch::new();
        let repo = scratch.0.join("repo");
        fs::create_dir_all(repo.join("child")).unwrap();
        write(&repo, "main.go", "package main\nfunc main() {}\n");
        write(&repo, "child/extra.go", "package child\nfunc Extra() {}\n");
        write(&repo, ".cbmignore", "ignored.go\n");
        write(&repo, "ignored.go", "package main\nfunc Ignored() {}\n");
        write(&repo, "not-source.blob", "unknown language\n");
        std::os::unix::fs::symlink("main.go", repo.join("alias.go")).unwrap();
        for (key, value) in [("auto_index", "true"), ("auto_index_limit", limit)]
        {
            let config = scratch
                .command()
                .args(["config", "set", key, value])
                .output()
                .unwrap();
            assert!(config.status.success(), "{config:?}");
        }
        let mut command = scratch.command();
        command
            .current_dir(&repo)
            .env("CBM_ALLOWED_ROOT", &repo)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        let mut input = child.stdin.take().unwrap();
        input
            .write_all(concat!(
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"linked-test","version":"1"}}}"#,
                "\n",
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                "\n",
            ).as_bytes())
            .unwrap();
        let log = scratch.0.join("cache/logs/cbm-daemon.log");
        let deadline = Instant::now() + Duration::from_secs(30);
        let events = loop {
            let events: Vec<Json> = fs::read_to_string(&log)
                .unwrap_or_default()
                .lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect();
            if events.iter().any(|event| event["event"] == expected_event)
                || Instant::now() >= deadline
                || child.try_wait().unwrap().is_some()
            {
                break events;
            }
            thread::sleep(Duration::from_millis(25));
        };
        drop(input);
        let output = child.wait_with_output().unwrap();
        let stopped =
            scratch.command().args(["daemon", "stop"]).output().unwrap();
        assert!(stopped.status.success(), "{stopped:?}");
        assert!(output.status.success(), "{output:?}");
        let reply: Json = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(reply["id"], 1, "{reply}");
        assert!(reply.get("result").is_some(), "{reply}");
        assert!(
            events.iter().any(|event| {
                event["event"] == "discover.admission_backend"
                    && event["implementation"] == "rust"
            }),
            "{events:?}"
        );
        let result = events
            .iter()
            .find(|event| event["event"] == expected_event)
            .unwrap_or_else(|| panic!("{events:?}"));
        if limit == "1" {
            assert_eq!(result["reason"], "too_many_files", "{result}");
            assert_eq!(result["files"], "2", "{result}");
            assert_eq!(result["limit"], "1", "{result}");
        } else {
            assert_eq!(result["status"], "indexed", "{result}");
        }
    }
}
