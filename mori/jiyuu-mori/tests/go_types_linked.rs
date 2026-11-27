//! Verify typed call ownership and provenance through both index drivers.
#![cfg(all(feature = "c-backend", unix))]

mod support;

use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let output = Command::new("mktemp")
            .args(["-d", "/tmp/mori-go-types-XXXXXXXX"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        Self(
            OsStr::from_bytes(output.stdout.strip_suffix(b"\n").unwrap()).into(),
        )
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug, PartialEq)]
struct Edge {
    caller: String,
    target: String,
    properties: Value,
}

fn index(
    bin: &Path,
    scratch: &Path,
    repo: &Path,
    workers: &str,
    expected_mode: Option<&str>,
) -> Vec<Edge> {
    let cache = scratch.join("db");
    let runtime = scratch.join("rt");
    for path in [&cache, &runtime] {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
            .unwrap();
    }
    let command = || {
        let mut command = Command::new(bin);
        command
            .current_dir("/")
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", std::env::var_os("HOME").unwrap_or_default())
            .env("CBM_CACHE_DIR", &cache)
            .env("CBM_RUNTIME_DIR", &runtime)
            .env("CBM_LOG_LEVEL", "debug")
            .env("CBM_WORKERS", workers);
        command
    };
    let args = json!({"repo_path": repo, "name": "go-types", "mode": "full"})
        .to_string();
    let result = command()
        .args(["cli", "--quiet", "index_repository", &args])
        .output();
    let stop = command().args(["daemon", "stop"]).output().unwrap();
    assert!(stop.status.success(), "{stop:?}");
    let result = result.unwrap();
    assert!(result.status.success(), "{result:?}");
    let response: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(response["status"], "indexed", "{response}");

    if let Some(mode) = expected_mode {
        let expected = format!("msg=pipeline.mode mode={mode}");
        let logs = fs::read_dir(cache.join("logs")).unwrap();
        assert!(
            logs.filter_map(Result::ok).any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".worker-log-")
                    && fs::read_to_string(entry.path())
                        .is_ok_and(|log| log.contains(&expected))
            }),
            "missing worker log {expected}",
        );
    }

    let db = Connection::open_with_flags(
        cache.join("go-types.db"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    db.prepare(
        "SELECT caller.name, target.file_path, edge.properties
         FROM edges edge
         JOIN nodes caller ON caller.id = edge.source_id
         JOIN nodes target ON target.id = edge.target_id
         WHERE edge.type = 'CALLS'
           AND caller.name IN ('LocalUse', 'CrossUse')
           AND target.name = 'Mark'
         ORDER BY 1, 2",
    )
    .unwrap()
    .query_map([], |row| {
        let properties = row.get::<_, String>(2)?;
        Ok(Edge {
            caller: row.get(0)?,
            target: row.get(1)?,
            properties: serde_json::from_str(&properties).unwrap(),
        })
    })
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

#[test]
fn resolves_local_and_imported_receiver_calls_in_both_index_drivers() {
    let scratch = Scratch::new();
    let repo = scratch.0.join("repo");
    for dir in ["app", "lib", "decoy", "padding"] {
        fs::create_dir_all(repo.join(dir)).unwrap();
    }
    fs::write(repo.join("go.mod"), "module example.net/types\n\ngo 1.22\n")
        .unwrap();
    fs::write(
        repo.join("app/app.go"),
        r#"package app
import api "example.net/types/lib"
type Local struct{}
func (*Local) Mark() int { return 1 }
func LocalUse(value *Local) int { return value.Mark() }
func CrossUse(value *api.External) int { return value.Mark() }
"#,
    )
    .unwrap();
    fs::write(
        repo.join("lib/lib.go"),
        "package lib\ntype External struct{}\n",
    )
    .unwrap();
    fs::write(
        repo.join("lib/methods.go"),
        "package lib\nfunc (*External) Mark() int { return 2 }\n",
    )
    .unwrap();
    fs::write(
        repo.join("decoy/decoy.go"),
        "package decoy\ntype External struct{}\nfunc (*External) Mark() int { return 0 }\n",
    )
    .unwrap();
    // Force enough discovered Go files for the two-worker driver.
    for i in 0..60 {
        fs::write(
            repo.join(format!("padding/pad{i}.go")),
            format!("package padding\nvar Pad{i} = {i}\n"),
        )
        .unwrap();
    }
    let candidate = Path::new(env!("CARGO_BIN_EXE_mori"));
    let serial = index(
        candidate,
        &scratch.0.join("serial"),
        &repo,
        "1",
        Some("sequential"),
    );
    let parallel = index(
        candidate,
        &scratch.0.join("parallel"),
        &repo,
        "2",
        Some("parallel"),
    );
    assert_eq!(serial, parallel);

    if let Some(reference) = support::c_reference_bin() {
        let expected =
            index(&reference, &scratch.0.join("reference"), &repo, "1", None);
        assert_eq!(serial, expected);
    }
    assert_eq!(serial.len(), 2, "{serial:?}");
    for (edge, (caller, target)) in serial
        .iter()
        .zip([("CrossUse", "lib/methods.go"), ("LocalUse", "app/app.go")])
    {
        assert_eq!(edge.caller, caller);
        assert_eq!(edge.target, target);
        assert!(
            edge.properties["strategy"]
                .as_str()
                .is_some_and(|strategy| strategy.starts_with("lsp_")),
            "{edge:?}",
        );
    }
}
