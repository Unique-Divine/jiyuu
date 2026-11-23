//! Exercises explicit GraphQL relinks through the linked Mori executable.
#![cfg(feature = "c-backend")]

use std::fs::{self, DirBuilder};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use rusqlite::{Connection, OpenFlags, params};
use serde_json::{Value, json};

#[path = "../src/cypher_linear.rs"]
mod cypher_linear;

const CLIENT: &str = "client";
const MORI: &str = env!("CARGO_BIN_EXE_mori");

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

    fn command(&self, bin: &Path) -> Command {
        let mut command = Command::new(bin);
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
            .command(Path::new(MORI))
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

    fn query_output(&self, bin: &Path, project: &str, cypher: &str) -> Output {
        let args =
            json!({"project": project, "query": cypher, "format": "json"});
        let output = self
            .command(bin)
            .args(["cli", "--quiet", "query_graph", &args.to_string()])
            .output()
            .expect("query the graph");
        let stop = self
            .command(bin)
            .args(["daemon", "stop"])
            .output()
            .expect("stop fixture daemon");
        assert!(stop.status.success(), "{stop:?}");
        output
    }

    fn query(&self, bin: &Path, project: &str, cypher: &str) -> Value {
        let output = self.query_output(bin, project, cypher);
        assert!(output.status.success(), "{output:?}");
        serde_json::from_slice(&output.stdout).expect("parse query response")
    }

    fn compare_rust_query(
        &self,
        project: &str,
        cypher: &str,
        reference: &Value,
    ) {
        let db = self.0.join("cache").join(format!("{project}.db"));
        let query = cypher_linear::classify(cypher)
            .expect("query belongs to the Rust subset");
        let answer = query.execute(&db, project).expect("execute in Rust");
        assert_eq!(reference["columns"], json!(answer.columns), "{cypher}");
        assert_eq!(reference["rows"], json!(answer.rows), "{cypher}");
        assert_eq!(reference["total"], answer.rows.len(), "{cypher}");
        assert_eq!(reference["total_relation"], "eq");
    }

    fn query_reference(&self, project: &str, cypher: &str) -> Value {
        self.query(Self::reference_binary(), project, cypher)
    }

    fn reference_binary() -> &'static Path {
        if option_env!("MORI_C_REFERENCE") == Some("1") {
            return Path::new(concat!(
                env!("MORI_C_BUILD_DIR"),
                "/codebase-memory-mcp"
            ));
        }
        Path::new(MORI)
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

#[test]
fn linear_query_reference_keeps_order_and_duplicate_projections() {
    let fixture = Fixture::new();
    let db = fixture.index(CLIENT);
    let first = node(&db, CLIENT, "Caller", "other", "caller.other", "z.py");
    let second = node(&db, CLIENT, "Caller", "kept", "caller.kept", "a.py");
    let last = node(&db, CLIENT, "Caller", "kept", "caller.last", "b.py");
    let target = node(
        &db,
        CLIENT,
        "Service",
        "served",
        "service.served",
        "service.py",
    );
    let peer = node(
        &db,
        CLIENT,
        "Service",
        "served",
        "service.other",
        "other.py",
    );
    // Insert the higher-id target first so sorting by node ID changes the answer.
    for (source, destination, tag) in [
        (first, target, "first"),
        (second, peer, "peer"),
        (second, target, "target"),
        (last, target, "last"),
    ] {
        edge(
            &db,
            CLIENT,
            source,
            destination,
            "CALLS",
            json!({"tag": tag}),
        );
    }
    for (source, destination) in [(peer, last), (target, last), (target, first)]
    {
        edge(&db, CLIENT, source, destination, "HANDLES", json!({}));
    }
    drop(db);

    for (query, columns, rows) in [
        (
            "MATCH (a:Caller)-[r:CALLS]->(b:Service) WHERE a.name = 'kept' AND b.name = 'served' RETURN a, b.name, r.missing LIMIT 3",
            json!(["a", "b.name", "r.missing"]),
            json!([
                ["kept", "served", ""],
                ["kept", "served", ""],
                ["kept", "served", ""]
            ]),
        ),
        (
            "MATCH (a:Caller)-[r:CALLS]->(b:Service {qualified_name: 'service.served'}) RETURN a.qualified_name, r.tag LIMIT 2",
            json!(["a.qualified_name", "r.tag"]),
            json!([["caller.other", "first"], ["caller.kept", "target"]]),
        ),
        (
            "MATCH (a:Caller)-[r:CALLS]->(b:Service) RETURN a.qualified_name, r.tag LIMIT 4",
            json!(["a.qualified_name", "r.tag"]),
            json!([
                ["caller.other", "first"],
                ["caller.kept", "peer"],
                ["caller.kept", "target"],
                ["caller.last", "last"]
            ]),
        ),
        (
            "MATCH (a:Caller)-[:CALLS]->(b:Service)-[:HANDLES]->(c:Caller) WHERE a.qualified_name = 'caller.kept' RETURN b.qualified_name, c.qualified_name LIMIT 3",
            json!(["b.qualified_name", "c.qualified_name"]),
            json!([
                ["service.other", "caller.last"],
                ["service.served", "caller.last"],
                ["service.served", "caller.other"]
            ]),
        ),
        (
            "MATCH (a:Caller)-[:CALLS]->(b:Service) RETURN b.name LIMIT 0",
            json!(["b.name"]),
            json!([]),
        ),
        (
            "MATCH (a:Caller)<-[:CALLS]-(b:Service) RETURN b.name",
            json!(["b.name"]),
            json!([]),
        ),
    ] {
        let result = fixture.query_reference(CLIENT, query);
        fixture.compare_rust_query(CLIENT, query, &result);
        assert_eq!(result["columns"], columns, "{query}");
        assert_eq!(result["rows"], rows, "{query}");
        assert_eq!(result["total"], rows.as_array().unwrap().len(), "{query}");
    }
}

#[test]
fn documented_trades_query_returns_resolver_and_named_client_callers() {
    const PROVIDER: &str = "provider";
    const FIELD_PATH: &str = "Query.perp.trades";
    const ROUTE: &str = "__graphql__Query.perp.trades";
    // Keep this query in sync with "Who uses a resolver" in Mori's README.
    const QUERY: &str = r#"MATCH (h)-[:HANDLES]->(rt:Route)<-[x:CROSS_GRAPHQL_CALLS]-()
WHERE h.name = "Trades" AND rt.qualified_name = "__graphql__Query.perp.trades"
RETURN rt.name, h.name, h.file_path, x.callers LIMIT 100"#;

    let fixture = Fixture::new();
    seed_client(&fixture.index(CLIENT), ROUTE);
    let provider = fixture.index(PROVIDER);
    let route = node(
        &provider,
        PROVIDER,
        "Route",
        FIELD_PATH,
        ROUTE,
        "schema.graphqls",
    );
    let resolver = node(
        &provider,
        PROVIDER,
        "Method",
        "Trades",
        "provider.resolver.perpResolver.Trades",
        "resolver.go",
    );
    edge(
        &provider,
        PROVIDER,
        resolver,
        route,
        "HANDLES",
        json!({"via": "gqlgen_resolver"}),
    );
    drop(provider);
    fixture.relink(CLIENT, &[PROVIDER], 2);

    let args = json!({"project": PROVIDER, "query": QUERY, "format": "json"})
        .to_string();
    let query = |bin: &Path| {
        let output = fixture
            .command(bin)
            .args(["cli", "--quiet", "query_graph", &args])
            .output()
            .expect("query the provider graph");
        let stop = fixture
            .command(bin)
            .args(["daemon", "stop"])
            .output()
            .expect("stop fixture daemon");
        assert!(stop.status.success(), "{stop:?}");
        assert!(output.status.success(), "{output:?}");
        output
    };
    let linked = query(Path::new(MORI));
    if option_env!("MORI_C_REFERENCE") == Some("1") {
        let reference_bin =
            Path::new(env!("MORI_C_BUILD_DIR")).join("codebase-memory-mcp");
        let reference = query(&reference_bin);
        assert_eq!(linked.stdout, reference.stdout);
        assert_eq!(linked.stderr, reference.stderr);
    }

    let result: Value =
        serde_json::from_slice(&linked.stdout).expect("parse query result");
    fixture.compare_rust_query(PROVIDER, QUERY, &result);
    assert_eq!(
        result["columns"],
        json!(["rt.name", "h.name", "h.file_path", "x.callers"])
    );
    assert_eq!(result["total"], 1, "{result}");
    let [row] = result["rows"].as_array().expect("query rows").as_slice() else {
        panic!("expected one resolver row: {result}");
    };
    assert_eq!(row[0], FIELD_PATH);
    assert_eq!(row[1], "Trades");
    assert_eq!(row[2], "resolver.go");
    let callers: Value =
        serde_json::from_str(row[3].as_str().expect("caller record cell"))
            .expect("parse caller records");
    assert_eq!(
        callers,
        json!({
            "client:client.A.load": {"project": CLIENT, "qualified_name": "client.A.load", "function": "load", "file": "client.ts"},
            "client:client.B.load": {"project": CLIENT, "qualified_name": "client.B.load", "function": "load", "file": "client.ts"},
        })
    );

    // Add callers to the same provider edge to cross the native cell cap.
    // Keep that clipped value in parity until the public contract changes.
    let db = fixture.db(PROVIDER);
    let mut expanded = callers;
    for n in 0..12 {
        let qualified = format!("client.Extra{n}.load");
        expanded[format!("client:{qualified}")] = json!({"project": CLIENT, "qualified_name": qualified, "function": "load", "file": "long/client/path.ts"});
    }
    assert_eq!(
        db.execute(
            "UPDATE edges SET properties = ? WHERE type = 'CROSS_GRAPHQL_CALLS'",
            [json!({"callers": expanded}).to_string()],
        )
        .unwrap(),
        1,
    );
    drop(db);
    let reference = fixture.query_reference(PROVIDER, QUERY);
    let caller_cell = reference["rows"][0][3].as_str().unwrap();
    assert_eq!(caller_cell.len(), 511);
    assert!(serde_json::from_str::<Value>(caller_cell).is_err());
    fixture.compare_rust_query(PROVIDER, QUERY, &reference);
}

#[test]
fn linear_cypher_reads_through_the_callers_transaction() {
    let fixture = Fixture::new();
    let mut db = fixture.index(CLIENT);
    let snapshot = db.transaction().expect("begin caller's transaction");
    let caller = node(&snapshot, CLIENT, "Caller", "a", "caller.a", "a.py");
    let callee = node(&snapshot, CLIENT, "Callee", "b", "callee.b", "b.py");
    edge(&snapshot, CLIENT, caller, callee, "CALLS", json!({}));

    let query = cypher_linear::classify(
        "MATCH (a:Caller)-[:CALLS]->(b:Callee) RETURN b.name",
    )
    .expect("classify bounded query");
    let answer = query
        .execute_on(&snapshot, CLIENT)
        .expect("read caller's snapshot");
    assert_eq!(answer.rows, [["b"]]);

    snapshot.rollback().expect("caller owns transaction");
    let answer = query.execute_on(&db, CLIENT).expect("read after rollback");
    assert!(answer.rows.is_empty());
}

#[test]
fn linear_cypher_matches_the_native_result_ceiling() {
    const MATCH: &str = "MATCH (a:Caller)-[:CALLS]->(b:Service)-[:HANDLES]->(c:Target) RETURN c.name";
    let fixture = Fixture::new();
    let mut db = fixture.index(CLIENT);
    let tx = db.transaction().expect("begin fixture write");
    let service = node(&tx, CLIENT, "Service", "relay", "relay", "relay.py");
    // 650 edges expand to exactly 100,000 two-hop result rows.
    for n in 0..250 {
        let name = format!("caller{n}");
        let caller = node(&tx, CLIENT, "Caller", &name, &name, "caller.py");
        edge(&tx, CLIENT, caller, service, "CALLS", json!({}));
    }
    for n in 0..400 {
        let name = format!("target{n}");
        let target = node(&tx, CLIENT, "Target", &name, &name, "target.py");
        edge(&tx, CLIENT, service, target, "HANDLES", json!({}));
    }
    tx.commit().expect("publish fixture data");

    let below_ceiling = format!("{MATCH} LIMIT 99999");
    let reference = fixture.query_reference(CLIENT, &below_ceiling);
    let query = cypher_linear::classify(&below_ceiling).expect("classify query");
    let answer = query
        .execute_on(&db, CLIENT)
        .expect("execute below ceiling");
    assert_eq!(reference["total"], answer.rows.len());
    assert_eq!(answer.rows.len(), 99_999);
    assert_eq!(reference["total_relation"], "eq");

    let at_ceiling = format!("{MATCH} LIMIT 100000");
    let output =
        fixture.query_output(Fixture::reference_binary(), CLIENT, &at_ceiling);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("result exceeded 100k rows")
    );
    for cypher in [&at_ceiling, MATCH] {
        let query = cypher_linear::classify(cypher).expect("classify query");
        assert!(matches!(
            query.execute_on(&db, CLIENT),
            Err(cypher_linear::ExecutionError::RowCeiling)
        ));
    }
}

#[test]
fn unsupported_syntax_and_supported_storage_errors_are_distinct() {
    for query in [
        "MATCH (a) RETURN a.name",
        "MATCH (a)-[:CALLS]->(b) RETURN b.name ORDER BY b.name",
        "MATCH (a)-[:CALLS*1..2]->(b) RETURN b.name",
        "MATCH (a)-[:CALLS]-(b) RETURN b.name",
        "MATCH (a)-[:CALLS]- >(b) RETURN b.name",
        "MATCH (count)-[:CALLS]->(b) RETURN b.name",
        "MATCH (a)-[:CALLS]->(b) WHERE a.name = 'x' OR b.name = 'y' RETURN b.name",
        "MATCH (a)-[:CALLS]->(b) RETURN count(b)",
        "MATCH (a)-[:CALLS]->(b) RETURN typo.name",
        "MATCH (a)-[:CALLS]->(b) RETURN a.name MATCH (n)",
        "MATCH (a {name: '.*'})-[:CALLS]->(b) RETURN b.name",
    ] {
        assert!(cypher_linear::classify(query).is_err(), "{query}");
    }

    let fixture = Fixture::new();
    let path = fixture.0.join("missing.db");
    let query = cypher_linear::classify("MATCH (a)-[:CALLS]->(b) RETURN b.name")
        .unwrap();
    let Err(cypher_linear::ExecutionError::Storage(
        rusqlite::Error::SqliteFailure(error, _),
    )) = query.execute(&path, CLIENT)
    else {
        panic!("missing database must be a storage failure");
    };
    assert_eq!(error.code, rusqlite::ErrorCode::CannotOpen);
    assert!(!path.exists(), "execution must not silently create a cache");
}
