//! Compares Markdown heading identities through the linked index worker.
#![cfg(all(feature = "c-backend", unix))]

use std::fs::{self, DirBuilder};
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::Command;

use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};

struct Fixture(PathBuf);

impl Fixture {
    fn new(backend: &str) -> Self {
        let path = PathBuf::from(format!(
            "/tmp/mori-md-{}-{backend}",
            std::process::id()
        ));
        let mut builder = DirBuilder::new();
        builder.mode(0o700);
        builder.create(&path).expect("create fixture");
        for name in ["repo", "cache", "rt"] {
            builder
                .create(path.join(name))
                .expect("create fixture subdirectory");
        }
        Self(path)
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mori"));
        command
            .current_dir("/")
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", std::env::var_os("HOME").unwrap_or_default())
            .env("CBM_CACHE_DIR", self.0.join("cache"))
            .env("CBM_RUNTIME_DIR", self.0.join("rt"))
            .env("CBM_PROFILE", "1")
            .env("CBM_LOG_LEVEL", "debug");
        command
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.command().args(["daemon", "stop"]).output();
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn markdown_heading_extraction_matches_c() {
    fn index(backend: &str) -> (Vec<(String, String, String)>, String) {
        let fixture = Fixture::new(backend);
        let repo = fixture.0.join("repo");
        fs::write(
            repo.join("doc.md"),
            concat!(
                "# Provider\n\n## Shared ###\n\nProvider prose.\n\n",
                "# Client\n\n## Shared\n\nClient prose.\n\n",
                "> Setext café\n> second line\n> ---------\n\n"
            ),
        )
        .expect("write Markdown fixture");
        let args = json!({"repo_path": repo, "name": "markdown"});
        let output = fixture
            .command()
            .env("MORI_IMPL_MARKDOWN_EXTRACT", backend)
            .args(["cli", "--quiet", "index_repository", &args.to_string()])
            .output()
            .expect("index Markdown");
        assert!(output.status.success(), "{output:?}");
        let response: Value =
            serde_json::from_slice(&output.stdout).expect("index response");
        assert_eq!(response["status"], "indexed", "{response}");

        let db = Connection::open_with_flags(
            fixture.0.join("cache/markdown.db"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .expect("open graph");
        let mut statement = db
            .prepare(
                "SELECT name, qualified_name, properties FROM nodes \
                 WHERE label = 'Section' ORDER BY qualified_name",
            )
            .expect("select sections");
        let sections = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("read sections")
            .collect::<Result<_, _>>()
            .expect("section rows");
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
        (sections, logs)
    }

    let (native, native_logs) = index("c");
    let (rust, rust_logs) = index("rust");
    assert_eq!(rust, native);
    assert_eq!(
        rust.iter().filter(|row| row.0 == "Shared").count(),
        2,
        "{rust:?}"
    );
    assert!(
        rust.iter().any(|row| row.0.contains("Setext café")),
        "{rust:?}"
    );
    let marker = "msg=extract.markdown_heading implementation=rust";
    assert!(!native_logs.contains(marker), "{native_logs}");
    assert!(rust_logs.contains(marker), "{rust_logs}");
}
