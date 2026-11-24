//! Exercise provider-triggered link repair through the daemon's named watcher.
#![cfg(all(feature = "c-backend", unix))]

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, DirBuilder, File};
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const CLIENT: &str = "client";
const FIRST: &str = "primary";
const SECOND: &str = "secondary";
const UNRELATED: &str = "unrelated";

struct Fixture {
    root: PathBuf,
    cli_repair_opt_in: bool,
    recovery_impl: Option<&'static str>,
}

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
        let fixture = Self {
            root,
            cli_repair_opt_in: false,
            recovery_impl: None,
        };
        for dir in ["cache", "rt"] {
            builder
                .create(fixture.root.join(dir))
                .expect("create fixture directory");
        }
        fixture
    }

    fn repo(&self, project: &str) -> PathBuf {
        self.root.join("inputs").join(project)
    }

    fn db_path(&self, project: &str) -> PathBuf {
        self.root.join("cache").join(format!("{project}.db"))
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
            .env("CBM_CACHE_DIR", self.root.join("cache"))
            .env("CBM_RUNTIME_DIR", self.root.join("rt"));
        if self.cli_repair_opt_in {
            command.env("MORI_PROVIDER_REPAIR", "1");
        }
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

    fn start_daemon(&self, selector: Option<&str>, watch_project: Option<&str>) {
        let mut command = self.command();
        if let Some(implementation) = self.recovery_impl {
            command.env("MORI_IMPL_REPAIR_RECOVERY", implementation);
        }
        if let Some(selector) = selector {
            command.env(selector, "1");
        }
        if let Some(project) = watch_project {
            let watch = self.root.join("watch.json");
            fs::write(
                &watch,
                json!([{"project": project, "root": self.repo(project)}])
                    .to_string(),
            )
            .unwrap();
            command.env("MORI_NAMED_WATCH_FILE", watch);
        }
        let output = command.args(["daemon", "start"]).output().unwrap();
        assert!(output.status.success(), "{output:?}");
    }

    fn stop_daemon(&self) {
        let output = self.command().args(["daemon", "stop"]).output().unwrap();
        assert!(output.status.success(), "{output:?}");
    }

    fn hold_shared_project_set_lease(&self) -> File {
        // Source workers can share this lease. The wildcard mutation needs
        // exclusive ownership of the same native lock.
        let hash = Sha256::digest(b"cbm-project-set-v1");
        let lock = format!("cbm-{hash:x}.rw");
        let path = fs::read_dir(self.root.join("rt"))
            .unwrap()
            .map(|entry| entry.unwrap().path().join(&lock))
            .find(|path| path.is_file())
            .expect("project-set lock");
        let file = File::open(path).unwrap();
        file.lock_shared().unwrap();
        file
    }

    fn log(&self) -> String {
        fs::read_to_string(self.root.join("cache/logs/cbm-daemon.log"))
            .unwrap_or_default()
    }

    fn wait_for_log(&self, fields: &[&str]) {
        let deadline = Instant::now() + Duration::from_secs(35);
        loop {
            let log = self.log();
            if log
                .lines()
                .any(|line| fields.iter().all(|field| line.contains(field)))
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "missing {fields:?} in daemon log:\n{log}"
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

    fn generation(&self, project: &str) -> (String, String) {
        self.db(project)
            .query_row(
                "SELECT uid.v, gen.v FROM store_meta uid, store_meta gen
                 WHERE uid.k = 'db_uid' AND gen.k = 'mutation_gen'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
    }

    fn index(&self, project: &str) {
        let reply = self
            .cli(
                "index_repository",
                &json!({"name": project, "repo_path": self.repo(project)}),
            )
            .unwrap();
        assert_eq!(reply["status"], "indexed", "{reply}");
        assert_eq!(reply["project"], project, "{reply}");
    }

    fn publish(&self, project: &str) {
        let before = self.publication_id(project);
        self.index(project);
        let after = self.publication_id(project).expect("source graph exists");
        assert_ne!(before, Some(after));
    }

    fn wait_for_repair(&self, status: &str) {
        self.wait_for_log(&[
            "cross_repo.repair_result",
            "provider=primary",
            "client=client",
            "targets=2",
            status,
        ]);
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

    fn lock_graph_exclusive(&self, project: &str) -> Connection {
        let db = Connection::open(self.db_path(project)).unwrap();
        db.execute_batch("PRAGMA journal_mode=DELETE; BEGIN EXCLUSIVE;")
            .unwrap();
        db
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
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn names(items: &[&str]) -> Vec<String> {
    items.iter().map(|&item| item.to_owned()).collect()
}

#[test]
fn published_provider_repair_preserves_other_links_without_client_sources() {
    let fixture = Fixture::new();
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

    fixture.start_daemon(Some("MORI_PROVIDER_REPAIR"), Some(FIRST));
    for project in [CLIENT, SECOND, UNRELATED] {
        fixture.publish(project);
    }
    fixture.wait_for_log(&[
        "daemon.index.result",
        "project=primary",
        "status=indexed",
    ]);
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

    // Withhold the client source and one of its two published provider graphs.
    let client_source = fixture.repo(CLIENT);
    fs::rename(&client_source, fixture.root.join("client-offline")).unwrap();
    let client_id = fixture.publication_id(CLIENT);
    let client_generation = fixture.generation(CLIENT);
    let original_provider_generation = fixture.generation(FIRST);

    let second_graph = fixture.db_path(SECOND);
    let saved_graph = fixture.root.join("secondary-offline");
    fs::rename(&second_graph, &saved_graph).unwrap();
    fs::rename(
        fixture.repo(FIRST).join("schema.graphqls"),
        fixture.repo(FIRST).join("intermediate.graphqls"),
    )
    .unwrap();
    fixture.wait_for_repair("status=pending");
    let intermediate_generation = fixture.generation(FIRST);
    assert_ne!(intermediate_generation, original_provider_generation);
    assert_eq!(fixture.cross_status(CLIENT)["status"], "stale");
    assert_eq!(fixture.forward_targets(CLIENT), initial_forward);
    assert_eq!(fixture.cross_edges(UNRELATED), unrelated_edges);
    assert_eq!(fixture.generation(CLIENT), client_generation);
    let expected_targets = names(&[FIRST, SECOND]);
    assert_eq!(
        fixture.saved_targets(CLIENT),
        Some(expected_targets.clone())
    );

    fs::rename(saved_graph, second_graph).unwrap();
    fs::rename(
        fixture.repo(FIRST).join("intermediate.graphqls"),
        fixture.repo(FIRST).join("moved.graphqls"),
    )
    .unwrap();
    fixture.wait_for_repair("status=repaired");
    let status = fixture.cross_status(CLIENT);
    assert_eq!(status["status"], "ran");
    assert!(status["reverse_pending"].is_null());
    let reciprocal_status = fixture.cross_status(FIRST);
    assert_eq!(reciprocal_status["status"], "ran");
    assert!(reciprocal_status["reverse_pending"].is_null());
    assert_eq!(fixture.saved_targets(CLIENT), Some(expected_targets));
    assert_eq!(fixture.publication_id(CLIENT), client_id);
    assert_eq!(fixture.generation(CLIENT), client_generation);
    assert_ne!(fixture.generation(FIRST), intermediate_generation);
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
    assert!(!client_source.exists());
    let log = fixture.log();
    let indexed = |project: &str| {
        log.lines()
            .filter(|line| {
                line.contains("daemon.index.result")
                    && line.contains(&format!("project={project} "))
                    && line.contains("status=indexed")
            })
            .count()
    };
    assert_eq!(indexed(CLIENT), 1, "{log}");
    assert_eq!(indexed(FIRST), 3, "{log}");
    assert_eq!(
        log.lines()
            .filter(|line| {
                line.contains("cross_repo.repair_result")
                    && line.contains("provider=primary client=client")
            })
            .count(),
        2,
        "{log}"
    );
}

#[test]
fn opted_in_daemon_retries_a_client_with_pending_reverse_links() {
    let fixture = Fixture::new();
    fixture.start_daemon(Some("MORI_PROVIDER_REPAIR"), None);
    fixture.client(CLIENT, &["first", "second"]);
    fixture.write(FIRST, "schema.graphqls", "type Query { first: String }\n");
    fixture.write(SECOND, "schema.graphqls", "type Query { second: String }\n");
    for project in [CLIENT, FIRST, SECOND] {
        fixture.publish(project);
    }
    let targets = names(&[FIRST, SECOND]);
    fixture.link(CLIENT, &targets).unwrap();

    // A normal source publication records reverse work when its saved provider
    // is unavailable. Hold that marker through a failed automatic repair.
    fs::rename(
        fixture.db_path(SECOND),
        fixture.root.join("secondary-offline"),
    )
    .unwrap();
    fixture.client(CLIENT, &["second", "first"]);
    fixture.publish(CLIENT);
    fixture.wait_for_log(&["cross_repo.reverse_pending", "project=client"]);
    assert_eq!(fixture.cross_status(CLIENT)["reverse_pending"], true);
    fs::rename(fixture.repo(CLIENT), fixture.root.join("client-offline"))
        .unwrap();
    let generation = fixture.generation(CLIENT);

    fs::rename(
        fixture.repo(FIRST).join("schema.graphqls"),
        fixture.repo(FIRST).join("moved.graphqls"),
    )
    .unwrap();
    fixture.publish(FIRST);
    fixture.wait_for_repair("status=pending");
    let pending = fixture.cross_status(CLIENT);
    assert_eq!(pending["status"], "stale");
    assert_eq!(pending["reverse_pending"], true);
    let provider_generation = fixture.generation(FIRST);
    fixture.index(FIRST);
    assert_eq!(fixture.generation(FIRST), provider_generation);
    assert_eq!(
        fixture
            .log()
            .lines()
            .filter(|line| line.contains("cross_repo.repair_result"))
            .count(),
        1
    );

    // The return of the missing provider supplies the next notification.
    fixture.publish(SECOND);
    fixture.wait_for_log(&[
        "cross_repo.repair_result",
        "client=client",
        "status=repaired",
    ]);
    let repaired = fixture.cross_status(CLIENT);
    assert_eq!(repaired["status"], "ran");
    assert!(repaired["reverse_pending"].is_null());
    assert_eq!(fixture.generation(CLIENT), generation);
    assert_eq!(fixture.saved_targets(CLIENT), Some(targets));
    assert_eq!(
        fixture.forward_targets(CLIENT),
        BTreeMap::from([
            (FIRST.to_owned(), "moved.graphqls".to_owned()),
            (SECOND.to_owned(), "schema.graphqls".to_owned()),
        ])
    );
}

#[test]
fn blocked_wildcard_retries_the_first_provider_after_another_publishes() {
    let fixture = Fixture::new();
    fixture.start_daemon(Some("MORI_PROVIDER_REPAIR"), None);
    fixture.client(CLIENT, &["first"]);
    fixture.write(FIRST, "schema.graphqls", "type Query { first: String }\n");
    fixture.write(SECOND, "schema.graphqls", "type Query { second: String }\n");
    for project in [CLIENT, FIRST, SECOND] {
        fixture.publish(project);
        fixture.wait_for_log(&[
            "cross_repo.repair_scan_complete",
            &format!("provider={project}"),
        ]);
    }
    fixture.link(CLIENT, &names(&[FIRST])).unwrap();
    let initial_links = fixture.forward_targets(CLIENT);
    let callers = fixture.callers(FIRST, "schema.graphqls");
    assert_eq!(callers.len(), 1);
    let generation = fixture.generation(CLIENT);
    fs::rename(fixture.repo(CLIENT), fixture.root.join("client-offline"))
        .unwrap();

    std::thread::scope(|scope| {
        let lease = fixture.hold_shared_project_set_lease();
        for project in [FIRST, SECOND] {
            fs::rename(
                fixture.repo(project).join("schema.graphqls"),
                fixture.repo(project).join("moved.graphqls"),
            )
            .unwrap();
        }
        let (sent, received) = mpsc::channel();
        let fixture = &fixture;
        scope.spawn(move || {
            fixture.publish(FIRST);
            fixture.wait_for_log(&[
                "cross_repo.repair_deferred",
                "provider=primary",
                "reason=lease",
            ]);
            fixture.publish(SECOND);
            sent.send(()).unwrap();
        });
        received.recv_timeout(Duration::from_secs(25)).expect(
            "both index replies arrive while the wildcard lease is blocked",
        );
        assert_eq!(fixture.cross_status(CLIENT)["status"], "stale");
        assert_eq!(fixture.forward_targets(CLIENT), initial_links);
        drop(lease);
    });

    fixture.wait_for_log(&[
        "cross_repo.repair_result",
        "provider=primary",
        "client=client",
        "targets=1",
        "status=repaired",
    ]);
    assert_eq!(fixture.cross_status(CLIENT)["status"], "ran");
    assert_eq!(fixture.generation(CLIENT), generation);
    assert_eq!(
        fixture.forward_targets(CLIENT),
        BTreeMap::from([(FIRST.to_owned(), "moved.graphqls".to_owned())])
    );
    assert_eq!(fixture.callers(FIRST, "moved.graphqls"), callers);
}

#[test]
fn provider_repair_is_off_by_default_and_observation_only_logs_candidates() {
    let fixture = Fixture::new();
    fixture.start_daemon(None, None);
    fixture.client(CLIENT, &["first"]);
    fixture.write(FIRST, "schema.graphqls", "type Query { first: String }\n");
    for project in [CLIENT, FIRST] {
        fixture.publish(project);
    }
    fixture.link(CLIENT, &names(&[FIRST])).unwrap();
    let links = fixture.forward_targets(CLIENT);
    let generation = fixture.generation(CLIENT);

    fs::rename(
        fixture.repo(FIRST).join("schema.graphqls"),
        fixture.repo(FIRST).join("intermediate.graphqls"),
    )
    .unwrap();
    fixture.publish(FIRST);
    assert_eq!(fixture.cross_status(CLIENT)["status"], "stale");
    assert_eq!(fixture.forward_targets(CLIENT), links);
    assert!(!fixture.log().contains("cross_repo.repair_scan_complete"));
    fixture.stop_daemon();

    fixture.start_daemon(Some("MORI_OBSERVE_PROVIDER_REPAIR"), None);
    fs::rename(
        fixture.repo(FIRST).join("intermediate.graphqls"),
        fixture.repo(FIRST).join("moved.graphqls"),
    )
    .unwrap();
    fixture.publish(FIRST);
    fixture
        .wait_for_log(&["cross_repo.repair_scan_complete", "provider=primary"]);
    let log = fixture.log();
    assert!(log.lines().any(|line| {
        line.contains("cross_repo.repair_candidate")
            && line.contains("provider=primary client=client targets=1")
    }));
    assert!(!log.contains("cross_repo.repair_result"), "{log}");
    assert_eq!(fixture.cross_status(CLIENT)["status"], "stale");
    assert_eq!(fixture.forward_targets(CLIENT), links);
    assert_eq!(fixture.generation(CLIENT), generation);
}

#[test]
fn provider_repair_opt_in_does_not_apply_to_a_one_shot_daemon() {
    let mut fixture = Fixture::new();
    fixture.cli_repair_opt_in = true;
    fixture.client(CLIENT, &["first"]);
    fixture.write(FIRST, "schema.graphqls", "type Query { first: String }\n");
    for project in [CLIENT, FIRST] {
        fixture.publish(project);
    }
    fixture.link(CLIENT, &names(&[FIRST])).unwrap();
    let links = fixture.forward_targets(CLIENT);
    fs::rename(
        fixture.repo(FIRST).join("schema.graphqls"),
        fixture.repo(FIRST).join("moved.graphqls"),
    )
    .unwrap();
    fixture.publish(FIRST);
    assert_eq!(fixture.cross_status(CLIENT)["status"], "stale");
    assert_eq!(fixture.forward_targets(CLIENT), links);
    assert!(!fixture.log().contains("cross_repo.repair_scan_complete"));
}

fn assert_no_source_worker(fixture: &Fixture, previous_log: &str) {
    let log = fixture.log();
    let startup = log
        .strip_prefix(previous_log)
        .expect("startup log appended");
    assert!(startup.contains("daemon.start "), "{startup}");
    for event in [
        "daemon.index.worker_budget",
        "daemon.index.result",
        "index.supervisor.reap",
    ] {
        assert!(!startup.contains(event), "{startup}");
    }
}

#[test]
fn startup_repairs_stale_and_reverse_pending_graphs_without_a_source_worker() {
    let fixture = Fixture::new();
    fixture.start_daemon(Some("MORI_OBSERVE_PROVIDER_REPAIR"), None);
    fixture.client(CLIENT, &["first", "second"]);
    fixture.client(UNRELATED, &["second"]);
    fixture.write(FIRST, "schema.graphqls", "type Query { first: String }\n");
    fixture.write(SECOND, "schema.graphqls", "type Query { second: String }\n");
    for project in [CLIENT, FIRST, SECOND, UNRELATED] {
        fixture.publish(project);
    }
    fixture.link(CLIENT, &names(&[FIRST, SECOND])).unwrap();
    fixture.link(UNRELATED, &names(&[SECOND])).unwrap();
    let initial_links = fixture.forward_targets(CLIENT);
    let callers = fixture.callers(FIRST, "schema.graphqls");
    let secondary_callers = fixture.callers(SECOND, "schema.graphqls");
    assert_eq!(callers.len(), 1);
    assert_eq!(secondary_callers.len(), 2);

    fixture.write(UNRELATED, "extra.ts", "export const extra = 1;\n");
    {
        // The source can publish its new generation, but the daemon cannot
        // clear the staged reverse marker while this shared lease is held.
        let _lease = fixture.hold_shared_project_set_lease();
        fixture.publish(UNRELATED);
        fixture.wait_for_log(&[
            "cross_repo.repair_scan_complete",
            "provider=unrelated",
        ]);
        let status = fixture.cross_status(UNRELATED);
        assert_eq!(status["status"], "ran");
        assert_eq!(status["reverse_pending"], true);
    }
    fs::rename(
        fixture.repo(FIRST).join("schema.graphqls"),
        fixture.repo(FIRST).join("moved.graphqls"),
    )
    .unwrap();
    fixture.publish(FIRST);
    fixture
        .wait_for_log(&["cross_repo.repair_scan_complete", "provider=primary"]);
    assert_eq!(fixture.cross_status(CLIENT)["status"], "stale");
    assert_eq!(fixture.forward_targets(CLIENT), initial_links);
    assert!(fixture.callers(FIRST, "moved.graphqls").is_empty());
    fs::rename(fixture.repo(CLIENT), fixture.root.join("client-offline"))
        .unwrap();

    fixture.stop_daemon();
    let generations = [CLIENT, FIRST, SECOND, UNRELATED]
        .map(|project| fixture.generation(project));
    let client_id = fixture.publication_id(CLIENT);
    let previous_log = fixture.log();
    {
        let _lease = fixture.hold_shared_project_set_lease();
        fixture.start_daemon(Some("MORI_PROVIDER_REPAIR"), None);
        assert_eq!(fixture.cross_status(CLIENT)["status"], "stale");
    }
    fixture.wait_for_log(&["cross_repo.recovery_scan_complete", "pending=0"]);
    assert_eq!(
        fixture.forward_targets(CLIENT),
        BTreeMap::from([
            (FIRST.to_owned(), "moved.graphqls".to_owned()),
            (SECOND.to_owned(), "schema.graphqls".to_owned()),
        ])
    );
    assert_eq!(fixture.callers(FIRST, "moved.graphqls"), callers);
    assert_eq!(
        fixture.callers(SECOND, "schema.graphqls"),
        secondary_callers
    );
    assert_eq!(fixture.cross_status(CLIENT)["status"], "ran");
    assert!(fixture.cross_status(UNRELATED)["reverse_pending"].is_null());
    assert_eq!(
        [CLIENT, FIRST, SECOND, UNRELATED]
            .map(|project| fixture.generation(project)),
        generations
    );
    assert_eq!(fixture.publication_id(CLIENT), client_id);
    assert_no_source_worker(&fixture, &previous_log);
}

#[test]
fn startup_retry_repairs_a_restored_provider_without_a_publication() {
    assert_startup_retry("rust");
}

#[test]
fn native_startup_retry_repairs_a_restored_provider_without_a_publication() {
    assert_startup_retry("c");
}

fn assert_startup_retry(implementation: &'static str) {
    let mut fixture = Fixture::new();
    fixture.recovery_impl = Some(implementation);
    fixture.start_daemon(None, None);
    fixture.client(CLIENT, &["first", "second"]);
    fixture.write(FIRST, "schema.graphqls", "type Query { first: String }\n");
    fixture.write(SECOND, "schema.graphqls", "type Query { second: String }\n");
    for project in [CLIENT, FIRST, SECOND] {
        fixture.publish(project);
    }
    fixture.link(CLIENT, &names(&[FIRST, SECOND])).unwrap();
    let initial_links = fixture.forward_targets(CLIENT);
    let client_generation = fixture.generation(CLIENT);
    fs::rename(
        fixture.repo(FIRST).join("schema.graphqls"),
        fixture.repo(FIRST).join("moved.graphqls"),
    )
    .unwrap();
    fixture.publish(FIRST);
    fixture.stop_daemon();
    let previous_log = fixture.log();
    let offline = fixture.root.join("secondary-offline.db");
    fs::rename(fixture.db_path(SECOND), &offline).unwrap();

    fixture.start_daemon(Some("MORI_PROVIDER_REPAIR"), None);
    fixture.wait_for_log(&[
        "cross_repo.recovery_driver",
        &format!("implementation={implementation}"),
    ]);
    fixture.wait_for_log(&["cross_repo.recovery_scan_complete", "pending=1"]);
    assert_eq!(fixture.cross_status(CLIENT)["status"], "stale");
    assert_eq!(fixture.forward_targets(CLIENT), initial_links);
    fs::rename(offline, fixture.db_path(SECOND)).unwrap();
    fixture.wait_for_log(&[
        "cross_repo.repair_result",
        "provider=recovery",
        "client=client",
        "status=repaired",
    ]);
    assert_eq!(fixture.cross_status(CLIENT)["status"], "ran");
    assert_eq!(fixture.generation(CLIENT), client_generation);
    assert_eq!(fixture.callers(FIRST, "moved.graphqls").len(), 1);
    assert_no_source_worker(&fixture, &previous_log);
}

#[test]
fn provider_scan_retries_a_locked_client_without_another_publication() {
    let fixture = Fixture::new();
    fixture.start_daemon(Some("MORI_PROVIDER_REPAIR"), None);
    fixture.client(CLIENT, &["first"]);
    fixture.write(FIRST, "schema.graphqls", "type Query { first: String }\n");
    for project in [CLIENT, FIRST] {
        fixture.publish(project);
        fixture.wait_for_log(&[
            "cross_repo.repair_scan_complete",
            &format!("provider={project}"),
        ]);
    }
    fixture.link(CLIENT, &names(&[FIRST])).unwrap();
    let generation = fixture.generation(CLIENT);
    let callers = fixture.callers(FIRST, "schema.graphqls");
    assert_eq!(callers.len(), 1);
    let lock = fixture.lock_graph_exclusive(CLIENT);
    fs::rename(
        fixture.repo(FIRST).join("schema.graphqls"),
        fixture.repo(FIRST).join("moved.graphqls"),
    )
    .unwrap();
    fixture.publish(FIRST);
    fixture.wait_for_log(&[
        "cross_repo.repair_deferred",
        "provider=primary",
        "reason=client_scan",
    ]);
    fixture.wait_for_log(&["cross_repo.recovery_scan_complete", "pending=1"]);
    let previous_log = fixture.log();
    drop(lock);
    fixture.wait_for_log(&[
        "cross_repo.repair_result",
        "provider=recovery",
        "client=client",
        "status=repaired",
    ]);
    assert_eq!(fixture.cross_status(CLIENT)["status"], "ran");
    assert_eq!(fixture.generation(CLIENT), generation);
    assert_eq!(fixture.callers(FIRST, "moved.graphqls"), callers);
    let log = fixture.log();
    assert!(
        !log.strip_prefix(&previous_log)
            .unwrap()
            .contains("daemon.index.result")
    );
}

#[test]
fn startup_leaves_wildcard_null_peer_pending_without_retries() {
    let fixture = Fixture::new();
    fixture.start_daemon(Some("MORI_OBSERVE_PROVIDER_REPAIR"), None);
    fixture.client(CLIENT, &["first", "second"]);
    fixture.write(FIRST, "schema.graphqls", "type Query { first: String }\n");
    fixture.write(SECOND, "schema.graphqls", "type Query { second: String }\n");
    for project in [CLIENT, FIRST, SECOND] {
        fixture.publish(project);
    }
    fixture.link(CLIENT, &names(&["*"])).unwrap();
    let offline = fixture.root.join("secondary-offline.db");
    fs::rename(fixture.db_path(SECOND), &offline).unwrap();
    fixture.write(CLIENT, "extra.ts", "export const extra = 1;\n");
    fixture.publish(CLIENT);
    fixture
        .wait_for_log(&["cross_repo.repair_scan_complete", "provider=client"]);
    let generation = fixture.generation(CLIENT);
    let links = fixture.forward_targets(CLIENT);
    fixture.stop_daemon();
    let previous_log = fixture.log();

    fixture.start_daemon(Some("MORI_PROVIDER_REPAIR"), None);
    fixture.wait_for_log(&[
        "cross_repo.recovery_skipped",
        "project=client",
        "reason=wildcard_unknown_peer",
    ]);
    fixture.wait_for_log(&["cross_repo.recovery_scan_complete", "pending=0"]);
    fs::rename(offline, fixture.db_path(SECOND)).unwrap();
    let status = fixture.cross_status(CLIENT);
    assert_eq!(status["status"], "stale");
    assert_eq!(status["reverse_pending"], true);
    assert_eq!(fixture.saved_targets(CLIENT), Some(names(&["*"])));
    assert_eq!(fixture.forward_targets(CLIENT), links);
    assert_eq!(fixture.generation(CLIENT), generation);
    assert_no_source_worker(&fixture, &previous_log);

    // The explicit wildcard relink is the supported way to refresh this set.
    // Its new links come from both online provider graphs.
    fixture.link(CLIENT, &names(&["*"])).unwrap();
    assert_eq!(fixture.cross_status(CLIENT)["status"], "ran");
    assert_eq!(fixture.forward_targets(CLIENT).len(), 2);
}

#[test]
fn startup_retries_a_locked_current_generation_reverse_marker() {
    let fixture = Fixture::new();
    fixture.start_daemon(Some("MORI_OBSERVE_PROVIDER_REPAIR"), None);
    fixture.client(CLIENT, &["first"]);
    fixture.write(FIRST, "schema.graphqls", "type Query { first: String }\n");
    for project in [CLIENT, FIRST] {
        fixture.publish(project);
    }
    fixture.link(CLIENT, &names(&[FIRST])).unwrap();
    fixture.write(CLIENT, "extra.ts", "export const extra = 1;\n");
    {
        let _lease = fixture.hold_shared_project_set_lease();
        fixture.publish(CLIENT);
        fixture.wait_for_log(&[
            "cross_repo.repair_scan_complete",
            "provider=client",
        ]);
    }
    let status = fixture.cross_status(CLIENT);
    assert_eq!(status["status"], "ran");
    assert_eq!(status["reverse_pending"], true);
    fixture.stop_daemon();
    let previous_log = fixture.log();
    let generation = fixture.generation(CLIENT);
    let links = fixture.forward_targets(CLIENT);
    let lock = fixture.lock_graph_exclusive(CLIENT);

    fixture.start_daemon(Some("MORI_PROVIDER_REPAIR"), None);
    fixture.wait_for_log(&["cross_repo.recovery_scan_complete", "pending=1"]);
    drop(lock);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let status = fixture.cross_status(CLIENT);
        if status["reverse_pending"].is_null() {
            assert_eq!(status["status"], "ran");
            break;
        }
        assert!(Instant::now() < deadline, "{status}");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(fixture.generation(CLIENT), generation);
    assert_eq!(fixture.forward_targets(CLIENT), links);
    assert_eq!(fixture.callers(FIRST, "schema.graphqls").len(), 1);
    assert_no_source_worker(&fixture, &previous_log);
}
