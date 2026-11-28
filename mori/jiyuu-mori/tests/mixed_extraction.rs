//! Compare newly ported extraction stages in one graph with a retained C file.
#![cfg(all(feature = "c-backend", unix))]

use std::fs::{self, DirBuilder};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};

const SELECTORS: &[(&str, &str)] = &[
    ("MORI_IMPL_GRAPHQL_EXTRACT", "extract.graphql_field_spans"),
    ("MORI_IMPL_TS_STATIC_IMPORTS", "extract.ts_static_import"),
    ("MORI_IMPL_RUST_IMPORTS", "extract.rust_imports"),
];

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root =
            PathBuf::from(format!("/tmp/mori-mixed-{}", std::process::id()));
        DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .expect("create fixture root");
        let fixture = Self(root);
        fs::create_dir(fixture.0.join("repo")).expect("create source directory");
        fs::create_dir(fixture.0.join("repo/golib")).expect("create Go package");
        for (name, source) in [
            (
                "schema.graphqls",
                "schema { query: Query }\ntype Query { title: String }\n",
            ),
            (
                "guide.md",
                "# Guide\n\nSee [the Go entry](main.go) and `main.go`.\n",
            ),
            ("go.mod", "module example.test/mixed\n\ngo 1.22\n"),
            (
                "golib/helper.go",
                "package golib\n\nfunc Describe() string { return \"mixed\" }\n",
            ),
            (
                "main.go",
                "package mixed\n\nimport g \"example.test/mixed/golib\"\n\nfunc GoMixed() string { return g.Describe() }\n",
            ),
            (
                "ts_helper.ts",
                "export function describe() { return 'mixed'; }\n",
            ),
            (
                "entry.ts",
                "import { describe as tsDescribe } from './ts_helper';\nexport function fromTs() { return tsDescribe(); }\n",
            ),
            (
                "entry.tsx",
                "import { describe } from './ts_helper';\nexport function Panel() { return <span>{describe()}</span>; }\n",
            ),
            ("py_helper.py", "def describe():\n    return 'mixed'\n"),
            (
                "entry.py",
                "from py_helper import describe as py_describe\n\ndef from_python():\n    return py_describe()\n",
            ),
            (
                "rust_helper.rs",
                "pub fn describe() -> &'static str { \"mixed\" }\n",
            ),
            (
                "lib.rs",
                "mod rust_helper;\nuse crate::rust_helper::{describe as rust_describe};\npub fn from_rust() -> &'static str { rust_describe() }\n",
            ),
            ("retained.c", "int retained_native(void) { return 42; }\n"),
        ] {
            fs::write(fixture.0.join("repo").join(name), source)
                .expect("write source");
        }
        fixture
    }

    fn command(&self, side: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mori"));
        command
            .current_dir("/")
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", std::env::var_os("HOME").unwrap_or_default())
            .env("CBM_CACHE_DIR", self.0.join(side).join("cache"))
            .env("CBM_RUNTIME_DIR", self.0.join(side).join("rt"))
            .env("CBM_PROFILE", "1")
            .env("CBM_LOG_LEVEL", "debug");
        command
    }

    fn index(&self, side: &str) -> (GraphRows, String) {
        for dir in [
            side.to_owned(),
            format!("{side}/cache"),
            format!("{side}/rt"),
        ] {
            DirBuilder::new()
                .mode(0o700)
                .create(self.0.join(&dir))
                .expect("create isolated runtime paths");
        }
        let mut command = self.command(side);
        for (selector, _) in SELECTORS {
            command.env(selector, side);
        }
        let arguments =
            json!({"repo_path": self.0.join("repo"), "name": "mixed"});
        let output = command
            .args(["cli", "--quiet", "index_repository", &arguments.to_string()])
            .output()
            .expect("index mixed fixture");
        assert!(output.status.success(), "{output:?}");
        let response: Value =
            serde_json::from_slice(&output.stdout).expect("index response");
        assert_eq!(response["status"], "indexed", "{response}");
        let cache = self.0.join(side).join("cache");
        let graph = GraphRows::read(&cache.join("mixed.db"));
        let logs = fs::read_dir(cache.join("logs"))
            .expect("worker log directory")
            .map(|entry| entry.expect("worker log entry"))
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".worker-log-")
            })
            .map(|entry| fs::read_to_string(entry.path()).expect("worker log"))
            .collect::<String>();
        (graph, logs)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for side in ["c", "rust"] {
            if self.0.join(side).join("rt").is_dir() {
                let _ = self.command(side).args(["daemon", "stop"]).output();
            }
        }
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug, PartialEq, Eq)]
struct GraphRows {
    symbols: Vec<String>,
    edges: Vec<String>,
}

impl GraphRows {
    fn read(path: &Path) -> Self {
        let db =
            Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .expect("open graph");
        Self {
            symbols: query_rows(
                &db,
                "SELECT json_array(label, name, qualified_name, file_path, start_line, end_line, properties) \
                 AS row FROM nodes ORDER BY row",
            ),
            edges: query_rows(
                &db,
                "SELECT json_array(src.qualified_name, dst.qualified_name, edge.type, edge.properties) \
                 AS row FROM edges AS edge JOIN nodes AS src ON src.id = edge.source_id \
                 JOIN nodes AS dst ON dst.id = edge.target_id ORDER BY row",
            ),
        }
    }
}

fn query_rows(db: &Connection, sql: &str) -> Vec<String> {
    db.prepare(sql)
        .expect("prepare graph comparison")
        .query_map([], |row| row.get(0))
        .expect("read graph rows")
        .collect::<Result<_, _>>()
        .expect("decode graph rows")
}

#[test]
fn mixed_language_extraction_matches_native_graph_rows() {
    let fixture = Fixture::new();
    let (native, native_logs) = fixture.index("c");
    let (rust, rust_logs) = fixture.index("rust");
    assert!(!rust.symbols.is_empty() && !rust.edges.is_empty());
    assert_eq!(rust, native);
    assert!(
        rust.symbols
            .iter()
            .any(|row| row.contains("\"retained_native\""))
    );
    let markdown = "msg=extract.markdown_sections implementation=rust";
    assert!(native_logs.contains(markdown), "{native_logs}");
    assert!(rust_logs.contains(markdown), "{rust_logs}");
    let go_marker = "msg=extract.go_imports implementation=rust";
    assert!(native_logs.contains(go_marker) && rust_logs.contains(go_marker));
    let python = "msg=extract.python_imports implementation=rust";
    assert!(native_logs.contains(python), "{native_logs}");
    assert!(rust_logs.contains(python), "{rust_logs}");
    for (_, event) in SELECTORS {
        let marker = format!("msg={event} implementation=rust");
        assert!(
            !native_logs.contains(&marker),
            "unexpected native witness {marker}"
        );
        assert!(rust_logs.contains(&marker), "missing Rust witness {marker}");
    }
}
