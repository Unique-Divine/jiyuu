//! Exercises the default Markdown path through the linked index worker.
#![cfg(all(feature = "c-backend", unix))]

use std::fs::{self, DirBuilder};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};

mod support;

const MORI: &str = env!("CARGO_BIN_EXE_mori");
const WITNESS: &str = "msg=extract.markdown_sections implementation=rust";

struct Fixture {
    root: PathBuf,
    bin: PathBuf,
}

impl Fixture {
    fn new(side: &str, bin: &Path) -> Self {
        let root =
            PathBuf::from(format!("/tmp/mori-md-{}-{side}", std::process::id()));
        let mut builder = DirBuilder::new();
        builder.mode(0o700);
        builder.create(&root).expect("create fixture");
        for name in ["repo/src", "cache", "rt"] {
            builder
                .recursive(true)
                .create(root.join(name))
                .expect("create fixture subdirectory");
        }
        Self {
            root,
            bin: bin.to_owned(),
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.bin);
        command
            .current_dir("/")
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", std::env::var_os("HOME").unwrap_or_default())
            .env("CBM_CACHE_DIR", self.root.join("cache"))
            .env("CBM_RUNTIME_DIR", self.root.join("rt"))
            .env("CBM_PROFILE", "1")
            .env("CBM_LOG_LEVEL", "debug");
        command
    }

    fn index(&self) -> (Graph, String) {
        let repo = self.root.join("repo");
        fs::write(
            repo.join("doc.md"),
            concat!(
                "Before any heading, see `src/ledger.py::before`.\n\n",
                "# A.Shared\n\n# A.Shared~2\n\n",
                "# A\n\n## Shared\n\nProvider prose. See `src/ledger.py::balance`.\n\n",
                "```mermaid\ngraph LR\n  Ledger --> Balance\n```\n\n",
                "## Trailing ###\n\nOnly trailing prose.\n\n",
                "B\n===\n\nShared\n------\n\nClient prose. See `src/ledger.py::post_entry`.\n\n",
                "> Setext café\n> second line\n> ---------\n>\n> Quoted prose. See `src/ledger.py::quoted`.\n\n",
                "# Repeat\n\n## Local\n\nSee `src/ledger.py::local_a`.\n\n",
                "## Local\n\nSee `src/ledger.py::local_b`.\n"
            ),
        )
        .expect("write Markdown fixture");
        fs::write(
            repo.join("src/ledger.py"),
            "def before(): pass\ndef balance(): pass\ndef post_entry(): pass\n\
             def local_a(): pass\ndef local_b(): pass\ndef quoted(): pass\n",
        )
        .expect("write reference targets");
        let args = json!({"repo_path": repo, "name": "markdown"});
        let output = self
            .command()
            .args(["cli", "--quiet", "index_repository", &args.to_string()])
            .output()
            .expect("index Markdown");
        assert!(output.status.success(), "{output:?}");
        let response: Value =
            serde_json::from_slice(&output.stdout).expect("index response");
        assert_eq!(response["status"], "indexed", "{response}");

        let db = Connection::open_with_flags(
            self.root.join("cache/markdown.db"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .expect("open graph");
        let sections = db
            .prepare(
                "SELECT name, qualified_name, start_line, end_line, \
                 json_extract(properties, '$.docstring') FROM nodes \
                 WHERE label = 'Section' ORDER BY qualified_name",
            )
            .expect("select sections")
            .query_map([], |row| {
                Ok(Section {
                    name: row.get(0)?,
                    qn: row.get(1)?,
                    start: row.get(2)?,
                    end: row.get(3)?,
                    body: row.get(4)?,
                })
            })
            .expect("read sections")
            .collect::<Result<_, _>>()
            .expect("section rows");
        let references = db
            .prepare(
                "SELECT src.label, src.qualified_name, dst.qualified_name FROM edges e \
                 JOIN nodes src ON src.id = e.source_id JOIN nodes dst ON dst.id = e.target_id \
                 WHERE e.type = 'MENTIONS' AND src.file_path = 'doc.md' \
                 ORDER BY src.label, src.qualified_name, dst.qualified_name",
            )
            .expect("select references")
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("read references")
            .collect::<Result<_, _>>()
            .expect("reference rows");
        let logs = fs::read_dir(self.root.join("cache/logs"))
            .expect("worker logs")
            .map(|entry| entry.expect("worker log"))
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".worker-log-")
            })
            .map(|entry| fs::read_to_string(entry.path()).expect("read log"))
            .collect::<String>();
        (
            Graph {
                sections,
                references,
            },
            logs,
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.command().args(["daemon", "stop"]).output();
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Section {
    name: String,
    qn: String,
    start: u32,
    end: u32,
    body: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct Graph {
    sections: Vec<Section>,
    references: Vec<(String, String, String)>,
}

#[test]
fn markdown_sections_use_rust_in_the_default_worker() {
    let fixture = Fixture::new("rust", Path::new(MORI));
    let (graph, logs) = fixture.index();
    assert!(logs.contains(WITNESS), "{logs}");
    let section = |suffix: &str| {
        let qn = format!("markdown.doc.{suffix}");
        graph
            .sections
            .iter()
            .find(|row| row.qn == qn)
            .unwrap_or_else(|| panic!("missing {qn}: {graph:?}"))
    };
    assert_eq!(section("A.Shared").name, "A.Shared");
    assert_eq!(section("A.Shared~2").name, "A.Shared~2");
    let provider = section("A.Shared~3").body.as_deref().unwrap();
    assert!(
        provider.contains("Provider prose")
            && provider.contains("Ledger Balance"),
        "{provider}"
    );
    assert!(
        !provider.contains("-->") && !provider.contains("Only trailing prose"),
        "{provider}"
    );
    assert_eq!(
        section("Trailing-###").body.as_deref(),
        Some("Only trailing prose.")
    );
    assert_eq!(section("Trailing-###").name, "Trailing ###");
    assert!(
        section("B.Shared")
            .body
            .as_deref()
            .unwrap()
            .starts_with("Client prose.")
    );
    assert_eq!(section("Setext-café-second-line").name, "Setext café");
    assert!(
        section("Setext-café-second-line")
            .body
            .as_deref()
            .unwrap()
            .contains("Quoted prose.")
    );
    assert_eq!(
        graph
            .sections
            .iter()
            .filter(|row| row.name == "Local")
            .count(),
        1
    );

    for (label, section, target) in [
        ("File", "", "before"),
        ("Section", ".A.Shared~3", "balance"),
        ("Section", ".B.Shared", "post_entry"),
        ("Section", ".Setext-café-second-line", "quoted"),
        ("File", "", "local_a"),
        ("File", "", "local_b"),
    ] {
        assert!(
            graph.references.iter().any(|(src_label, src, dst)| {
                src_label == label
                    && (section.is_empty() || src.ends_with(section))
                    && dst.ends_with(&format!(".ledger.{target}"))
            }),
            "missing {label} {section} -> {target}: {graph:?}"
        );
    }

    if let Some(reference) = support::c_reference_bin() {
        let reference = Fixture::new("c", &reference);
        let (expected, reference_logs) = reference.index();
        assert!(!reference_logs.contains(WITNESS), "{reference_logs}");
        assert_eq!(graph, expected);
    }
}
