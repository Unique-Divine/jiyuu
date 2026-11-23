//! Verify the Rust no-op decision through the same index worker as the C build.
#![cfg(all(feature = "c-backend", unix))]

use rusqlite::{Connection, OpenFlags};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let output = Command::new("mktemp")
            .args(["-d", "/tmp/mori-manifest-XXXXXXXX"])
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

struct IndexSession {
    root: PathBuf,
    implementation: &'static str,
    seen_logs: HashSet<PathBuf>,
}

impl IndexSession {
    fn new(root: PathBuf, implementation: &'static str) -> Self {
        let mut dirs = fs::DirBuilder::new();
        dirs.recursive(true).mode(0o700);
        dirs.create(root.join("cache")).unwrap();
        dirs.create(root.join("rt")).unwrap();
        Self {
            root,
            implementation,
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
            .env("CBM_LOG_LEVEL", "info")
            .env("MORI_IMPL_INDEX_MANIFEST", self.implementation);
        command
    }

    fn index(&mut self, repo: &Path, step: &Step) -> (String, Vec<String>) {
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
        let mut logs = String::new();
        for entry in fs::read_dir(self.root.join("cache/logs")).unwrap() {
            let entry = entry.unwrap();
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(".worker-log-")
                && self.seen_logs.insert(entry.path())
            {
                logs.push_str(&fs::read_to_string(entry.path()).unwrap());
            }
        }
        assert!(logs.contains(step.log), "{logs}");
        let used_rust = logs
            .contains("msg=incremental.manifest_compare implementation=rust");
        assert_eq!(
            used_rust,
            step.existing_generation && self.implementation == "rust",
            "{logs}"
        );

        let name = output["project"].as_str().unwrap();
        let cache = self.root.join("cache");
        let db = Connection::open_with_flags(
            cache.join(format!("{name}.db")),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let generation: String = db
            .query_row(
                "SELECT (SELECT v FROM store_meta WHERE k='db_uid') || ':' ||
                    (SELECT v FROM store_meta WHERE k='mutation_gen')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let routes = db
            .prepare("SELECT name FROM nodes WHERE label='Route' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        (generation, routes)
    }
}

impl Drop for IndexSession {
    fn drop(&mut self) {
        let _ = self.command().args(["daemon", "stop"]).output();
    }
}

struct Step {
    edit: Option<(&'static str, &'static str)>,
    log: &'static str,
    existing_generation: bool,
    publishes: bool,
    route_count: usize,
}

#[test]
fn rust_manifest_preserves_native_update_paths_and_generation() {
    let fixture = Fixture::new();
    let repo = fixture.0.join("source");
    fs::create_dir_all(repo.join("server")).unwrap();
    let initial_schema = "type Query { value: Int }\n";
    for (path, contents) in [
        (
            "gqlgen.yml",
            "schema:\n  - schema.graphqls\nresolver:\n  layout: follow-schema\n  dir: server\n",
        ),
        ("schema.graphqls", initial_schema),
        (
            "server/handler.go",
            "package server\ntype queryResolver struct{}\nfunc (q *queryResolver) Value() int { return 1 }\n",
        ),
        ("op.graphql", "query Operation { value }\n"),
    ] {
        fs::write(repo.join(path), contents).unwrap();
    }
    let steps = [
        Step {
            edit: None,
            log: "msg=pipeline.route path=full",
            existing_generation: false,
            publishes: true,
            route_count: 1,
        },
        Step {
            edit: None,
            log: "msg=incremental.noop reason=semantic_manifest_equal",
            existing_generation: true,
            publishes: false,
            route_count: 1,
        },
        Step {
            edit: Some(("op.graphql", "query OperationRenamed { value }\n")),
            log: "msg=incremental.route route=closure_repair",
            existing_generation: true,
            publishes: true,
            route_count: 1,
        },
        Step {
            edit: Some((
                "schema.graphqls",
                "type Query { value: Int another: Int }\n",
            )),
            log: "msg=incremental.force_full",
            existing_generation: true,
            publishes: true,
            route_count: 2,
        },
    ];

    let mut native = IndexSession::new(fixture.0.join("control"), "c");
    let mut rust = IndexSession::new(fixture.0.join("candidate"), "rust");
    let mut previous_native = String::new();
    let mut previous_rust = String::new();
    for step in steps {
        if let Some((path, content)) = step.edit {
            fs::write(repo.join(path), content).unwrap();
        }
        let (native_generation, native_routes) = native.index(&repo, &step);
        let (rust_generation, rust_routes) = rust.index(&repo, &step);
        assert_eq!(native_generation != previous_native, step.publishes);
        assert_eq!(rust_generation != previous_rust, step.publishes);
        assert_eq!(native_routes.len(), step.route_count);
        assert_eq!(rust_routes, native_routes);
        previous_native = native_generation;
        previous_rust = rust_generation;
    }
}
