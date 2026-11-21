//! Check the hidden exporter against a database with generated and FTS columns.
#![cfg(feature = "c-backend")]

use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};

const MORI: &str = env!("CARGO_BIN_EXE_mori");

fn fixture(path: &Path) {
    let script = r#"
import sqlite3
import sys
with sqlite3.connect(sys.argv[1]) as db:
    db.executescript("""
        CREATE TABLE projects (name TEXT PRIMARY KEY, root_path TEXT NOT NULL);
        CREATE TABLE nodes (id INTEGER PRIMARY KEY, project TEXT, label TEXT,
            qualified_name TEXT, file_path TEXT, properties TEXT, start_line INTEGER);
        CREATE TABLE edges (id INTEGER PRIMARY KEY, project TEXT, source_id INTEGER,
            target_id INTEGER, type TEXT, properties TEXT,
            local_name_gen TEXT GENERATED ALWAYS AS (json_extract(properties, '$.local_name')));
        CREATE TABLE file_hashes (project TEXT, rel_path TEXT, hash BLOB,
            PRIMARY KEY (project, rel_path));
        CREATE VIRTUAL TABLE nodes_fts USING fts5(body);
        INSERT INTO projects VALUES ('demo', '/example/demo/');
        INSERT INTO nodes VALUES (9, 'demo', 'Function', 'demo.z', 'src/z', '{"x":0.12345678}', NULL);
        INSERT INTO nodes VALUES (29, 'demo', 'File', 'demo.a', 'src\\a', '{{"bad"}', 1);
        INSERT INTO edges (id, project, source_id, target_id, type, properties)
            VALUES (21, 'demo', 9, 29, 'IMPORTS', '{"local_name":"a","root":"/example/demo/src/a"}');
        INSERT INTO file_hashes VALUES ('demo', 'src/a', x'616263');
        INSERT INTO nodes_fts VALUES ('one two'), ('one');
        UPDATE nodes SET properties = CAST(x'41E381' AS TEXT) WHERE id = 29;
    """)
"#;
    let output = Command::new("python3")
        .args(["-c", script])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "fixture: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn export_is_read_only_and_normalizes_graph_rows() {
    let dir =
        std::env::temp_dir().join(format!("mori-export-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let db = dir.join("graph with spaces.db");
    fixture(&db);
    let before = fs::read(&db).unwrap();

    let run = |extra: &[&str]| {
        Command::new(MORI)
            .args(["dev", "export", "--db"])
            .arg(&db)
            .args(extra)
            .output()
            .unwrap()
    };
    let output = run(&[]);
    assert!(
        output.status.success(),
        "export: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, run(&[]).stdout, "export order changed");
    assert_eq!(fs::read(&db).unwrap(), before, "export modified database");

    let rows: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let nodes: Vec<_> =
        rows.iter().filter(|row| row["table"] == "nodes").collect();
    assert_eq!(nodes[0]["key"], json!(["a", "File", "src/a"]));
    assert_eq!(nodes[0]["properties"]["properties"], "A\\xe3\\x81");
    assert_eq!(nodes[1]["key"], json!(["z", "Function", "src/z"]));
    assert_eq!(nodes[1]["properties"]["properties"]["x"], 0.123457);
    let edge = rows.iter().find(|row| row["table"] == "edges").unwrap();
    assert_eq!(edge["key"], json!(["z", "a", "IMPORTS", "a"]));
    assert_eq!(edge["properties"]["properties"]["root"], "{{root}}/src/a");
    let hash = rows
        .iter()
        .find(|row| row["table"] == "file_hashes" && row["kind"] == "row")
        .unwrap();
    assert_eq!(hash["key"], json!(["{{project}}", "src/a"]));
    assert_eq!(hash["properties"]["hash"], "blob:sha256:ba7816bf8f01cfea:3");
    assert!(
        rows.contains(
            &json!({"kind": "fts", "term": "one", "doc": 2, "count": 2})
        )
    );

    let only = run(&["--tables", "nodes"]);
    assert!(only.status.success());
    assert_eq!(String::from_utf8(only.stdout).unwrap().lines().count(), 2);
    assert!(!run(&["--tables", "unknown"]).status.success());

    fs::remove_dir_all(dir).unwrap();
    let output = run(&[]);
    assert!(
        !output.status.success(),
        "export created a missing database"
    );
    assert!(!db.exists());
}
