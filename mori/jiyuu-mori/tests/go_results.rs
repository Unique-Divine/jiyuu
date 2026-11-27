//! Verify Go result-list encoding through linked cross-file resolution.
#![cfg(all(feature = "c-backend", unix))]

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
            .args(["-d", "/tmp/mori-go-results-XXXXXXXX"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        Self(PathBuf::from(OsStr::from_bytes(
            output.stdout.strip_suffix(b"\n").unwrap(),
        )))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug, PartialEq, Eq)]
struct CallEdge {
    caller: String,
    callee: String,
    target_file: String,
    properties: String,
}

fn index(root: &Path, repo: &Path, workers: &str) -> Vec<CallEdge> {
    const PROJECT: &str = "go-results-probe";
    let cache = root.join("cache");
    let runtime = root.join("rt");
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
            .env("CBM_LOG_LEVEL", "debug")
            .env("CBM_WORKERS", workers);
        command
    };
    let args = json!({"repo_path": repo, "name": PROJECT, "mode": "full"});
    let result = command()
        .args(["cli", "--quiet", "index_repository", &args.to_string()])
        .output()
        .unwrap();
    let stop = command().args(["daemon", "stop"]).output().unwrap();
    assert!(stop.status.success(), "{stop:?}");
    assert!(result.status.success(), "{result:?}");
    let response: Value = serde_json::from_slice(&result.stdout)
        .unwrap_or_else(|error| panic!("{error}: {result:?}"));
    assert_eq!(response["status"], "indexed", "{response}");

    let ran_rust = fs::read_dir(cache.join("logs")).unwrap().any(|entry| {
        let entry = entry.unwrap();
        entry
            .file_name()
            .to_string_lossy()
            .starts_with(".worker-log-")
            && fs::read_to_string(entry.path())
                .unwrap()
                .contains("msg=lsp.go_result_list implementation=rust")
    });
    assert!(ran_rust, "worker did not run the Rust Go result encoder");

    let db = Connection::open_with_flags(
        cache.join(format!("{PROJECT}.db")),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    db.prepare(
        "SELECT caller.name, target.name, target.file_path, edge.properties
         FROM edges edge
         JOIN nodes caller ON caller.id = edge.source_id
         JOIN nodes target ON target.id = edge.target_id
         WHERE edge.type = 'CALLS'
           AND caller.name IN ('UseTuple', 'UseGrouped', 'UseNamed')
           AND target.name IN ('TupleMark', 'GroupedMark', 'NamedMark')
         ORDER BY 1, 2, 3",
    )
    .unwrap()
    .query_map([], |row| {
        Ok(CallEdge {
            caller: row.get(0)?,
            callee: row.get(1)?,
            target_file: row.get(2)?,
            properties: row.get(3)?,
        })
    })
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

#[test]
fn go_results_keep_typed_cross_file_call_provenance() {
    let scratch = Scratch::new();
    let repo = scratch.0.join("repo");
    for package in ["app", "encoder", "decoy"] {
        fs::create_dir_all(repo.join(package)).unwrap();
    }
    fs::write(repo.join("go.mod"), "module example.com/fx\n\ngo 1.22\n")
        .unwrap();
    fs::write(
        repo.join("encoder/encoder.go"),
        r#"package encoder
type Encoder struct {}
func NewTuple() (*Encoder, error) { return &Encoder{}, nil }
func NewGrouped() (a, b *Encoder) { return &Encoder{}, &Encoder{} }
func NewNamed() (result *Encoder) { return &Encoder{} }
func (e *Encoder) TupleMark() int { return 1 }
func (e *Encoder) GroupedMark() int { return 2 }
func (e *Encoder) NamedMark() int { return 3 }
"#,
    )
    .unwrap();
    fs::write(
        repo.join("app/app.go"),
        r#"package app
import "example.com/fx/encoder"
func UseTuple() int {
    x, _ := encoder.NewTuple()
    return x.TupleMark()
}
func UseGrouped() int {
    _, x := encoder.NewGrouped()
    return x.GroupedMark()
}
func UseNamed() int {
    x := encoder.NewNamed()
    return x.NamedMark()
}
"#,
    )
    .unwrap();
    fs::write(
        repo.join("decoy/decoy.go"),
        r#"package decoy
type First struct {}
type Second struct {}
func (First) TupleMark() int { return 0 }
func (Second) TupleMark() int { return 0 }
func (First) GroupedMark() int { return 0 }
func (Second) GroupedMark() int { return 0 }
func (First) NamedMark() int { return 0 }
func (Second) NamedMark() int { return 0 }
"#,
    )
    .unwrap();

    let padding = repo.join("padding");
    fs::create_dir(&padding).unwrap();
    for i in 0..60 {
        fs::write(
            padding.join(format!("pad{i}.go")),
            "package padding
",
        )
        .unwrap();
    }
    let serial = index(&scratch.0.join("serial"), &repo, "1");
    let rust = index(&scratch.0.join("parallel"), &repo, "2");
    assert_eq!(rust, serial);
    let expected = [
        ("UseGrouped", "GroupedMark"),
        ("UseNamed", "NamedMark"),
        ("UseTuple", "TupleMark"),
    ];
    assert_eq!(rust.len(), expected.len(), "{rust:?}");
    for (edge, (caller, callee)) in rust.iter().zip(expected) {
        assert_eq!(edge.caller, caller);
        assert_eq!(edge.callee, callee);
        assert_eq!(edge.target_file, "encoder/encoder.go");
        let properties: Value = serde_json::from_str(&edge.properties).unwrap();
        assert!(
            properties["strategy"]
                .as_str()
                .is_some_and(|s| s.starts_with("lsp_")),
            "{edge:?}"
        );
    }
}
