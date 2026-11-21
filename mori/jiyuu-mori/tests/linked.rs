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
    run_in(bin, &Scratch::new(side), None, args)
}

fn run_in(
    bin: &Path,
    scratch: &Scratch,
    graphql: Option<&str>,
    args: &[&str],
) -> Output {
    let mut command = command_in(bin, scratch);
    if let Some(planner) = graphql {
        command.env("MORI_IMPL_GRAPHQL", planner);
    }
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
fn project_pages_use_internal_identity_and_preserve_projection() {
    let scratch = Scratch::new("projects");
    let cache = scratch.0.join("cache");
    let create_project_db = |file: &str, name: &str, extra: &str| {
        let db = rusqlite::Connection::open(cache.join(file)).unwrap();
        db.execute_batch(
            "CREATE TABLE projects (name TEXT PRIMARY KEY, indexed_at TEXT, root_path TEXT);
             CREATE TABLE nodes (project TEXT);
             CREATE TABLE edges (project TEXT);",
        ).unwrap();
        for project in [name, extra] {
            db.execute(
                "INSERT INTO projects VALUES (?1, 'now', '/tmp/graph-root')",
                [project],
            )
            .unwrap();
            db.execute("INSERT INTO nodes VALUES (?1), (?1)", [project])
                .unwrap();
            db.execute("INSERT INTO edges VALUES (?1)", [project])
                .unwrap();
        }
    };
    create_project_db("renamed.db", "alpha", "alpha::missed");
    create_project_db("second.db", "beta", "beta::missed");
    create_project_db("ambiguous.db", "skip-one", "skip-two");
    fs::write(cache.join("corrupt.db"), "not a SQLite database").unwrap();

    let cases = [
        r#"{"format":"json"}"#,
        r#"{"metadata_only":true,"limit":1}"#,
        r#"{"detail":"stats","limit":1,"offset":1,"format":"json"}"#,
        r#"{"include_details":true,"offset":99,"format":"json"}"#,
    ];
    let run_all = |bin: &Path| {
        let outputs = cases.map(|args| {
            run_in(
                bin,
                &scratch,
                None,
                &["cli", "--quiet", "list_projects", args],
            )
        });
        let stop = run_in(bin, &scratch, None, &["daemon", "stop"]);
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
    assert_eq!(identity["total"], 2);
    assert_eq!(identity["projects"][0]["name"], "alpha");
    let stats: serde_json::Value =
        serde_json::from_slice(&rust[2].stdout).unwrap();
    assert_eq!(stats["projects"][0]["name"], "beta");
    assert_eq!(stats["projects"][0]["nodes"], 2);
    assert_eq!(stats["projects"][0]["edges"], 1);
}

#[test]
fn unknown_tool_error_matches_c() {
    assert_same(&["cli", "no_such_tool"]);
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
    let run = |bin: &Path, side| {
        let scratch = Scratch::new(side);
        let requests = scratch.0.join("requests");
        fs::write(&requests, &input).expect("write MCP requests");
        let output = command_in(bin, &scratch)
            .stdin(Stdio::from(
                fs::File::open(requests).expect("open MCP requests"),
            ))
            .output()
            .expect("run MCP server");
        assert!(output.status.success(), "{side}: {output:?}");
        String::from_utf8(output.stdout).expect("UTF-8 response")
    };
    let reference = run(&c_binary(), "c");
    let linked = run(Path::new(MORI), "rs");
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
fn invalid_query_flag_diagnostic_matches_c() {
    assert_same(&["cli", "query_graph", "--offst", "1"]);
}

/// A small repository with Python and Go calls and a gqlgen server. Both
/// binaries index the same path, so project names match.
struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let dir = PathBuf::from(format!("/tmp/mori-fx{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let files = [
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
            (
                "gqlgen.yml",
                "schema:\n  - graphql/**/schema?.graphqls\nresolver:\n  layout: follow-schema\n  dir: server\n",
            ),
            (
                "graphql/nested/deep/schema1.graphqls",
                "type Query { value: Int oneOwner: Int duplicate: Int noOwner: Int }\n",
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
        for (rel, text) in files {
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

/// The value of `"key":<number>` in the index result.
fn json_number(text: &str, key: &str) -> Option<u64> {
    let start = text.find(&format!("\"{key}\":"))? + key.len() + 3;
    let digits: String = text[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// Indexes the fixture with each binary, which goes through the temporary
/// daemon and the index worker that the binary re-executes as itself. Compare
/// graph output from standalone C and both linked GraphQL planner modes.
#[test]
fn index_through_worker_reexec_matches_c() {
    let fixture = Fixture::new();
    let project = fixture.project();
    let repo = fixture.0.to_string_lossy().into_owned();
    let sides = [
        ("standalone", c_binary(), None),
        ("c", PathBuf::from(MORI), Some("c")),
        ("rust", PathBuf::from(MORI), Some("rust")),
    ];
    let mut results = Vec::new();
    for (side, bin, graphql) in &sides {
        let scratch = Scratch::new(side);
        let index = run_in(
            bin,
            &scratch,
            *graphql,
            &["cli", "--quiet", "index_repository", "--repo_path", &repo],
        );
        let text = String::from_utf8_lossy(&index.stdout).into_owned();
        assert!(
            index.status.success() && text.contains("\"status\":\"indexed\""),
            "{side} index failed: {text}\n{}",
            String::from_utf8_lossy(&index.stderr)
        );
        // Profiling preserves the daemon's worker log after a clean exit.
        let planner = format!("planner={}", graphql.unwrap_or("c"));
        let logged_planner = fs::read_dir(scratch.0.join("cache/logs"))
            .expect("read worker log directory")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".worker-log-")
            })
            .any(|entry| {
                fs::read_to_string(entry.path()).is_ok_and(|log| {
                    log.lines().any(|line| {
                        line.contains("msg=pass.route_nodes.graphql")
                            && line
                                .split_ascii_whitespace()
                                .any(|kv| kv == planner)
                    })
                })
            });
        assert!(logged_planner, "{side} worker didn't report {planner}");
        let counts = (json_number(&text, "nodes"), json_number(&text, "edges"));
        assert!(counts.0.is_some_and(|n| n > 0), "{side}: {text}");
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
            vec![
                "cli",
                "query_graph",
                "--project",
                &project,
                "--query",
                "MATCH (h)-[:HANDLES]->(r:Route) WHERE r.qualified_name = '__graphql__Query.value' RETURN h.name",
                "--max-rows=+7",
                "--max_rows",
                "4",
                "--offset",
                "0",
                "--",
                "ignored",
            ],
            vec![
                "cli",
                "query_graph",
                "--project",
                &project,
                "--query",
                "MATCH (h)-[:HANDLES]->(r:Route) WHERE r.qualified_name STARTS WITH '__graphql__Query.' \
                 RETURN r.name, h.name, h.label, r.ambiguous, r.file_path ORDER BY r.name",
            ],
        ]
        .iter()
        .map(|args| run_in(bin, &scratch, *graphql, args))
        .collect();
        results.push((counts, queries));
    }
    let (c, linked) = results.split_first().expect("standalone C result");
    for rs in linked {
        assert_eq!(c.0, rs.0, "node and edge counts differ");
        for (c_out, rs_out) in c.1.iter().zip(&rs.1) {
            assert!(c_out.status.success(), "{c_out:?}");
            assert!(
                (c_out.status.code(), &c_out.stdout, &c_out.stderr)
                    == (rs_out.status.code(), &rs_out.stdout, &rs_out.stderr),
                "query output differs\nC: {:?}\n{}\n{}\nRust: {:?}\n{}\n{}",
                c_out.status.code(),
                String::from_utf8_lossy(&c_out.stdout),
                String::from_utf8_lossy(&c_out.stderr),
                rs_out.status.code(),
                String::from_utf8_lossy(&rs_out.stdout),
                String::from_utf8_lossy(&rs_out.stderr)
            );
        }
    }
    let trace = String::from_utf8_lossy(&c.1[1].stdout);
    assert!(trace.contains(".go.Twice"), "Add caller missing: {trace}");
    let route =
        String::from_utf8_lossy(&c.1.last().expect("route query").stdout);
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
