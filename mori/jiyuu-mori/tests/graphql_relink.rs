//! Exercises explicit GraphQL relinks through the linked Mori executable.
#![cfg(feature = "c-backend")]

use std::fs::{self, DirBuilder};
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use rusqlite::{Connection, OpenFlags, params};
use serde_json::{Value, json};

const CLIENT: &str = "client";

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = PathBuf::from(format!(
            "/tmp/mori-gql-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut builder = DirBuilder::new();
        builder.mode(0o700).create(&dir).expect("create fixture");
        let fixture = Self(dir);
        for subdir in ["cache", "rt"] {
            builder
                .create(fixture.0.join(subdir))
                .expect("create runtime directory");
        }
        fixture
    }

    fn index(&self, project: &str) -> Connection {
        let repo = self.0.join(project);
        fs::create_dir(&repo).expect("create project directory");
        fs::write(
            repo.join("placeholder.py"),
            "def placeholder():\n    pass\n",
        )
        .expect("write placeholder source");
        let response = self.cli(json!({"repo_path": repo, "name": project}));
        assert_eq!(response["status"], "indexed", "{response}");
        assert_eq!(response["project"], project, "{response}");
        self.db(project)
    }

    fn cli(&self, args: Value) -> Value {
        let output = Command::new(env!("CARGO_BIN_EXE_mori"))
            .current_dir("/")
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", std::env::var_os("HOME").unwrap_or_default())
            .env("CBM_CACHE_DIR", self.0.join("cache"))
            .env("CBM_RUNTIME_DIR", self.0.join("rt"))
            .env("MORI_IMPL_GRAPHQL", "rust")
            .args(["cli", "--quiet", "index_repository", &args.to_string()])
            .output()
            .expect("run Mori");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "{args}: {stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_str(&stdout)
            .unwrap_or_else(|error| panic!("{args}: {error}: {stdout}"))
    }

    fn relink(&self, project: &str, targets: &[&str], resolved: usize) {
        let response = self.cli(json!({
            "repo_path": self.0.join(project),
            "name": project,
            "mode": "cross-repo-intelligence",
            "target_projects": targets,
        }));
        assert_eq!(response["status"], "success", "{response}");
        assert_eq!(response["cross_graphql_calls"], resolved, "{response}");
    }

    fn db(&self, project: &str) -> Connection {
        let db = Connection::open_with_flags(
            self.0.join("cache").join(format!("{project}.db")),
            OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .expect("open indexed database");
        db.execute_batch("PRAGMA foreign_keys = ON;")
            .expect("enable foreign keys");
        db
    }

    fn cross_links(&self, project: &str) -> Vec<Value> {
        let db = self.db(project);
        let mut rows = db
            .prepare("SELECT properties FROM edges WHERE type = 'CROSS_GRAPHQL_CALLS'")
            .expect("select cross-repo links");
        rows.query_map([], |row| row.get::<_, String>(0))
            .expect("read cross-repo properties")
            .map(|row| {
                serde_json::from_str(&row.expect("read property"))
                    .expect("parse property")
            })
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn node(
    db: &Connection,
    project: &str,
    label: &str,
    name: &str,
    qn: &str,
    file: &str,
) -> i64 {
    db.execute(
        "INSERT INTO nodes(project, label, name, qualified_name, file_path) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![project, label, name, qn, file],
    )
    .expect("seed node");
    db.last_insert_rowid()
}

fn edge(
    db: &Connection,
    project: &str,
    source: i64,
    target: i64,
    kind: &str,
    props: Value,
) {
    db.execute(
        "INSERT INTO edges(project, source_id, target_id, type, properties) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![project, source, target, kind, props.to_string()],
    )
    .expect("seed edge");
}

fn seed_client(db: &Connection, route_qn: &str) {
    let route = node(db, CLIENT, "Route", route_qn, route_qn, "client.ts");
    // Both methods have the same display name and file. Their qualified names
    // must still produce distinct entries in the provider's reverse link.
    for qn in ["client.A.load", "client.B.load"] {
        let method = node(db, CLIENT, "Method", "load", qn, "client.ts");
        edge(
            db,
            CLIENT,
            method,
            route,
            "GRAPHQL_CALLS",
            json!({"operation": "Fetch"}),
        );
    }
}

fn seed_provider(
    db: &Connection,
    project: &str,
    route_qn: &str,
    ambiguous: bool,
) {
    let route =
        node(db, project, "Route", route_qn, route_qn, "schema.graphqls");
    db.execute(
        "UPDATE nodes SET properties = ?1 WHERE id = ?2",
        params![
            json!({"source": "graphql_schema", "ambiguous": ambiguous})
                .to_string(),
            route
        ],
    )
    .expect("set Route properties");
    let field = node(
        db,
        project,
        "Field",
        "field",
        &format!("{project}.Query.field"),
        "schema.graphqls",
    );
    let resolver = node(
        db,
        project,
        "Method",
        "Resolve",
        &format!("{project}.Query.Resolve"),
        "resolver.go",
    );
    edge(
        db,
        project,
        field,
        route,
        "HANDLES",
        json!({"via": "graphql_schema_field"}),
    );
    edge(
        db,
        project,
        resolver,
        route,
        "HANDLES",
        json!({"via": "gqlgen_resolver"}),
    );
    if !ambiguous {
        let prefix = &route_qn[..511];
        let decoy_route =
            node(db, project, "Route", prefix, prefix, "schema.graphqls");
        let decoy = node(
            db,
            project,
            "Method",
            "Decoy",
            &format!("{project}.Query.Decoy"),
            "decoy.go",
        );
        edge(
            db,
            project,
            decoy,
            decoy_route,
            "HANDLES",
            json!({"via": "gqlgen_resolver"}),
        );
    }
}

fn assert_resolved(
    fixture: &Fixture,
    provider: &str,
    expected_callers: &[&str],
) {
    let forward = fixture.cross_links(CLIENT);
    assert_eq!(forward.len(), expected_callers.len(), "{forward:?}");
    for row in forward {
        assert_eq!(row["target_project"], provider, "{row}");
        assert_eq!(row["target_function"], "Resolve", "{row}");
        assert_eq!(row["target_file"], "resolver.go", "{row}");
        assert_eq!(row["ambiguous"], false, "{row}");
    }
    let reverse = fixture.cross_links(provider);
    let [row] = reverse.as_slice() else {
        panic!("expected one reverse link, got {reverse:?}");
    };
    let callers = row["callers"].as_object().expect("reverse callers");
    assert_eq!(callers.len(), expected_callers.len(), "{callers:?}");
    for caller in expected_callers {
        assert!(callers.contains_key(*caller), "{callers:?}");
    }
}

#[test]
fn graphql_relinks_keep_exact_resolver_and_reconcile_callers() {
    let route_qn = format!("__graphql__Query.{}", "f".repeat(505));
    // Target projects are sorted internally. Change the project names as well
    // as the argument order to exercise both matching orders.
    for (ambiguous, provider) in
        [("a-ambiguous", "z-provider"), ("z-ambiguous", "a-provider")]
    {
        let fixture = Fixture::new();
        seed_client(&fixture.index(CLIENT), &route_qn);
        seed_provider(&fixture.index(ambiguous), ambiguous, &route_qn, true);
        seed_provider(&fixture.index(provider), provider, &route_qn, false);

        let both_callers = ["client:client.A.load", "client:client.B.load"];
        fixture.relink(CLIENT, &[ambiguous], 0);
        let unresolved = fixture.cross_links(CLIENT);
        assert_eq!(unresolved.len(), both_callers.len());
        for row in unresolved {
            assert_eq!(row["target_project"], ambiguous, "{row}");
            assert_eq!(row["target_function"], "", "{row}");
            assert_eq!(row["target_file"], "", "{row}");
            assert_eq!(row["ambiguous"], true, "{row}");
        }
        assert!(fixture.cross_links(ambiguous).is_empty());
        for targets in [[ambiguous, provider], [provider, ambiguous]] {
            fixture.relink(CLIENT, &targets, 2);
            assert_resolved(&fixture, provider, &both_callers);
            assert!(fixture.cross_links(ambiguous).is_empty());
        }
        fixture.relink(provider, &[CLIENT], 2);
        fixture.relink(ambiguous, &[CLIENT], 0);
        assert_resolved(&fixture, provider, &both_callers);
        assert!(fixture.cross_links(ambiguous).is_empty());

        fixture
            .db(CLIENT)
            .execute(
                "DELETE FROM nodes WHERE qualified_name = 'client.A.load'",
                [],
            )
            .expect("remove first caller");
        fixture.relink(CLIENT, &[provider], 1);
        assert_resolved(&fixture, provider, &both_callers[1..]);

        fixture
            .db(provider)
            .execute("DELETE FROM nodes WHERE qualified_name = ?1", [&route_qn])
            .expect("remove exact Route while retaining its prefix decoy");
        fixture.relink(provider, &[CLIENT], 0);
        assert!(fixture.cross_links(CLIENT).is_empty());
        assert!(fixture.cross_links(provider).is_empty());
        let calls: i64 = fixture
            .db(CLIENT)
            .query_row(
                "SELECT count(*) FROM edges WHERE type = 'GRAPHQL_CALLS'",
                [],
                |row| row.get(0),
            )
            .expect("count local GraphQL calls");
        assert_eq!(calls, 1, "relink must preserve the remaining source call");
    }
}
