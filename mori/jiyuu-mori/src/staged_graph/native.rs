//! Bridge from the native graph buffer's normalized dump records.

use super::{
    EdgeRecord, NodeRecord, NodeVector, ProjectRecord, TokenVector,
    write_graph_records,
};
use rusqlite::{Connection, ffi::sqlite3};
use std::ffi::{CStr, OsStr, c_char, c_int, c_void};
use std::io::Write;
use std::ptr::NonNull;

#[repr(C)]
struct DumpNode {
    id: i64,
    project: *const c_char,
    label: *const c_char,
    name: *const c_char,
    qualified_name: *const c_char,
    file_path: *const c_char,
    start_line: c_int,
    end_line: c_int,
    properties: *const c_char,
}

#[repr(C)]
struct DumpEdge {
    id: i64,
    project: *const c_char,
    source_id: i64,
    target_id: i64,
    kind: *const c_char,
    properties: *const c_char,
    url_path: *const c_char,
    local_name: *const c_char,
}

#[repr(C)]
struct DumpVector {
    node_id: i64,
    project: *const c_char,
    vector: *const u8,
    vector_len: c_int,
}

#[repr(C)]
struct DumpTokenVector {
    id: i64,
    project: *const c_char,
    token: *const c_char,
    vector: *const u8,
    vector_len: c_int,
    idf: f32,
}

#[repr(C)]
struct Dump {
    path: *const c_char,
    project: *const c_char,
    indexed_at: *const c_char,
    root_path: *const c_char,
    nodes: *const DumpNode,
    node_count: c_int,
    edges: *const DumpEdge,
    edge_count: c_int,
    vectors: *const DumpVector,
    vector_count: c_int,
    tokens: *const DumpTokenVector,
    token_count: c_int,
}

unsafe extern "C" {
    fn cbm_gbuf_set_staged_writer(
        writer: unsafe extern "C" fn(dump: *const Dump) -> c_int,
    );
    fn cbm_store_open_path(path: *const c_char) -> *mut c_void;
    fn cbm_store_get_db(store: *mut c_void) -> *mut sqlite3;
    fn cbm_store_close(store: *mut c_void);
}

struct Store(NonNull<c_void>);

impl Drop for Store {
    fn drop(&mut self) {
        // SAFETY: this guard owns the store and outlives the borrowed connection.
        unsafe { cbm_store_close(self.0.as_ptr()) }
    }
}

// SAFETY: each non-null pointer must refer to a NUL-terminated string that
// remains live for the duration of the callback.
unsafe fn text<'a>(value: *const c_char) -> &'a [u8] {
    if value.is_null() {
        &[]
    } else {
        // SAFETY: supplied by the caller.
        unsafe { CStr::from_ptr(value).to_bytes() }
    }
}

// SAFETY: nonempty arrays must be allocated and initialized for `count` rows by C.
unsafe fn records<'a, T>(ptr: *const T, count: c_int) -> Option<&'a [T]> {
    if count == 0 {
        return Some(&[]);
    }
    if ptr.is_null() || count < 0 {
        return None;
    }
    // SAFETY: the caller ensures C owns these rows for the lifetime of the borrow.
    Some(unsafe { std::slice::from_raw_parts(ptr, count as usize) })
}

unsafe extern "C" fn write(dump: *const Dump) -> c_int {
    // SAFETY: the graph buffer owns all of these arrays and any bytes they
    // borrow. C invokes the callback synchronously, before freeing any record.
    let result = unsafe {
        let Some(dump) = dump.as_ref() else {
            return -1;
        };
        let (Some(nodes), Some(edges), Some(vectors), Some(tokens)) = (
            records(dump.nodes, dump.node_count),
            records(dump.edges, dump.edge_count),
            records(dump.vectors, dump.vector_count),
            records(dump.tokens, dump.token_count),
        ) else {
            return -1;
        };
        if dump.path.is_null()
            || vectors
                .iter()
                .any(|value| value.vector_len <= 0 || value.vector.is_null())
            || tokens.iter().any(|value| {
                value.vector_len <= 0
                    || value.vector.is_null()
                    || !value.idf.is_finite()
            })
        {
            return -1;
        }

        // The Rust connection borrows from the native store and gets dropped first.
        let Some(store) =
            NonNull::new(cbm_store_open_path(dump.path)).map(Store)
        else {
            return -1;
        };
        let Ok(mut connection) =
            Connection::from_handle(cbm_store_get_db(store.0.as_ptr()))
        else {
            return -1;
        };
        write_graph_records(
            &mut connection,
            ProjectRecord {
                name: text(dump.project),
                indexed_at: text(dump.indexed_at),
                root_path: text(dump.root_path),
            },
            nodes.iter().map(|node| NodeRecord {
                id: node.id,
                label: text(node.label),
                name: text(node.name),
                qualified_name: text(node.qualified_name),
                file_path: text(node.file_path),
                start_line: node.start_line,
                end_line: node.end_line,
                properties: text(node.properties),
            }),
            edges.iter().map(|edge| EdgeRecord {
                id: edge.id,
                source_id: edge.source_id,
                target_id: edge.target_id,
                kind: text(edge.kind),
                properties: text(edge.properties),
            }),
            vectors.iter().map(|value| NodeVector {
                node_id: value.node_id,
                bytes: std::slice::from_raw_parts(
                    value.vector,
                    value.vector_len as usize,
                ),
            }),
            tokens.iter().map(|value| TokenVector {
                id: value.id,
                token: text(value.token),
                bytes: std::slice::from_raw_parts(
                    value.vector,
                    value.vector_len as usize,
                ),
                idf: value.idf,
            }),
        )
    };
    match result {
        Ok(()) => 0,
        Err(error) => {
            let _ =
                writeln!(std::io::stderr(), "Mori Rust graph writer: {error}");
            -1
        }
    }
}

/// Register the opt-in Rust writer before the native runtime starts.
///
/// # Safety
/// No threads can be using the C graph buffer when the callback is installed.
pub unsafe fn register() {
    if std::env::var_os("MORI_IMPL_GRAPH_WRITER").as_deref()
        == Some(OsStr::new("rust"))
    {
        // SAFETY: the caller registers before starting the runtime. The static
        // callback remains valid for the lifetime of the process.
        unsafe { cbm_gbuf_set_staged_writer(write) }
    }
}
