//! Follow A -> B -> C through the permanent daemon's publication callback.
#![cfg(all(feature = "c-backend", unix))]

use std::fs::{self, DirBuilder};
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let fixture = Self(
            std::env::temp_dir()
                .join(format!("mori-inbound-{}", std::process::id())),
        );
        let mut dir = DirBuilder::new();
        dir.mode(0o700);
        dir.create(&fixture.0).unwrap();
        for path in ["cache", "rt"] {
            dir.create(fixture.0.join(path)).unwrap();
        }
        fixture
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

    fn cli(&self, args: Value) -> Value {
        let output = self
            .command()
            .args(["cli", "--quiet", "index_repository", &args.to_string()])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn write(&self, path: &str, source: &str) {
        let path = self.0.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, source).unwrap();
    }

    fn index(&self, project: &str) {
        let response = self.cli(json!({
            "name": project,
            "repo_path": self.0.join(project),
        }));
        assert_eq!(response["status"], "indexed", "{response}");
    }

    fn link(&self, project: &str, targets: &[&str]) {
        let response = self.cli(json!({
            "name": project,
            "repo_path": self.0.join(project),
            "mode": "cross-repo-intelligence",
            "target_projects": targets,
        }));
        assert_eq!(response["status"], "success", "{response}");
    }

    fn db(&self, project: &str, flags: OpenFlags) -> Connection {
        let path = self.0.join("cache").join(format!("{project}.db"));
        Connection::open_with_flags(path, flags).unwrap()
    }

    fn rows(&self, project: &str, sql: &str) -> Vec<String> {
        let db = self.db(project, OpenFlags::SQLITE_OPEN_READ_ONLY);
        db.prepare(sql)
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    fn channel(&self, project: &str, function: &str, edge_types: &[&str]) {
        let mut db = self.db(project, OpenFlags::SQLITE_OPEN_READ_WRITE);
        let transaction = db.transaction().unwrap();
        transaction
            .execute(
                "INSERT INTO nodes(project, label, name, qualified_name)
                 VALUES (?1, 'Channel', 'orders', ?2)",
                (project, format!("{project}.orders")),
            )
            .unwrap();
        let channel = transaction.last_insert_rowid();
        for edge_type in edge_types {
            assert_eq!(
                transaction
                    .execute(
                        "INSERT INTO edges(project, source_id, target_id, type)
                         SELECT ?1, id, ?3, ?4 FROM nodes WHERE project=?1 AND name=?2",
                        (project, function, channel, edge_type),
                    )
                    .unwrap(),
                1
            );
        }
        transaction.commit().unwrap();
    }

    fn wait_for_repair(&self, status: &str) {
        let deadline = Instant::now() + Duration::from_secs(35);
        loop {
            let log =
                fs::read_to_string(self.0.join("cache/logs/cbm-daemon.log"))
                    .unwrap_or_default();
            if log.lines().any(|line| {
                [
                    "cross_repo.repair_result",
                    "provider=c",
                    "client=b",
                    "targets=2",
                    status,
                ]
                .iter()
                .all(|field| line.contains(field))
            }) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "missing {status}:
{log}"
            );
            thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.command().args(["daemon", "stop"]).output();
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn publishing_c_repairs_b_without_erasing_callers_from_a() {
    let fixture = Fixture::new();
    for (project, requests) in [
        (
            "a",
            "export const fromA = graphql(`query FromA { upstream }`);",
        ),
        (
            "b",
            "export const fromB = graphql(`query FromB { downstream }`);
             export const other = graphql(`query Other { extra }`);",
        ),
    ] {
        fixture.write(
            &format!("{project}/codegen/gql.ts"),
            "export function graphql(source: string): string { return source; }",
        );
        fixture.write(
            &format!("{project}/request.ts"),
            &format!("import {{ graphql }} from './codegen/gql';\n{requests}"),
        );
    }
    fixture.write("b/schema.graphqls", "type Query { upstream: String }");
    fixture.write("c/schema.graphqls", "type Query { downstream: String }");
    fixture.write("d/schema.graphqls", "type Query { extra: String }");
    let output = fixture
        .command()
        .env("MORI_PROVIDER_REPAIR", "1")
        .args(["daemon", "start"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");

    for project in ["a", "b", "c", "d"] {
        fixture.index(project);
    }
    // The same local function listens and emits. Both directions use the
    // same CROSS_CHANNEL row in B's graph.
    fixture.channel("a", "fromA", &["EMITS"]);
    fixture.channel("b", "fromB", &["EMITS", "LISTENS_ON"]);
    fixture.channel("d", "extra", &["LISTENS_ON"]);
    fixture.link("b", &["c", "d"]);
    fixture.link("a", &["b"]);

    let generation = "SELECT v FROM store_meta WHERE k IN ('db_uid', 'mutation_gen') ORDER BY k";
    let a_generation = fixture.rows("a", generation);
    let b_generation = fixture.rows("b", generation);
    let outgoing = "SELECT json_array(
                        json_extract(properties, '$.target_project'),
                        json_extract(properties, '$.target_file'))
                    FROM edges WHERE type='CROSS_GRAPHQL_CALLS'
                    AND json_type(properties, '$.callers') IS NULL ORDER BY 1";
    let incoming =
        "SELECT properties FROM edges WHERE type='CROSS_GRAPHQL_CALLS'
                    AND json_type(properties, '$.callers')='object' ORDER BY 1";
    let a_links = fixture.rows("a", outgoing);
    let b_callers = fixture.rows("b", incoming);
    let initial_b_links = fixture.rows("b", outgoing);
    let d_callers = fixture.rows("d", incoming);
    let channels = "SELECT json_array(json_extract(properties, '$.target_project'),
                       json_extract(properties, '$.channel_forward.target_project'),
                       (SELECT json_extract(value, '$.project')
                        FROM json_each(properties, '$.callers')))
                    FROM edges WHERE type='CROSS_CHANNEL'";
    assert_eq!(a_links, [r#"["b","schema.graphqls"]"#]);
    assert_eq!(b_callers.len(), 1);
    assert!(b_callers[0].contains("fromA"), "{b_callers:?}");
    assert_eq!(
        initial_b_links,
        [r#"["c","schema.graphqls"]"#, r#"["d","schema.graphqls"]"#]
    );
    assert_eq!(d_callers.len(), 1);
    assert_eq!(fixture.rows("a", channels), [r#"["b","b",null]"#]);
    assert_eq!(fixture.rows("b", channels), [r#"["a","d","a"]"#]);
    for project in ["a", "b"] {
        fs::rename(
            fixture.0.join(project),
            fixture.0.join(format!("{project}-offline")),
        )
        .unwrap();
    }

    // D stops listening. B must drop the forward half of its channel row
    // when it replays the complete target set.
    let db = fixture.db("d", OpenFlags::SQLITE_OPEN_READ_WRITE);
    db.execute("DELETE FROM edges WHERE type='LISTENS_ON'", ())
        .unwrap();
    drop(db);
    fs::rename(
        fixture.0.join("c/schema.graphqls"),
        fixture.0.join("c/moved.graphqls"),
    )
    .unwrap();
    fixture.index("c");
    fixture.wait_for_repair("status=repaired");

    assert_eq!(fixture.rows("a", outgoing), a_links);
    assert_eq!(fixture.rows("a", generation), a_generation);
    assert_eq!(fixture.rows("b", generation), b_generation);
    assert_eq!(fixture.rows("b", incoming), b_callers);
    let repaired_b_links =
        [r#"["c","moved.graphqls"]"#, r#"["d","schema.graphqls"]"#];
    assert_eq!(fixture.rows("b", outgoing), repaired_b_links);
    let c_callers = fixture.rows("c", incoming);
    assert_eq!(c_callers.len(), 1);
    assert!(c_callers[0].contains("fromB"), "{c_callers:?}");
    assert_eq!(fixture.rows("d", incoming), d_callers);
    assert_eq!(fixture.rows("a", channels), [r#"["b","b",null]"#]);
    assert_eq!(fixture.rows("b", channels), [r#"["a",null,"a"]"#]);
    assert!(fixture.rows("d", channels).is_empty());
    let saved_targets =
        "SELECT v FROM store_meta WHERE k='cross_repo_targets:b'";
    assert_eq!(fixture.rows("b", saved_targets), [r#"["c","d"]"#]);

    // Fail a source write after outgoing cleanup. The whole forward refresh
    // must roll back, including any changes to the complete target set.
    let db = fixture.db("b", OpenFlags::SQLITE_OPEN_READ_WRITE);
    db.execute_batch(
        "CREATE TRIGGER fail_repair BEFORE INSERT ON edges
         WHEN NEW.type='CROSS_GRAPHQL_CALLS'
         BEGIN SELECT RAISE(ABORT, 'forced repair failure'); END;",
    )
    .unwrap();
    drop(db);
    fs::rename(
        fixture.0.join("c/moved.graphqls"),
        fixture.0.join("c/blocked.graphqls"),
    )
    .unwrap();
    fixture.index("c");
    fixture.wait_for_repair("status=pending");
    assert_eq!(fixture.rows("b", outgoing), repaired_b_links);
    assert_eq!(fixture.rows("b", incoming), b_callers);
    assert_eq!(fixture.rows("b", generation), b_generation);
    assert_eq!(fixture.rows("b", channels), [r#"["a",null,"a"]"#]);
    assert_eq!(fixture.rows("b", saved_targets), [r#"["c","d"]"#]);
    assert_eq!(fixture.rows("d", incoming), d_callers);
}
