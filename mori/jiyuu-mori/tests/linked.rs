//! Runs `mori` and the C `codebase-memory-mcp` from the same build directory
//! and checks that they behave the same. Needs feature `c-backend` and the
//! C reference binary (`MORI_C_REFERENCE=1`, set by `just check-linked`), so
//! a pure-Rust `cargo test` compiles this file to nothing.
#![cfg(feature = "c-backend")]

use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
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
fn top_level_command_precedence_matches_c() {
    for args in [
        &["--profile", "--help", "cli"][..],
        &["cli", "--help", "--version"],
        &["cli", "list_projects", "--help"],
        &["daemon", "--help"],
        &["daemon", "cli", "--help"],
        &["config", "--help"],
        &["allow-root", "--help"],
        &["cli", "--version"],
        &["hook-augment", "--event", "invalid"],
        &["--tool-profile=invalid"],
    ] {
        assert_same(args);
    }
}

#[test]
fn malformed_internal_roles_are_rejected_before_dispatch() {
    let fingerprint = "0123456789abcdef".repeat(4);
    let worker = [
        "cli",
        "--index-worker",
        "--index-worker-build",
        &fingerprint,
        "index_repository",
        "{}",
        "--response-out",
        "response",
    ];
    let mut worker_with_daemon_marker = worker;
    worker_with_daemon_marker[7] = "--cbm-daemon-internal";
    let overflow = format!("{}0", usize::MAX);
    let cases = [
        vec!["--help", "--cbm-daemon-internal"],
        vec!["--cbm-daemon-internal", "--cbm-daemon-permanent", "--help"],
        vec!["--help", "--index-worker"],
        worker_with_daemon_marker.to_vec(),
        [&worker[..], &["--index-worker-memory-budget-bytes", "0"]].concat(),
        [
            &worker[..],
            &["--index-worker-memory-budget-bytes", &overflow],
        ]
        .concat(),
        [
            &worker[..],
            &[
                "--index-worker-publish-fence",
                "fence",
                "--index-worker-quarantine",
                "quarantine",
            ],
        ]
        .concat(),
    ];
    for bin in [&c_binary(), Path::new(MORI)] {
        for args in &cases {
            let output = run(bin, "invalid-role", args);
            assert_eq!(output.status.code(), Some(1), "{bin:?} {args:?}");
            assert!(output.stdout.is_empty(), "{bin:?} {args:?}");
            assert_eq!(
                output.stderr, b"mori: invalid internal process arguments\n",
                "{bin:?} {args:?}"
            );
        }
    }
}

#[test]
fn top_level_selection_preserves_non_utf8_argv() {
    for bin in [&c_binary(), Path::new(MORI)] {
        let output = command_in(bin, &Scratch::new("argv"))
            .args([
                "cli".as_ref(),
                OsStr::from_bytes(b"tool-\xff"),
                "--help".as_ref(),
            ])
            .output()
            .expect("run binary");
        assert_eq!(output.status.code(), Some(1), "{bin:?}");
        assert!(output.stdout.is_empty(), "{bin:?}");
        assert_eq!(
            output.stderr, b"error: unknown tool 'tool-\xff'\n",
            "{bin:?}"
        );
    }
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
    let flag_cases: &[(&[&str], usize)] = &[
        (
            &[
                "list_projects",
                "--include-details=yes",
                "--metadata_only=",
                "--limit",
                "+1",
                "--offset=1",
                "--offset",
                "-1",
                "--format=json",
                "--",
                "ignored",
            ],
            2,
        ),
        (
            &[
                "index_status",
                "--project",
                "alpha",
                "--verbose",
                "--diagnostics=summary",
                "--format",
                "json",
            ],
            4,
        ),
    ];
    let run_all = |bin: &Path| {
        let outputs = cases.map(|(tool, args)| {
            run_in(bin, &scratch, &["cli", "--quiet", tool, args])
        });
        for &(flags, index) in flag_cases {
            let mut args = vec!["cli", "--quiet"];
            args.extend_from_slice(flags);
            let output = run_in(bin, &scratch, &args);
            let expected = &outputs[index];
            assert_eq!(
                (output.status.code(), &output.stdout, &output.stderr),
                (expected.status.code(), &expected.stdout, &expected.stderr),
                "{flags:?}",
            );
        }
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
    let output = run_stdio_in(bin, &Scratch::new(side), args, input);
    assert!(output.status.success(), "{side}: {output:?}");
    String::from_utf8(output.stdout).expect("UTF-8 response")
}

fn run_stdio_in(
    bin: &Path,
    scratch: &Scratch,
    args: &[&str],
    input: &str,
) -> Output {
    let requests = scratch.0.join("requests");
    fs::write(&requests, input).expect("write MCP requests");
    command_in(bin, scratch)
        .args(args)
        .stdin(Stdio::from(
            fs::File::open(requests).expect("open MCP requests"),
        ))
        .output()
        .expect("run MCP server")
}

#[test]
fn mcp_method_dispatch_matches_c() {
    let input = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":\"ping\",\"method\":\"ping\"}\n",
        "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":null,\"method\":\"ping\"}\n",
        "{\"jsonrpc\":\"2.0\",\"method\":\"ping\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":\"resources\",\"method\":\"resources/list\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":\"templates\",\"method\":\"resources/templates/list\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":\"discover\",\"method\":\"server/discover\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":\"unknown\",\"method\":\"ping/extra\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":\"cancel-request\",\"method\":\"notifications/cancelled\"}\n",
    );
    let reference = run_stdio(&c_binary(), "c", &[], input);
    let linked = run_stdio(Path::new(MORI), "rs", &[], input);
    assert_eq!(linked, reference);
    let replies: Vec<serde_json::Value> = linked
        .lines()
        .map(|line| serde_json::from_str(line).expect("parse response"))
        .collect();
    assert_eq!(replies.len(), 7, "{linked}");
    assert_eq!(replies[0]["id"], "ping");
    assert_eq!(replies[0]["result"], serde_json::json!({}));
    // The C envelope uses -1 for a present null ID. A missing ID is silent.
    assert_eq!(replies[1]["id"], -1);
    assert_eq!(replies[1]["result"], serde_json::json!({}));
    assert_eq!(replies[2]["result"]["resources"], serde_json::json!([]));
    assert_eq!(
        replies[3]["result"]["resourceTemplates"],
        serde_json::json!([])
    );
    for (reply, id) in
        replies[4..]
            .iter()
            .zip(["discover", "unknown", "cancel-request"])
    {
        assert_eq!(reply["id"], id);
        assert_eq!(reply["error"]["code"], -32601);
    }
}

#[test]
fn mcp_tools_call_params_and_ids_match_c_through_daemon() {
    // Keep the duplicate keys in the wire input so a JSON map cannot
    // collapse them before the server sees the request.
    let input = [
        r#"{"jsonrpc":"2.0","id":0,"method":"initialize"}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":"missing","method":"tools/call"}"#,
        r#"{"jsonrpc":"2.0","id":"null","method":"tools/call","params":null}"#,
        r#"{"jsonrpc":"2.0","id":"array","method":"tools/call","params":[]}"#,
        r#"{"jsonrpc":"2.0","id":"wrong-name","method":"tools/call","params":{"name":null,"name":"list_projects"}}"#,
        r#"{"jsonrpc":"2.0","id":"no-args","method":"tools/call","params":{"name":"list_projects"}}"#,
        r#"{"jsonrpc":"2.0","id":"null-args","method":"tools/call","params":{"name":"list_projects","arguments":null}}"#,
        r#"{"jsonrpc":"2.0","id":"scalar-args","method":"tools/call","params":{"name":"list_projects","arguments":1}}"#,
        r#"{"jsonrpc":"2.0","id":"duplicate-name","method":"tools/call","params":{"name":"list_projects","name":"unknown","arguments":{"format":"json"}}}"#,
        r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"list_projects","arguments":{"format":"json","format":"tree"},"arguments":{"format":"tree"}}}"#,
        r#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"list_projects"}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":"9"}}"#,
        r#"{"jsonrpc":"2.0","id":"end","method":"ping"}"#,
        r#"{"jsonrpc":"2.0","id":"tools","method":"tools/list"}"#,
        r#"{"jsonrpc":"2.0","id":"prompts","method":"prompts/list"}"#,
        r#"{"jsonrpc":"2.0","id":"prompt","method":"prompts/get","params":{"name":"explore_codebase","arguments":{"project":"payments","question":"How are refunds routed?"}}}"#,
        r#"{"jsonrpc":"2.0","id":"bad-prompt","method":"prompts/get","params":{"name":"explore_codebase","arguments":[]}}"#,
        r#"{"jsonrpc":"2.0","id":"probe","method":"server/discover"}"#,
        r#"{"jsonrpc":"2.0","id":"unknown","method":"unknown"}"#,
        "",
    ]
    .join("\n");

    let forwarded = |bin: &Path, side: &str, rust: bool| {
        let scratch = Scratch::new(side);
        let start = command_in(bin, &scratch)
            .env("CBM_LOG_LEVEL", "debug")
            .args(["daemon", "start"])
            .output()
            .unwrap();
        assert!(start.status.success(), "{start:?}");
        let output = run_stdio_in(bin, &scratch, &[], &input);
        let stop = run_in(bin, &scratch, &["daemon", "stop"]);
        assert!(stop.status.success(), "{stop:?}");
        assert!(output.status.success(), "{output:?}");

        let log =
            fs::read_to_string(scratch.0.join("cache/logs/cbm-daemon.log"))
                .unwrap();
        for (method, count) in [
            ("initialize", 1),
            ("tools/call", 9),
            ("tools/list", 1),
            ("prompts/list", 1),
            ("prompts/get", 2),
            ("server/discover", 1),
            ("unknown", 1),
            ("ping", 1),
        ] {
            let event = format!(
                "msg=mcp.dispatch backend=rust method={method} has_id=true"
            );
            assert_eq!(
                log.lines().filter(|line| line.contains(&event)).count(),
                if rust { count } else { 0 },
                "{side}: {method} dispatch count",
            );
        }
        for (method, status) in [("server/discover", "ok"), ("unknown", "error")]
        {
            assert!(log.contains(&format!(
                "msg=mcp.request protocol=jsonrpc method={method} status={status}"
            )));
        }
        assert_eq!(
            log.lines()
                .filter(|line| line.contains(
                    "msg=mcp.request protocol=jsonrpc method=tools/call"
                ))
                .count(),
            9,
            "{side} daemon must handle each request with an ID exactly once",
        );
        assert_eq!(
            log.lines()
                .filter(|line| line
                    .contains("msg=mcp.tools_call.decode backend=rust"))
                .count(),
            if rust { 9 } else { 0 },
            "{side} decoder must handle each request with an ID exactly once",
        );
        output.stdout
    };

    let reference = forwarded(&c_binary(), "c", false);
    let linked = forwarded(Path::new(MORI), "rs", true);
    assert_eq!(linked, reference);
    let replies: Vec<serde_json::Value> = std::str::from_utf8(&linked)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).expect("parse response"))
        .collect();
    let ids: Vec<_> = replies.iter().map(|reply| reply["id"].clone()).collect();
    assert_eq!(
        ids,
        &serde_json::json!([
            0,
            "missing",
            "null",
            "array",
            "wrong-name",
            "no-args",
            "null-args",
            "scalar-args",
            "duplicate-name",
            9,
            "end",
            "tools",
            "prompts",
            "prompt",
            "bad-prompt",
            "probe",
            "unknown",
        ])
        .as_array()
        .unwrap()[..],
    );
    for reply in &replies[1..5] {
        assert_eq!(reply["result"]["isError"], true);
        assert_eq!(
            reply["result"]["structuredContent"]["error"],
            "missing tool name"
        );
    }
    for reply in &replies[5..8] {
        assert_eq!(reply["result"]["isError"], false);
        assert!(
            reply["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .starts_with("projects: 0")
        );
    }
    for reply in &replies[8..10] {
        assert_eq!(
            reply["result"]["structuredContent"]["projects"],
            serde_json::json!([])
        );
    }
    assert!(replies[11]["result"]["tools"].is_array());
    assert!(replies[12]["result"]["prompts"].is_array());
    assert!(
        replies[13]["result"]["messages"][0]["content"]["text"]
            .as_str()
            .unwrap()
            .contains("How are refunds routed?")
    );
    assert_eq!(replies[14]["error"]["code"], -32602);
    assert_eq!(replies[15]["error"]["code"], -32601);
    assert_eq!(replies[16]["error"]["code"], -32601);
}

#[test]
#[ignore = "requires the opt-in C worker test seams"]
fn mcp_queued_and_active_cancellation_match_c_through_daemon() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::Child;
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::time::{Duration, Instant};

    use serde_json::{Value, json};

    struct Frontend(Child);
    impl Drop for Frontend {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    assert_eq!(
        option_env!("TEST_SEAMS"),
        Some("1"),
        "build both binaries with TEST_SEAMS=1 to enable the worker marker"
    );
    let c = c_binary();
    for (id, queued_id) in [(json!(17), json!("17")), (json!("17"), json!(17))] {
        let run = |bin: &Path, side: &str| {
            let scratch = Scratch::new(side);
            let repo = scratch.0.join("repo");
            fs::create_dir(&repo).expect("create test repository");
            fs::write(repo.join("hang_me.py"), "def smoke(): return 1\n")
                .expect("write worker hang fixture");
            let marker = scratch.0.join("worker-descendant.pid");

            // An MCP frontend starts its own isolated daemon. Only the
            // worker spawned for the held index run writes the marker.
            let mut frontend = Frontend(
                command_in(bin, &scratch)
                    .env("CBM_TEST_HANG_ON", "hang_me")
                    .env("CBM_TEST_WORKER_DESCENDANT_PID_FILE", &marker)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .spawn()
                    .expect("start MCP frontend"),
            );
            let mut stdin = frontend.0.stdin.take().expect("MCP stdin");
            let stdout = frontend.0.stdout.take().expect("MCP stdout");
            let (send, receive) = mpsc::channel();
            let reader = std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    if send.send(line).is_err() {
                        break;
                    }
                }
            });

            let mut request = |message: Value| {
                writeln!(stdin, "{message}").expect("send MCP request");
                stdin.flush().expect("flush MCP stdin");
            };
            let response = || -> Value {
                let line = receive
                    .recv_timeout(Duration::from_secs(20))
                    .expect("wait for MCP response")
                    .expect("read MCP response");
                serde_json::from_str(&line).expect("parse MCP response")
            };

            request(json!({"jsonrpc":"2.0","id":"init","method":"initialize"}));
            let initialized = response();
            assert_eq!(initialized["id"], "init");
            assert!(initialized.get("result").is_some(), "{initialized}");
            request(
                json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            );
            request(json!({
                "jsonrpc":"2.0", "id":id, "method":"tools/call",
                "params":{"name":"index_repository", "arguments":{"repo_path":repo,"mode":"fast"}},
            }));

            let deadline = Instant::now() + Duration::from_secs(20);
            while !marker.is_file() {
                assert!(
                    Instant::now() < deadline,
                    "{side} worker did not start"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            // The first request is still running. Queue a different ID with
            // the same spelling, then cancel it before releasing the worker.
            request(json!({"jsonrpc":"2.0","id":queued_id,"method":"ping"}));
            request(json!({
                "jsonrpc":"2.0","method":"notifications/cancelled",
                "params":{"requestId":queued_id},
            }));
            request(json!({
                "jsonrpc":"2.0","method":"notifications/cancelled",
                "params":{"requestId":id},
            }));
            request(json!({"jsonrpc":"2.0","id":"after","method":"ping"}));
            let cancelled = response();
            assert_eq!(cancelled["id"], id);
            assert_eq!(cancelled["error"]["code"], -32800);
            assert!(cancelled.get("result").is_none(), "{cancelled}");
            let queued = response();
            assert_eq!(queued["id"], queued_id);
            assert_eq!(queued["error"]["code"], -32800);
            assert!(queued.get("result").is_none(), "{queued}");
            assert_eq!(
                response(),
                json!({"jsonrpc":"2.0","id":"after","result":{}}),
                "cancellation must leave the session open and have no reply of its own"
            );

            drop(stdin);
            assert!(
                matches!(
                    receive.recv_timeout(Duration::from_secs(20)),
                    Err(RecvTimeoutError::Disconnected)
                ),
                "expected EOF after the ping reply"
            );
            reader.join().expect("join MCP stdout reader");
            assert!(frontend.0.wait().expect("wait for MCP frontend").success());
            [cancelled, queued]
        };

        assert_eq!(run(&c, "c"), run(Path::new(MORI), "rs"));
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
fn invalid_read_flag_diagnostics_match_c() {
    assert_same(&["cli", "query_graph", "--offst", "1"]);
    assert_same(&["cli", "list_projects", "--offst", "1"]);
    assert_same(&["cli", "index_status", "--verbos", "true"]);
    assert_same(&["cli", "index_status", "--verbose=true", "stray"]);
    assert_same(&["cli", "get_graph_schema", "--limt", "1"]);
    assert_same(&["cli", "get_code_snippet", "--source-mod", "full"]);
    assert_same(&["cli", "search_code", "--patten", "hello"]);
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
#[ignore = "requires an embedded UI build with MORI_WITH_UI=1"]
fn standalone_ui_signals_cancel_active_reads() {
    use std::io::Write;
    use std::net::{Ipv4Addr, TcpListener, TcpStream};
    use std::process::Child;
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    struct Ui(Child);
    impl Drop for Ui {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    let bin = Path::new(MORI);
    let writer = Scratch::new("ui-index");
    let repo = writer.0.join("repo");
    fs::create_dir(&repo).unwrap();
    fs::write(repo.join("app.py"), "def main(): return 1\n").unwrap();
    let index = index_fixture(bin, &writer, repo.to_str().unwrap());
    let project = index["project"].as_str().unwrap();
    let stopped = run_in(bin, &writer, &["daemon", "stop"]);
    assert!(stopped.status.success(), "{stopped:?}");
    let db_file = format!("{project}.db");
    let graph_db = writer.0.join("cache").join(&db_file);

    let changes = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "detect_changes", "arguments": {
            "project": project, "scope": "files", "base_branch": "HEAD"
        }}
    })
    .to_string();
    let projects = serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": "list_projects", "arguments": {"format": "json"}}
    })
    .to_string();
    let repo_info = format!("/api/repo-info?project={project}");
    let impact = format!("/api/impact-analysis?project={project}&file=app.py");

    for (signal, method, target, body, cancelled_rpc) in [
        (libc::SIGINT, "POST", "/rpc", changes.as_str(), true),
        (libc::SIGTERM, "POST", "/rpc", changes.as_str(), true),
        (libc::SIGINT, "POST", "/rpc", projects.as_str(), false),
        (libc::SIGTERM, "GET", repo_info.as_str(), "", false),
        (libc::SIGTERM, "GET", impact.as_str(), "", false),
    ] {
        let scratch = Scratch::new("ui-signal");
        fs::copy(&graph_db, scratch.0.join("cache").join(&db_file)).unwrap();
        let fixture_bin = scratch.0.join("bin");
        fs::create_dir(&fixture_bin).unwrap();
        let git = fixture_bin.join("git");
        fs::write(
            &git,
            r#"#!/bin/sh
[ "$3" = rev-parse ] || [ "$1" = --no-optional-locks ] || exit 2
printf '%s\n' "$$" > "$MORI_TEST_GIT_ENTERED"
exec /bin/sleep 30
"#,
        )
        .unwrap();
        fs::set_permissions(&git, fs::Permissions::from_mode(0o700)).unwrap();
        let entered = scratch.0.join("git-entered");
        let log = scratch.0.join("ui.log");
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let mut ui = Ui(command_in(bin, &scratch)
            .args(["ui", "--port", &addr.port().to_string()])
            .env("PATH", &fixture_bin)
            .env("CBM_LOG_LEVEL", "info")
            .env("CBM_LOG_FORMAT", "text")
            .env("TMPDIR", &scratch.0)
            .env("MORI_TEST_GIT_ENTERED", &entered)
            .stderr(fs::File::create(&log).unwrap())
            .spawn()
            .unwrap());

        let log_text = || fs::read_to_string(&log).unwrap_or_default();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut conn = loop {
            if let Ok(conn) = TcpStream::connect(addr) {
                break conn;
            }
            assert!(
                Instant::now() < deadline,
                "UI did not start: {}",
                log_text()
            );
            sleep(Duration::from_millis(10));
        };
        write!(
            conn,
            "{method} {target} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\n\r\n{body}",
            body.len(),
        )
        .unwrap();
        let git_pid: libc::pid_t = loop {
            if let Some(pid) = fs::read_to_string(&entered)
                .ok()
                .and_then(|marker| marker.trim().parse().ok())
            {
                break pid;
            }
            assert!(
                Instant::now() < deadline,
                "Git was not entered: {}",
                log_text()
            );
            sleep(Duration::from_millis(10));
        };

        // SAFETY: the child is owned by ui and has not been reaped.
        assert_eq!(unsafe { libc::kill(ui.0.id() as libc::pid_t, signal) }, 0);
        let deadline = Instant::now() + Duration::from_secs(3);
        let status = loop {
            if let Some(status) = ui.0.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "UI did not cancel: {}",
                log_text()
            );
            sleep(Duration::from_millis(10));
        };
        let logs = log_text();
        assert!(status.success(), "{status}: {logs}");
        if cancelled_rpc {
            assert!(
                logs.lines().any(|line| line.contains("tool=detect_changes")
                    && line.contains("cancelled=true")
                    && line.contains("timed_out=false")),
                "{logs}"
            );
        } else {
            // SAFETY: signal 0 only probes the child PID recorded by our fixture.
            assert_eq!(unsafe { libc::kill(git_pid, 0) }, -1, "{logs}");
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ESRCH)
            );
        }
        assert_eq!(fs::read_dir(scratch.0.join("rt")).unwrap().count(), 0);
    }
}

#[test]
fn trace_ingestion_matches_native_replies_and_stored_observations() {
    use serde_json::{Value, json};

    let fixture = Scratch::new("trace-input");
    let repo = fixture.0.join("repo");
    fs::create_dir(&repo).unwrap();
    fs::write(
        repo.join("app.py"),
        "def caller():\n    return callee()\n\ndef callee():\n    return 1\n",
    )
    .unwrap();

    let exercise = |bin: &Path, side: &str| {
        let scratch = Scratch::new(side);
        let index = index_fixture(bin, &scratch, repo.to_str().unwrap());
        let project = index["project"].as_str().unwrap();
        let db = Connection::open_with_flags(
            scratch.0.join("cache").join(format!("{project}.db")),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let symbol_name = |name: &str| {
            db.query_row(
                "SELECT qualified_name FROM nodes WHERE label = 'Function' AND name = ?1",
                [name],
                |row| row.get::<_, String>(0),
            )
            .unwrap()
        };
        let caller = symbol_name("caller");
        let callee = symbol_name("callee");

        let start = command_in(bin, &scratch)
            .env("CBM_LOG_LEVEL", "debug")
            .args(["daemon", "start"])
            .output()
            .unwrap();
        assert!(start.status.success(), "{start:?}");

        let arguments = json!({
            "project": project,
            "traces": [
                {"path": [&caller, &callee, &caller], "count": 3},
                {"caller": &caller, "callee": &callee, "count": -5},
                {"caller": &caller, "callee": &callee, "count": "8", "path": null},
                {"path": [&caller, "missing", &callee]},
                {"path": [&caller], "caller": &caller, "callee": &callee},
                {"path": [&caller, false]},
                {"path": vec![&caller; 257]},
            ],
        });
        let ingest = run_in(
            bin,
            &scratch,
            &["cli", "--quiet", "ingest_traces", &arguments.to_string()],
        );
        let stop = run_in(bin, &scratch, &["daemon", "stop"]);
        assert!(stop.status.success(), "{stop:?}");
        assert!(ingest.status.success(), "{ingest:?}");

        let reply: Value = serde_json::from_slice(&ingest.stdout).unwrap();
        let rows = |sql: &str| {
            let mut query = db.prepare(sql).unwrap();
            query
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .map(|row| serde_json::from_str::<Value>(&row.unwrap()).unwrap())
                .collect::<Vec<_>>()
        };
        let pairs = rows(
            "SELECT json_array(caller_qn, callee_qn, label, count) FROM observed_calls
             ORDER BY caller_qn, callee_qn",
        );
        let paths = rows(
            "SELECT json_array(json(path_json), label, count) FROM observed_paths
             ORDER BY path_json",
        );
        let log =
            fs::read_to_string(scratch.0.join("cache/logs/cbm-daemon.log"))
                .unwrap();
        let used_rust = log.lines().any(|line| {
            line.contains("ingest_traces.normalize")
                && line.contains("backend=rust")
        });
        assert_eq!(
            reply,
            json!({
                "status": "stored",
                "label": "unlabeled",
                "pairs_stored": 4,
                "paths_stored": 2,
                "pairs_unmatched": 2,
                "unmatched_sample": ["missing"],
                "malformed": 3,
                "pair_total": 2,
            })
        );
        assert_eq!(
            pairs,
            [
                json!([&callee, &caller, "unlabeled", 3]),
                json!([&caller, &callee, "unlabeled", 5]),
            ],
        );
        assert_eq!(
            paths,
            [
                json!([[&caller, &callee, &caller], "unlabeled", 3]),
                json!([null, "unlabeled", 1]),
            ],
        );
        (reply, pairs, paths, used_rust)
    };

    let (native_reply, native_pairs, native_paths, native_rust) =
        exercise(&c_binary(), "trace-c");
    let (rust_reply, rust_pairs, rust_paths, rust_used) =
        exercise(Path::new(MORI), "trace-rust");
    assert_eq!(
        (native_reply, native_pairs, native_paths),
        (rust_reply, rust_pairs, rust_paths),
    );
    assert!(!native_rust && rust_used);
}

#[test]
fn scalar_read_flags_match_json_and_c_on_one_cache() {
    let scratch = Scratch::new("scalar-read");
    let repo = scratch.0.join("repo");
    fs::create_dir(&repo).unwrap();
    fs::write(
        repo.join("app.py"),
        "def sample():\n    return 'mori_scalar_read'\n",
    )
    .unwrap();
    let c = c_binary();
    let rust = Path::new(MORI);
    let rust_runtime = Scratch::new("scalar-rust");
    // Read calls can start a daemon. Share the graph cache, with one runtime
    // per binary so each call reaches its own implementation.
    let invoke = |bin: &Path, args: &[&str]| {
        let runtime = if bin == rust { &rust_runtime } else { &scratch };
        command_in(bin, runtime)
            .env("CBM_CACHE_DIR", scratch.0.join("cache"))
            .args(args)
            .output()
            .unwrap()
    };
    let index = index_fixture(&c, &scratch, repo.to_str().unwrap());
    let project = index["project"].as_str().expect("indexed project");
    let stop = invoke(&c, &["daemon", "stop"]);
    assert!(stop.status.success(), "{stop:?}");

    let cases = [
        (
            "get_graph_schema",
            vec!["--diagnostics=full", "--limit", "+2", "--limit=1"],
            r#""diagnostics":"full","limit":2,"limit":1"#,
            "node_labels",
        ),
        (
            "get_code_snippet",
            vec![
                "--qualified-name",
                "sample",
                "--source-mode=full",
                "--include-neighbors=",
                "--max-lines=2",
            ],
            r#""qualified_name":"sample","source_mode":"full","include_neighbors":false,"max_lines":2"#,
            "mori_scalar_read",
        ),
        (
            "search_code",
            vec![
                "--pattern=^def sample",
                "--regex",
                "--mode=files",
                "--file-pattern=*.py",
                "--result-limit=1",
            ],
            r#""pattern":"^def sample","regex":true,"mode":"files","file_pattern":"*.py","result_limit":1"#,
            "app.py",
        ),
    ];
    for (tool, flags, raw_options, expected) in cases {
        let mut argv = vec![
            "cli",
            "--quiet",
            tool,
            "--project",
            project,
            "--format=json",
        ];
        argv.extend(flags);
        let json_args = format!(
            r#"{{"project":{},"format":"json",{raw_options}}}"#,
            serde_json::to_string(project).unwrap()
        );
        let reference = invoke(&c, &argv);
        let stdout = String::from_utf8_lossy(&reference.stdout);
        assert!(
            reference.status.success() && stdout.contains(expected),
            "{tool}: {reference:?}"
        );
        let stable_stdout = |output: &Output| {
            if tool != "search_code" {
                return output.stdout.clone();
            }
            let mut result: serde_json::Value =
                serde_json::from_slice(&output.stdout).unwrap();
            let elapsed = result.as_object_mut().unwrap().remove("elapsed_ms");
            assert!(elapsed.is_some_and(|value| value.is_number()));
            serde_json::to_vec(&result).unwrap()
        };
        let expected_stdout = stable_stdout(&reference);
        let json_argv = ["cli", "--quiet", tool, &json_args];
        for bin in [c.as_path(), rust] {
            for args in [argv.as_slice(), &json_argv] {
                let actual = invoke(bin, args);
                assert_eq!(
                    (actual.status, &stable_stdout(&actual), &actual.stderr),
                    (reference.status, &expected_stdout, &reference.stderr),
                    "{bin:?} {args:?}"
                );
            }
        }
    }
    for bin in [c.as_path(), rust] {
        let stop = invoke(bin, &["daemon", "stop"]);
        assert!(stop.status.success(), "{bin:?}: {stop:?}");
    }
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
            "paths": ["./nested\\partial.py", "nested/missing.py", "verified.py", "changed.py",
                      "nested/../partial.py", "C:relative.py", "nested/bad\n.py", ".", "", null],
            "scopes": ["./nested//", ".", "/outside", "nested/../scope"],
            "scope_limit": 1, "format": format
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
    let rust = Path::new(MORI);
    let start = command_in(rust, &scratch)
        .env("CBM_LOG_LEVEL", "debug")
        .args(["daemon", "start"])
        .output()
        .unwrap();
    assert!(start.status.success(), "{start:?}");
    let linked = read(rust);
    for (reference, linked) in reference.iter().zip(&linked) {
        assert!(reference.status.success() && linked.status.success());
        assert_eq!(reference.stdout, linked.stdout);
        assert_eq!(reference.stderr, linked.stderr);
    }
    let json: serde_json::Value =
        serde_json::from_slice(&linked[0].stdout).unwrap();
    let paths = json["paths"].as_array().unwrap();
    assert_eq!(paths[0]["path"], "nested/partial.py");
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
    assert_eq!(paths[4]["status"], "outside_project");
    assert_eq!(paths[5]["status"], "outside_project");
    assert_eq!(paths[6]["status"], "invalid_path");
    assert_eq!(paths[7]["status"], "invalid_path");
    assert_eq!(paths[8]["status"], "outside_project");
    assert_eq!(paths[9]["status"], "invalid_path");
    let scopes = json["scopes"].as_array().unwrap();
    assert_eq!(scopes[0]["scope"], "nested");
    assert_eq!(scopes[1]["scope"], ".");
    assert_eq!(scopes[2]["status"], "outside_project");
    assert_eq!(scopes[3]["status"], "outside_project");
    let log =
        fs::read_to_string(scratch.0.join("cache/logs/cbm-daemon.log")).unwrap();
    assert!(log.lines().any(|line| {
        line.contains("check_index_coverage.normalize")
            && line.contains("backend=rust")
    }));
}

#[test]
fn snippet_source_windows_match_c_in_cli_and_forwarded_mcp() {
    let scratch = Scratch::new("snippet-window");
    let repo = scratch.0.join("repo");
    fs::create_dir(&repo).unwrap();
    let file = repo.join("app.py");
    let mut source = "class Big:\n    def first(self): return 1\n".to_owned();
    source.push_str(&"    # filler\n".repeat(600));
    source.push_str("    def last(self): return 2\n");
    fs::write(&file, &source).unwrap();

    let c = c_binary();
    let rust = Path::new(MORI);
    let index = index_fixture(&c, &scratch, repo.to_str().unwrap());
    let project = index["project"].as_str().unwrap();
    let stop = |bin: &Path| {
        let output = run_in(bin, &scratch, &["daemon", "stop"]);
        assert!(output.status.success(), "{output:?}");
    };
    stop(&c);
    let graph = Connection::open_with_flags(
        scratch.0.join("cache").join(format!("{project}.db")),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let qualified_name = |name: &str| -> String {
        graph
            .query_row(
                "SELECT qualified_name FROM nodes WHERE name = ?",
                [name],
                |row| row.get(0),
            )
            .unwrap()
    };
    let big = qualified_name("Big");
    let first = qualified_name("first");
    drop(graph);

    let requests = [
        serde_json::json!({"qualified_name": big}),
        serde_json::json!({"qualified_name": big, "source_mode": "full"}),
        serde_json::json!({
            "qualified_name": big, "source_mode": "full",
            "start_line": 200, "max_lines": 3, "max_output_tokens": 1,
        }),
        serde_json::json!({
            "qualified_name": big, "source_mode": "outline",
            "start_line": 200, "max_lines": 3,
        }),
        serde_json::json!({
            "qualified_name": first, "start_line": 999,
        }),
        serde_json::json!({"qualified_name": "unknown_symbol"}),
    ]
    .map(|mut args| {
        args["project"] = serde_json::json!(project);
        args["format"] = serde_json::json!("json");
        args
    });
    let read = |bin: &Path, args: &serde_json::Value| {
        run_in(
            bin,
            &scratch,
            &["cli", "--quiet", "get_code_snippet", &args.to_string()],
        )
    };
    let assert_parity = |args| {
        let reference = read(&c, args);
        let candidate = read(rust, args);
        assert_eq!(candidate.status.code(), reference.status.code(), "{args}");
        assert_eq!(candidate.stdout, reference.stdout, "{args}");
        assert_eq!(candidate.stderr, reference.stderr, "{args}");
        candidate
    };
    let responses = requests.each_ref().map(assert_parity);
    let json = |output: &Output| -> serde_json::Value {
        assert!(output.status.success(), "{output:?}");
        serde_json::from_slice(&output.stdout).unwrap()
    };
    assert_eq!(json(&responses[0])["source_mode"], "outline");
    let full = json(&responses[1]);
    assert_eq!(full["end_line"], 500);
    assert_eq!(full["next_start_line"], 501);
    assert_eq!(full["content_verified"], true);
    let page = json(&responses[2]);
    assert_eq!(page["start_line"], 200);
    assert_eq!(page["end_line"], 202);
    assert_eq!(page["next_start_line"], 203);
    assert_eq!(json(&responses[3])["start_line"], 1);
    let one_line = json(&responses[4]);
    assert_eq!(one_line["start_line"], one_line["end_line"]);
    assert!(
        String::from_utf8_lossy(&responses[5].stderr)
            .contains("symbol not found")
            || String::from_utf8_lossy(&responses[5].stdout)
                .contains("symbol not found")
    );

    // Repeat a successful request through MCP stdio. The debug marker is
    // written by the daemon only after the Rust callback returns.
    let startup = command_in(rust, &scratch)
        .env("CBM_LOG_LEVEL", "debug")
        .args(["daemon", "start"])
        .output()
        .unwrap();
    assert!(startup.status.success(), "{startup:?}");
    let rpc = scratch.0.join("requests");
    let call = serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": "get_code_snippet", "arguments": requests[2]},
    });
    fs::write(
        &rpc,
        format!(
            "{}\n{}\n{call}\n",
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"linked-test","version":"1"}}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        ),
    )
    .unwrap();
    let forwarded = command_in(rust, &scratch)
        .stdin(Stdio::from(fs::File::open(rpc).unwrap()))
        .output()
        .unwrap();
    stop(rust);
    assert!(forwarded.status.success(), "{forwarded:?}");
    let reply: serde_json::Value = String::from_utf8(forwarded.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .find(|reply| reply["id"] == 2)
        .unwrap();
    let forwarded_result: serde_json::Value = serde_json::from_str(
        reply["result"]["content"][0]["text"].as_str().unwrap(),
    )
    .unwrap();
    assert_eq!(forwarded_result, page);
    let log =
        fs::read_to_string(scratch.0.join("cache/logs/cbm-daemon.log")).unwrap();
    assert!(log.lines().any(|line| {
        line.contains("get_code_snippet.window") && line.contains("backend=rust")
    }));

    fs::write(&file, format!("# shifted\n{source}")).unwrap();
    let drift = json(&assert_parity(&requests[2]));
    assert_eq!(drift["source_drift"], true);
    assert_eq!(drift["source"], "(source not available)");
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
fn search_code_ranking_reads_call_degrees_in_rust() {
    let scratch = Scratch::new("search-degrees");
    let repo = scratch.0.join("repo");
    fs::create_dir(&repo).unwrap();
    fs::write(
        repo.join("app.py"),
        "def alpha():\n    return 'mori_search_needle'\n\n\
         def zeta():\n    return 'mori_search_needle'\n\n\
         def zero():\n    return 'mori_search_needle'\n",
    )
    .unwrap();
    let c = c_binary();
    let rust = Path::new(MORI);
    let index = index_fixture(&c, &scratch, repo.to_str().unwrap());
    let project = index["project"].as_str().unwrap();
    let stop = |bin: &Path| {
        let output = run_in(bin, &scratch, &["daemon", "stop"]);
        assert!(output.status.success(), "{output:?}");
    };
    stop(&c);

    let db =
        Connection::open(scratch.0.join("cache").join(format!("{project}.db")))
            .unwrap();
    let id = |name: &str| -> i64 {
        db.query_row(
            "SELECT id FROM nodes WHERE project = ? AND label = 'Function' AND name = ?",
            [project, name],
            |row| row.get(0),
        )
        .unwrap()
    };
    for (source, target, kind) in
        [("alpha", "zeta", "CALLS"), ("zeta", "zero", "USAGE")]
    {
        db.execute(
            "INSERT INTO edges (project, source_id, target_id, type, properties) \
             VALUES (?1, ?2, ?3, ?4, '{}')",
            rusqlite::params![project, id(source), id(target), kind],
        )
        .unwrap();
    }
    drop(db);

    let read = |bin: &Path, limit: usize| {
        let args = serde_json::json!({"project": project, "pattern": "mori_search_needle",
                                     "format": "json", "result_limit": limit});
        let output = run_in(
            bin,
            &scratch,
            &["cli", "--quiet", "search_code", &args.to_string()],
        );
        assert!(output.status.success(), "{output:?}");
        let mut response: serde_json::Value =
            serde_json::from_slice(&output.stdout).unwrap();
        response.as_object_mut().unwrap().remove("elapsed_ms");
        response
    };
    let reference = [read(&c, 8), read(&c, 1)];
    stop(&c);
    let start = command_in(rust, &scratch)
        .env("CBM_LOG_LEVEL", "debug")
        .args(["daemon", "start"])
        .output()
        .unwrap();
    assert!(start.status.success(), "{start:?}");
    let candidate = [read(rust, 8), read(rust, 1)];
    stop(rust);
    assert_eq!(candidate, reference);
    let rows = candidate[0]["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    for (row, (name, inbound, outbound)) in
        rows.iter()
            .zip([("zeta", 1, 0), ("alpha", 0, 1), ("zero", 0, 0)])
    {
        assert!(
            row[0].as_str().unwrap().ends_with(&format!(".{name}")),
            "{row}"
        );
        assert_eq!(row[6], inbound, "{row}");
        assert_eq!(row[7], outbound, "{row}");
    }
    assert_eq!(candidate[1]["rows"], serde_json::json!([rows[0]]));
    let log =
        fs::read_to_string(scratch.0.join("cache/logs/cbm-daemon.log")).unwrap();
    assert!(log.lines().any(|line| {
        line.contains("search_code.degrees") && line.contains("backend=rust")
    }));
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
