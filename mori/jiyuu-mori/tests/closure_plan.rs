//! Verify the default closure classifier through the worker and compare with clean rebuilds.
#![cfg(all(feature = "c-backend", unix))]

use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::{self, DirBuilder};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let output = Command::new("mktemp")
            .args(["-d", "/tmp/mori-closure-plan-XXXXXXXX"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        Self(PathBuf::from(OsStr::from_bytes(
            output.stdout.strip_suffix(b"\n").unwrap(),
        )))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Session {
    root: PathBuf,
    seen_logs: HashSet<PathBuf>,
}

impl Session {
    fn new(root: PathBuf) -> Self {
        for dir in ["cache", "rt"] {
            DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(root.join(dir))
                .unwrap();
        }
        Self {
            root,
            seen_logs: HashSet::new(),
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mori"));
        command
            .current_dir("/")
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", std::env::var_os("HOME").unwrap_or_default())
            .env("CBM_CACHE_DIR", self.root.join("cache"))
            .env("CBM_RUNTIME_DIR", self.root.join("rt"))
            .env("CBM_PROFILE", "1")
            .env("CBM_LOG_LEVEL", "info");
        command
    }

    fn index(
        &mut self,
        repo: &Path,
        updates: &[&str],
        classification: Option<&str>,
    ) -> [u8; 32] {
        let result = self
            .command()
            .args(["cli", "--quiet", "index_repository", "--repo_path"])
            .arg(repo)
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
        let output: serde_json::Value = serde_json::from_slice(&result.stdout)
            .unwrap_or_else(|error| {
                panic!("index response: {error}: {result:?}")
            });
        assert_eq!(output["status"], "indexed", "{output}");

        let logs: String = fs::read_dir(self.root.join("cache/logs"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name().is_some_and(|name| {
                    name.as_bytes().starts_with(b".worker-log-")
                }) && self.seen_logs.insert(path.clone())
            })
            .map(|path| fs::read_to_string(path).unwrap())
            .collect();
        for update in updates {
            assert!(logs.contains(update), "{logs}");
        }
        let marker =
            "msg=incremental.closure_manifest implementation=rust outcome=";
        let witnessed: Vec<_> = logs
            .lines()
            .filter_map(|line| line.split_once(marker))
            .map(|(_, tail)| tail.split_whitespace().next().unwrap())
            .collect();
        let expected: Vec<_> = classification.into_iter().collect();
        assert_eq!(witnessed, expected, "{logs}");

        let project = output["project"].as_str().unwrap();
        let export = self
            .command()
            .args([
                "dev",
                "export",
                "--tables",
                "nodes,edges,file_hashes",
                "--db",
            ])
            .arg(self.root.join("cache").join(format!("{project}.db")))
            .output()
            .unwrap();
        assert!(export.status.success(), "{export:?}");
        assert!(!export.stdout.is_empty(), "{export:?}");
        Sha256::digest(export.stdout).into()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.command().args(["daemon", "stop"]).output();
    }
}

#[test]
fn closure_plan_update_paths_match_clean_full() {
    let fixture = Fixture::new();
    let repo = fixture.0.join("source");
    fs::create_dir_all(repo.join("server")).unwrap();
    for (path, contents) in [
        (
            "gqlgen.yml",
            "schema:\n  - schema.graphqls\nresolver:\n  layout: follow-schema\n  dir: server\n",
        ),
        ("schema.graphqls", "type Query { value: Int }\n"),
        (
            "server/handler.go",
            "package server\ntype queryResolver struct{}\nfunc (q *queryResolver) Value() int { return 1 }\n",
        ),
        ("op.graphql", "query Operation { value }\n"),
    ] {
        fs::write(repo.join(path), contents).unwrap();
    }
    let mut rust = Session::new(fixture.0.join("candidate"));
    let full = "msg=pipeline.route path=full";
    let mut compare = |updates: &[&str], classification| {
        rust.index(&repo, updates, classification)
    };
    let initial = compare(&[full], None);
    assert_eq!(
        initial,
        compare(
            &["msg=incremental.noop reason=semantic_manifest_equal"],
            None
        )
    );

    fs::write(
        repo.join("op.graphql"),
        "query OperationRenamed { value }\n",
    )
    .unwrap();
    let repaired = compare(
        &["msg=incremental.route route=closure_repair"],
        Some("classified"),
    );
    let mut clean = Session::new(fixture.0.join("clean-after-edit"));
    assert_eq!(repaired, clean.index(&repo, &[full], None));

    fs::write(
        repo.join("new.ts"),
        "export function added() { return 1 }\n",
    )
    .unwrap();
    let rebuilt = compare(
        &["msg=incremental.closure_decline reason=new_file", full],
        Some("new_file"),
    );
    let mut clean = Session::new(fixture.0.join("clean-after-add"));
    assert_eq!(rebuilt, clean.index(&repo, &[full], None));

    fs::write(
        repo.join("schema.graphqls"),
        "type Query { value: Int another: Int }\n",
    )
    .unwrap();
    let rebuilt = compare(
        &[
            "msg=incremental.closure_decline reason=graphql_schema_changed",
            full,
        ],
        Some("classified"),
    );
    let mut clean = Session::new(fixture.0.join("clean-after-schema"));
    assert_eq!(rebuilt, clean.index(&repo, &[full], None));
}
