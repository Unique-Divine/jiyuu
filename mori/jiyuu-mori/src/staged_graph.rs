//! Write resolved graph records to a private staging database.

use rusqlite::{
    Connection, params,
    types::{ToSql, ToSqlOutput, ValueRef},
};

struct SqlText<'a>(&'a [u8]);

impl ToSql for SqlText<'_> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Borrowed(ValueRef::Text(self.0)))
    }
}

struct ProjectRecord<'a> {
    name: &'a [u8],
    indexed_at: &'a [u8],
    root_path: &'a [u8],
}

#[derive(Clone, Copy)]
struct NodeRecord<'a> {
    id: i64,
    label: &'a [u8],
    name: &'a [u8],
    qualified_name: &'a [u8],
    file_path: &'a [u8],
    start_line: i32,
    end_line: i32,
    properties: &'a [u8],
}

#[derive(Clone, Copy)]
struct EdgeRecord<'a> {
    id: i64,
    source_id: i64,
    target_id: i64,
    kind: &'a [u8],
    properties: &'a [u8],
}

/// Insert the graph into an empty stage initialized with Mori's store schema.
/// IDs are final. Edges refer to those IDs. Keep graph records and generated
/// edge keys in one transaction so a failed insert leaves no partial graph.
fn write_graph_records<'a>(
    stage: &mut Connection,
    project: ProjectRecord<'a>,
    nodes: impl IntoIterator<Item = NodeRecord<'a>>,
    edges: impl IntoIterator<Item = EdgeRecord<'a>>,
) -> rusqlite::Result<()> {
    stage.pragma_update(None, "foreign_keys", true)?;
    let tx = stage.transaction()?;
    let project_name = SqlText(project.name);
    tx.execute(
        "INSERT INTO projects (name, indexed_at, root_path) VALUES (?1, ?2, ?3)",
        params![
            project_name,
            SqlText(project.indexed_at),
            SqlText(project.root_path)
        ],
    )?;
    {
        let mut insert_node = tx.prepare(
            "INSERT INTO nodes (id, project, label, name, qualified_name, file_path,\
                                    start_line, end_line, properties)\
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )?;
        for node in nodes {
            insert_node.execute(params![
                node.id,
                project_name,
                SqlText(node.label),
                SqlText(node.name),
                SqlText(node.qualified_name),
                SqlText(node.file_path),
                node.start_line,
                node.end_line,
                SqlText(node.properties),
            ])?;
        }

        let mut insert_edge = tx.prepare(
            "INSERT INTO edges (id, project, source_id, target_id, type, properties)\
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for edge in edges {
            insert_edge.execute(params![
                edge.id,
                project_name,
                edge.source_id,
                edge.target_id,
                SqlText(edge.kind),
                SqlText(edge.properties),
            ])?;
        }
    }
    tx.commit()
}

#[cfg(not(test))]
pub(crate) mod native;

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{CString, c_char, c_int, c_void};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    unsafe extern "C" {
        fn cbm_store_open_path(path: *const c_char) -> *mut c_void;
        fn cbm_store_close(store: *mut c_void);
        fn cbm_gbuf_new(
            project: *const c_char,
            root_path: *const c_char,
        ) -> *mut c_void;
        fn cbm_gbuf_free(graph: *mut c_void);
        fn cbm_gbuf_upsert_node(
            graph: *mut c_void,
            label: *const c_char,
            name: *const c_char,
            qualified_name: *const c_char,
            file_path: *const c_char,
            start_line: c_int,
            end_line: c_int,
            properties: *const c_char,
        ) -> i64;
        fn cbm_gbuf_insert_edge(
            graph: *mut c_void,
            source_id: i64,
            target_id: i64,
            kind: *const c_char,
            properties: *const c_char,
        ) -> i64;
        fn cbm_gbuf_dump_to_sqlite(
            graph: *mut c_void,
            path: *const c_char,
        ) -> c_int;
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let created = Command::new("mktemp")
                .arg("-d")
                .arg(std::env::temp_dir().join("mori-stage-XXXXXXXX"))
                .output()
                .unwrap();
            assert!(created.status.success(), "{created:?}");
            use std::os::unix::ffi::OsStrExt;
            Self(PathBuf::from(std::ffi::OsStr::from_bytes(
                created.stdout.strip_suffix(b"\n").unwrap(),
            )))
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct GraphRecords<'a> {
        project: ProjectRecord<'a>,
        nodes: Vec<NodeRecord<'a>>,
        edges: Vec<EdgeRecord<'a>>,
    }

    impl GraphRecords<'_> {
        fn write(self, stage: &mut Connection) -> rusqlite::Result<()> {
            write_graph_records(stage, self.project, self.nodes, self.edges)
        }
    }

    fn fixture<'a>() -> GraphRecords<'a> {
        let node = |id,
                    name: &'a [u8],
                    qualified_name: &'a [u8],
                    file_path: &'a [u8]| NodeRecord {
            id,
            label: b"Function",
            name,
            qualified_name,
            file_path,
            start_line: 3,
            end_line: 8,
            properties: br#"{"exported":true}"#,
        };
        let edge = |id, kind: &'a [u8], properties: &'a [u8]| EdgeRecord {
            id,
            source_id: 1,
            target_id: 2,
            kind,
            properties,
        };
        GraphRecords {
            project: ProjectRecord {
                name: b"comparison",
                indexed_at: b"2026-01-01T00:00:00Z",
                root_path: b"/fixture",
            },
            nodes: vec![
                node(1, b"serve", b"comparison.serve", b"main.rs"),
                node(2, b"client", b"comparison.client", b"client_\xff.rs"),
            ],
            edges: vec![
                edge(1, b"IMPORTS", br#"{"local_name":"one"}"#),
                edge(2, b"IMPORTS", br#"{"local_name":"two"}"#),
                edge(3, b"HTTP_CALLS", br#"{"url_path":"/x/\"y"}"#),
            ],
        }
    }

    fn c_text(bytes: &[u8]) -> CString {
        CString::new(bytes).unwrap()
    }

    fn c_path(path: &Path) -> CString {
        c_text(path.as_os_str().as_encoded_bytes())
    }

    fn open_empty_stage(path: &Path) -> Connection {
        let native_path = c_path(path);
        // SAFETY: C reads the live path. Close the returned store before Rust
        // opens the same private stage for writing.
        unsafe {
            let store = cbm_store_open_path(native_path.as_ptr());
            assert!(!store.is_null());
            cbm_store_close(store);
        }
        Connection::open(path).unwrap()
    }

    struct NativeGraph(*mut c_void);

    impl Drop for NativeGraph {
        fn drop(&mut self) {
            // SAFETY: this guard owns the graph buffer.
            unsafe { cbm_gbuf_free(self.0) }
        }
    }

    fn native_dump(path: &Path, input: &GraphRecords<'_>) {
        let project = c_text(input.project.name);
        let root_path = c_text(input.project.root_path);
        // SAFETY: both NUL-terminated strings remain live through construction.
        let graph = NativeGraph(unsafe {
            cbm_gbuf_new(project.as_ptr(), root_path.as_ptr())
        });
        assert!(!graph.0.is_null());
        let node_ids: Vec<_> = input
            .nodes
            .iter()
            .map(|node| {
                let [label, name, qn, file, properties] = [
                    node.label,
                    node.name,
                    node.qualified_name,
                    node.file_path,
                    node.properties,
                ]
                .map(c_text);
                // SAFETY: the buffer is live and C copies each NUL-terminated field.
                let id = unsafe {
                    cbm_gbuf_upsert_node(
                        graph.0,
                        label.as_ptr(),
                        name.as_ptr(),
                        qn.as_ptr(),
                        file.as_ptr(),
                        node.start_line,
                        node.end_line,
                        properties.as_ptr(),
                    )
                };
                assert!(id > 0);
                id
            })
            .collect();
        for edge in &input.edges {
            let kind = c_text(edge.kind);
            let properties = c_text(edge.properties);
            // SAFETY: the graph buffer owns both endpoint IDs. The strings
            // stay live through the insert, which copies them into the buffer.
            let inserted = unsafe {
                cbm_gbuf_insert_edge(
                    graph.0,
                    node_ids[(edge.source_id - 1) as usize],
                    node_ids[(edge.target_id - 1) as usize],
                    kind.as_ptr(),
                    properties.as_ptr(),
                )
            };
            assert!(inserted > 0);
        }
        let path = c_path(path);
        // SAFETY: the buffer and destination path stay live through the dump.
        assert_eq!(
            unsafe { cbm_gbuf_dump_to_sqlite(graph.0, path.as_ptr()) },
            0
        );
    }

    fn rows(
        connection: &Connection,
        sql: &str,
    ) -> Vec<Vec<rusqlite::types::Value>> {
        let mut query = connection.prepare(sql).unwrap();
        let columns = query.column_count();
        query
            .query_map([], |row| {
                (0..columns).map(|column| row.get(column)).collect()
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    #[test]
    fn matches_native_graph_columns_and_generated_edge_keys() {
        let scratch = Scratch::new();
        let native_path = scratch.0.join("native.db");
        let mut graph = fixture();
        native_dump(&native_path, &graph);
        let reference = Connection::open(native_path).unwrap();
        let reference_time: String = reference
            .query_row("SELECT indexed_at FROM projects", [], |row| row.get(0))
            .unwrap();

        let mut stage = open_empty_stage(&scratch.0.join("rust.db"));
        graph.project.indexed_at = reference_time.as_bytes();
        graph.write(&mut stage).unwrap();
        for sql in [
            "SELECT name, indexed_at, root_path FROM projects ORDER BY name",
            "SELECT id, project, label, name, qualified_name, CAST(file_path AS BLOB), start_line, end_line, properties FROM nodes ORDER BY id",
            "SELECT id, project, source_id, target_id, type, properties, url_path_gen, local_name_gen FROM edges ORDER BY id",
        ] {
            assert_eq!(rows(&reference, sql), rows(&stage, sql), "{sql}");
        }
        assert_eq!(
            rows(&stage, "PRAGMA integrity_check"),
            vec![vec![rusqlite::types::Value::Text("ok".to_owned())]]
        );
        assert!(rows(&stage, "PRAGMA foreign_key_check").is_empty());
    }

    #[test]
    fn invalid_edge_rolls_back_project_nodes_and_edges() {
        let scratch = Scratch::new();
        let mut stage = open_empty_stage(&scratch.0.join("rust.db"));
        let mut graph = fixture();
        graph.edges[2].target_id = 999;
        let err = graph.write(&mut stage).unwrap_err();
        assert_eq!(
            err.sqlite_error_code(),
            Some(rusqlite::ErrorCode::ConstraintViolation)
        );
        for sql in [
            "SELECT * FROM projects",
            "SELECT * FROM nodes",
            "SELECT * FROM edges",
        ] {
            assert!(rows(&stage, sql).is_empty(), "{sql}");
        }
        fixture().write(&mut stage).unwrap();
    }
}
