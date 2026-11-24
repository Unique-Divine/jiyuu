//! Exercises explicit GraphQL relinks through the linked Mori executable.
#![cfg(feature = "c-backend")]

use std::cell::Cell;
use std::fs::{self, DirBuilder};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

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
        assert_eq!(reference["rows"], json!(utf8_rows(&answer)), "{cypher}");
        assert_eq!(reference["total"], answer.rows.len(), "{cypher}");
        assert!(!answer.truncated, "{cypher}");
        assert_eq!(reference["total_relation"], "eq");
    }

    fn query_reference(&self, project: &str, cypher: &str) -> Value {
        self.query(Self::reference_binary(), project, cypher)
    }

    fn start_query_daemon(&self, bin: &Path, backend: &str) {
        self.start_query_daemon_with_env(bin, backend, &[]);
    }

    fn start_query_daemon_with_env(
        &self,
        bin: &Path,
        backend: &str,
        environment: &[(&str, &str)],
    ) {
        let output = self
            .command(bin)
            .envs(environment.iter().copied())
            .env("MORI_IMPL_QUERY", backend)
            .env("CBM_LOG_LEVEL", "debug")
            .args(["daemon", "start"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }

    fn rust_query_count(&self) -> usize {
        fs::read_to_string(self.0.join("cache/logs/cbm-daemon.log"))
            .unwrap()
            .lines()
            .filter(|line| {
                line.contains("query_graph.evaluator")
                    && line.contains("backend=rust")
            })
            .count()
    }

    fn mcp_query_text(&self, bin: &Path, args: Value) -> String {
        let call = json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
            "name":"query_graph","arguments":args
        }});
        let requests = self.0.join("mcp-query-requests");
        fs::write(&requests, format!(
            "{}\n{}\n{call}\n",
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"query-test","version":"1"}}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        )).unwrap();
        let output = self
            .command(bin)
            .stdin(Stdio::from(fs::File::open(requests).unwrap()))
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let stdout = String::from_utf8(output.stdout).unwrap();
        let response = stdout
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .find(|response| response["id"] == 2)
            .unwrap();
        assert_ne!(response["result"]["isError"], true, "{response}");
        response["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn compare_query_pages(&self, cases: &[(&str, usize, bool)]) -> Vec<Value> {
        self.compare_query_pages_with_env(cases, &[])
    }

    fn compare_query_pages_with_env(
        &self,
        cases: &[(&str, usize, bool)],
        environment: &[(&str, &str)],
    ) -> Vec<Value> {
        let run = |bin: &Path, backend: &str| {
            self.start_query_daemon_with_env(bin, backend, environment);
            let mut pages = Vec::new();
            for &(query, offset, supported) in cases {
                let before = self.rust_query_count();
                let args = json!({
                    "project": CLIENT, "query": query, "format": "json",
                    "max_rows": 1, "offset": offset,
                });
                let output = self
                    .command(bin)
                    .args(["cli", "--quiet", "query_graph", &args.to_string()])
                    .output()
                    .unwrap();
                assert!(output.status.success(), "{query}: {output:?}");
                assert_eq!(
                    self.rust_query_count() - before,
                    usize::from(backend == "rust" && supported),
                    "{query}",
                );
                pages.push(output.stdout);
            }
            let stop =
                self.command(bin).args(["daemon", "stop"]).output().unwrap();
            assert!(stop.status.success(), "{stop:?}");
            pages
        };
        let reference = run(Self::reference_binary(), "c");
        let candidate = run(Path::new(MORI), "rust");
        cases
            .iter()
            .zip(reference.iter().zip(&candidate))
            .map(|((query, offset, _), (c, rust))| {
                assert_eq!(c, rust, "{query}, offset {offset}");
                serde_json::from_slice(rust).expect("parse matched query page")
            })
            .collect()
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

fn utf8_rows(result: &cypher_linear::ResultRows) -> Vec<Vec<&str>> {
    result
        .rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|cell| {
                    std::str::from_utf8(cell).expect("UTF-8 fixture cell")
                })
                .collect()
        })
        .collect()
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

fn fail_node_read(db: &Connection, id: i64) {
    // Shadow the table on this connection only. SQLite raises an execution
    // error if a scan actually reads the selected row's properties.
    db.execute_batch(&format!(
        "CREATE TEMP VIEW nodes AS
         SELECT id, project, label, name, qualified_name, file_path, start_line, end_line,
                CASE WHEN id = {id} THEN abs(-9223372036854775808) ELSE properties END AS properties
         FROM main.nodes;"
    ))
    .expect("inject a SQLite read failure");
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
    assert_eq!(utf8_rows(&answer), [["b"]]);

    snapshot.rollback().expect("caller owns transaction");
    let answer = query.execute_on(&db, CLIENT).expect("read after rollback");
    assert!(answer.rows.is_empty());
}

#[test]
fn linear_cypher_stops_inside_high_fanout_adjacency() {
    use cypher_linear::{ExecutionBudget, ExecutionError};

    let fixture = Fixture::new();
    let mut db = fixture.index(CLIENT);
    let tx = db.transaction().expect("begin fixture write");
    let caller = node(&tx, CLIENT, "Caller", "a", "a", "a.py");
    let relay = node(&tx, CLIENT, "Relay", "b", "b", "b.py");
    edge(&tx, CLIENT, caller, relay, "CALLS", json!({}));
    let mut tail = 0;
    for n in 0..2048 {
        let name = format!("target{n}");
        let target = node(&tx, CLIENT, "Other", &name, &name, "c.py");
        edge(&tx, CLIENT, relay, target, "CALLS", json!({}));
        tail = target;
    }
    tx.commit().expect("publish fixture data");

    let query = cypher_linear::classify(
        "MATCH (a:Caller)-[:CALLS]->(b:Relay)-[:CALLS]->(c:Wanted) RETURN c.name",
    )
    .expect("classify bounded query");
    assert!(query.execute_on(&db, CLIENT).unwrap().rows.is_empty());

    // If the budget were tested only after collecting adjacency, the scan
    // would return this storage error instead of the deadline or cancellation.
    fail_node_read(&db, tail);
    assert!(matches!(
        query.execute_on(&db, CLIENT),
        Err(ExecutionError::Storage(rusqlite::Error::SqliteFailure(_, Some(message))))
            if message == "integer overflow"
    ));

    let start = Instant::now();
    let checks = Cell::new(0);
    let clock = || {
        checks.set(checks.get() + 1);
        start
            + Duration::from_secs(match checks.get() {
                1 => 0,
                2 => 29,
                _ => 30,
            })
    };
    let mut deadline = ExecutionBudget::new(&clock, &|| false);
    assert!(matches!(
        query.execute_with_budget(&db, CLIENT, &mut deadline),
        Err(ExecutionError::Deadline)
    ));
    assert_eq!(checks.get(), 3);

    let checks = Cell::new(0);
    let cancelled = || {
        checks.set(checks.get() + 1);
        checks.get() == 2
    };
    let now = || start;
    let mut budget = ExecutionBudget::new(&now, &cancelled);
    assert!(matches!(
        query.execute_with_budget(&db, CLIENT, &mut budget),
        Err(ExecutionError::Cancelled)
    ));
    assert_eq!(checks.get(), 2);
}

#[test]
fn linear_cypher_filters_each_bound_node_before_expanding_it() {
    let fixture = Fixture::new();
    let mut db = fixture.index(CLIENT);
    let tx = db.transaction().expect("begin fixture write");
    let rejected_start =
        node(&tx, CLIENT, "Start", "ignored", "start.skip", "a.py");
    let start = node(&tx, CLIENT, "Start", "selected", "start.keep", "a.py");
    let rejected_end =
        node(&tx, CLIENT, "Finish", "ignored", "finish.skip", "b.py");
    let end = node(&tx, CLIENT, "Finish", "selected", "finish.keep", "b.py");
    let rejected_relay =
        node(&tx, CLIENT, "Relay", "ignored", "relay.skip", "c.py");
    let relay = node(&tx, CLIENT, "Relay", "selected", "relay.keep", "c.py");
    let target = node(&tx, CLIENT, "Target", "answer", "target", "d.py");
    for n in 0..3072 {
        let name = format!("other{n}");
        let other = node(&tx, CLIENT, "Other", &name, &name, "other.py");
        edge(&tx, CLIENT, rejected_start, other, "FWD", json!({}));
        edge(&tx, CLIENT, other, rejected_end, "REV", json!({}));
        edge(&tx, CLIENT, rejected_relay, other, "TAIL_LINK", json!({}));
    }
    edge(&tx, CLIENT, start, target, "FWD", json!({}));
    edge(&tx, CLIENT, start, end, "REV", json!({}));
    edge(&tx, CLIENT, start, rejected_relay, "JOIN", json!({}));
    edge(&tx, CLIENT, start, relay, "JOIN", json!({}));
    edge(&tx, CLIENT, relay, target, "TAIL_LINK", json!({}));
    tx.commit().expect("publish fixture data");

    for (cypher, expected) in [
        (
            "MATCH (a:Start)-[:FWD]->(b:Target) WHERE a.name = 'selected' RETURN b.name",
            "answer",
        ),
        (
            "MATCH (a:Start)-[:REV]->(b:Finish) WHERE a.file_path = 'a.py' AND b.name = 'selected' RETURN a.name",
            "selected",
        ),
        (
            "MATCH (a:Start)-[:JOIN]->(b:Relay)-[:TAIL_LINK]->(c:Target) WHERE b.name = 'selected' RETURN c.name",
            "answer",
        ),
    ] {
        let started = Instant::now();
        let checks = Cell::new(0);
        let clock = || {
            checks.set(checks.get() + 1);
            if checks.get() <= 3 {
                started
            } else {
                started + Duration::from_secs(30)
            }
        };
        let mut budget = cypher_linear::ExecutionBudget::new(&clock, &|| false);
        let query = cypher_linear::classify(cypher)
            .unwrap_or_else(|_| panic!("classify bounded query: {cypher}"));
        let answer = query
            .execute_with_budget(&db, CLIENT, &mut budget)
            .unwrap_or_else(|error| panic!("{cypher}: {error:?}"));
        let rows = utf8_rows(&answer);
        assert_eq!(rows, [[expected]], "{cypher}");
        assert_eq!(checks.get(), 3, "expanded rejected adjacency: {cypher}");
        assert_eq!(
            fixture.query_reference(CLIENT, cypher)["rows"],
            json!(rows),
            "{cypher}",
        );
    }
}

#[test]
fn linear_cypher_keeps_its_graph_when_a_replacement_publishes() {
    let fixture = Fixture::new();
    let mut db = fixture.index(CLIENT);
    db.execute_batch("PRAGMA journal_mode = DELETE;")
        .expect("use a single-file database");
    let tx = db.transaction().expect("begin fixture write");
    let caller = node(&tx, CLIENT, "Caller", "a", "a", "a.py");
    for n in 0..1024 {
        let qualified_name = format!("target{n}");
        let target = node(&tx, CLIENT, "Target", "old", &qualified_name, "b.py");
        edge(&tx, CLIENT, caller, target, "CALLS", json!({}));
    }
    tx.commit().expect("publish fixture data");

    let live_path = fixture.0.join("cache").join("client.db");
    let staged_path = live_path.with_extension("stage");
    fs::copy(&live_path, &staged_path).expect("copy the staged generation");
    let staged = Connection::open(&staged_path).expect("open staged generation");
    staged
        .execute("UPDATE nodes SET name = 'new' WHERE label = 'Target'", [])
        .expect("prepare the replacement graph");
    drop(staged);

    let query = cypher_linear::classify(
        "MATCH (a:Caller)-[:CALLS]->(b:Target) RETURN b.name",
    )
    .expect("classify bounded query");
    let start = Instant::now();
    let checks = Cell::new(0);
    let clock = || {
        checks.set(checks.get() + 1);
        if checks.get() == 3 {
            fs::rename(&staged_path, &live_path).expect("publish replacement");
        }
        start
    };
    let mut budget = cypher_linear::ExecutionBudget::new(&clock, &|| false);
    let original = query
        .execute_with_budget(&db, CLIENT, &mut budget)
        .expect("read the caller's graph during publication");
    assert!(checks.get() > 3, "publish inside adjacency traversal");
    assert_eq!(original.rows.len(), 1024);
    assert!(utf8_rows(&original).iter().all(|row| row == &["old"]));
    let replacement = query
        .execute(&live_path, CLIENT)
        .expect("read the published replacement");
    assert_eq!(replacement.rows.len(), 1024);
    assert!(utf8_rows(&replacement).iter().all(|row| row == &["new"]));
}

#[test]
fn opt_in_rust_query_preserves_cursor_and_daemon_dispatch() {
    const QUERY: &str = "MATCH (a:Caller)-[:CALLS]->(b:Target) RETURN b.name";
    const ORDERED: &str = "MATCH (a:Caller)-[:CALLS]->(b:Target) RETURN b.name ORDER BY b.file_path";
    let fixture = Fixture::new();
    let db = fixture.index(CLIENT);
    let caller = node(&db, CLIENT, "Caller", "a", "a", "a.py");
    for name in ["b", "c"] {
        let callee = node(&db, CLIENT, "Target", name, name, "b.py");
        edge(&db, CLIENT, caller, callee, "CALLS", json!({}));
    }
    drop(db);

    let request = json!({"project": CLIENT, "query": QUERY, "format": "json", "max_rows": 1});
    let reference_bin = Fixture::reference_binary();
    let reference = fixture
        .command(reference_bin)
        .args(["cli", "--quiet", "query_graph", &request.to_string()])
        .output()
        .unwrap();
    assert!(reference.status.success(), "{reference:?}");
    let reference: Value = serde_json::from_slice(&reference.stdout).unwrap();
    let stop = |binary| {
        let result = fixture
            .command(binary)
            .args(["daemon", "stop"])
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
    };
    stop(reference_bin);
    let reference_ordered = fixture.query_reference(CLIENT, ORDERED);

    let candidate_bin = Path::new(MORI);
    let start = fixture
        .command(candidate_bin)
        .env("MORI_IMPL_QUERY", "rust")
        .env("CBM_LOG_LEVEL", "debug")
        .args(["daemon", "start"])
        .output()
        .unwrap();
    assert!(start.status.success(), "{start:?}");
    let candidate = fixture
        .command(candidate_bin)
        .args(["cli", "--quiet", "query_graph", &request.to_string()])
        .output()
        .unwrap();
    assert!(candidate.status.success(), "{candidate:?}");
    let candidate: Value = serde_json::from_slice(&candidate.stdout).unwrap();
    assert_eq!(candidate, reference);
    let cursor = candidate["next_cursor"].as_str().expect("cursor");

    let next = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
        "name": "query_graph",
        "arguments": {"project": CLIENT, "query": QUERY, "cursor": cursor, "format": "json"}
    }});
    let ordered = json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {
        "name": "query_graph",
        "arguments": {"project": CLIENT, "query": ORDERED, "format": "json"}
    }});
    let requests = fixture.0.join("query-requests");
    fs::write(
        &requests,
        format!(
            "{}\n{}\n{next}\n{ordered}\n",
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"query-test","version":"1"}}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        ),
    )
    .unwrap();
    let forwarded = fixture
        .command(candidate_bin)
        .stdin(Stdio::from(fs::File::open(requests).unwrap()))
        .output()
        .unwrap();
    stop(candidate_bin);
    assert!(forwarded.status.success(), "{forwarded:?}");
    let output = String::from_utf8(forwarded.stdout).unwrap();
    let responses: Vec<Value> = output
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let result = |id| -> Value {
        let reply = responses.iter().find(|reply| reply["id"] == id).unwrap();
        let text = reply["result"]["content"][0]["text"].as_str().unwrap();
        serde_json::from_str(text).unwrap()
    };
    assert_eq!(result(2)["rows"], json!([["c"]]));
    assert_eq!(result(3), reference_ordered);
    let log =
        fs::read_to_string(fixture.0.join("cache/logs/cbm-daemon.log")).unwrap();
    assert_eq!(
        log.lines()
            .filter(|line| line.contains("query_graph.evaluator")
                && line.contains("backend=rust"))
            .count(),
        2,
        "the CLI and MCP cursor used Rust, and the unprojected sort key used C: {log}"
    );
}

#[test]
fn zero_hop_impact_query_uses_rust_and_matches_c_pages() {
    // Graph UI declarationsInFiles("Function", ["a.py", "z.py"]).
    const IMPACT: &str = r#"MATCH (n:Function) WHERE n.file_path = "a.py" OR n.file_path = "z.py" RETURN n.name, n.qualified_name, n.file_path, n.start_line, n.end_line, n.is_test LIMIT 1000"#;
    let fixture = Fixture::new();
    let db = fixture.index(CLIENT);
    let z = node(&db, CLIENT, "Function", "zeta", "client.zeta", "z.py");
    db.execute(
        r#"UPDATE nodes SET start_line = 7, end_line = 11, properties = '{"is_test":true}' WHERE id = ?"#,
        [z],
    )
    .unwrap();
    node(&db, CLIENT, "Function", "alpha", "client.alpha", "a.py");
    node(
        &db,
        CLIENT,
        "Function",
        "excluded",
        "client.excluded",
        "b.py",
    );
    node(&db, CLIENT, "Method", "method", "client.method", "a.py");
    drop(db);

    let cases = [
        (IMPACT, 0, true),
        (IMPACT, 1, true),
        (
            "MATCH (n) WHERE n.name = 'alpha' OR n.name = 'zeta' RETURN n, n.file, n.name",
            0,
            true,
        ),
        (
            "MATCH (n:Function {file_path: 'a.py'}) WHERE n.name = 'alpha' AND n.is_test = '' RETURN n.name",
            0,
            true,
        ),
        ("MATCH (n:Missing) RETURN n.name", 0, true),
        ("MATCH (n:Function) RETURN n.name LIMIT 0", 0, true),
        (
            "MATCH (n:Function) WHERE n.file_path = 'a.py' OR n.file_path = 'z.py' RETURN n.name LIMIT 1",
            0,
            true,
        ),
        (
            "MATCH (n:Function) RETURN n.name ORDER BY n.start_line",
            0,
            false,
        ),
        (
            "MATCH (n:Function) WHERE n.name = 'alpha' OR n.name = 'zeta' AND n.file_path = 'z.py' RETURN n.name",
            0,
            false,
        ),
        ("MATCH (n:Function) RETURN count(n)", 0, true),
    ];
    let candidate = fixture.compare_query_pages(&cases);
    let page = |i: usize| &candidate[i];
    let first = page(0);
    assert_eq!(
        first["columns"],
        json!([
            "n.name",
            "n.qualified_name",
            "n.file_path",
            "n.start_line",
            "n.end_line",
            "n.is_test"
        ])
    );
    assert_eq!(
        first["rows"],
        json!([["zeta", "client.zeta", "z.py", "7", "11", "true"]])
    );
    assert_eq!(first["total"], 2);
    assert_eq!(first["has_more"], true);
    assert_eq!(
        page(1)["rows"],
        json!([["alpha", "client.alpha", "a.py", "0", "0", ""]])
    );
    assert_eq!(page(2)["rows"], json!([["alpha", "a.py", "alpha"]]));
    assert_eq!(page(3)["rows"], json!([["alpha"]]));
    assert_eq!(page(4)["rows"], json!([]));
    assert_eq!(page(5)["rows"], json!([]));
    assert_eq!(page(6)["rows"], json!([["zeta"]]));
    assert_eq!(page(6)["total"], 1);
}

#[test]
fn zero_hop_property_byte_boundaries_and_wide_projection_match_c() {
    let fixture = Fixture::new();
    let db = fixture.index(CLIENT);
    let split = format!("{}é", "a".repeat(510));
    let exact = format!("{}é", "a".repeat(509));
    let capped = "a".repeat(511);
    for (name, value) in [
        ("split", split.clone()),
        ("exact", exact.clone()),
        ("capped", format!("{capped}é")),
    ] {
        let id = node(&db, CLIENT, "Boundary", name, name, "boundary.py");
        db.execute(
            "UPDATE nodes SET properties = ? WHERE id = ?",
            params![json!({"value": value}).to_string(), id],
        )
        .unwrap();
    }

    let keys = (0..32).map(|i| format!("prop{i}"));
    let values: Vec<_> = (0..32).map(|i| format!("value{i}")).collect();
    let properties: serde_json::Map<_, _> = keys
        .clone()
        .zip(values.iter().map(|value| json!(value)))
        .collect();
    let id = node(&db, CLIENT, "Wide", "wide", "wide", "wide.py");
    db.execute(
        "UPDATE nodes SET properties = ? WHERE id = ?",
        params![Value::Object(properties).to_string(), id],
    )
    .unwrap();
    let columns: Vec<_> = keys.map(|key| format!("n.{key}")).collect();
    let wide = format!("MATCH (n:Wide) RETURN {}", columns.join(", "));
    assert!(cypher_linear::classify(&format!("{wide}, n.name")).is_err());

    let no_match =
        format!("MATCH (n:Boundary) WHERE n.value = '{split}' RETURN n.name");
    let exact_match = format!(
        "MATCH (n:Boundary) WHERE n.value = '{exact}' RETURN n.name, n.value"
    );
    let capped_match = format!(
        "MATCH (n:Boundary {{value: '{capped}'}}) RETURN n.name, n.value"
    );
    let pages = fixture.compare_query_pages(&[
        (&no_match, 0, true),
        (&exact_match, 0, true),
        (&capped_match, 0, true),
        (&wide, 0, true),
    ]);
    assert_eq!(pages[0]["rows"], json!([]));
    assert_eq!(pages[1]["rows"], json!([["exact", exact]]));
    assert_eq!(pages[2]["rows"], json!([["capped", capped]]));
    assert_eq!(pages[3]["columns"], json!(columns));
    assert_eq!(pages[3]["rows"], json!([values]));
}

#[test]
fn split_projection_uses_native_mcp_serialization_and_cursor() {
    const QUERY: &str = "MATCH (n:Projection) RETURN n.value";
    let fixture = Fixture::new();
    let db = fixture.index(CLIENT);
    let first = "a".repeat(510);
    let second = "b".repeat(510);
    for (name, value) in [
        ("first", format!("{first}é")),
        ("second", format!("{second}€")),
    ] {
        let id = node(&db, CLIENT, "Projection", name, name, "projection.py");
        db.execute(
            "UPDATE nodes SET properties = ? WHERE id = ?",
            params![json!({"value": value}).to_string(), id],
        )
        .unwrap();
    }
    let materialized = cypher_linear::classify(QUERY)
        .unwrap()
        .execute_on(&db, CLIENT)
        .unwrap();
    assert_eq!(
        materialized.rows,
        [
            [[first.as_bytes(), &[0xc3]].concat()],
            [[second.as_bytes(), &[0xe2]].concat()],
        ]
    );
    drop(db);

    let run = |bin: &Path, backend: &str| {
        fixture.start_query_daemon(bin, backend);
        let before = fixture.rust_query_count();
        let query = |args| fixture.mcp_query_text(bin, args);
        let first_page = query(
            json!({"project":CLIENT,"query":QUERY,"format":"json","max_rows":1}),
        );
        let page: Value = serde_json::from_str(&first_page).unwrap();
        let cursor = page["next_cursor"].as_str().expect("first page cursor");
        let next_page = query(
            json!({"project":CLIENT,"query":QUERY,"format":"json","cursor":cursor}),
        );
        let text = query(json!({"project":CLIENT,"query":QUERY}));
        let budget = query(
            json!({"project":CLIENT,"query":QUERY,"format":"json","max_output_tokens":128}),
        );
        assert_eq!(
            fixture.rust_query_count() - before,
            if backend == "rust" { 4 } else { 0 }
        );

        let stop = fixture
            .command(bin)
            .args(["daemon", "stop"])
            .output()
            .unwrap();
        assert!(stop.status.success(), "{stop:?}");
        [first_page, next_page, text, budget]
    };
    let reference = run(Fixture::reference_binary(), "c");
    let candidate = run(Path::new(MORI), "rust");
    assert_eq!(candidate, reference);
    let page =
        |index: usize| serde_json::from_str::<Value>(&candidate[index]).unwrap();
    // The native MCP tool payload encodes invalid UTF-8 losslessly.
    assert_eq!(
        page(0)["rows"],
        json!([[format!("@bytes:{}c3", "61".repeat(510))]])
    );
    assert_eq!(
        page(1)["rows"],
        json!([[format!("@bytes:{}e2", "62".repeat(510))]])
    );
    assert_eq!(page(3)["truncation_reason"], "output_budget");
}

#[test]
fn ordered_graph_ui_calls_match_native_ties_limits_and_cursor_pages() {
    // callsOut("client.caller", 4) and callsIn("client.target", 4).
    const OUT: &str = r#"MATCH (a)-[r:CALLS]->(b) WHERE a.qualified_name = "client.caller" RETURN b.name, b.qualified_name, b.file_path, b.start_line, r.line ORDER BY r.line LIMIT 4"#;
    const IN: &str = r#"MATCH (a)-[r:CALLS]->(b) WHERE b.qualified_name = "client.target" RETURN a.name, a.qualified_name, a.file_path, a.start_line, a.is_test, r.line ORDER BY r.line LIMIT 4"#;
    const ALIASED: &str = r#"MATCH (a)-[r:CALLS]->(b) WHERE b.qualified_name = "client.target" RETURN a.name AS symbol, r.line AS line ORDER BY line DESC, symbol ASC LIMIT 4"#;
    let fixture = Fixture::new();
    let db = fixture.index(CLIENT);
    let caller = node(
        &db,
        CLIENT,
        "Function",
        "caller",
        "client.caller",
        "calls.py",
    );
    let target = node(
        &db,
        CLIENT,
        "Function",
        "target",
        "client.target",
        "calls.py",
    );
    for (name, props) in [
        ("ten", json!({"line": 10})),
        ("two-z", json!({"line": 2})),
        ("two-a", json!({"line": "02"})),
        ("negative", json!({"line": -1})),
        ("missing", json!({})),
        ("prefix", json!({"line": "2x"})),
    ] {
        let called = node(
            &db,
            CLIENT,
            "Function",
            name,
            &format!("out.{name}"),
            "calls.py",
        );
        edge(&db, CLIENT, caller, called, "CALLS", props.clone());
        let calling = node(
            &db,
            CLIENT,
            "Function",
            name,
            &format!("in.{name}"),
            "calls.py",
        );
        edge(&db, CLIENT, calling, target, "CALLS", props);
    }
    drop(db);

    let run = |bin: &Path, backend: &str| {
        fixture.start_query_daemon(bin, backend);
        let query = |args| {
            let before = fixture.rust_query_count();
            let page = fixture.mcp_query_text(bin, args);
            assert_eq!(
                fixture.rust_query_count() - before,
                usize::from(backend == "rust")
            );
            page
        };
        let mut pages = Vec::new();
        for cypher in [OUT, IN, ALIASED] {
            let first = query(
                json!({"project":CLIENT,"query":cypher,"format":"json","max_rows":2}),
            );
            let page: Value = serde_json::from_str(&first).unwrap();
            let cursor = page["next_cursor"].as_str().expect("ordered cursor");
            let second = query(
                json!({"project":CLIENT,"query":cypher,"format":"json","max_rows":2,"cursor":cursor}),
            );
            pages.push([first, second]);
        }
        let stop = fixture
            .command(bin)
            .args(["daemon", "stop"])
            .output()
            .unwrap();
        assert!(stop.status.success(), "{stop:?}");
        pages
    };
    let reference = run(Fixture::reference_binary(), "c");
    let candidate = run(Path::new(MORI), "rust");
    assert_eq!(candidate, reference);
    for (index, pages) in candidate.iter().enumerate() {
        let [first, second] = pages
            .each_ref()
            .map(|page| serde_json::from_str::<Value>(page).unwrap());
        assert_eq!(first["total"], 4);
        assert_eq!(first["has_more"], true);
        assert_eq!(second["offset"], 2);
        assert_eq!(second["has_more"], false);
        let names: Vec<_> = [&first, &second]
            .into_iter()
            .flat_map(|page| {
                page["rows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|row| row[0].as_str().unwrap())
            })
            .collect();
        if index == 2 {
            assert_eq!(first["columns"], json!(["symbol", "line"]));
            assert_eq!(names, ["ten", "prefix", "two-a", "two-z"]);
        } else {
            assert_eq!(names, ["negative", "missing", "two-z", "two-a"]);
        }
    }
}

#[test]
fn graph_ui_span_and_string_predicates_execute_in_rust() {
    let fixture = Fixture::new();
    let db = fixture.index(CLIENT);
    for (name, path, start, end) in [
        ("parse_outer", "span.py", 1, 40),
        ("parse_inner", "span.py", 10, 10),
        ("parse_after", "span.py", 11, 20),
        ("helper_inner", "other.py", 10, 10),
    ] {
        let id = node(&db, CLIENT, "SpanProbe", name, name, path);
        db.execute(
            "UPDATE nodes SET start_line = ?, end_line = ? WHERE id = ?",
            params![start, end, id],
        )
        .unwrap();
    }
    let caller = node(&db, CLIENT, "TextProbe", "caller", "caller", "span.py");
    for name in ["parse_first", "parse_second", "skip"] {
        let callee = node(&db, CLIENT, "TextProbe", name, name, "span.py");
        edge(&db, CLIENT, caller, callee, "CALLS", json!({}));
    }
    drop(db);

    // Matches the Graph UI enclosing-symbol query, including span boundaries.
    let span = "MATCH (n:SpanProbe) WHERE n.file_path = 'span.py' AND n.start_line <= 10 AND n.end_line >= 10 RETURN n.name, n.start_line ORDER BY n.start_line DESC LIMIT 10";
    // The depth-one replay shape exercises a non-equality predicate on the far
    // binding, where it must not act as a seed equality.
    let prefix = "MATCH (a:TextProbe)-[:CALLS]->(b) WHERE b.name STARTS WITH 'parse' RETURN b.name ORDER BY b.name LIMIT 10";
    let text = "MATCH (n:SpanProbe) WHERE n.name CONTAINS 'inner' AND n.name ENDS WITH 'inner' AND n.name <> 'helper_inner' AND NOT n.name STARTS WITH 'helper' RETURN n.name";
    let strict = "MATCH (n:SpanProbe) WHERE n.start_line > 10 AND n.end_line < 40 AND n.file_path != 'other.py' RETURN n.name";
    let empty = "MATCH (n:SpanProbe) WHERE n.name CONTAINS '' OR n.name STARTS WITH 'absent' RETURN n.name ORDER BY n.name LIMIT 1";
    let cases = [
        ((span, 0, true), json!([["parse_inner", "10"]])),
        ((span, 1, true), json!([["parse_outer", "1"]])),
        ((prefix, 0, true), json!([["parse_first"]])),
        ((prefix, 1, true), json!([["parse_second"]])),
        ((text, 0, true), json!([["parse_inner"]])),
        ((strict, 0, true), json!([["parse_after"]])),
        ((empty, 0, true), json!([["helper_inner"]])),
    ];
    let inputs: Vec<_> = cases.iter().map(|(case, _)| *case).collect();
    for (page, ((query, _, _), rows)) in
        fixture.compare_query_pages(&inputs).iter().zip(cases)
    {
        assert_eq!(page["rows"], rows, "{query}");
    }
}

#[test]
fn bounded_count_projects_native_groups_and_empty_results_in_rust() {
    let fixture = Fixture::new();
    let db = fixture.index(CLIENT);
    let first = node(&db, CLIENT, "SourceProbe", "seed", "seed", "group.py");
    for (name, bucket) in [
        ("one", "beta"),
        ("two", "alpha"),
        ("three", "beta"),
        ("four", "gamma"),
        ("five", "alpha"),
    ] {
        let id = node(&db, CLIENT, "CountProbe", name, name, "group.py");
        db.execute(
            "UPDATE nodes SET properties = ? WHERE id = ?",
            params![json!({"bucket":bucket}).to_string(), id],
        )
        .unwrap();
        if name == "one" {
            db.execute(
                "UPDATE nodes SET start_line = ?1, end_line = ?2 WHERE id = ?3",
                params![2147483648_i64, b"17 lines".as_slice(), id],
            )
            .unwrap();
        }
        edge(&db, CLIENT, first, id, "CALLS", json!({}));
    }
    // The GROUP BY cell clips direct string fields at the same boundary as
    // native, even when the prefix ends midway through a UTF-8 scalar.
    let prefix = "a".repeat(510);
    for (name, qn) in [
        (format!("{prefix}é"), "one_clip"),
        (format!("{prefix}ê"), "two_clip"),
    ] {
        node(&db, CLIENT, "ClipProbe", &name, qn, "group.py");
    }
    drop(db);

    let grouped = "MATCH (n) WHERE n.file_path = 'group.py' RETURN n.label AS label, count(*) AS total ORDER BY total DESC, label";
    let ties =
        "MATCH (n:CountProbe) RETURN n.bucket, COUNT(*) ORDER BY COUNT(*) DESC";
    let one_hop = "MATCH (a:SourceProbe)-[:CALLS]->(b) RETURN b.bucket AS key, count(b) AS total ORDER BY total DESC, key LIMIT 2";
    let first_group = "MATCH (n:CountProbe) RETURN n.bucket, count(*) LIMIT 1";
    let wide_count = "MATCH (n:CountProbe) WHERE n.name = 'one' RETURN count(*)";
    let wide_lines = "MATCH (n:CountProbe) WHERE n.name = 'one' RETURN n.start_line, n.end_line";
    let missing_prop = "MATCH (n:CountProbe) RETURN count(n.missing)";
    let empty = "MATCH (n:MissingProbe) RETURN count(*) AS total";
    let empty_group = "MATCH (n:MissingProbe) RETURN n.label, count(*)";
    let zero_limit = "MATCH (n:CountProbe) RETURN count(*) LIMIT 0";
    let clip = "MATCH (n:ClipProbe) RETURN n.name, COUNT(n)";
    let compound = "MATCH (n:CountProbe) RETURN n.label, n.file_path, count(*)";
    let cases = [
        ((grouped, 0, true), json!([["CountProbe", "5"]])),
        ((grouped, 1, true), json!([["ClipProbe", "2"]])),
        ((ties, 0, true), json!([["beta", "2"]])),
        ((ties, 1, true), json!([["alpha", "2"]])),
        ((one_hop, 0, true), json!([["alpha", "2"]])),
        ((one_hop, 1, true), json!([["beta", "2"]])),
        ((first_group, 0, true), json!([["beta", "2"]])),
        ((wide_count, 0, true), json!([["1"]])),
        ((wide_lines, 0, true), json!([["-2147483648", "17"]])),
        ((missing_prop, 0, true), json!([["5"]])),
        ((empty, 0, true), json!([["0"]])),
        ((empty_group, 0, true), json!([])),
        ((zero_limit, 0, true), json!([])),
        (
            (clip, 0, true),
            json!([[format!("@bytes:{}c3", "61".repeat(510)), "2"]]),
        ),
        (
            (compound, 0, false),
            json!([["CountProbe", "group.py", "5"]]),
        ),
    ];
    let inputs: Vec<_> = cases.iter().map(|(case, _)| *case).collect();
    for (page, ((query, _, _), rows)) in
        fixture.compare_query_pages(&inputs).iter().zip(cases)
    {
        assert_eq!(page["rows"], rows, "{query}");
    }
}

#[cfg(debug_assertions)]
#[test]
fn ordered_count_overflow_falls_back_to_rank_late_groups() {
    use cypher_linear::{ExecutionBudget, ExecutionError};

    const QUERY: &str = "MATCH (n:RankProbe) RETURN n.name, count(*) AS total ORDER BY total DESC LIMIT 1";
    const ZERO: &str = "MATCH (n:RankProbe) RETURN n.name, count(*) AS total ORDER BY total DESC LIMIT 0";
    let fixture = Fixture::new();
    let db = fixture.index(CLIENT);
    let mut tail = 0;
    for (index, name) in ["alpha", "beta", "winner", "winner", "winner"]
        .into_iter()
        .enumerate()
    {
        tail = node(
            &db,
            CLIENT,
            "RankProbe",
            name,
            &format!("rank.{index}"),
            "rank.py",
        );
    }
    let mut budget = ExecutionBudget::new(&Instant::now, &|| false);
    budget.set_ordered_count_group_limit_for_tests(2);
    let query = cypher_linear::classify(QUERY).unwrap();
    assert!(matches!(
        query.execute_with_budget(&db, CLIENT, &mut budget),
        Err(ExecutionError::Unsupported)
    ));
    drop(db);

    let pages = fixture.compare_query_pages_with_env(
        &[(QUERY, 0, false), (ZERO, 0, true)],
        &[("MORI_TEST_ORDERED_COUNT_GROUP_LIMIT", "2")],
    );
    assert_eq!(pages[0]["rows"], json!([["winner", "3"]]));
    assert_eq!(pages[1]["rows"], json!([]));

    // A later SQLite error must still be detected for LIMIT 0.
    let db = fixture.db(CLIENT);
    fail_node_read(&db, tail);
    let query = cypher_linear::classify(ZERO).unwrap();
    assert!(matches!(
        query.execute_on(&db, CLIENT),
        Err(ExecutionError::Storage(rusqlite::Error::SqliteFailure(_, Some(message))))
            if message == "integer overflow"
    ));
}

#[test]
fn untyped_one_hop_and_introspection_match_native_in_rust() {
    let fixture = Fixture::new();
    let db = fixture.index(CLIENT);
    let root = node(&db, CLIENT, "AdjRoot", "root", "adj.root", "adj.py");
    let alpha = node(&db, CLIENT, "AdjLeaf", "alpha", "adj.alpha", "adj.py");
    let beta = node(&db, CLIENT, "AdjLeaf", "beta", "adj.beta", "adj.py");
    let peer = node(&db, CLIENT, "AdjPeer", "peer", "adj.peer", "adj.py");
    for (from, to, kind) in [
        (root, alpha, "CALLS"),
        (root, alpha, "IMPORTS"),
        (root, beta, "IMPORTS"),
        (peer, root, "REFERENCES"),
    ] {
        edge(&db, CLIENT, from, to, kind, json!({}));
    }
    node(
        &db,
        CLIENT,
        &format!("{}€", "a".repeat(508)),
        "raw",
        "adj.raw",
        "adj.py",
    );
    drop(db);

    let groups = "MATCH (:AdjRoot)-[r]->(:AdjLeaf) RETURN type(r) AS t, count(*) AS n ORDER BY t";
    let reversed = "MATCH (a)-[r]->(b:AdjLeaf) WHERE b.qualified_name = 'adj.alpha' RETURN a.name, b.name, TYPE(r) AS t ORDER BY t";
    let inbound = "MATCH (b:AdjRoot)<-[r]-(a) RETURN a.name, type(r)";
    let unnamed = "MATCH (a:AdjRoot)-[]->(b) RETURN COUNT(b)";
    let labels = "MATCH (n:AdjLeaf) RETURN LABELS(n), n.name ORDER BY n.name";
    let label_groups = "MATCH (n:AdjLeaf) RETURN labels(n) AS l, count(*) AS n";
    let raw = "MATCH (n) WHERE n.name = 'raw' RETURN labels(n)";
    let cases = [
        ((groups, 0, true), json!([["CALLS", "1"]])),
        ((groups, 1, true), json!([["IMPORTS", "2"]])),
        ((reversed, 0, true), json!([["root", "alpha", "CALLS"]])),
        ((reversed, 1, true), json!([["root", "alpha", "IMPORTS"]])),
        ((inbound, 0, true), json!([["peer", "REFERENCES"]])),
        ((unnamed, 0, true), json!([["3"]])),
        ((labels, 0, true), json!([["[\"AdjLeaf\"]", "alpha"]])),
        ((label_groups, 0, true), json!([["[\"AdjLeaf\"]", "2"]])),
        (
            (raw, 0, true),
            json!([[format!("@bytes:5b22{}e2", "61".repeat(508))]]),
        ),
    ];
    let inputs: Vec<_> = cases.iter().map(|(case, _)| *case).collect();
    let pages = fixture.compare_query_pages(&inputs);
    for (page, ((query, _, _), rows)) in pages.iter().zip(cases) {
        assert_eq!(page["rows"], rows, "{query}");
    }
    assert_eq!(pages[4]["columns"], json!(["a.name", "type(r)"]));
    assert_eq!(pages[6]["columns"], json!(["labels(n)", "n.name"]));
    // The native parser accepts an alias above, and rejects this expression.
    assert!(
        cypher_linear::classify(
            "MATCH (a)-[r]->(b) RETURN type(r), count(*) ORDER BY type(r)"
        )
        .is_err()
    );
}

#[test]
fn distinct_edge_types_labels_and_tuples_match_native_in_rust() {
    let fixture = Fixture::new();
    let db = fixture.index(CLIENT);
    let root = node(
        &db,
        CLIENT,
        "DistinctRoot",
        "root",
        "distinct.root",
        "distinct.py",
    );
    for index in 0..24 {
        let name = format!("leaf{index}");
        let leaf =
            node(&db, CLIENT, "DistinctLeaf", &name, &name, "distinct.py");
        edge(&db, CLIENT, root, leaf, "CALLS", json!({}));
        if index == 23 {
            edge(&db, CLIENT, root, leaf, "HAS_BRANCH", json!({}));
            edge(&db, CLIENT, root, leaf, "REFERENCES", json!({}));
        }
    }
    for (index, (name, value)) in [("a", "b:c"), ("a:b", "c"), ("a", "b:c")]
        .into_iter()
        .enumerate()
    {
        let id = node(
            &db,
            CLIENT,
            "DistinctTuple",
            name,
            &format!("tuple{index}"),
            "tuple.py",
        );
        db.execute(
            "UPDATE nodes SET properties = ? WHERE id = ?",
            params![json!({"value": value}).to_string(), id],
        )
        .unwrap();
    }
    for (index, raw) in
        [b"raw\xff\0one", b"raw\xff\0two"].into_iter().enumerate()
    {
        let id = node(
            &db,
            CLIENT,
            "DistinctBytes",
            "raw",
            &format!("raw{index}"),
            "raw.py",
        );
        db.execute(
            "UPDATE nodes SET name = ? WHERE id = ?",
            params![raw.as_slice(), id],
        )
        .unwrap();
    }
    drop(db);

    let edge_types = "MATCH (:DistinctRoot)-[r]->(:DistinctLeaf) RETURN DISTINCT type(r) AS t LIMIT 2";
    let ordered = "MATCH (:DistinctRoot)-[r]->(:DistinctLeaf) RETURN DISTINCT type(r) AS t ORDER BY t DESC LIMIT 2";
    let ordered_zero = "MATCH (:DistinctRoot)-[r]->(:DistinctLeaf) RETURN DISTINCT type(r) AS t ORDER BY t LIMIT 0";
    let labels = "MATCH (n) WHERE n.file_path = 'distinct.py' RETURN DISTINCT labels(n) AS l ORDER BY l";
    let tuples = "MATCH (n:DistinctTuple) RETURN DISTINCT n.name AS symbol, n.value AS payload";
    let ties = "MATCH (n:DistinctTuple) RETURN DISTINCT n.file_path AS p, n.name, n.value ORDER BY p";
    let raw = "MATCH (n:DistinctBytes) RETURN DISTINCT n.name";
    let skip = "MATCH (:DistinctRoot)-[r]->(:DistinctLeaf) RETURN DISTINCT type(r) AS t ORDER BY t SKIP 1 LIMIT 1";
    let aggregate = "MATCH (n:DistinctLeaf) RETURN DISTINCT n.label, count(*)";
    let cases = [
        ((edge_types, 0, true), json!([["CALLS"]])),
        ((edge_types, 1, true), json!([["HAS_BRANCH"]])),
        ((ordered, 0, true), json!([["REFERENCES"]])),
        ((ordered, 1, true), json!([["HAS_BRANCH"]])),
        ((ordered_zero, 0, true), json!([])),
        ((labels, 0, true), json!([["[\"DistinctLeaf\"]"]])),
        ((labels, 1, true), json!([["[\"DistinctRoot\"]"]])),
        ((tuples, 0, true), json!([["a", "b:c"]])),
        ((tuples, 1, true), json!([["a:b", "c"]])),
        ((raw, 0, true), json!([["@bytes:726177ff"]])),
        ((skip, 0, false), json!([["HAS_BRANCH"]])),
        ((aggregate, 0, false), json!([["DistinctLeaf", "24"]])),
        ((ties, 0, true), json!([["tuple.py", "a", "b:c"]])),
        ((ties, 1, true), json!([["tuple.py", "a:b", "c"]])),
    ];
    let inputs: Vec<_> = cases.iter().map(|(case, _)| *case).collect();
    let pages = fixture.compare_query_pages(&inputs);
    for (page, ((query, _, _), rows)) in pages.iter().zip(cases) {
        assert_eq!(page["rows"], rows, "{query}");
    }
    assert_eq!(pages[1]["total_relation"], "eq");
    assert_eq!(pages[9]["total"], 1);
}

#[test]
fn ordered_aliases_keep_lexical_keys_and_raw_split_bytes() {
    const QUERY: &str = "MATCH (n:SortProbe) RETURN n.name AS symbol, n.value AS key ORDER BY key ASC";
    let fixture = Fixture::new();
    let db = fixture.index(CLIENT);
    let prefix = "a".repeat(510);
    for (name, key) in [
        ("euro", format!("{prefix}€")),
        ("accent", format!("{prefix}é")),
        ("two", "2".to_owned()),
        ("ten", "10".to_owned()),
    ] {
        let id = node(&db, CLIENT, "SortProbe", name, name, "sort.py");
        db.execute(
            "UPDATE nodes SET properties = ? WHERE id = ?",
            params![json!({"value": key}).to_string(), id],
        )
        .unwrap();
    }
    drop(db);
    let empty = format!("{QUERY} LIMIT 0");
    let pages = fixture.compare_query_pages(&[
        (QUERY, 0, true),
        (QUERY, 1, true),
        (QUERY, 2, true),
        (QUERY, 3, true),
        (&empty, 0, true),
    ]);
    assert_eq!(pages[0]["rows"], json!([["ten", "10"]]));
    assert_eq!(pages[1]["rows"], json!([["two", "2"]]));
    assert_eq!(
        pages[2]["rows"],
        json!([["accent", format!("@bytes:{}c3", "61".repeat(510))]])
    );
    assert_eq!(
        pages[3]["rows"],
        json!([["euro", format!("@bytes:{}e2", "61".repeat(510))]])
    );
    assert_eq!(pages[4]["rows"], json!([]));
}

#[test]
fn ordered_cross_edge_queries_preserve_malformed_sqlite_text() {
    const PROJECTION: &str = "MATCH (n:RawSource)-[r:CALLS]->(m) WHERE n.anchor = 'visible' AND n.hidden = '' RETURN n, n.name, n.raw, n.hidden, m.raw, r.note, r ORDER BY r.note LIMIT 2";
    let fixture = Fixture::new();
    let db = fixture.index(CLIENT);
    let source = node(&db, CLIENT, "RawSource", "source", "source", "raw.py");
    let target = node(&db, CLIENT, "Function", "target", "target", "raw.py");
    edge(&db, CLIENT, source, target, "CALLS", json!({}));
    db.execute(
        "UPDATE nodes SET name = CAST(? AS TEXT), properties = CAST(? AS TEXT) WHERE id = ?",
        params![
            b"n\xff\0suffix".as_slice(),
            b"{\"anchor\":\"visible\",\"raw\":\"a\xff\"}\0{\"hidden\":\"tail\"}".as_slice(),
            source,
        ],
    )
    .unwrap();
    db.execute(
        "UPDATE nodes SET properties = CAST(? AS TEXT) WHERE id = ?",
        params![b"{\"raw\":\"b\x80\"}".as_slice(), target],
    )
    .unwrap();
    db.execute(
        "UPDATE edges SET properties = CAST(? AS TEXT) WHERE source_id = ? AND target_id = ?",
        params![b"{\"note\":\"e\xfe\"}\0ignored".as_slice(), source, target],
    )
    .unwrap();
    drop(db);

    let no_channel = "MATCH (n:RawSource)-[r:CROSS_CHANNEL]->(m) RETURN n.raw, r.note ORDER BY r.note LIMIT 2";
    let no_http = "MATCH (n:RawSource)-[r:CROSS_HTTP_CALLS]->(m) RETURN n.raw, r.note ORDER BY r.note LIMIT 2";
    let pages = fixture.compare_query_pages(&[
        (no_channel, 0, true),
        (no_http, 0, true),
        (PROJECTION, 0, true),
    ]);
    assert_eq!(pages[0]["rows"], json!([]));
    assert_eq!(pages[1]["rows"], json!([]));
    assert_eq!(
        pages[2]["rows"],
        json!([[
            "@bytes:6eff",
            "@bytes:6eff",
            "@bytes:61ff",
            "",
            "@bytes:6280",
            "@bytes:65fe",
            "@bytes:7b226e6f7465223a2265fe227d"
        ]])
    );
}

#[test]
fn cypher_projects_blobs_in_text_affinity_columns_like_c() {
    const QUERY: &str = "MATCH (n:BlobSource)-[r:CALLS]->(m) WHERE n.anchor = 'visible' RETURN n.name, n.raw, n.hidden, r.note, r ORDER BY r.note";
    let fixture = Fixture::new();
    let db = fixture.index(CLIENT);
    let source = node(&db, CLIENT, "BlobSource", "source", "source", "blob.py");
    let target = node(&db, CLIENT, "Function", "target", "target", "blob.py");
    edge(&db, CLIENT, source, target, "CALLS", json!({}));
    db.execute(
        "UPDATE nodes SET name = ?, properties = ? WHERE id = ?",
        params![
            b"n\xff\0suffix".as_slice(),
            b"{\"anchor\":\"visible\",\"raw\":\"a\xff\"}\0{\"hidden\":\"tail\"}"
                .as_slice(),
            source,
        ],
    )
    .unwrap();
    db.execute(
        "UPDATE edges SET properties = ? WHERE source_id = ? AND target_id = ?",
        params![br#"{"note":"same"}"#.as_slice(), source, target],
    )
    .unwrap();
    let storage_classes: (String, String, String) = db
        .query_row(
            "SELECT typeof(n.name), typeof(n.properties), typeof(e.properties)
         FROM nodes n JOIN edges e ON e.source_id = n.id WHERE n.id = ?",
            [source],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        storage_classes,
        ("blob".into(), "blob".into(), "blob".into())
    );
    drop(db);

    let pages = fixture.compare_query_pages(&[(QUERY, 0, true)]);
    assert_eq!(
        pages[0]["rows"],
        json!([[
            "@bytes:6eff",
            "@bytes:61ff",
            "",
            "same",
            "{\"note\":\"same\"}"
        ]])
    );
}

#[test]
fn unordered_aliases_fall_back_for_rotating_edge_property_buffers() {
    const WIDE: &str = "MATCH (a:AliasSource)-[r:CALLS]->(b) RETURN r.one AS first, r.two, r.three, r.four, r.five, r.six, r.seven, r.eight, r.nine AS last";
    const NARROW: &str = "MATCH (a:AliasSource)-[r:CALLS]->(b) RETURN r.one AS first, r.nine AS last";
    let fixture = Fixture::new();
    let db = fixture.index(CLIENT);
    let caller =
        node(&db, CLIENT, "AliasSource", "caller", "caller", "calls.py");
    let target = node(&db, CLIENT, "Function", "target", "target", "calls.py");
    edge(
        &db,
        CLIENT,
        caller,
        target,
        "CALLS",
        json!({"nine": "ninth"}),
    );
    drop(db);

    let pages =
        fixture.compare_query_pages(&[(WIDE, 0, false), (NARROW, 0, true)]);
    // In C the ninth lookup reuses the first empty property's buffer.
    assert_eq!(
        pages[0]["rows"],
        json!([["ninth", "", "", "", "", "", "", "", "ninth"]])
    );
    assert_eq!(pages[1]["columns"], json!(["first", "last"]));
    assert_eq!(pages[1]["rows"], json!([["", "ninth"]]));
}

#[test]
fn zero_hop_scan_checks_budget_and_matches_native_row_ceiling() {
    use cypher_linear::{ExecutionBudget, ExecutionError};

    const MATCH: &str = "MATCH (n:Ceiling) RETURN n.name";
    let fixture = Fixture::new();
    let db = fixture.index(CLIENT);
    db.execute_batch(
        "WITH RECURSIVE seq(n) AS (VALUES(1) UNION ALL SELECT n + 1 FROM seq WHERE n < 100000)
         INSERT INTO nodes(project, label, name, qualified_name)
         SELECT 'client', 'Ceiling', CASE n WHEN 100000 THEN 'best' ELSE 'node' END,
                printf('node%d', n) FROM seq;",
    )
    .expect("seed the scan");
    let candidate = |query: &str| {
        let bin = Path::new(MORI);
        fixture.start_query_daemon(bin, "rust");
        let before = fixture.rust_query_count();
        let output = fixture.query_output(bin, CLIENT, query);
        assert_eq!(fixture.rust_query_count() - before, 1, "{query}");
        output
    };
    let reference_bin = Fixture::reference_binary();
    let below = format!("{MATCH} LIMIT 99999");
    let c = fixture.query_output(reference_bin, CLIENT, &below);
    let rust = candidate(&below);
    assert!(
        c.status.success() && rust.status.success(),
        "{c:?}\n{rust:?}"
    );
    assert_eq!(c.stdout, rust.stdout);
    let result: Value = serde_json::from_slice(&rust.stdout).unwrap();
    assert_eq!(result["total"], 99_999);

    for query in [MATCH, &format!("{MATCH} LIMIT 100000")] {
        let c = fixture.query_output(reference_bin, CLIENT, query);
        let rust = candidate(query);
        assert_eq!(c.status.code(), Some(1), "{c:?}");
        assert_eq!(rust.status.code(), c.status.code(), "{rust:?}");
        assert_eq!(rust.stdout, c.stdout);
        for output in [c, rust] {
            assert!(
                String::from_utf8_lossy(&output.stderr)
                    .contains("result exceeded 100k rows"),
                "{output:?}"
            );
        }
    }

    let ordered = format!("{MATCH} ORDER BY n.name LIMIT 1");
    let at_limit = fixture.compare_query_pages(&[(&ordered, 0, true)]);
    assert_eq!(at_limit[0]["rows"], json!([["best"]]));
    assert_eq!(at_limit[0]["total_relation"], "eq");

    let query = cypher_linear::classify(&ordered).unwrap();
    let checks = Cell::new(0);
    // Initial check, one per 1024 scanned rows, then the pre-sort check.
    // Cancel at the next poll while preparing the sort keys.
    let cancelled = || {
        checks.set(checks.get() + 1);
        checks.get() == 3 + 100_000 / 1024
    };
    let mut budget = ExecutionBudget::new(&Instant::now, &cancelled);
    assert!(matches!(
        query.execute_with_budget(&db, CLIENT, &mut budget),
        Err(ExecutionError::Cancelled)
    ));

    node(&db, CLIENT, "Ceiling", "aaa", "node100001", "ceiling.py");
    let two = format!("{MATCH} ORDER BY n.name LIMIT 2");
    let zero = format!("{MATCH} ORDER BY n.name LIMIT 0");
    let over_limit = fixture.compare_query_pages(&[
        (&two, 0, true),
        (&two, 1, true),
        (&zero, 0, true),
    ]);
    assert_eq!(over_limit[0]["rows"], json!([["best"]]));
    assert_eq!(over_limit[1]["rows"], json!([["node"]]));
    assert_eq!(over_limit[0]["total"], 2);
    assert_eq!(over_limit[1]["total"], 2);
    assert_eq!(over_limit[0]["truncation_reason"], "page_limit");
    assert_eq!(over_limit[1]["truncation_reason"], "engine_limit");
    assert_eq!(over_limit[2]["rows"], json!([]));
    assert_eq!(over_limit[2]["total"], 0);
    assert_eq!(over_limit[2]["truncation_reason"], "engine_limit");
    for page in over_limit {
        assert_eq!(page["total_relation"], "gte");
        assert_eq!(page["has_more"], true);
        assert_eq!(page["truncated"], true);
    }

    let distinct =
        "MATCH (n:Ceiling) RETURN DISTINCT n.name ORDER BY n.name LIMIT 3";
    let distinct_cap = "MATCH (n:Ceiling) RETURN DISTINCT n.qualified_name ORDER BY n.qualified_name LIMIT 1";
    let pages = fixture.compare_query_pages(&[
        (distinct, 0, true),
        (distinct, 2, true),
        (distinct_cap, 0, true),
    ]);
    assert_eq!(pages[0]["rows"], json!([["aaa"]]));
    assert_eq!(pages[1]["rows"], json!([["node"]]));
    assert_eq!(pages[1]["total"], 3);
    assert_eq!(pages[1]["total_relation"], "eq");
    assert_eq!(pages[2]["total_relation"], "gte");
    assert_eq!(pages[2]["truncation_reason"], "engine_limit");

    // All predicates miss. A budget check only at the end of the scan would
    // reach this unreadable row and report a storage error first.
    let tail = db
        .query_row(
            "SELECT id FROM nodes WHERE qualified_name = 'node2048'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    fail_node_read(&db, tail);
    let distinct = cypher_linear::classify(
        "MATCH (n:Ceiling) RETURN DISTINCT n.name LIMIT 1",
    )
    .unwrap();
    assert!(matches!(
        distinct.execute_on(&db, CLIENT),
        Err(ExecutionError::Storage(rusqlite::Error::SqliteFailure(_, Some(message))))
            if message == "integer overflow"
    ));
    let query = cypher_linear::classify(
        "MATCH (n:Ceiling) WHERE n.name = 'absent' RETURN n.name LIMIT 1",
    )
    .unwrap();
    assert!(matches!(
        query.execute_on(&db, CLIENT),
        Err(ExecutionError::Storage(rusqlite::Error::SqliteFailure(_, Some(message))))
            if message == "integer overflow"
    ));
    let checks = Cell::new(0);
    let cancelled = || {
        checks.set(checks.get() + 1);
        checks.get() == 2
    };
    let mut budget = ExecutionBudget::new(&Instant::now, &cancelled);
    assert!(matches!(
        query.execute_with_budget(&db, CLIENT, &mut budget),
        Err(ExecutionError::Cancelled)
    ));
    assert_eq!(checks.get(), 2);

    let checks = Cell::new(0);
    let start = Instant::now();
    let clock = || {
        checks.set(checks.get() + 1);
        start + Duration::from_secs(if checks.get() >= 3 { 30 } else { 0 })
    };
    let mut budget = ExecutionBudget::new(&clock, &|| false);
    assert!(matches!(
        query.execute_with_budget(&db, CLIENT, &mut budget),
        Err(ExecutionError::Deadline)
    ));
    assert_eq!(checks.get(), 3);
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

    // Reading the next start node would fail in Rust. At the ceiling the
    // result is already known, so the evaluator must stop before that read.
    let tail = node(&db, CLIENT, "Caller", "tail", "tail", "caller.py");
    fail_node_read(&db, tail);
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
        "MATCH (a) RETURN a.in_degree",
        "MATCH (a)-[:CALLS]->(b) RETURN b.name ORDER BY b.file_path",
        "MATCH (a)-[:CALLS]->(b)-[:CALLS]->(c) RETURN c.name ORDER BY c.name",
        "MATCH (a)-[r:CALLS]->(b) RETURN r.one, r.two, r.three, r.four, r.five, r.six, r.seven, r.eight, r.nine ORDER BY r.one",
        "MATCH (a) RETURN a.name AS x ORDER BY a.name",
        "MATCH (a) RETURN count(DISTINCT a) AS n ORDER BY n",
        "MATCH (a)-[:CALLS*1..2]->(b) RETURN b.name",
        "MATCH (a)-[:CALLS]-(b) RETURN b.name",
        "MATCH (a)-[:CALLS]- >(b) RETURN b.name",
        "MATCH (count)-[:CALLS]->(b) RETURN b.name",
        "MATCH (a)-[:CALLS]->(b) WHERE a.name = 'x' OR b.name = 'y' RETURN b.name",
        "MATCH (a)-[:CALLS]->(b) RETURN count(DISTINCT b)",
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

#[test]
fn graphql_field_extraction_matches_c_with_escaped_schema_text() {
    fn index(backend: &str) -> (Vec<(String, String)>, String) {
        let fixture = Fixture::new();
        let repo = fixture.0.join("schema");
        fs::create_dir(&repo).expect("create schema repository");
        fs::write(
            repo.join("schema.graphqls"),
            r#"type Query {
  old: [String!]! @deprecated(reason: "Use \"new\"")
  later: Int
}
extend type Query {
  cafe: String @deprecated(reason: "caf\u00e9")
}
"#,
        )
        .expect("write SDL");
        let args = json!({"repo_path": repo, "name": "schema"});
        let output = fixture
            .command(Path::new(MORI))
            .env("CBM_PROFILE", "1")
            .env("CBM_LOG_LEVEL", "debug")
            .env("MORI_IMPL_GRAPHQL_EXTRACT", backend)
            .args(["cli", "--quiet", "index_repository", &args.to_string()])
            .output()
            .expect("index schema");
        assert!(output.status.success(), "{output:?}");
        let response: Value =
            serde_json::from_slice(&output.stdout).expect("index response");
        assert_eq!(response["status"], "indexed", "{response}");
        let db = fixture.db("schema");
        let mut statement = db
            .prepare("SELECT name, properties FROM nodes WHERE label = 'Field' ORDER BY name")
            .expect("select fields");
        let fields = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("read fields")
            .collect::<Result<_, _>>()
            .expect("field rows");
        let mut logs = String::new();
        for entry in
            fs::read_dir(fixture.0.join("cache/logs")).expect("worker logs")
        {
            let entry = entry.expect("worker log");
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(".worker-log-")
            {
                logs.push_str(
                    &fs::read_to_string(entry.path()).expect("read log"),
                );
            }
        }
        (fields, logs)
    }

    let (native, native_logs) = index("c");
    let (rust, rust_logs) = index("rust");
    assert_eq!(rust, native);
    assert_eq!(
        rust.iter().map(|row| row.0.as_str()).collect::<Vec<_>>(),
        ["cafe", "later", "old"]
    );
    assert!(rust[0].1.contains("café"), "{rust:?}");
    assert!(rust[2].1.contains("[String!]!"), "{rust:?}");
    for marker in ["extract.graphql_parse_input", "extract.graphql_field_spans"]
    {
        assert!(!native_logs.contains(marker), "{native_logs}");
        assert!(rust_logs.contains(marker), "{rust_logs}");
    }
}
