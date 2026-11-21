//! Runs `mori` and the C `codebase-memory-mcp` from the same build directory
//! and checks that they behave the same. Needs feature `c-backend` and the
//! C reference binary (`MORI_C_REFERENCE=1`, set by `just check-linked`), so
//! a pure-Rust `cargo test` compiles this file to nothing.
#![cfg(feature = "c-backend")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
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
    run_in(bin, &Scratch::new(side), None, args)
}

fn run_in(
    bin: &Path,
    scratch: &Scratch,
    graphql: Option<&str>,
    args: &[&str],
) -> Output {
    let mut command = Command::new(bin);
    command
        .args(args)
        .current_dir("/")
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", std::env::var_os("HOME").unwrap_or_default())
        .env("CBM_CACHE_DIR", scratch.0.join("cache"))
        .env("CBM_RUNTIME_DIR", scratch.0.join("rt"));
    if let Some(planner) = graphql {
        command.env("MORI_IMPL_GRAPHQL", planner);
    }
    if args.contains(&"index_repository") {
        command.env("CBM_PROFILE", "1");
    }
    command.output().expect("run binary")
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
fn unknown_tool_error_matches_c() {
    assert_same(&["cli", "no_such_tool"]);
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
        let implementation = graphql.unwrap_or("c");
        let planner = format!("planner={implementation}");
        let writer = format!("writer={implementation}");
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
                fs::read_to_string(entry.path()).is_ok_and(|log| {
                    log.lines().any(|line| {
                        line.contains("msg=pass.route_nodes.graphql")
                            && line
                                .split_ascii_whitespace()
                                .any(|kv| kv == planner)
                            && line
                                .split_ascii_whitespace()
                                .any(|kv| kv == writer)
                    })
                })
            });
        assert!(
            logged_implementation,
            "{side} worker didn't report {planner} and {writer}"
        );
        let counts = (json_number(&text, "nodes"), json_number(&text, "edges"));
        assert!(counts.0.is_some_and(|n| n > 0), "{side}: {text}");
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
        results.push((counts, queries, graphql_rows(&scratch, &project)));
    }
    let (c, linked) = results.split_first().expect("standalone C result");
    assert_eq!(c.2.len(), 4, "expected one handler per GraphQL Route");
    assert!(c.2.iter().any(|row| row.contains("newValue")));
    for rs in linked {
        assert_eq!(c.0, rs.0, "node and edge counts differ");
        assert_eq!(c.2, rs.2, "published Route or HANDLES rows differ");
        for (c_out, rs_out) in c.1.iter().zip(&rs.1) {
            assert!(c_out.status.success(), "{c_out:?}");
            assert_eq!(
                (&c_out.status.code(), &c_out.stdout, &c_out.stderr),
                (&rs_out.status.code(), &rs_out.stdout, &rs_out.stderr),
                "query output differs"
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
