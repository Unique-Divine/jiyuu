//! TypeScript static imports through the published indexer and graph store.
#![cfg(all(feature = "c-backend", unix))]

use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};

const MORI: &str = env!("CARGO_BIN_EXE_mori");
const PROJECT: &str = "ts-imports";

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct IndexRun {
    root: PathBuf,
    implementation: &'static str,
}

impl IndexRun {
    fn new(root: &Path, implementation: &'static str) -> Self {
        let root = root.join(implementation);
        let mut dirs = fs::DirBuilder::new();
        dirs.recursive(true).mode(0o700);
        dirs.create(root.join("cache")).unwrap();
        dirs.create(root.join("rt")).unwrap();
        Self {
            root,
            implementation,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(MORI);
        command
            .current_dir("/")
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", std::env::var_os("HOME").unwrap_or_default())
            .env("CBM_CACHE_DIR", self.root.join("cache"))
            .env("CBM_RUNTIME_DIR", self.root.join("rt"))
            .env("CBM_PROFILE", "1")
            .env("CBM_LOG_LEVEL", "debug")
            .env("CBM_WORKERS", "1")
            .env("MORI_IMPL_TS_STATIC_IMPORTS", self.implementation);
        command
    }

    fn index(&self, repo: &Path) {
        let args = json!({"repo_path": repo, "name": PROJECT});
        let output = self
            .command()
            .args(["cli", "--quiet", "index_repository", &args.to_string()])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(reply["status"], "indexed", "{reply}");
    }

    fn db_path(&self) -> PathBuf {
        self.root.join("cache").join(format!("{PROJECT}.db"))
    }

    fn graph(&self) -> Vec<u8> {
        let output = self
            .command()
            .args([
                "dev",
                "export",
                "--tables",
                "nodes,edges,file_hashes",
                "--db",
            ])
            .arg(self.db_path())
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        output.stdout
    }

    fn worker_markers(&self) -> Vec<String> {
        let mut markers = Vec::new();
        for entry in fs::read_dir(self.root.join("cache/logs")).unwrap() {
            let entry = entry.unwrap();
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(".worker-log-")
            {
                let text = fs::read_to_string(entry.path()).unwrap();
                markers.extend(
                    text.lines()
                        .filter(|line| line.contains("msg=extract.ts_static_import implementation=rust"))
                        .map(str::to_owned),
                );
            }
        }
        markers
    }
}

impl Drop for IndexRun {
    fn drop(&mut self) {
        let _ = self.command().args(["daemon", "stop"]).output();
    }
}

#[test]
fn static_ts_imports_publish_the_native_graph() {
    let temp = Command::new("mktemp")
        .args(["-d", "/tmp/mori-ts-imports-XXXXXXXX"])
        .output()
        .unwrap();
    assert!(temp.status.success(), "{temp:?}");
    let scratch = Scratch(PathBuf::from(std::ffi::OsStr::from_bytes(
        temp.stdout.strip_suffix(b"\n").unwrap(),
    )));
    let repo = scratch.0.join("repo");
    fs::create_dir_all(repo.join("src")).unwrap();

    for (file, text) in [
        (
            "app.ts",
            r#"import makeDefault, { realExport as renamed, direct } from "./library";
import * as toolkit from './toolkit';
import './setup';
import type { Input } from './input';
import {} from './empty';
import '';
const legacy = require('./legacy');
export function invoke(): number {
    return makeDefault() + renamed(3) + direct() + toolkit.extra() + legacy.previous();
}
export function describe(x: Input): number { return x.value; }
export async function loadLater() { return import('./deferred'); }
export { RealBadge } from './components';
"#,
        ),
        (
            "view.tsx",
            r#"import { RealBadge as Badge } from "./components";
import './view_setup';
export function render() { return <Badge />; }
export function callBadge() { return Badge(); }
"#,
        ),
        (
            "library.ts",
            r#"export default function defaultFromLibrary(): number { return 1; }
export function realExport(x: number): number { return x + 1; }
export function direct(): number { return 2; }
"#,
        ),
        (
            "components.tsx",
            "export function RealBadge() { return <span />; }\n",
        ),
        ("empty.ts", "export const emptyValue = 1;\n"),
        ("setup.ts", "export const setupValue = 1;\n"),
        ("view_setup.ts", "export const viewSetupValue = 1;\n"),
        ("input.ts", "export interface Input { value: number; }\n"),
        (
            "toolkit.ts",
            "export function extra(): number { return 1; }\n",
        ),
        (
            "legacy.ts",
            "export function previous(): number { return 1; }\n",
        ),
        (
            "deferred.ts",
            "export function later(): number { return 1; }\n",
        ),
        (
            "control.js",
            "import { direct } from './library'; export function jsControl() { return direct(); }\n",
        ),
        (
            "control.ets",
            "import { direct } from './library'; export function arkControl(): number { return direct(); }\n",
        ),
        (
            "embedded.vue",
            "<script lang=\"ts\">\nimport { direct } from './library';\nexport function embeddedControl(): number { return direct(); }\n</script>\n",
        ),
    ] {
        fs::write(repo.join("src").join(file), text).unwrap();
    }

    let native = IndexRun::new(&scratch.0, "c");
    native.index(&repo);
    let rust = IndexRun::new(&scratch.0, "rust");
    rust.index(&repo);
    assert_eq!(rust.graph(), native.graph(), "persistent graph differs");

    let db = Connection::open_with_flags(
        rust.db_path(),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let import_exists = |source: &str, target: &str, local: &str| -> bool {
        db.query_row(
            "SELECT EXISTS(SELECT 1 FROM edges e
               JOIN nodes s ON s.id=e.source_id JOIN nodes t ON t.id=e.target_id
               WHERE e.type='IMPORTS' AND s.file_path=?1 AND t.file_path=?2
                 AND json_extract(e.properties,'$.local_name')=?3)",
            [source, target, local],
            |row| row.get(0),
        )
        .unwrap()
    };
    assert!(import_exists("src/app.ts", "src/toolkit.ts", "toolkit"));
    assert!(import_exists("src/app.ts", "src/setup.ts", "setup"));
    assert!(import_exists(
        "src/view.tsx",
        "src/view_setup.ts",
        "view_setup"
    ));
    let aliased_call: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM edges e
           JOIN nodes s ON s.id=e.source_id JOIN nodes t ON t.id=e.target_id
           WHERE e.type='CALLS' AND s.name='invoke'
             AND t.file_path='src/library.ts' AND t.name='realExport')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(aliased_call, "named import alias lost its exported member");

    assert!(native.worker_markers().is_empty());
    let markers = rust.worker_markers();
    for file in ["src/app.ts", "src/view.tsx"] {
        assert!(
            markers
                .iter()
                .any(|line| line.ends_with(&format!("file={file}"))),
            "{markers:?}"
        );
    }
    assert!(
        markers.iter().all(|line| line.ends_with("file=src/app.ts")
            || line.ends_with("file=src/view.tsx")),
        "Rust handled another language or embedded script: {markers:?}"
    );
}
