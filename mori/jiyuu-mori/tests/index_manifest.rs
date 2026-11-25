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
    routing_override: Option<&'static str>,
    rust_manifest: bool,
    seen_logs: HashSet<PathBuf>,
}

impl IndexSession {
    fn new(root: PathBuf, routing_override: Option<&'static str>) -> Self {
        let mut dirs = fs::DirBuilder::new();
        dirs.recursive(true).mode(0o700);
        dirs.create(root.join("cache")).unwrap();
        dirs.create(root.join("rt")).unwrap();
        Self {
            root,
            routing_override,
            rust_manifest: false,
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
        if let Some(routing) = self.routing_override {
            command.env("MORI_IMPL_INDEX_ROUTE", routing);
        }
        if self.rust_manifest {
            command.env("MORI_IMPL_INDEX_MANIFEST", "rust");
        }
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
            outcome.filter(|_| self.routing_override.is_none())
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
        let rust_full = matches!(step.decision, None | Some(("staged", "full")));
        assert_eq!(
            logs.contains("msg=pipeline.full_build_driver implementation=rust"),
            rust_full,
            "{logs}"
        );
        assert!(
            logs.contains(
                "msg=pipeline.source_publish implementation=rust staged_rc=0"
            ),
            "{logs}"
        );
        assert!(logs.contains(step.log), "{logs}");
        let fts_mode = match step.decision {
            None | Some(("staged", "full")) => Some("full"),
            Some(("staged", "closure")) => Some("closure"),
            _ => None,
        };
        let mut staged = logs.lines().filter(|line| {
            line.contains("msg=pipeline.stage_publish implementation=rust")
        });
        if let Some(kind) = fts_mode.map(|mode| {
            if mode == "full" {
                "kind=dump"
            } else {
                "kind=patched"
            }
        }) {
            assert!(
                staged.next().is_some_and(|line| line.contains(kind)),
                "{logs}"
            );
        }
        assert!(staged.next().is_none(), "{logs}");
        let fts_witness = "msg=store.fts_rebuild implementation=rust";
        let fts_witnesses: Vec<_> = logs
            .lines()
            .filter(|line| line.contains(fts_witness))
            .collect();
        assert_eq!(
            fts_witnesses.len(),
            usize::from(fts_mode.is_some()),
            "{logs}"
        );
        if let Some(mode) = fts_mode {
            assert!(
                fts_witnesses[0].contains(&format!("{fts_witness} mode={mode}")),
                "{logs}"
            );
        }
        let importance_pass = match step.decision {
            None | Some(("staged", "full")) => Some("pass.importance"),
            Some(("staged", "closure")) => Some("pass.importance_store"),
            _ => None,
        };
        if let Some(pass) = importance_pass {
            assert!(
                logs.lines().any(|line| {
                    line.contains(&format!("msg={pass} "))
                        && line.contains("implementation=rust")
                }),
                "{logs}"
            );
        }
        let used_rust = logs
            .contains("msg=incremental.manifest_compare implementation=rust");
        assert_eq!(
            used_rust,
            step.decision.is_some() && self.rust_manifest,
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

    fn fts(&self, name: &str, terms: [&str; 2]) -> FtsContents {
        let connection = Connection::open_with_flags(
            self.db_path(name),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        connection
            .execute_batch("CREATE VIRTUAL TABLE temp.vocabulary USING fts5vocab(main, nodes_fts, instance)")
            .unwrap();
        let raw_terms = connection
            .prepare("SELECT term, count(DISTINCT doc), count(*) FROM vocabulary GROUP BY term ORDER BY term")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let tokens = connection
            .prepare(
                "SELECT json_array(v.term, n.qualified_name, v.col, v.offset)
                 FROM vocabulary v JOIN nodes n ON n.id=v.doc ORDER BY 1",
            )
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let mut search = connection
            .prepare(
                "SELECT n.qualified_name, bm25(nodes_fts) AS rank
                 FROM nodes_fts JOIN nodes n ON n.id=nodes_fts.rowid
                 WHERE nodes_fts MATCH ?1 ORDER BY rank, n.qualified_name",
            )
            .unwrap();
        let rankings = terms.map(|term| {
            search
                .query_map([term], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        });
        FtsContents {
            raw_terms,
            tokens,
            rankings,
        }
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
        assert!(!logs.contains("msg=pipeline.stage_publish"), "{logs}");
        assert!(
            logs.contains(
                "msg=pipeline.source_publish implementation=rust staged_rc=-2"
            ),
            "{logs}"
        );
        assert_eq!(self.state(name), before);
        assert!(
            fs::read(&db_path).unwrap() == before_graph,
            "serving DB bytes changed"
        );
        eprintln!(
            "{} route kept the serving generation after ADR migration failed",
            self.routing_override.unwrap_or("default")
        );
    }
}

impl Drop for IndexSession {
    fn drop(&mut self) {
        let _ = self.command().args(["daemon", "stop"]).output();
    }
}

#[derive(Debug, PartialEq)]
struct FtsContents {
    raw_terms: Vec<(String, i64, i64)>,
    tokens: Vec<String>,
    rankings: [Vec<(String, f64)>; 2],
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
fn default_rust_routing_preserves_native_update_paths_and_generation() {
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
        (
            "README.md",
            "# Guide\n\nThe retention ledger remains readable.\n",
        ),
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

    let mut native = IndexSession::new(fixture.0.join("control"), Some("c"));
    let mut rust = IndexSession::new(fixture.0.join("candidate"), None);
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
        let terms = ["name:value", "body:retention"];
        let native_fts = native.fts(&name, terms);
        assert!(!native_fts.tokens.is_empty());
        assert!(native_fts.rankings.iter().all(|rows| !rows.is_empty()));
        assert_eq!(rust.fts(&name, terms), native_fts);
        if step.decision == Some(("staged", "closure")) {
            let mut clean =
                IndexSession::new(fixture.0.join("clean"), Some("unrecognized"));
            let (clean_name, clean_generation, _) = clean.index(&repo, &fresh);
            assert_eq!(clean_name, name);
            let (_, clean_after_noop, _) = clean.index(&repo, &steps[1]);
            assert_eq!(clean_after_noop, clean_generation);
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

    let stopped = rust.command().args(["daemon", "stop"]).output().unwrap();
    assert!(stopped.status.success(), "{stopped:?}");
    rust.rust_manifest = true;
    fs::write(
        repo.join("schema.graphqls"),
        "type Query { value: Int another: Int latest: Int }\n",
    )
    .unwrap();
    let (name, fallback_generation, fallback_routes) =
        rust.index(&repo, &steps[3]);
    let (_, _, native_routes) = native.index(&repo, &steps[3]);
    assert_ne!(fallback_generation, previous_rust);
    assert_eq!(fallback_routes.len(), 3);
    assert_eq!(fallback_routes, native_routes);
    assert_eq!(rust.graph(&name), native.graph(&name));
}

#[test]
fn repeated_closure_publications_refresh_fts_from_live_nodes() {
    let fixture = Fixture::new();
    let repo = fixture.0.join("source");
    fs::create_dir_all(repo.join("lib")).unwrap();
    fs::create_dir_all(repo.join("docs")).unwrap();
    fs::write(
        repo.join("lib/math.ts"),
        r#"export function add(a: number, b: number): number { return a + b }
export function sum(xs: number[]): number { return xs.reduce(add, 0) }
"#,
    )
    .unwrap();
    fs::write(
        repo.join("lib/stats.test.ts"),
        "import { mean } from './stats';
test('mean', () => { expect(mean([1, 2, 3])).toBe(2) })
",
    )
    .unwrap();
    fs::write(
        repo.join("docs/guide.md"),
        "# Guide

## Stats

Use `mean` from `lib/stats.ts`.

## Math

`sum` folds with `add`.
",
    )
    .unwrap();
    let original = r#"import { sum } from "./math"
export function mean(xs: number[]): number {
  return xs.length ? sum(xs) / xs.length : 0
}
export class Summary {
  constructor(private xs: number[]) {}
  mean(): number { return mean(this.xs) }
}
function total(xs: number[]): number { return sum(xs) }
"#;
    let edited =
        original.replace("sum(xs) / xs.length", "total(xs) / xs.length");
    let path = repo.join("lib/stats.ts");
    fs::write(&path, original).unwrap();

    let run = |session: &mut IndexSession| {
        let output = session.run_index(&repo);
        assert!(output.status.success(), "{output:?}");
        let reply: serde_json::Value =
            serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(reply["status"], "indexed", "{reply}");
        (
            reply["project"].as_str().unwrap().to_owned(),
            session.take_worker_logs(),
        )
    };
    let mut warm = IndexSession::new(fixture.0.join("warm"), None);
    let (project, _) = run(&mut warm);
    let terms = ["docs", "guide"];

    for (i, source) in [edited.as_str(), original].into_iter().enumerate() {
        fs::write(&path, source).unwrap();
        let (_, logs) = run(&mut warm);
        assert!(
            logs.contains("msg=incremental.route route=closure_repair"),
            "{logs}"
        );
        assert_eq!(
            logs.matches(
                "msg=store.fts_rebuild implementation=rust mode=closure"
            )
            .count(),
            1,
            "{logs}"
        );

        let mut clean =
            IndexSession::new(fixture.0.join(format!("clean-{i}")), None);
        let (clean_project, _) = run(&mut clean);
        assert_eq!(project, clean_project);
        let expected = clean.fts(&project, terms);
        assert!(expected.rankings.iter().all(|rows| !rows.is_empty()));
        assert_eq!(warm.fts(&project, terms), expected);

        let (_, logs) = run(&mut warm);
        assert!(
            logs.contains("msg=incremental.noop reason=semantic_manifest_equal"),
            "{logs}"
        );
        assert!(!logs.contains("msg=store.fts_rebuild"), "{logs}");
        assert_eq!(warm.fts(&project, terms), expected);
    }

    let db_path = warm.db_path(&project);
    let db = Connection::open(&db_path).unwrap();
    db.execute_batch("DROP TABLE nodes_fts_config").unwrap();
    drop(db);
    let previous_generation = warm.state(&project);
    let previous_db = fs::read(&db_path).unwrap();
    fs::write(&path, &edited).unwrap();
    let failed = warm.run_index(&repo);
    let response = [failed.stdout.as_slice(), failed.stderr.as_slice()]
        .into_iter()
        .flat_map(|stream| stream.split(|&byte| byte == b'\n'))
        .filter_map(|line| {
            serde_json::from_slice::<serde_json::Value>(line).ok()
        })
        .find(|reply| reply["status"].is_string())
        .unwrap_or_else(|| panic!("missing failure response: {failed:?}"));
    assert_eq!(response["status"], "persist_failed", "{failed:?}");
    let logs = warm.take_worker_logs();
    assert!(
        logs.contains("msg=incremental.route route=closure_repair"),
        "{logs}"
    );
    assert!(
        logs.lines().any(|line| {
            line.contains("msg=publish.fts_failed")
                && line.contains("phase=refresh")
                && line.contains("vtable_constructor_failed:_nodes_fts")
        }),
        "{logs}"
    );
    assert_eq!(warm.state(&project), previous_generation);
    assert!(
        fs::read(&db_path).unwrap() == previous_db,
        "serving DB bytes changed"
    );
}
