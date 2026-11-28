//! Python import extraction on persisted IMPORTS and CALLS edges.
#![cfg(all(feature = "c-backend", unix))]

use std::fs::{self, DirBuilder};
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::Command;

use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let dir = PathBuf::from(format!("/tmp/mori-py-{}", std::process::id()));
        let mut dirs = DirBuilder::new();
        dirs.mode(0o700).create(&dir).expect("create fixture");
        for name in ["repo", "cache", "rt"] {
            dirs.create(dir.join(name))
                .expect("create fixture directory");
        }
        let fixture = Self(dir);
        for (path, source) in [
            ("alpha/__init__.py", ""),
            ("alpha/tools.py", "def alpha_tool():\n    return 1\n"),
            ("beta.py", "def beta_tool():\n    return 2\n"),
            ("pkg/__init__.py", ""),
            ("pkg/nested/__init__.py", ""),
            ("pkg/shared.py", "def parent_tool():\n    return 3\n"),
            ("pkg/nested/helper.py", "def local_tool():\n    return 4\n"),
            ("pkg/nested/extra.py", "def extra_tool():\n    return 5\n"),
            ("wildcards.py", "def wildcard_tool():\n    return 6\n"),
            (
                "pkg/nested/imports.py",
                concat!(
                    "import alpha.tools\n",
                    "import beta as beta_alias\n",
                    "from . import helper as sibling_alias\n",
                    "from ..shared import parent_tool as parent_alias\n",
                    "from .extra import extra_tool\n",
                    "from wildcards import *\n",
                    "\ndef use():\n",
                    "    return beta_alias.beta_tool() + parent_alias()\n",
                ),
            ),
            (
                "forms.py",
                concat!(
                    "import alpha.tools as deep_tools\n",
                    "import beta, wildcards\n",
                    "from pkg import (shared, nested)\n",
                    "from pkg.nested import helper as nested_helper, extra\n",
                    "try:\n    import runtime\nexcept ImportError:\n",
                    "    runtime = None\n",
                    "\ndef local():\n    import typed\n",
                    "    return typed.OnlyTyped()\n",
                ),
            ),
            (
                "pkg/nested/more.py",
                concat!(
                    "from .. import shared\n",
                    "from ..shared import *\n",
                    "from .helper import (local_tool, local_tool as again)\n",
                ),
            ),
            (
                "contracts.py",
                "class Contract:\n    def process_batch(self) -> None:\n        ...\n",
            ),
            (
                "trainer.py",
                concat!(
                    "from __future__ import annotations\n",
                    "from typing import TYPE_CHECKING\n\n",
                    "if TYPE_CHECKING:\n    from contracts import Contract\n\n",
                    "class Trainer:\n",
                    "    def __init__(self, strategies: Contract) -> None:\n",
                    "        self.strategies = strategies\n",
                ),
            ),
            (
                "loop.py",
                concat!(
                    "from __future__ import annotations\n",
                    "import typing\n\n",
                    "if typing.TYPE_CHECKING:\n    from trainer import Trainer\n\n",
                    "def run(trainer: Trainer) -> None:\n",
                    "    trainer.strategies.process_batch()\n\n",
                    "def bad():\n    return Trainer()\n",
                ),
            ),
            (
                "typed.py",
                "class OnlyTyped:\n    pass\nclass Rebound:\n    pass\n",
            ),
            ("runtime.py", "class Runtime:\n    pass\n"),
            (
                "shadow.py",
                concat!(
                    "from typing import TYPE_CHECKING\n",
                    "if TYPE_CHECKING:\n",
                    "    from typed import OnlyTyped as Kept, Rebound as Alias\n",
                    "else:\n",
                    "    try:\n",
                    "        from runtime import Runtime as Alias\n",
                    "    except ImportError:\n",
                    "        Alias = None\n",
                ),
            ),
        ] {
            let path = fixture.0.join("repo").join(path);
            fs::create_dir_all(path.parent().expect("source parent"))
                .expect("source directory");
            fs::write(path, source).expect("fixture source");
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
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.command().args(["daemon", "stop"]).output();
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Edge {
    kind: String,
    source_path: String,
    source_name: String,
    target_path: String,
    target_name: String,
    properties: String,
}

impl Edge {
    fn property(&self, name: &str) -> Value {
        let props: Value =
            serde_json::from_str(&self.properties).expect("edge properties");
        props[name].clone()
    }
}

fn index() -> (Vec<Edge>, String) {
    let fixture = Fixture::new();
    let args =
        json!({"repo_path": fixture.0.join("repo"), "name": "py-imports"});
    let output = fixture
        .command()
        .env("CBM_PROFILE", "1")
        .env("CBM_LOG_LEVEL", "debug")
        .args(["cli", "--quiet", "index_repository", &args.to_string()])
        .output()
        .expect("index the Python fixture");
    assert!(output.status.success(), "{output:?}");
    let response: Value =
        serde_json::from_slice(&output.stdout).expect("index reply");
    assert_eq!(response["status"], "indexed", "{response}");

    let db = Connection::open_with_flags(
        fixture.0.join("cache/py-imports.db"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open graph");
    let mut statement = db
        .prepare(
            "SELECT e.type, s.file_path, s.name, t.file_path, t.name, e.properties \
             FROM edges e JOIN nodes s ON s.id=e.source_id JOIN nodes t ON t.id=e.target_id \
             WHERE e.type IN ('IMPORTS','CALLS') \
               AND s.file_path IN ('pkg/nested/imports.py','loop.py','trainer.py','shadow.py', \
                 'forms.py','pkg/nested/more.py') \
             ORDER BY 1,2,3,4,5,6",
        )
        .expect("select import and call edges");
    let edges = statement
        .query_map([], |row| {
            Ok(Edge {
                kind: row.get(0)?,
                source_path: row.get(1)?,
                source_name: row.get(2)?,
                target_path: row.get(3)?,
                target_name: row.get(4)?,
                properties: row.get(5)?,
            })
        })
        .expect("read graph edges")
        .collect::<Result<_, _>>()
        .expect("typed graph rows");
    let logs = fs::read_dir(fixture.0.join("cache/logs"))
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
    (edges, logs)
}

#[test]
fn python_imports_keep_aliases_relative_paths_and_type_checking() {
    let (rust, rust_logs) = index();
    let marker = "msg=extract.python_imports implementation=rust";
    assert!(rust_logs.contains(marker), "{rust_logs}");

    for (target, alias) in [
        ("alpha", "alpha"),
        ("beta.py", "beta_alias"),
        ("pkg/nested/helper.py", "sibling_alias"),
        ("pkg/shared.py", "parent_alias"),
        ("pkg/nested/extra.py", "extra_tool"),
        ("wildcards.py", "wildcards"),
    ] {
        assert!(
            rust.iter().any(|edge| edge.kind == "IMPORTS"
                && edge.source_path == "pkg/nested/imports.py"
                && edge.target_path == target
                && edge.property("local_name") == alias),
            "missing import {alias} -> {target}: {rust:?}"
        );
    }
    let mut form_imports: Vec<_> = rust
        .iter()
        .filter(|edge| {
            edge.kind == "IMPORTS"
                && matches!(
                    edge.source_path.as_str(),
                    "forms.py" | "pkg/nested/more.py"
                )
        })
        .map(|edge| {
            let local = edge.property("local_name");
            format!(
                "{} -> {} as {}",
                edge.source_path,
                edge.target_path,
                local.as_str().unwrap_or_default()
            )
        })
        .collect();
    form_imports.sort();
    // Rows generated from the retired C extractor: it bound only the first
    // name of `import beta, wildcards` and skipped imports nested in try
    // blocks and functions.
    assert_eq!(
        form_imports,
        [
            "forms.py -> alpha as deep_tools",
            "forms.py -> beta.py as beta",
            "forms.py -> pkg as nested",
            "forms.py -> pkg as shared",
            "forms.py -> pkg/nested as extra",
            "forms.py -> pkg/nested as nested_helper",
            "pkg/nested/more.py -> pkg/nested/helper.py as again",
            "pkg/nested/more.py -> pkg/nested/helper.py as local_tool",
            "pkg/nested/more.py -> pkg/shared.py as shared",
        ],
        "{rust:?}"
    );
    let filtered_imports: Vec<_> = rust
        .iter()
        .filter(|edge| edge.kind == "IMPORTS" && edge.source_path == "shadow.py")
        .map(|edge| (edge.target_path.as_str(), edge.property("local_name")))
        .collect();
    assert_eq!(filtered_imports, [("typed.py", json!("Kept"))], "{rust:?}");
    assert!(
        rust.iter().any(|edge| edge.kind == "CALLS"
            && edge.source_path == "loop.py"
            && edge.source_name == "run"
            && edge.target_path == "contracts.py"
            && edge.target_name == "process_batch"
            && edge.property("strategy") == "lsp_method"),
        "the annotation-only import should resolve the parameter call: {rust:?}"
    );
    assert!(
        !rust.iter().any(|edge| edge.kind == "CALLS"
            && edge.source_path == "loop.py"
            && edge.source_name == "bad"
            && edge.target_name == "Trainer"
            && matches!(
                edge.property("strategy").as_str(),
                Some("lsp" | "import_map")
            )),
        "annotation-only Trainer was used as a runtime binding: {rust:?}"
    );
}
