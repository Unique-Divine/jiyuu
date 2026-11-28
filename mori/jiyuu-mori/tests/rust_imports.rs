//! Rust use trees through the native indexer and published graph.
#![cfg(all(feature = "c-backend", unix))]

use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};

const MORI: &str = env!("CARGO_BIN_EXE_mori");
const PROJECT: &str = "rust-imports";

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct IndexRun {
    root: PathBuf,
    implementation: Option<&'static str>,
}

impl IndexRun {
    fn new(root: &Path, implementation: Option<&'static str>) -> Self {
        let root = root.join(implementation.unwrap_or("default"));
        let mut dirs = fs::DirBuilder::new();
        dirs.recursive(true).mode(0o700);
        dirs.create(root.join("cache")).unwrap();
        dirs.create(root.join("rt")).unwrap();
        Self {
            root,
            implementation,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(MORI);
        command
            .current_dir("/")
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", std::env::var_os("HOME").unwrap_or_default())
            .env("CBM_CACHE_DIR", self.root.join("cache"))
            .env("CBM_RUNTIME_DIR", self.root.join("rt"))
            .env("CBM_PROFILE", "1")
            .env("CBM_LOG_LEVEL", "debug")
            .env("CBM_WORKERS", "1");
        if let Some(implementation) = self.implementation {
            command.env("MORI_IMPL_RUST_IMPORTS", implementation);
        }
        command
    }

    fn index(&self, repo: &Path) {
        let args = json!({"repo_path": repo, "name": PROJECT});
        let output = self
            .command()
            .args(["cli", "--quiet", "index_repository", &args.to_string()])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(reply["status"], "indexed", "{reply}");
    }

    fn db_path(&self) -> PathBuf {
        self.root.join("cache").join(format!("{PROJECT}.db"))
    }

    fn graph(&self) -> Vec<u8> {
        let output = self
            .command()
            .args([
                "dev",
                "export",
                "--tables",
                "nodes,edges,file_hashes",
                "--db",
            ])
            .arg(self.db_path())
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        output.stdout
    }

    fn worker_markers(&self) -> Vec<String> {
        let mut markers = Vec::new();
        for entry in fs::read_dir(self.root.join("cache/logs")).unwrap() {
            let entry = entry.unwrap();
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(".worker-log-")
            {
                let text = fs::read_to_string(entry.path()).unwrap();
                markers.extend(
                    text.lines()
                        .filter(|line| {
                            line.contains(
                                "msg=extract.rust_imports implementation=rust",
                            )
                        })
                        .map(str::to_owned),
                );
            }
        }
        markers
    }
}

impl Drop for IndexRun {
    fn drop(&mut self) {
        let _ = self.command().args(["daemon", "stop"]).output();
    }
}

#[test]
fn rust_use_trees_publish_the_native_graph() {
    let temp = Command::new("mktemp")
        .args(["-d", "/tmp/mori-rust-imports-XXXXXXXX"])
        .output()
        .unwrap();
    assert!(temp.status.success(), "{temp:?}");
    let scratch = Scratch(PathBuf::from(std::ffi::OsStr::from_bytes(
        temp.stdout.strip_suffix(b"\n").unwrap(),
    )));
    let repo = scratch.0.join("repo");
    fs::create_dir_all(repo.join("src/sub")).unwrap();

    for (file, text) in [
        (
            "Cargo.toml",
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        (
            "src/main.rs",
            r#"mod h;
mod ops;
mod sub;
use crate::{h::{self, /* group comment */ Handler, Other as Alias}, ops::*};
use crate::h::Handler;
pub use self::h::Other as LocalOther;
use std :: collections::{BTreeMap, HashMap as OrderedAlias};

fn grouped() -> u32 {
    let _ = Alias;
    Handler::new().n + do_op()
}
fn main() { grouped(); }
"#,
        ),
        (
            "src/h.rs",
            "pub struct Handler { pub n: u32 }\n\
             impl Handler { pub fn new() -> Handler { Handler { n: 1 } } }\n\
             pub struct Other;\n",
        ),
        ("src/ops.rs", "pub fn do_op() -> u32 { 2 }\n"),
        (
            "src/sub/mod.rs",
            r#"use super::h::{self, Other as Relative};
pub fn relative() -> Relative { Relative }
"#,
        ),
    ] {
        fs::write(repo.join(file), text).unwrap();
    }

    let native = IndexRun::new(&scratch.0, Some("c"));
    native.index(&repo);
    let rust = IndexRun::new(&scratch.0, None);
    rust.index(&repo);
    assert_eq!(rust.graph(), native.graph(), "persistent graph differs");

    let db = Connection::open_with_flags(
        rust.db_path(),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let import_count = |source: &str, local: &str| -> i64 {
        db.query_row(
            "SELECT count(*) FROM edges e JOIN nodes s ON s.id=e.source_id
             WHERE e.type='IMPORTS' AND s.file_path=?1
               AND json_extract(e.properties,'$.local_name')=?2",
            [source, local],
            |row| row.get(0),
        )
        .unwrap()
    };
    assert_eq!(import_count("src/main.rs", "Handler"), 1);
    assert_eq!(import_count("src/main.rs", "Alias"), 1);
    let associated_call: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM edges e
               JOIN nodes s ON s.id=e.source_id JOIN nodes t ON t.id=e.target_id
               WHERE e.type='CALLS' AND s.name='grouped'
                 AND t.file_path='src/h.rs' AND t.name='new')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        associated_call,
        "grouped use lost the typed associated call"
    );

    assert!(native.worker_markers().is_empty());
    let markers = rust.worker_markers();
    for file in ["src/main.rs", "src/sub/mod.rs"] {
        assert!(
            markers
                .iter()
                .any(|line| line.ends_with(&format!("file={file}"))),
            "{markers:?}",
        );
    }
    assert!(
        markers.iter().all(|line| line.ends_with("file=src/main.rs")
            || line.ends_with("file=src/sub/mod.rs")),
        "unexpected Rust import marker: {markers:?}",
    );
}
