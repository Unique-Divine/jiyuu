//! Graph projections of the legacy LSP occurrence tests. The graph retains call
//! lines and collapses repeated calls to the same target. The fixture table keeps
//! the original raw byte spans so the occurrence tests can also reuse the inputs.
#![cfg(all(feature = "c-backend", unix))]

mod support;

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const PROJECT: &str = "test";
const EXPECTED: &str =
    include_str!("../../tests/fixtures/resolver/expected.tsv");

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let output = Command::new("mktemp")
            .args(["-d", "/tmp/mori-resolve-XXXXXXXX"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        Self(PathBuf::from(OsStr::from_bytes(
            output.stdout.strip_suffix(b"\n").unwrap(),
        )))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Call {
    caller_file: String,
    caller: String,
    target_file: String,
    target: String,
    target_label: String,
    callee: String,
}

fn write_fixtures(
    repo: &Path,
) -> (BTreeSet<&'static str>, BTreeMap<Call, Vec<usize>>) {
    let fixtures =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/resolver");
    let mut paths = BTreeSet::new();
    let mut calls = BTreeMap::new();
    let mut write = |path: &'static str| {
        let source = fs::read(fixtures.join(path)).unwrap();
        if paths.insert(path) {
            let destination = repo.join(path);
            fs::create_dir_all(destination.parent().unwrap()).unwrap();
            fs::write(destination, &source).unwrap();
        }
        source
    };
    for row in EXPECTED.lines().filter(|row| !row.starts_with('#')) {
        let columns: Vec<_> = row.split('\t').collect();
        // A row with only a path adds a file that no expected call starts in.
        if let [path] = columns[..] {
            write(path);
            continue;
        }
        let [path, caller, target, target_label, spans, text, callee] =
            columns[..]
        else {
            panic!("invalid resolver fixture row {row:?}");
        };
        let source = write(path);
        let module = path.split_once('/').unwrap().0;
        let qualified = |name: &str| {
            if name.is_empty() {
                format!("{PROJECT}.{module}.main")
            } else {
                format!("{PROJECT}.{module}.main.{name}")
            }
        };
        let lines = spans
            .split(',')
            .map(|span| {
                let (start, end) = span.split_once(':').unwrap();
                let start = start.parse::<usize>().unwrap();
                let end = end.parse::<usize>().unwrap();
                assert!(start < end, "{path}: {span}");
                assert_eq!(
                    source.get(start..end),
                    Some(text.as_bytes()),
                    "{path}: {span}"
                );
                1 + source[..start]
                    .iter()
                    .filter(|&&byte| byte == b'\n')
                    .count()
            })
            .collect();
        // `file.py:name` names a target in another file of the fixture.
        let (target_file, target) = match target.split_once(':') {
            Some((file, name)) => (
                format!("{module}/{file}"),
                format!(
                    "{PROJECT}.{module}.{}.{name}",
                    file.trim_end_matches(".py").replace('/', ".")
                ),
            ),
            None => (path.to_owned(), qualified(target)),
        };
        let call = Call {
            caller_file: path.to_owned(),
            caller: qualified(caller),
            target_file,
            target,
            target_label: target_label.to_owned(),
            callee: callee.to_owned(),
        };
        assert!(calls.insert(call, lines).is_none(), "duplicate row {row:?}");
    }
    (paths, calls)
}

fn index(bin: &Path, repo: &Path, run: &Path, mode_log: &str) -> Connection {
    let cache = run.join("cache");
    let runtime = run.join("rt");
    let mut dirs = fs::DirBuilder::new();
    dirs.recursive(true).mode(0o700);
    dirs.create(&cache).unwrap();
    dirs.create(&runtime).unwrap();
    let command = || {
        let mut command = Command::new(bin);
        command
            .current_dir("/")
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", std::env::var_os("HOME").unwrap_or_default())
            .env("CBM_CACHE_DIR", &cache)
            .env("CBM_RUNTIME_DIR", &runtime)
            .env("CBM_WORKERS", "2")
            .env("CBM_LOG_LEVEL", "info")
            .env("CBM_PROFILE", "1");
        command
    };
    let indexed = command()
        .args(["cli", "--quiet", "index_repository", "--repo_path"])
        .arg(repo)
        .args(["--name", PROJECT, "--mode", "full"])
        .output()
        .unwrap();
    let stopped = command().args(["daemon", "stop"]).output().unwrap();
    assert!(stopped.status.success(), "{stopped:?}");
    assert!(indexed.status.success(), "{indexed:?}");
    let reply: Value = serde_json::from_slice(&indexed.stdout)
        .unwrap_or_else(|error| panic!("{error}: {indexed:?}"));
    assert_eq!(reply["status"], "indexed", "{reply}");
    assert_eq!(reply["skipped_count"], 0, "{reply}");

    let logs: String = fs::read_dir(cache.join("logs"))
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".worker-log-")
        })
        .map(|entry| fs::read_to_string(entry.path()).unwrap())
        .collect();
    assert!(logs.contains(mode_log), "{}: {logs}", bin.display());
    Connection::open_with_flags(
        cache.join(format!("{PROJECT}.db")),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
}

fn assert_calls(
    db: &Connection,
    paths: &BTreeSet<&str>,
    expected: &BTreeMap<Call, Vec<usize>>,
    parallel: bool,
) {
    let mut stmt = db
        .prepare(
            "SELECT caller.file_path, caller.qualified_name, target.file_path,
                    target.qualified_name, target.label,
                    json_extract(edge.properties, '$.callee'),
                    json_extract(edge.properties, '$.line')
             FROM edges AS edge
             JOIN nodes AS caller ON caller.id = edge.source_id
             JOIN nodes AS target ON target.id = edge.target_id
             WHERE edge.type = 'CALLS'",
        )
        .unwrap();
    // Both baselines emit the Perl fixture edges in sequential indexing only.
    // Keep the Perl inputs in both runs so the parallel graph still uses the
    // same sources, and leave parallel Perl resolution outside these assertions.
    let covered = |path: &str| {
        paths.contains(path) && !(parallel && path.ends_with(".pl"))
    };
    let mut actual = Vec::new();
    let mut rows = stmt.query([]).unwrap();
    while let Some(row) = rows.next().unwrap() {
        let caller_file: String = row.get(0).unwrap();
        if covered(&caller_file) {
            let call = Call {
                caller_file,
                caller: row.get(1).unwrap(),
                target_file: row.get(2).unwrap(),
                target: row.get(3).unwrap(),
                target_label: row.get(4).unwrap(),
                callee: row.get(5).unwrap(),
            };
            let line = usize::try_from(row.get::<_, i64>(6).unwrap()).unwrap();
            if let Some(lines) = expected.get(&call) {
                assert!(
                    lines.contains(&line),
                    "{call:?}: line {line}, expected {lines:?}"
                );
            }
            actual.push(call);
        }
    }
    actual.sort_unstable();
    assert_eq!(
        actual.iter().collect::<Vec<_>>(),
        expected
            .keys()
            .filter(|call| covered(&call.caller_file))
            .collect::<Vec<_>>(),
    );
}

#[test]
fn resolver_occurrences_reach_the_graph() {
    let scratch = Scratch::new();
    let repo = scratch.0.join("repo");
    let (paths, expected) = write_fixtures(&repo);
    let mori = Path::new(env!("CARGO_BIN_EXE_mori"));
    let reference = support::c_reference_bin();

    let serial_log =
        format!("msg=pipeline.mode mode=sequential files={}", paths.len());
    let parallel_log = "msg=pipeline.mode mode=parallel workers=2 files=51";
    for (label, parallel, mode_log) in [
        ("serial", false, serial_log.as_str()),
        ("parallel", true, parallel_log),
    ] {
        if parallel {
            for n in paths.len()..51 {
                fs::write(
                    repo.join(format!("padding_{n}.py")),
                    format!("def padding_{n}():\n    pass\n"),
                )
                .unwrap();
            }
        }
        for (side, bin) in
            [("current", Some(mori)), ("reference", reference.as_deref())]
        {
            if let Some(bin) = bin {
                let db = index(
                    bin,
                    &repo,
                    &scratch.0.join(format!("{side}-{label}")),
                    mode_log,
                );
                assert_calls(&db, &paths, &expected, parallel);
            }
        }
    }
}
