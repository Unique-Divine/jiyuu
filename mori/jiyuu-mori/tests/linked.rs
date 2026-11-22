//! Runs `mori` and the C `codebase-memory-mcp` from the same build directory
//! and checks that they behave the same. Needs feature `c-backend` and the
//! C reference binary (`MORI_C_REFERENCE=1`, set by `just check-linked`), so
//! a pure-Rust `cargo test` compiles this file to nothing.
#![cfg(feature = "c-backend")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use rusqlite::{Connection, OpenFlags};

const MORI: &str = env!("CARGO_BIN_EXE_mori");

fn c_binary() -> PathBuf {
    // build.rs sets MORI_C_BUILD_DIR whenever c-backend is on.
    let bin = Path::new(env!("MORI_C_BUILD_DIR")).join("codebase-memory-mcp");
    assert!(
        bin.is_file(),
        "{} is missing; build with MORI_C_REFERENCE=1 (just check-linked)",
        bin.display()
    );
    bin
}

/// A short, mode-700 scratch directory: long runtime paths overflow the
/// Unix socket path limit.
struct Scratch(PathBuf);

impl Scratch {
    fn new(side: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = PathBuf::from(format!(
            "/tmp/mori-t{}-{n}-{side}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        for sub in ["cache", "rt"] {
            let path = dir.join(sub);
            fs::create_dir_all(&path).expect("create scratch dir");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                .expect("chmod scratch dir");
        }
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Runs `bin` with a minimal environment pointed at its own scratch caches.
fn run(bin: &Path, side: &str, args: &[&str]) -> Output {
    run_in(bin, &Scratch::new(side), args)
}

fn run_in(bin: &Path, scratch: &Scratch, args: &[&str]) -> Output {
    let mut command = command_in(bin, scratch);
    if args.contains(&"index_repository") {
        command.env("CBM_PROFILE", "1");
    }
    command.args(args).output().expect("run binary")
}

fn command_in(bin: &Path, scratch: &Scratch) -> Command {
    let mut command = Command::new(bin);
    command
        .current_dir("/")
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", std::env::var_os("HOME").unwrap_or_default())
        .env("CBM_CACHE_DIR", scratch.0.join("cache"))
        .env("CBM_RUNTIME_DIR", scratch.0.join("rt"));
    command
}

fn assert_same(args: &[&str]) {
    let c = run(&c_binary(), "c", args);
    let rs = run(Path::new(MORI), "rs", args);
    let show = |o: &Output| {
        format!(
            "status {:?}\nstdout {}\nstderr {}",
            o.status.code(),
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        )
    };
    assert_eq!(
        (c.status.code(), &c.stdout, &c.stderr),
        (rs.status.code(), &rs.stdout, &rs.stderr),
        "{args:?} differs\n--- C\n{}\n--- mori\n{}",
        show(&c),
        show(&rs)
    );
}

/// `mori --version` names `mori`; the version and everything else match C.
#[test]
fn version_matches_c() {
    let c = run(&c_binary(), "c", &["--version"]);
    let rs = run(Path::new(MORI), "rs", &["--version"]);
    assert!(c.status.success() && rs.status.success());
    let c_out = String::from_utf8_lossy(&c.stdout);
    let rs_out = String::from_utf8_lossy(&rs.stdout);
    let version = c_out
        .strip_prefix("codebase-memory-mcp ")
        .unwrap_or_else(|| panic!("C --version printed {c_out:?}"));
    assert_eq!(rs_out, format!("mori {version}"));
    assert_eq!(c.stderr, rs.stderr);
}

#[test]
fn list_projects_matches_c() {
    assert_same(&["cli", "list_projects"]);
}

#[test]
fn cli_output_controls_match_c() {
    assert_same(&["cli", "--quiet", "list_projects"]);
    assert_same(&["cli", "--verbose", "list_projects", "--quiet"]);
    assert_same(&["cli", "list_projects", "--progress", "--quiet"]);
}

#[test]
fn project_pages_use_internal_identity_and_preserve_projection() {
    let scratch = Scratch::new("projects");
    let cache = scratch.0.join("cache");
    let create_project_db = |file: &str, name: &str, extra: &str, root: &str| {
        let db = rusqlite::Connection::open(cache.join(file)).unwrap();
        db.execute_batch(
            "CREATE TABLE projects (name TEXT PRIMARY KEY, indexed_at TEXT, root_path TEXT);
             CREATE TABLE nodes (project TEXT);
             CREATE TABLE edges (project TEXT);",
        ).unwrap();
        for project in [name, extra] {
            db.execute(
                "INSERT INTO projects VALUES (?1, 'now', ?2)",
                [project, root],
            )
            .unwrap();
            db.execute("INSERT INTO nodes VALUES (?1), (?1)", [project])
                .unwrap();
            db.execute("INSERT INTO edges VALUES (?1)", [project])
                .unwrap();
        }
    };
    create_project_db("renamed.db", "alpha", "alpha::missed", "/tmp/graph-root");
    create_project_db("second.db", "beta", "beta::missed", "/tmp/graph-root");
    create_project_db("a-copy.db", "beta", "beta::missed", "/tmp/graph-root");
    create_project_db("0-copy.db", "beta", "beta::missed", "/tmp/z-graph-root");
    rusqlite::Connection::open(cache.join("a-copy.db"))
        .unwrap()
        .execute("INSERT INTO nodes VALUES ('beta')", [])
        .unwrap();
    create_project_db("ambiguous.db", "skip-one", "skip-two", "/tmp/graph-root");
    fs::write(cache.join("empty.db"), "").unwrap();
    fs::write(cache.join("corrupt.db"), "not a SQLite database").unwrap();

    let cases = [
        ("list_projects", r#"{"format":"json"}"#),
        (
            "list_projects",
            r#"{"metadata_only":true,"detail":"stats","include_details":true,"limit":0,"offset":-1}"#,
        ),
        (
            "list_projects",
            r#"{"detail":"stats","limit":1,"offset":1,"format":"json"}"#,
        ),
        (
            "list_projects",
            r#"{"include_details":true,"offset":99,"format":"json"}"#,
        ),
        (
            "index_status",
            r#"{"project":"alpha","verbose":true,"diagnostics":"summary","format":"json"}"#,
        ),
        (
            "list_projects",
            r#"{"include_details":true,"offset":2,"limit":501,"format":"json"}"#,
        ),
    ];
    let run_all = |bin: &Path| {
        let outputs = cases.map(|(tool, args)| {
            run_in(bin, &scratch, &["cli", "--quiet", tool, args])
        });
        let stop = run_in(bin, &scratch, &["daemon", "stop"]);
        assert!(stop.status.success(), "{stop:?}");
        outputs
    };
    let c = run_all(&c_binary());
    let rust = run_all(Path::new(MORI));
    for (reference, candidate) in c.iter().zip(&rust) {
        assert!(reference.status.success(), "{reference:?}");
        assert_eq!(
            (
                reference.status.code(),
                &reference.stdout,
                &reference.stderr
            ),
            (
                candidate.status.code(),
                &candidate.stdout,
                &candidate.stderr
            )
        );
    }
    let identity: serde_json::Value =
        serde_json::from_slice(&rust[0].stdout).unwrap();
    assert_eq!(identity["total"], 4);
    assert_eq!(identity["projects"][0]["name"], "alpha");
    let stats: serde_json::Value =
        serde_json::from_slice(&rust[2].stdout).unwrap();
    assert_eq!(stats["projects"][0]["name"], "beta");
    assert_eq!(stats["projects"][0]["nodes"], 3);
    assert_eq!(stats["projects"][0]["edges"], 1);
    assert_eq!(stats["next_offset"], 2);
    let tail: serde_json::Value =
        serde_json::from_slice(&rust[5].stdout).unwrap();
    assert_eq!(tail["limit"], 500);
    assert_eq!(tail["returned"], 2);
    assert_eq!(tail["projects"][0]["nodes"], 2);
    assert_eq!(tail["projects"][1]["root_path"], "/tmp/z-graph-root");
    let status: serde_json::Value =
        serde_json::from_slice(&rust[4].stdout).unwrap();
    assert_eq!(status["status"], "ready");
    assert_eq!(status["root_path"], "/tmp/graph-root");
    assert_eq!(status["indexed_at"], "now");
    assert_eq!(status["nodes"], 2);
    assert_eq!(status["edges"], 1);

    // A readable project with an unreadable count keeps its metadata and
    // reports a table-specific error rather than an empty index.
    rusqlite::Connection::open(cache.join("renamed.db"))
        .unwrap()
        .execute_batch("DROP TABLE edges;")
        .unwrap();
    let args = [
        "cli",
        "--quiet",
        "index_status",
        r#"{"project":"alpha","format":"json"}"#,
    ];
    let c = run_in(&c_binary(), &scratch, &args);
    let stop = run_in(&c_binary(), &scratch, &["daemon", "stop"]);
    assert!(stop.status.success(), "{stop:?}");
    let rust = run_in(Path::new(MORI), &scratch, &args);
    assert_eq!(
        (c.status.code(), &c.stdout, &c.stderr),
        (rust.status.code(), &rust.stdout, &rust.stderr)
    );
    let failed: serde_json::Value =
        serde_json::from_slice(&rust.stdout).unwrap();
    assert_eq!(failed["status"], "error");
    assert_eq!(failed["indexed_at"], "now");
    assert!(
        failed["hint"]
            .as_str()
            .unwrap()
            .contains("The edges table could not be read")
    );
}

#[test]
fn project_page_normalizes_root_bytes_in_both_formats() {
    let scratch = Scratch::new("project-bytes");
    let db = Connection::open(scratch.0.join("cache/bytes.db")).unwrap();
    db.execute_batch(
        "CREATE TABLE projects (name TEXT PRIMARY KEY, indexed_at TEXT, root_path TEXT);
         INSERT INTO projects VALUES ('bytes', 'now', CAST(X'2FFF00' AS TEXT));",
    )
    .unwrap();

    let cases = [
        (
            r#"{"format":"json","limit":"bad","offset":-1}"#,
            "@bytes:2fff",
        ),
        (r#"{"metadata_only":true,"detail":"stats"}"#, "@bytes:2fff"),
        (r#"{"format":"json","offset":1}"#, r#""returned":0"#),
    ];
    let run_all = |bin: &Path| {
        let outputs = cases.map(|(query, marker)| {
            let output = run_in(
                bin,
                &scratch,
                &["cli", "--quiet", "list_projects", query],
            );
            assert!(output.status.success(), "{output:?}");
            assert!(String::from_utf8_lossy(&output.stdout).contains(marker));
            (output.stdout, output.stderr)
        });
        let stop = run_in(bin, &scratch, &["daemon", "stop"]);
        assert!(stop.status.success(), "{stop:?}");
        outputs
    };
    assert_eq!(run_all(&c_binary()), run_all(Path::new(MORI)));
}

#[test]
fn index_status_preserves_diagnostics_and_freshness_in_both_formats() {
    let scratch = Scratch::new("index-status");
    let repo = scratch.0.join("repo");
    fs::create_dir(&repo).unwrap();
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .current_dir(&repo)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    };
    git(&["init", "-q"]);
    git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.com",
        "-c",
        "commit.gpgsign=false",
        "-c",
        "core.hooksPath=/dev/null",
        "commit",
        "--allow-empty",
        "-qm",
        "fixture",
    ]);

    let db = Connection::open(scratch.0.join("cache/fixture.db")).unwrap();
    db.execute_batch(
        "CREATE TABLE projects (name TEXT PRIMARY KEY, indexed_at TEXT, root_path TEXT);
         CREATE TABLE nodes (project TEXT);
         CREATE TABLE edges (project TEXT);
         CREATE TABLE index_coverage (project TEXT, rel_path TEXT, kind TEXT, detail TEXT);
         CREATE TABLE index_coverage_meta (
             project TEXT, generation TEXT, index_mode TEXT, recorded_at TEXT,
             recording_status TEXT, ignored_files_stored INTEGER, ignored_files_total INTEGER,
             coverage_version INTEGER, hash_records_complete INTEGER,
             unresolved_calls_complete INTEGER
         );
         CREATE TABLE index_checkout (project TEXT, indexed_checkout_sha TEXT);
         INSERT INTO index_coverage_meta VALUES ('fixture', 'now', 'full', 'now', 'complete',
                                                  0, 0, 1, 1, 1);
         INSERT INTO index_checkout VALUES ('fixture', '0000000000000000000000000000000000000000');
         INSERT INTO index_coverage VALUES ('fixture', 'partial.rs', 'parse_partial', '1-2');",
    )
    .unwrap();
    db.execute(
        "INSERT INTO projects VALUES ('fixture', 'now', ?1)",
        [repo.to_str().unwrap()],
    )
    .unwrap();

    let queries = [
        r#"{"project":"fixture","verbose":true,"diagnostics":"full","format":"json"}"#,
        r#"{"project":"fixture","verbose":true,"diagnostics":"summary"}"#,
        r#"{"project":"missing","format":"json"}"#,
    ];
    let run_all = |bin: &Path| {
        let outputs = queries.map(|query| {
            let output = run_in(
                bin,
                &scratch,
                &["cli", "--quiet", "index_status", query],
            );
            (output.status.code(), output.stdout, output.stderr)
        });
        let stop = run_in(bin, &scratch, &["daemon", "stop"]);
        assert!(stop.status.success(), "{stop:?}");
        outputs
    };
    let c = run_all(&c_binary());
    let rust = run_all(Path::new(MORI));
    assert_eq!(c, rust);
    assert_eq!(rust[0].0, Some(0));
    let report: serde_json::Value = serde_json::from_slice(&rust[0].1).unwrap();
    assert_eq!(report["status"], "empty");
    assert_eq!(report["freshness"]["verdict"], "stale");
    assert_eq!(report["parse_partial"]["files"][0]["path"], "partial.rs");
    assert_eq!(report["watch"]["watched"], false);
    assert!(
        report["hint"]
            .as_str()
            .unwrap()
            .contains("Project is empty")
    );
}

#[test]
fn unknown_tool_error_matches_c() {
    assert_same(&["cli", "no_such_tool"]);
}

fn run_stdio(bin: &Path, side: &str, args: &[&str], input: &str) -> String {
    let scratch = Scratch::new(side);
    let requests = scratch.0.join("requests");
    fs::write(&requests, input).expect("write MCP requests");
    let output = command_in(bin, &scratch)
        .args(args)
        .stdin(Stdio::from(
            fs::File::open(requests).expect("open MCP requests"),
        ))
        .output()
        .expect("run MCP server");
    assert!(output.status.success(), "{side}: {output:?}");
    String::from_utf8(output.stdout).expect("UTF-8 response")
}

#[test]
fn mcp_method_dispatch_matches_c() {
    let input = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":\"ping\",\"method\":\"ping\"}\n",
        "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":\"resources\",\"method\":\"resources/list\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":\"templates\",\"method\":\"resources/templates/list\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":\"discover\",\"method\":\"server/discover\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":\"unknown\",\"method\":\"ping/extra\"}\n",
    );
    let reference = run_stdio(&c_binary(), "c", &[], input);
    let linked = run_stdio(Path::new(MORI), "rs", &[], input);
    assert_eq!(linked, reference);
    let replies: Vec<serde_json::Value> = linked
        .lines()
        .map(|line| serde_json::from_str(line).expect("parse response"))
        .collect();
    assert_eq!(replies.len(), 5, "{linked}");
    assert_eq!(replies[0]["result"], serde_json::json!({}));
    assert_eq!(replies[1]["result"]["resources"], serde_json::json!([]));
    assert_eq!(
        replies[2]["result"]["resourceTemplates"],
        serde_json::json!([])
    );
    for (reply, id) in replies[3..].iter().zip(["discover", "unknown"]) {
        assert_eq!(reply["id"], id);
        assert_eq!(reply["error"]["code"], -32601);
    }
}

#[test]
fn stdio_mixed_framing_matches_c_after_input_eof() {
    let initialize = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"linked-test","version":"1"}}}"#;
    let initialized =
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
    let call = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_projects","arguments":{}}}"#;
    let input = format!(
        "{initialize}\n{initialized}\nContent-Length: {}\r\n\r\n{call}",
        call.len()
    );
    let reference = run_stdio(&c_binary(), "c", &[], &input);
    let linked = run_stdio(Path::new(MORI), "rs", &[], &input);
    assert_eq!(linked, reference);
    let (initialize_response, framed) =
        linked.split_once('\n').expect("newline response");
    assert!(initialize_response.contains("\"id\":1"));
    assert!(initialize_response.contains("\"protocolVersion\":\"2025-06-18\""));
    let (header, body) = framed.split_once("\r\n\r\n").expect("framed response");
    assert_eq!(header, format!("Content-Length: {}", body.len()));
    assert!(body.contains("\"id\":2"));
    assert!(body.contains("projects: 0"));
}

#[test]
fn initialize_negotiation_and_instructions_match_c() {
    for (args, request, protocol, instruction) in [
        (
            &[][..],
            r#""2024-11-05""#,
            "2024-11-05",
            "Mori is a code knowledge graph",
        ),
        (
            &["--tool-profile=scout"][..],
            r#""9999-01-01""#,
            "2025-11-25",
            "scout tool profile",
        ),
        (
            &["--tool-profile=analysis"][..],
            "null",
            "2025-11-25",
            "analysis tool profile",
        ),
    ] {
        let input = format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":\"handshake\",\"method\":\"initialize\",\"params\":{{\"protocolVersion\":{request}}}}}\n"
        );
        let reference = run_stdio(&c_binary(), "c", args, &input);
        let linked = run_stdio(Path::new(MORI), "rs", args, &input);
        assert_eq!(linked, reference);
        let value: serde_json::Value = serde_json::from_str(&linked).unwrap();
        assert_eq!(value["id"], "handshake");
        assert_eq!(value["result"]["protocolVersion"], protocol);
        assert_eq!(value["result"]["serverInfo"]["name"], "mori-graph");
        assert!(
            value["result"]["instructions"]
                .as_str()
                .unwrap()
                .starts_with(instruction)
        );
    }
}

#[test]
fn tools_list_profiles_and_cursor_pages_match_c() {
    let input = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/list\",\"params\":{\"cursor\":\"0\"}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"tools/list\",\"params\":{\"cursor\":\"8\"}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":5,\"method\":\"tools/list\",\"params\":{\"cursor\":\"bogus\"}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":6,\"method\":\"tools/list\",\"params\":{\"cursor\":null}}\n",
    );
    for (args, count) in [
        (&[][..], 17),
        (&["--tool-profile=analysis"][..], 13),
        (&["--tool-profile=scout"][..], 8),
    ] {
        let reference = run_stdio(&c_binary(), "c", args, input);
        let linked = run_stdio(Path::new(MORI), "rs", args, input);
        assert_eq!(linked, reference);
        let replies: Vec<serde_json::Value> = linked
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(replies.len(), 6);
        let full = &replies[1]["result"];
        let tools = full["tools"].as_array().unwrap();
        assert_eq!(tools.len(), count);
        assert!(full.get("nextCursor").is_none());
        let tool = tools
            .iter()
            .find(|tool| tool["name"] == "search_graph")
            .unwrap();
        assert_eq!(tool["annotations"]["readOnlyHint"], true);
        assert_eq!(tool["_meta"]["anthropic/alwaysLoad"], true);
        assert_eq!(tool["inputSchema"]["type"], "object");

        let first = &replies[2]["result"];
        assert_eq!(first["tools"].as_array().unwrap(), &tools[..8]);
        assert_eq!(first["nextCursor"].as_str(), (count > 8).then_some("8"));
        let second = &replies[3]["result"];
        assert_eq!(
            second["tools"].as_array().unwrap(),
            &tools[8..count.min(16)]
        );
        assert_eq!(second["nextCursor"].as_str(), (count > 16).then_some("16"));
        for invalid in [&replies[4]["result"], &replies[5]["result"]] {
            assert!(invalid["tools"].as_array().unwrap().is_empty());
            assert!(invalid.get("nextCursor").is_none());
        }
    }
}

#[test]
fn invalid_query_flag_diagnostic_matches_c() {
    assert_same(&["cli", "query_graph", "--offst", "1"]);
}

/// A small repository with Python and Go calls and an optional gqlgen server.
struct Fixture(PathBuf);

impl Fixture {
    fn new(graphql: bool) -> Self {
        let kind = if graphql { "gql" } else { "cli" };
        let dir =
            PathBuf::from(format!("/tmp/mori-fx{}-{kind}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let common_files = [
            (
                "py/app.py",
                "def helper(x):\n    return x + 1\n\n\ndef run():\n    \
                 return helper(2)\n",
            ),
            (
                "go/calc.go",
                "package calc\n\nfunc Add(a, b int) int {\n\treturn a + b\n\
                 }\n\nfunc Twice(a int) int {\n\treturn Add(a, a)\n}\n",
            ),
            ("go/go.mod", "module example.com/calc\n\ngo 1.22\n"),
        ];
        let graphql_files = [
            (
                "gqlgen.yml",
                "schema:\n  - graphql/**/schema?.graphqls\nresolver:\n  layout: follow-schema\n  dir: server\n",
            ),
            (
                "graphql/nested/deep/schema1.graphqls",
                r#"type Query {
                    value: Int @deprecated(reason: "use \"newValue\"\non a new line")
                    oneOwner: Int
                    duplicate: Int
                    noOwner: Int
                }"#,
            ),
            (
                "server/handlers.go",
                "package server\n\ntype queryResolver struct{}\n\n\
                 func (r *queryResolver) Value() int { return 1 }\n\
                 func (r *queryResolver) Duplicate() int { return 1 }\n",
            ),
            (
                "other/gqlgen.yml",
                "schema:\n  - schema.graphqls\nresolver:\n  layout: follow-schema\n  dir: resolver\n",
            ),
            (
                "other/schema.graphqls",
                "type Query { oneOwner: Int duplicate: Int noOwner: Int }\n",
            ),
            (
                "other/resolver/handlers.go",
                "package resolver\n\ntype queryResolver struct{}\n\n\
                 func (r *queryResolver) OneOwner() int { return 2 }\n\
                 func (r *queryResolver) Duplicate() int { return 2 }\n",
            ),
        ];
        let server_files = if graphql {
            graphql_files.as_slice()
        } else {
            &[]
        };
        for (rel, text) in common_files.iter().chain(server_files) {
            let path = dir.join(rel);
            fs::create_dir_all(path.parent().expect("parent"))
                .expect("create fixture dir");
            fs::write(&path, text).expect("write fixture file");
        }
        Fixture(dir)
    }

    fn project(&self) -> String {
        // Project names are the path with `/` turned into `-`.
        self.0
            .to_string_lossy()
            .trim_start_matches('/')
            .replace('/', "-")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn index_fixture(
    bin: &Path,
    scratch: &Scratch,
    repo: &str,
) -> serde_json::Value {
    let index = run_in(
        bin,
        scratch,
        &["cli", "--quiet", "index_repository", "--repo_path", repo],
    );
    assert!(index.status.success(), "index failed: {index:?}");
    let result: serde_json::Value = serde_json::from_slice(&index.stdout)
        .unwrap_or_else(|error| panic!("index response: {error}: {index:?}"));
    assert_eq!(result["status"], "indexed", "{result}");
    assert!(
        result["nodes"].as_u64().is_some_and(|nodes| nodes > 0),
        "{result}"
    );
    result
}

#[test]
fn exact_path_coverage_verdicts_match_c_on_one_cache() {
    let scratch = Scratch::new("coverage");
    let repo = scratch.0.join("repo");
    fs::create_dir_all(repo.join("nested")).unwrap();
    for path in ["nested/partial.py", "verified.py", "changed.py"] {
        fs::write(repo.join(path), "def sample():\n    return 1\n").unwrap();
    }
    let c = c_binary();
    let index = index_fixture(&c, &scratch, repo.to_str().unwrap());
    let project = index["project"].as_str().expect("indexed project");
    let stop = |bin: &Path| {
        let output = run_in(bin, &scratch, &["daemon", "stop"]);
        assert!(output.status.success(), "{output:?}");
    };
    stop(&c);

    // Use one generation for both binaries, with both parse kinds in store
    // order and an ancestor row that applies only without an exact row.
    let db =
        Connection::open(scratch.0.join("cache").join(format!("{project}.db")))
            .unwrap();
    db.execute("DELETE FROM index_coverage WHERE project = ?", [project])
        .unwrap();
    for (path, kind, detail) in [
        ("nested/partial.py", "parse_partial", "1"),
        ("nested/partial.py", "parse_unusable", "1-2"),
        ("nested", "not_indexed_dir", "ignored"),
    ] {
        db.execute(
            "INSERT INTO index_coverage (project, rel_path, kind, detail) VALUES (?, ?, ?, ?)",
            [project, path, kind, detail],
        )
        .unwrap();
    }
    assert_eq!(
        db.execute(
            "UPDATE index_coverage_meta SET recording_status = 'truncated',
             hash_records_complete = 1 WHERE project = ?",
            [project],
        )
        .unwrap(),
        1
    );
    drop(db);
    fs::write(
        repo.join("changed.py"),
        "def changed_source():\n    return 23\n",
    )
    .unwrap();

    let requests = ["json", "tree"].map(|format| {
        serde_json::json!({
            "project": project,
            "paths": ["nested/partial.py", "nested/missing.py", "verified.py", "changed.py"],
            "scopes": ["nested"], "scope_limit": 1, "format": format
        })
        .to_string()
    });
    let read = |bin: &Path| {
        let outputs = requests.each_ref().map(|args| {
            run_in(
                bin,
                &scratch,
                &["cli", "--quiet", "check_index_coverage", args],
            )
        });
        stop(bin);
        outputs
    };
    let reference = read(&c);
    let linked = read(Path::new(MORI));
    for (reference, linked) in reference.iter().zip(&linked) {
        assert!(reference.status.success() && linked.status.success());
        assert_eq!(reference.stdout, linked.stdout);
        assert_eq!(reference.stderr, linked.stderr);
    }
    let json: serde_json::Value =
        serde_json::from_slice(&linked[0].stdout).unwrap();
    let paths = json["paths"].as_array().unwrap();
    assert_eq!(paths[0]["status"], "partial");
    assert_eq!(
        paths[0]["recommended_action"],
        "read_ranges_and_verify_scope"
    );
    assert_eq!(paths[1]["status"], "excluded");
    assert_eq!(paths[1]["coverage"][0]["match"], "ancestor");
    assert_eq!(paths[2]["status"], "no_recorded_issue");
    assert_eq!(paths[2]["content_verified"], true);
    assert_eq!(paths[3]["status"], "coverage_unavailable");
    assert_eq!(paths[3]["recommended_action"], "read_source_and_reindex");
}

fn graphql_rows(scratch: &Scratch, project: &str) -> Vec<String> {
    let db = Connection::open_with_flags(
        scratch.0.join("cache").join(format!("{project}.db")),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open fixture graph");
    let mut query = db
        .prepare(
            "SELECT json_array(r.qualified_name, r.name, r.file_path, r.start_line, r.end_line,
                               r.properties, h.qualified_name, h.label, e.properties)
             FROM nodes AS r
             LEFT JOIN edges AS e ON e.target_id = r.id AND e.type = 'HANDLES'
             LEFT JOIN nodes AS h ON h.id = e.source_id
             WHERE r.label = 'Route' AND substr(r.qualified_name, 1, 11) = '__graphql__'
             ORDER BY r.qualified_name, h.qualified_name",
        )
        .expect("prepare GraphQL row query");
    query
        .query_map([], |row| row.get(0))
        .expect("query GraphQL rows")
        .collect::<rusqlite::Result<_>>()
        .expect("read GraphQL rows")
}

#[test]
fn index_through_worker_reexec_matches_c() {
    let fixture = Fixture::new(false);
    let project = fixture.project();
    let repo = fixture.0.to_string_lossy().into_owned();
    let sides = [("c", c_binary()), ("rust", PathBuf::from(MORI))];
    let mut results = Vec::new();
    for (side, bin) in &sides {
        let scratch = Scratch::new(side);
        let index = index_fixture(bin, &scratch, &repo);
        let counts = (index["nodes"].as_u64(), index["edges"].as_u64());
        // Raw CLI JSON reaches the daemon as bytes. Building these requests
        // as serde values would normalize negative zero and duplicate keys.
        let schema_negative_zero =
            format!(r#"{{"project":"{project}","limit":-0,"format":"json"}}"#);
        let schema_duplicate = format!(
            r#"{{"project":"{project}","limit":1,"limit":3,"format":"json"}}"#
        );
        let schema_nul = format!(
            r#"{{"project":"{project}","diagnostics":"full\u0000suffix","format":"json"}}"#
        );
        let outline_page = serde_json::json!({
            "project": project, "file_path": "./py//app.py", "labels": ["Function"],
            "limit": 1, "offset": 1, "format": "json"
        }).to_string();
        let outline_end = serde_json::json!({
            "project": project, "file_path": "py/app.py", "labels": ["Function"],
            "limit": 1, "offset": 900, "format": "json"
        })
        .to_string();
        let search_page = serde_json::json!({
            "project": project, "name_pattern": "^h", "label": "Function",
            "limit": 1, "offset": 1, "format": "json"
        })
        .to_string();
        let search_path = serde_json::json!({
            "project": project, "qn_pattern": r"\.helper$", "file_pattern": "**/app.py",
            "max_output_tokens": 128
        }).to_string();
        let ranked_search = serde_json::json!({
            "project": project, "query": "helper", "format": "json"
        })
        .to_string();
        let ranked_page = serde_json::json!({
            "project": project, "query": "helper", "file_pattern": "**/app.py",
            "label": "Function", "limit": 1, "offset": 1, "max_output_tokens": 128
        })
        .to_string();
        let no_rank_tokens = serde_json::json!({
            "project": project, "query": "!!!", "name_pattern": "^h", "format": "json"
        })
        .to_string();
        let search_invalid = serde_json::json!({
            "project": project, "name_pattern": "["
        })
        .to_string();
        let queries: Vec<Output> = [
            vec![
                "cli",
                "search_graph",
                "--project",
                &project,
                "--name-pattern",
                "^(Add|Twice|helper|run)$",
                "--format",
                "json",
            ],
            vec![
                "cli",
                "trace_path",
                "--project",
                &project,
                "--function-name",
                "Add",
                "--direction",
                "inbound",
            ],
            vec![
                "cli",
                "trace_path",
                "--project",
                &project,
                "--function-name",
                "helper",
                "--direction",
                "inbound",
            ],
            vec!["cli", "get_graph_schema", "--project", &project],
            vec!["cli", "--quiet", "get_graph_schema", &schema_negative_zero],
            vec!["cli", "--quiet", "get_graph_schema", &schema_duplicate],
            vec!["cli", "--quiet", "get_graph_schema", &schema_nul],
            vec![
                "cli",
                "get_graph_schema",
                "--project",
                &project,
                "--format",
                "json",
                "--diagnostics",
                "full",
                "--limit",
                "4",
                "--offset",
                "1",
            ],
            vec![
                "cli",
                "get_graph_schema",
                "--project",
                &project,
                "--limit",
                "1",
                "--offset",
                "9999",
            ],
            vec![
                "cli",
                "get_file_outline",
                "--project",
                &project,
                "--file-path",
                "py/app.py",
            ],
            vec!["cli", "get_file_outline", &outline_page],
            vec!["cli", "get_file_outline", &outline_end],
            vec!["cli", "search_graph", &search_page],
            vec!["cli", "search_graph", &search_path],
            vec!["cli", "search_graph", &ranked_search],
            vec!["cli", "search_graph", &ranked_page],
            vec!["cli", "search_graph", &no_rank_tokens],
            vec![
                "cli",
                "query_graph",
                "--project",
                &project,
                "--query",
                "MATCH (f:Function)-[:CALLS]->(g:Function) WHERE f.name = 'Twice' RETURN g.name",
                "--max-rows=+7",
                "--max_rows",
                "4",
                "--offset",
                "0",
                "--",
                "ignored",
            ],
        ]
        .iter()
        .map(|args| {
            let output = run_in(bin, &scratch, args);
            if args.last().copied() == Some(ranked_search.as_str()) {
                let ranked: serde_json::Value = serde_json::from_slice(&output.stdout)
                    .unwrap_or_else(|error| panic!("ranked response: {error}: {output:?}"));
                assert_eq!(ranked["search_mode"], "bm25", "{ranked}");
                assert!(
                    ranked["returned"].as_u64().is_some_and(|count| count > 0),
                    "{ranked}"
                );
            }
            output
        })
        .collect();
        let invalid =
            run_in(bin, &scratch, &["cli", "search_graph", &search_invalid]);
        assert!(!invalid.status.success(), "{invalid:?}");
        results.push((counts, queries, invalid));
    }
    let [
        (c_counts, c_queries, c_invalid),
        (rust_counts, rust_queries, rust_invalid),
    ] = results.as_slice()
    else {
        panic!("expected C and Rust index results");
    };
    assert_eq!(c_counts, rust_counts, "node and edge counts differ");
    for (c_out, rust_out) in c_queries.iter().zip(rust_queries) {
        assert!(c_out.status.success(), "{c_out:?}");
        assert_eq!(
            (c_out.status.code(), &c_out.stdout, &c_out.stderr),
            (rust_out.status.code(), &rust_out.stdout, &rust_out.stderr),
            "query output differs\nC: {c_out:?}\nRust: {rust_out:?}"
        );
    }
    assert_eq!(c_invalid.stdout, rust_invalid.stdout);
    assert_eq!(c_invalid.stderr, rust_invalid.stderr);
    let trace = String::from_utf8_lossy(&c_queries[1].stdout);
    assert!(trace.contains(".go.Twice"), "Add caller missing: {trace}");
}

fn log_has_graphql_backend(log: &str) -> bool {
    log.lines().any(|line| {
        line.contains("msg=pass.route_nodes.graphql")
            && line.split_ascii_whitespace().any(|kv| kv == "planner=rust")
            && line.split_ascii_whitespace().any(|kv| kv == "writer=rust")
    })
}

#[test]
fn graphql_update_paths_publish_expected_routes() {
    let work = Scratch::new("graphql-repo");
    let repo = work.0.join("repo");
    fs::create_dir_all(repo.join("server")).expect("create resolver directory");
    let repo = fs::canonicalize(repo).expect("resolve repository");
    let project = repo
        .to_string_lossy()
        .trim_start_matches('/')
        .replace('/', "-");
    let repo_path = repo.to_str().expect("UTF-8 repository path");
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
        (
            "server/helper.go",
            "package server\nfunc helper() int { return 1 }\n",
        ),
    ] {
        fs::write(repo.join(path), contents).expect("write fixture file");
    }
    let steps = [
        (None, "msg=pipeline.route path=full", 1, true),
        (
            None,
            "msg=incremental.noop reason=semantic_manifest_equal",
            1,
            false,
        ),
        (
            Some(("op.graphql", "query OperationRenamed { value }\n")),
            "msg=incremental.route route=closure_repair",
            1,
            false,
        ),
        (
            Some((
                "server/helper.go",
                "package server\nfunc helper() int { return 2 }\n",
            )),
            "msg=incremental.route route=closure_repair",
            1,
            false,
        ),
        (
            Some((
                "schema.graphqls",
                "type Query { value: Int another: Int }\n",
            )),
            "msg=incremental.force_full",
            2,
            true,
        ),
    ];
    let scratch = Scratch::new("graphql-update");
    let mut worker_logs = std::collections::HashSet::new();
    let mut first_rows = None;
    for (edit, expected, route_count, full) in steps {
        if let Some((path, contents)) = edit {
            fs::write(repo.join(path), contents).expect("edit fixture file");
        }
        index_fixture(Path::new(MORI), &scratch, repo_path);
        let log = fs::read_dir(scratch.0.join("cache/logs"))
            .expect("read worker log directory")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".worker-log-")
            })
            .filter(|entry| worker_logs.insert(entry.path()))
            .map(|entry| {
                fs::read_to_string(entry.path())
                    .expect("read profiled worker log")
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(log.contains(expected), "did not report {expected}: {log}");
        if full {
            assert!(log.contains("msg=pipeline.route path=full"), "{log}");
            assert!(
                log_has_graphql_backend(&log),
                "Rust planner or writer missing: {log}"
            );
        }
        let rows = graphql_rows(&scratch, &project);
        assert_eq!(rows.len(), route_count);
        assert!(
            rows.iter().any(|row| row.contains("Query.value")
                && row.contains("gqlgen_resolver"))
        );
        if !full {
            assert_eq!(
                first_rows.as_ref(),
                Some(&rows),
                "Route or HANDLES changed"
            );
        } else if route_count == 1 {
            first_rows = Some(rows);
        } else {
            assert!(rows.iter().any(|row| row.contains("Query.another")
                && row.contains("graphql_schema_field")));
        }
    }
}

#[test]
fn graphql_routes_through_worker_reexec() {
    let fixture = Fixture::new(true);
    let project = fixture.project();
    let repo = fixture.0.to_string_lossy().into_owned();
    let scratch = Scratch::new("graphql");
    index_fixture(Path::new(MORI), &scratch, &repo);
    // Profiling preserves the daemon's worker log after a clean exit.
    let logged_implementation = fs::read_dir(scratch.0.join("cache/logs"))
        .expect("read worker log directory")
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".worker-log-")
        })
        .any(|entry| {
            fs::read_to_string(entry.path())
                .is_ok_and(|log| log_has_graphql_backend(&log))
        });
    assert!(
        logged_implementation,
        "worker did not report the Rust planner and writer"
    );
    let rows = graphql_rows(&scratch, &project);
    assert_eq!(rows.len(), 4, "expected one handler per GraphQL Route");
    assert!(rows.iter().any(|row| row.contains("newValue")));
    let query = run_in(
        Path::new(MORI),
        &scratch,
        &[
            "cli",
            "query_graph",
            "--project",
            &project,
            "--query",
            "MATCH (h)-[:HANDLES]->(r:Route) WHERE r.qualified_name STARTS WITH '__graphql__Query.' \
             RETURN r.name, h.name, h.label, r.ambiguous, r.file_path ORDER BY r.name",
        ],
    );
    assert!(query.status.success(), "{query:?}");
    let route = String::from_utf8_lossy(&query.stdout);
    for (name, handler, label, schema) in [
        (
            "Query.value",
            "Value",
            "Method",
            "graphql/nested/deep/schema1.graphqls",
        ),
        (
            "Query.oneOwner",
            "OneOwner",
            "Method",
            "other/schema.graphqls",
        ),
        (
            "Query.duplicate",
            "duplicate",
            "Field",
            "graphql/nested/deep/schema1.graphqls",
        ),
        (
            "Query.noOwner",
            "noOwner",
            "Field",
            "graphql/nested/deep/schema1.graphqls",
        ),
    ] {
        assert!(
            route.lines().any(|line| [name, handler, label, schema]
                .iter()
                .all(|value| line.contains(value))),
            "unexpected handler for {name}: {route}"
        );
    }
}
