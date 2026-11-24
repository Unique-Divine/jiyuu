//! Run the daemon's worker recovery against a deterministic test-only crash.
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
        let path = PathBuf::from(format!(
            "/tmp/mori-job-recovery-{}",
            std::process::id()
        ));
        let mut dirs = DirBuilder::new();
        dirs.mode(0o700).create(&path).expect("create fixture");
        for name in ["repo", "cache", "rt"] {
            dirs.create(path.join(name))
                .expect("create fixture directory");
        }
        let repo = path.join("repo");
        fs::write(
            repo.join("good.py"),
            "def stays_in_graph():\n    return 1\n",
        )
        .expect("write good source");
        fs::write(repo.join("crasher.py"), "def test_crash():\n    return 2\n")
            .expect("write crash fixture");
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

#[test]
#[ignore = "requires TEST_SEAMS=1 to enable the opt-in native crash fixture"]
fn default_rust_worker_recovery_preserves_the_partial_graph() {
    assert_eq!(option_env!("TEST_SEAMS"), Some("1"));

    let fixture = Fixture::new();
    let started = fixture
        .command()
        .env("CBM_LOG_LEVEL", "debug")
        .env("CBM_INDEX_MAX_RESTARTS", "2")
        .env("CBM_INDEX_WORKER_TIMEOUT_S", "30")
        .env("CBM_TEST_CRASH_ON", "crasher.py")
        .args(["daemon", "start"])
        .output()
        .expect("start the isolated daemon");
    assert!(started.status.success(), "{started:?}");
    let args =
        json!({"repo_path": fixture.0.join("repo"), "name": "job-recovery"});
    let indexed = fixture
        .command()
        .args(["cli", "--quiet", "index_repository", &args.to_string()])
        .output()
        .expect("run supervised index");
    assert!(indexed.status.success(), "{indexed:?}");
    let response: Value =
        serde_json::from_slice(&indexed.stdout).expect("index reply");
    assert_eq!(response["status"], "indexed", "{response}");
    let logs = fixture.0.join("cache/logs");
    let daemon_log =
        fs::read_to_string(logs.join("cbm-daemon.log")).expect("daemon log");
    assert!(
        daemon_log.lines().any(|line| {
            line.contains("msg=daemon.index.recovery_driver")
                && line.ends_with("implementation=rust")
        }),
        "{daemon_log}"
    );
    assert!(
        daemon_log.contains("msg=daemon.index.recovery_partial"),
        "{daemon_log}"
    );
    assert_eq!(
        daemon_log
            .matches("msg=daemon.index.file_quarantined")
            .count(),
        1
    );
    assert!(
        fs::read_dir(&logs).expect("recovery files").all(|entry| {
            !entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .starts_with(".daemon-index-")
        }),
        "temporary marker and quarantine files must be removed after recovery"
    );
    let db = Connection::open_with_flags(
        fixture.0.join("cache/job-recovery.db"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open the published graph");
    let mut statement = db
        .prepare(
            "SELECT file_path, name FROM nodes
             WHERE label = 'Function' AND file_path IN ('good.py', 'crasher.py')
             ORDER BY file_path, name",
        )
        .expect("find retained definitions");
    let names = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .expect("read definitions")
        .collect::<Result<Vec<_>, _>>()
        .expect("definition names");
    assert_eq!(names, [("good.py".to_owned(), "stays_in_graph".to_owned())]);
}
