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

struct NodeVector<'a> {
    node_id: i64,
    bytes: &'a [u8],
}

struct TokenVector<'a> {
    id: i64,
    token: &'a [u8],
    bytes: &'a [u8],
    idf: f32,
}

/// Insert the graph into an empty stage initialized with Mori's store schema.
/// IDs are final. Edges refer to those IDs. Keep graph records and generated
/// edge keys in one transaction so a failed insert leaves no partial graph.
fn write_graph_records<'a>(
    stage: &mut Connection,
    project: ProjectRecord<'a>,
    nodes: impl IntoIterator<Item = NodeRecord<'a>>,
    edges: impl IntoIterator<Item = EdgeRecord<'a>>,
    vectors: impl IntoIterator<Item = NodeVector<'a>>,
    tokens: impl IntoIterator<Item = TokenVector<'a>>,
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
    // These tables are part of the C dump schema at every index depth,
    // including graph-only stages that have no vectors to insert.
    tx.execute_batch(
        "CREATE TABLE node_vectors (
            node_id INTEGER PRIMARY KEY, project TEXT NOT NULL, vector BLOB NOT NULL
        );
        CREATE TABLE token_vectors (
            id INTEGER PRIMARY KEY, project TEXT NOT NULL, token TEXT NOT NULL,
            vector BLOB NOT NULL, idf INTEGER NOT NULL
        );",
    )?;
    {
        let mut insert_vector = tx.prepare(
            "INSERT INTO node_vectors (node_id, project, vector) VALUES (?1, ?2, ?3)",
        )?;
        for vector in vectors {
            insert_vector.execute(params![
                vector.node_id,
                project_name,
                vector.bytes
            ])?;
        }
        let mut insert_token = tx.prepare(
            "INSERT INTO token_vectors (id, project, token, vector, idf) VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        for token in tokens {
            const IDF_FIXED_POINT_SCALE: f32 = 1000.0;
            let idf = (token.idf * IDF_FIXED_POINT_SCALE) as i64;
            insert_token.execute(params![
                token.id,
                project_name,
                SqlText(token.token),
                token.bytes,
                idf,
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
        fn cbm_gbuf_delete_by_file(
            graph: *mut c_void,
            file: *const c_char,
        ) -> c_int;
        fn cbm_gbuf_store_vector(
            graph: *mut c_void,
            node_id: i64,
            vector: *const u8,
            vector_len: c_int,
        ) -> c_int;
        fn cbm_gbuf_store_token_vector(
            graph: *mut c_void,
            token: *const c_char,
            vector: *const u8,
            vector_len: c_int,
            idf: f32,
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
        vectors: Vec<NodeVector<'a>>,
        tokens: Vec<TokenVector<'a>>,
    }

    impl GraphRecords<'_> {
        fn write(self, stage: &mut Connection) -> rusqlite::Result<()> {
            write_graph_records(
                stage,
                self.project,
                self.nodes,
                self.edges,
                self.vectors,
                self.tokens,
            )
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
            vectors: vec![NodeVector {
                node_id: 2,
                bytes: &[0, 127, 128, 255],
            }],
            tokens: vec![TokenVector {
                id: 1,
                token: b"sample",
                bytes: &[128, 1, 0, 255],
                idf: 1.2349,
            }],
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
        // SAFETY: the graph is live and C copies all input strings. Deleting
        // this temporary node leaves a gap between final IDs and buffer IDs.
        unsafe {
            assert!(
                cbm_gbuf_upsert_node(
                    graph.0,
                    c"Function".as_ptr(),
                    c"deleted".as_ptr(),
                    c"comparison.deleted".as_ptr(),
                    c"deleted.rs".as_ptr(),
                    1,
                    1,
                    c"{}".as_ptr(),
                ) > 0
            );
            assert_eq!(
                cbm_gbuf_delete_by_file(graph.0, c"deleted.rs".as_ptr()),
                1
            );
        }
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
        for vector in &input.vectors {
            let temp_id = node_ids[(vector.node_id - 1) as usize];
            assert_ne!(temp_id, vector.node_id);
            // SAFETY: the graph is live and C copies the given byte slice.
            assert_eq!(
                unsafe {
                    cbm_gbuf_store_vector(
                        graph.0,
                        temp_id,
                        vector.bytes.as_ptr(),
                        vector.bytes.len().try_into().unwrap(),
                    )
                },
                0
            );
        }
        for vector in &input.tokens {
            let token = c_text(vector.token);
            // SAFETY: the graph and inputs stay live through the copying call.
            assert_eq!(
                unsafe {
                    cbm_gbuf_store_token_vector(
                        graph.0,
                        token.as_ptr(),
                        vector.bytes.as_ptr(),
                        vector.bytes.len().try_into().unwrap(),
                        vector.idf,
                    )
                },
                0
            );
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
            "SELECT node_id, project, vector FROM node_vectors ORDER BY node_id",
            "SELECT id, project, token, vector, idf FROM token_vectors ORDER BY id",
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

    #[test]
    fn duplicate_vector_rolls_back_the_whole_stage() {
        let scratch = Scratch::new();
        let mut stage = open_empty_stage(&scratch.0.join("rust.db"));
        let mut graph = fixture();
        graph.vectors.push(NodeVector {
            node_id: 2,
            bytes: &[0],
        });
        let error = graph.write(&mut stage).unwrap_err();
        assert_eq!(
            error.sqlite_error_code(),
            Some(rusqlite::ErrorCode::ConstraintViolation)
        );
        assert!(rows(&stage, "SELECT * FROM projects").is_empty());
        assert!(rows(&stage, "SELECT name FROM sqlite_master WHERE name IN ('node_vectors', 'token_vectors')").is_empty());
        fixture().write(&mut stage).unwrap();
    }
}
