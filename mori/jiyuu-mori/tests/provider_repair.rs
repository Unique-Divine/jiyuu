//! Prototype provider-triggered link repair against published fixture graphs.
//! The fixture supplies publication events explicitly and runs an in-memory
//! queue. Production dispatch, quiescence, and restart recovery need their
//! own integration tests.
#![cfg(all(feature = "c-backend", unix))]

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, DirBuilder};
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::{Value, json};

const CLIENT: &str = "client";
const FIRST: &str = "primary";
const SECOND: &str = "secondary";
const UNRELATED: &str = "unrelated";
const PROJECTS: &[&str] = &[CLIENT, FIRST, SECOND, UNRELATED];

struct SourcePublication {
    project: String,
}

#[derive(Default)]
struct Repairs {
    clients: BTreeSet<String>,
}

fn named_targets(targets: &[String]) -> bool {
    !targets.is_empty() && targets.iter().all(|target| target != "*")
}

impl Repairs {
    fn notify(
        &mut self,
        publication: &SourcePublication,
        saved_clients: impl IntoIterator<Item = (String, Vec<String>)>,
    ) {
        let provider = &publication.project;
        self.clients.extend(
            saved_clients
                .into_iter()
                .filter(|(client, targets)| {
                    client != provider
                        && named_targets(targets)
                        && targets.contains(provider)
                })
                .map(|(client, _)| client),
        );
    }

    fn run(
        &mut self,
        mut saved_targets: impl FnMut(&str) -> Option<Vec<String>>,
        mut link: impl FnMut(&str, &[String]) -> bool,
    ) {
        self.clients.retain(|client| {
            saved_targets(client).is_none_or(|targets| {
                named_targets(&targets) && !link(client, &targets)
            })
        });
    }
}

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = PathBuf::from(format!(
            "/tmp/mori-prv-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut builder = DirBuilder::new();
        builder.mode(0o700).create(&root).expect("create fixture");
        let fixture = Self(root);
        for dir in ["cache", "rt"] {
            builder
                .create(fixture.0.join(dir))
                .expect("create fixture directory");
        }
        fixture
    }

    fn repo(&self, project: &str) -> PathBuf {
        self.0.join("inputs").join(project)
    }

    fn db_path(&self, project: &str) -> PathBuf {
        self.0.join("cache").join(format!("{project}.db"))
    }

    fn write(&self, project: &str, path: &str, source: &str) {
        let path = self.repo(project).join(path);
        fs::create_dir_all(path.parent().expect("source parent")).unwrap();
        fs::write(path, source).unwrap();
    }

    fn client(&self, project: &str, fields: &[&str]) {
        self.write(
            project,
            "codegen/gql.ts",
            "export function graphql(source: string): string { return source; }\n",
        );
        let mut source =
            String::from("import { graphql } from './codegen/gql';\n");
        for (id, field) in fields.iter().enumerate() {
            source.push_str(&format!(
                "export const request{id} = graphql(`query Lookup_{project}_{id} {{ {field} }}`);\n"
            ));
        }
        self.write(project, "request.ts", &source);
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mori"));
        command
            .current_dir("/")
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", std::env::var_os("HOME").unwrap_or_default())
            .env("CBM_CACHE_DIR", self.0.join("cache"))
            .env("CBM_RUNTIME_DIR", self.0.join("rt"));
        command
    }

    fn cli(&self, tool: &str, args: &Value) -> Result<Value, String> {
        let output = self
            .command()
            .args(["cli", "--quiet", tool, &args.to_string()])
            .output()
            .expect("run Mori");
        let stdout = String::from_utf8_lossy(&output.stdout);
        if !output.status.success() {
            return Err(format!(
                "{tool} {args}: {stdout}\n{}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        serde_json::from_str(&stdout)
            .map_err(|error| format!("{tool} {args}: {error}: {stdout}"))
    }

    fn observe_provider_repairs(&self) {
        let output = self
            .command()
            .env("MORI_OBSERVE_PROVIDER_REPAIR", "1")
            .args(["daemon", "start"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }

    fn expect_repair_candidate(
        &self,
        provider: &str,
        client: &str,
        count: usize,
    ) {
        let log_path = self.0.join("cache/logs/cbm-daemon.log");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let log = fs::read_to_string(&log_path).unwrap_or_default();
            if log.lines().any(|line| {
                line.contains("cross_repo.repair_scan_complete")
                    && line.contains(&format!("provider={provider}"))
            }) {
                let candidates: Vec<_> = log
                    .lines()
                    .filter(|line| line.contains("cross_repo.repair_candidate"))
                    .collect();
                let [candidate] = candidates.as_slice() else {
                    panic!("expected one candidate: {candidates:?}");
                };
                assert!(
                    candidate.contains(&format!("provider={provider}")),
                    "{candidate}"
                );
                assert!(
                    candidate.contains(&format!("client={client}")),
                    "{candidate}"
                );
                assert!(
                    candidate.contains(&format!("targets={count}")),
                    "{candidate}"
                );
                return;
            }
            assert!(
                Instant::now() < deadline,
                "no completed repair scan in {log_path:?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn publication_id(&self, project: &str) -> Option<(u64, u64)> {
        // A source index publishes by replacing the serving database.
        // Link runs update its SQLite rows without replacing that file.
        fs::metadata(self.db_path(project))
            .ok()
            .map(|meta| (meta.dev(), meta.ino()))
    }

    fn publish(&self, project: &str) -> Option<SourcePublication> {
        let before = self.publication_id(project);
        let reply = self
            .cli(
                "index_repository",
                &json!({"name": project, "repo_path": self.repo(project)}),
            )
            .unwrap();
        assert_eq!(reply["status"], "indexed", "{reply}");
        assert_eq!(reply["project"], project, "{reply}");
        let after = self.publication_id(project).expect("source graph exists");
        (before != Some(after)).then(|| SourcePublication {
            project: project.to_owned(),
        })
    }

    fn link(&self, client: &str, targets: &[String]) -> Result<Value, String> {
        self.cli(
            "index_repository",
            &json!({
                "name": client,
                "repo_path": self.repo(client),
                "mode": "cross-repo-intelligence",
                "target_projects": targets,
            }),
        )
    }

    fn cross_status(&self, project: &str) -> Value {
        let reply = self
            .cli(
                "index_status",
                &json!({"project": project, "format": "json"}),
            )
            .unwrap();
        reply["cross_repo"].clone()
    }

    fn db(&self, project: &str) -> Connection {
        Connection::open_with_flags(
            self.db_path(project),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .expect("open project graph")
    }

    fn saved_targets(&self, project: &str) -> Option<Vec<String>> {
        let value: Option<String> = self
            .db(project)
            .query_row(
                "SELECT v FROM store_meta WHERE k = ?",
                [format!("cross_repo_targets:{project}")],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        value.map(|targets| {
            serde_json::from_str(&targets).expect("parse saved targets")
        })
    }

    fn saved_clients(&self) -> impl Iterator<Item = (String, Vec<String>)> + '_ {
        PROJECTS.iter().filter_map(|&project| {
            self.saved_targets(project)
                .map(|targets| (project.to_owned(), targets))
        })
    }

    fn cross_edges(&self, project: &str) -> Vec<Value> {
        let db = self.db(project);
        let mut query = db
            .prepare(
                "SELECT json_array(n.name, n.file_path, json(e.properties))
             FROM edges AS e
             JOIN nodes AS n ON e.source_id = n.id
             WHERE e.type = 'CROSS_GRAPHQL_CALLS'
             ORDER BY n.qualified_name, e.target_id",
            )
            .unwrap();
        query
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|row| serde_json::from_str(&row.unwrap()).unwrap())
            .collect()
    }

    fn forward_targets(&self, project: &str) -> BTreeMap<String, String> {
        self.cross_edges(project)
            .into_iter()
            .filter(|row| row[2].get("callers").is_none())
            .map(|row| {
                let props = &row[2];
                (
                    props["target_project"].as_str().unwrap().to_owned(),
                    props["target_file"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    }

    fn callers(&self, provider: &str, handler_file: &str) -> BTreeSet<String> {
        self.cross_edges(provider)
            .iter()
            .filter(|row| row[1] == handler_file)
            .filter_map(|row| row[2]["callers"].as_object())
            .flat_map(|callers| callers.keys().cloned())
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.command().args(["daemon", "stop"]).output();
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn names(items: &[&str]) -> Vec<String> {
    items.iter().map(|&item| item.to_owned()).collect()
}

#[test]
fn published_provider_repair_preserves_other_links_without_client_sources() {
    let fixture = Fixture::new();
    fixture.observe_provider_repairs();
    fixture.client(CLIENT, &["first", "second"]);
    fixture.write(
        CLIENT,
        "schema.graphqls",
        "type Query { reciprocal: String }\n",
    );
    fixture.client(FIRST, &["reciprocal"]);
    fixture.write(FIRST, "schema.graphqls", "type Query { first: String }\n");
    fixture.write(SECOND, "schema.graphqls", "type Query { second: String }\n");
    fixture.client(UNRELATED, &["second"]);

    for project in PROJECTS {
        fixture
            .publish(project)
            .expect("initial source publication");
    }
    for (client, targets, expected_links) in [
        (FIRST, names(&[CLIENT]), 1),
        (UNRELATED, names(&[SECOND]), 1),
        // The native count includes the previously recorded reciprocal link.
        (CLIENT, names(&[FIRST, SECOND]), 3),
    ] {
        let reply = fixture.link(client, &targets).unwrap();
        assert_eq!(reply["status"], "success", "{reply}");
        assert_eq!(reply["cross_graphql_calls"], expected_links, "{reply}");
    }
    let initial_forward = fixture.forward_targets(CLIENT);
    assert_eq!(
        initial_forward.get(FIRST).map(String::as_str),
        Some("schema.graphqls")
    );
    assert_eq!(
        initial_forward.get(SECOND).map(String::as_str),
        Some("schema.graphqls")
    );
    let initial_callers = fixture.callers(FIRST, "schema.graphqls");
    let secondary_callers = fixture.callers(SECOND, "schema.graphqls");
    let reciprocal_forward =
        BTreeMap::from([(CLIENT.to_owned(), "schema.graphqls".to_owned())]);
    assert_eq!(fixture.forward_targets(FIRST), reciprocal_forward);
    let reciprocal_callers = fixture.callers(CLIENT, "schema.graphqls");
    let unrelated_edges = fixture.cross_edges(UNRELATED);
    assert_eq!(initial_callers.len(), 1, "{initial_callers:?}");
    assert!(
        initial_callers
            .iter()
            .all(|caller| caller.starts_with("client:")
                && caller.ends_with(".request0"))
    );
    assert_eq!(secondary_callers.len(), 2, "{secondary_callers:?}");
    assert_eq!(reciprocal_callers.len(), 1, "{reciprocal_callers:?}");
    assert!(
        reciprocal_callers
            .iter()
            .all(|caller| caller.starts_with("primary:"))
    );

    fs::rename(
        fixture.repo(FIRST).join("schema.graphqls"),
        fixture.repo(FIRST).join("moved.graphqls"),
    )
    .unwrap();
    let published = fixture.publish(FIRST).expect("provider-only publication");
    fixture.expect_repair_candidate(FIRST, CLIENT, 2);
    let mut repairs = Repairs::default();
    repairs.notify(&published, fixture.saved_clients());
    repairs.notify(&published, fixture.saved_clients());
    assert_eq!(repairs.clients, BTreeSet::from([CLIENT.to_owned()]));
    assert_eq!(fixture.cross_status(CLIENT)["status"], "stale");
    assert_eq!(fixture.forward_targets(CLIENT), initial_forward);
    assert_eq!(fixture.forward_targets(FIRST), reciprocal_forward);
    assert_eq!(
        fixture.callers(CLIENT, "schema.graphqls"),
        reciprocal_callers
    );

    // Keep the repo path admissible for the CLI, but make all original
    // client files unavailable until the repair has finished.
    let client_source = fixture.repo(CLIENT);
    fs::rename(&client_source, fixture.0.join("client-offline")).unwrap();
    fs::create_dir(&client_source).unwrap();
    assert!(fs::read_dir(&client_source).unwrap().next().is_none());
    let client_id = fixture.publication_id(CLIENT);
    let provider_id = fixture.publication_id(FIRST);

    let second_graph = fixture.db_path(SECOND);
    let saved_graph = fixture.0.join("secondary-offline");
    fs::rename(&second_graph, &saved_graph).unwrap();
    let run = |repairs: &mut Repairs| {
        let mut attempts = Vec::new();
        repairs.run(
            |client| fixture.saved_targets(client),
            |client, targets| {
                let result = fixture.link(client, targets);
                let complete = result.as_ref().is_ok_and(|reply| {
                    reply["status"] == "success"
                        && reply["skipped_projects"].is_null()
                        && fixture.cross_status(client)["reverse_pending"]
                            != true
                });
                attempts.push((client.to_owned(), targets.to_vec(), result));
                complete
            },
        );
        attempts
    };
    let missing = run(&mut repairs);
    let expected_targets = names(&[FIRST, SECOND]);
    let [(client, targets, Err(error))] = missing.as_slice() else {
        panic!("expected one native link failure: {missing:?}");
    };
    assert_eq!(client, CLIENT);
    assert_eq!(targets, &expected_targets);
    assert!(error.contains("cross-repo source or target project is missing, invalid, or not indexed"), "{error}");
    assert_eq!(repairs.clients, BTreeSet::from([CLIENT.to_owned()]));
    assert_eq!(fixture.cross_status(CLIENT)["status"], "stale");
    assert_eq!(fixture.forward_targets(CLIENT), initial_forward);
    assert_eq!(
        fixture.saved_targets(CLIENT),
        Some(expected_targets.clone())
    );

    fs::rename(saved_graph, second_graph).unwrap();
    let retried = run(&mut repairs);
    let [(client, targets, Ok(reply))] = retried.as_slice() else {
        panic!("expected one successful native retry: {retried:?}");
    };
    assert_eq!(client, CLIENT);
    assert_eq!(targets, &expected_targets);
    assert_eq!(reply["status"], "success");
    assert!(repairs.clients.is_empty(), "{:?}", repairs.clients);
    assert_eq!(fixture.cross_status(CLIENT)["status"], "ran");
    assert_eq!(fixture.saved_targets(CLIENT), Some(expected_targets));
    assert_eq!(fixture.publication_id(CLIENT), client_id);
    assert_eq!(fixture.publication_id(FIRST), provider_id);
    assert_eq!(
        fixture.forward_targets(CLIENT),
        BTreeMap::from([
            (FIRST.to_owned(), "moved.graphqls".to_owned()),
            (SECOND.to_owned(), "schema.graphqls".to_owned()),
        ])
    );
    assert!(fixture.callers(FIRST, "schema.graphqls").is_empty());
    assert_eq!(fixture.callers(FIRST, "moved.graphqls"), initial_callers);
    assert_eq!(fixture.forward_targets(FIRST), reciprocal_forward);
    assert_eq!(
        fixture.callers(SECOND, "schema.graphqls"),
        secondary_callers
    );
    assert_eq!(
        fixture.callers(CLIENT, "schema.graphqls"),
        reciprocal_callers
    );
    assert_eq!(fixture.cross_edges(UNRELATED), unrelated_edges);
    assert!(fs::read_dir(client_source).unwrap().next().is_none());
}

#[test]
fn pending_named_clients_reread_saved_targets_and_run_in_name_order() {
    let mut repairs = Repairs::default();
    let saved = [
        ("z-client".to_owned(), names(&["old", "provider"])),
        ("a-client".to_owned(), names(&["provider"])),
        ("wildcard".to_owned(), names(&["*"])),
        ("switches-to-wildcard".to_owned(), names(&["provider"])),
        ("provider".to_owned(), names(&["provider"])),
    ];
    repairs.notify(
        &SourcePublication {
            project: "provider".to_owned(),
        },
        saved,
    );
    assert_eq!(
        repairs.clients,
        BTreeSet::from_iter(names(&[
            "a-client",
            "switches-to-wildcard",
            "z-client"
        ]))
    );
    let mut actual = Vec::new();
    repairs.run(
        |client| {
            Some(match client {
                "a-client" => names(&["provider"]),
                "z-client" => names(&["provider", "new"]),
                "switches-to-wildcard" => names(&["*"]),
                _ => panic!("unexpected pending job: {client}"),
            })
        },
        |client, targets| {
            actual.push((client.to_owned(), targets.to_vec()));
            true
        },
    );
    assert_eq!(
        actual,
        [
            ("a-client".to_owned(), names(&["provider"])),
            ("z-client".to_owned(), names(&["provider", "new"])),
        ]
    );
    assert!(repairs.clients.is_empty());
}
