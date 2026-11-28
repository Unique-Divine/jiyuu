//! Verify Go import facts through the linked worker and persistent graph.
#![cfg(all(feature = "c-backend", unix))]

use std::ffi::OsStr;
use std::fmt::Write as _;
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
            .args(["-d", "/tmp/mori-go-imports-XXXXXXXX"])
            .output()
            .expect("create scratch directory");
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

type ImportEdge = (String, String, String);

fn index(root: &Path, repo: &Path) -> Vec<ImportEdge> {
    const PROJECT: &str = "go-import-probe";
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
            .env("CBM_LOG_LEVEL", "debug");
        command
    };
    let args = json!({"repo_path": repo, "name": PROJECT, "mode": "moderate"});
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
                .contains("msg=extract.go_imports implementation=rust")
    });
    assert!(ran_rust, "worker did not run the Rust Go import extractor");

    let db = Connection::open_with_flags(
        cache.join(format!("{PROJECT}.db")),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    db.prepare(
        "SELECT source.file_path, target.qualified_name,
                json_extract(edge.properties, '$.local_name')
         FROM edges edge
         JOIN nodes source ON source.id = edge.source_id
         JOIN nodes target ON target.id = edge.target_id
         WHERE edge.type = 'IMPORTS'
         ORDER BY 1, 2, 3",
    )
    .unwrap()
    .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

#[test]
fn go_import_edges_keep_single_grouped_and_recovered_imports() {
    let scratch = Scratch::new();
    let repo = scratch.0.join("repo");
    fs::create_dir(&repo).unwrap();
    fs::write(repo.join("go.mod"), "module example.test/mori\n\ngo 1.22\n")
        .unwrap();
    for package in ["alpha", "beta", "dotdep", "blankdep", "rawdep", "recovered"]
    {
        let dir = repo.join(package);
        fs::create_dir(&dir).unwrap();
        fs::write(
            dir.join("dep.go"),
            format!("package {package}\nfunc Dep() int {{ return 1 }}\n"),
        )
        .unwrap();
    }
    let mut main = String::from(
        r#"package main

import aliased "example.test/mori/alpha"
import (
    "example.test/mori/beta"
    . "example.test/mori/dotdep"
    _ "example.test/mori/blankdep"
    rawAlias `example.test/mori/rawdep`
    "C"
    ""
)
"#,
    );
    // A wide root exercises the sibling walk even with few import declarations.
    for index in 0..2048 {
        writeln!(main, "const decl{index} = {index}").unwrap();
    }
    fs::write(repo.join("main.go"), main).unwrap();
    fs::write(
        repo.join("broken.go"),
        "package main\nimport kept \"example.test/mori/recovered\"\nfunc broken() {\n",
    )
    .unwrap();
    fs::write(repo.join("c_decoy.go"), "package main\nfunc C() {}\n").unwrap();

    let rust = index(&scratch.0.join("rust"), &repo);
    for (file, alias, package) in [
        ("main.go", "aliased", "alpha"),
        ("main.go", "beta", "beta"),
        ("main.go", ".", "dotdep"),
        ("main.go", "_", "blankdep"),
        ("broken.go", "kept", "recovered"),
    ] {
        assert!(
            rust.iter().any(|(actual_file, target, actual_alias)| {
                actual_file == file
                    && actual_alias == alias
                    && target.split('.').any(|segment| segment == package)
            }),
            "missing {file} import {alias} -> {package}: {rust:?}"
        );
    }
    assert!(
        rust.iter()
            .all(|(_, _, alias)| alias != "C" && !alias.is_empty()),
        "{rust:?}"
    );
}
