//! Compare existing-generation decisions through the CLI and index worker.
#![cfg(all(feature = "c-backend", unix))]

use rusqlite::{Connection, OpenFlags};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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
            .env("MORI_IMPL_INDEX_MANIFEST", self.implementation)
            .env("MORI_IMPL_INDEX_ROUTE", self.implementation)
            .env("MORI_IMPL_FULL_BUILD_DRIVER", self.implementation);
        command
    }

    fn run_index(&self, repo: &Path) -> Output {
        self.command()
            .args(["cli", "--quiet", "index_repository", "--repo_path"])
            .arg(repo)
            .output()
            .unwrap()
    }

    fn take_worker_logs(&mut self) -> String {
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
        logs
    }

    fn check_route(&self, logs: &str, outcome: Option<(&str, &str)>) {
        let witnesses: Vec<_> = logs
            .lines()
            .filter(|line| {
                line.contains("msg=pipeline.index_route implementation=rust")
            })
            .collect();
        if let Some((generation, decision)) =
            outcome.filter(|_| self.implementation == "rust")
        {
            assert_eq!(witnesses.len(), 1, "{logs}");
            assert!(
                witnesses[0].contains(&format!(
                    "generation={generation} outcome={decision}"
                )),
                "{logs}"
            );
            eprintln!("{}", witnesses[0]);
        } else {
            assert!(witnesses.is_empty(), "{logs}");
        }
    }

    fn index(
        &mut self,
        repo: &Path,
        step: &Step,
    ) -> (String, String, Vec<String>) {
        let result = self.run_index(repo);
        assert!(result.status.success(), "{result:?}");
        let output: serde_json::Value = serde_json::from_slice(&result.stdout)
            .unwrap_or_else(|error| {
                panic!("index response: {error}: {result:?}")
            });
        assert_eq!(output["status"], "indexed", "{output}");
        let logs = self.take_worker_logs();
        self.check_route(&logs, step.decision);
        let rust_full = self.implementation == "rust"
            && matches!(step.decision, None | Some(("staged", "full")));
        assert_eq!(
            logs.contains("msg=pipeline.full_build_driver implementation=rust"),
            rust_full,
            "{logs}"
        );
        assert!(logs.contains(step.log), "{logs}");
        let used_rust = logs
            .contains("msg=incremental.manifest_compare implementation=rust");
        assert_eq!(
            used_rust,
            step.decision.is_some() && self.implementation == "rust",
            "{logs}"
        );

        let name = output["project"].as_str().unwrap();
        let (generation, routes) = self.state(name);
        (name.to_owned(), generation, routes)
    }

    fn db_path(&self, name: &str) -> PathBuf {
        self.root.join("cache").join(format!("{name}.db"))
    }

    fn state(&self, name: &str) -> (String, Vec<String>) {
        let db = Connection::open_with_flags(
            self.db_path(name),
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

    fn graph(&self, name: &str) -> String {
        let output = self
            .command()
            .args([
                "dev",
                "export",
                "--tables",
                "nodes,edges,file_hashes",
                "--db",
            ])
            .arg(self.db_path(name))
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap()
    }

    fn check_adr_failure(&mut self, repo: &Path, name: &str) {
        let db_path = self.db_path(name);
        let adr_path = db_path.with_extension("db.adr.db");
        let saved_sidecar = adr_path.with_extension("saved");
        let had_sidecar = adr_path.exists();
        if had_sidecar {
            fs::rename(&adr_path, &saved_sidecar).unwrap();
        }
        fs::create_dir(&adr_path).unwrap();
        let before = self.state(name);
        let before_graph = fs::read(&db_path).unwrap();
        let failed = self.run_index(repo);
        fs::remove_dir(&adr_path).unwrap();
        if had_sidecar {
            fs::rename(&saved_sidecar, &adr_path).unwrap();
        }

        let response = [failed.stdout.as_slice(), failed.stderr.as_slice()]
            .into_iter()
            .flat_map(|stream| stream.split(|&byte| byte == b'\n'))
            .filter_map(|line| {
                serde_json::from_slice::<serde_json::Value>(line).ok()
            })
            .find(|reply| reply["status"].is_string())
            .unwrap_or_else(|| panic!("missing failure response: {failed:?}"));
        assert_eq!(
            response["status"], "aborted_previous_preserved",
            "{failed:?}"
        );
        let logs = self.take_worker_logs();
        self.check_route(&logs, Some(("staged", "full")));
        assert!(logs.contains("reason=adr_migrate_failed"), "{logs}");
        assert!(!logs.contains("msg=pipeline.full_build_driver"), "{logs}");
        assert_eq!(self.state(name), before);
        assert!(
            fs::read(&db_path).unwrap() == before_graph,
            "serving DB bytes changed"
        );
        eprintln!(
            "{} route kept the serving generation after ADR migration failed",
            self.implementation
        );
    }
}

impl Drop for IndexSession {
    fn drop(&mut self) {
        let _ = self.command().args(["daemon", "stop"]).output();
    }
}

#[derive(Clone, Copy)]
struct Step {
    edit: Option<(&'static str, &'static str)>,
    log: &'static str,
    decision: Option<(&'static str, &'static str)>,
    publishes: bool,
    route_count: usize,
}

#[test]
fn rust_routing_preserves_native_update_paths_and_generation() {
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
    let fresh = Step {
        edit: None,
        log: "msg=pipeline.route path=full",
        decision: None,
        publishes: true,
        route_count: 1,
    };
    let steps = [
        fresh,
        Step {
            edit: None,
            log: "msg=incremental.noop reason=semantic_manifest_equal",
            decision: Some(("live", "noop")),
            publishes: false,
            route_count: 1,
        },
        Step {
            edit: Some(("op.graphql", "query OperationRenamed { value }\n")),
            log: "msg=incremental.route route=closure_repair",
            decision: Some(("staged", "closure")),
            publishes: true,
            route_count: 1,
        },
        Step {
            edit: Some((
                "schema.graphqls",
                "type Query { value: Int another: Int }\n",
            )),
            log: "msg=incremental.force_full",
            decision: Some(("staged", "full")),
            publishes: true,
            route_count: 2,
        },
    ];

    let mut native = IndexSession::new(fixture.0.join("control"), "c");
    let mut rust = IndexSession::new(fixture.0.join("candidate"), "rust");
    let mut previous_native = String::new();
    let mut previous_rust = String::new();
    let mut project: Option<String> = None;
    for step in steps {
        if let Some((path, content)) = step.edit {
            fs::write(repo.join(path), content).unwrap();
        }
        if step.decision == Some(("staged", "full")) {
            let name = project.as_deref().unwrap();
            native.check_adr_failure(&repo, name);
            rust.check_adr_failure(&repo, name);
        }
        let (name, native_generation, native_routes) =
            native.index(&repo, &step);
        let (rust_name, rust_generation, rust_routes) = rust.index(&repo, &step);
        assert_eq!(rust_name, name);
        if step.decision == Some(("staged", "closure")) {
            let mut clean = IndexSession::new(fixture.0.join("clean"), "c");
            let (clean_name, _, _) = clean.index(&repo, &fresh);
            assert_eq!(clean_name, name);
            let expected = clean.graph(&name);
            assert_eq!(native.graph(&name), expected);
            assert_eq!(rust.graph(&name), expected);
            eprintln!(
                "Closure repair matched clean full nodes, edges, and file hashes"
            );
        }
        if step.decision == Some(("staged", "full")) {
            assert_eq!(rust.graph(&name), native.graph(&name));
        }
        project = Some(name);
        assert_eq!(native_generation != previous_native, step.publishes);
        assert_eq!(rust_generation != previous_rust, step.publishes);
        assert_eq!(native_routes.len(), step.route_count);
        assert_eq!(rust_routes, native_routes);
        previous_native = native_generation;
        previous_rust = rust_generation;
    }
}
