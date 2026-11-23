//! Bridge from the native graph buffer's normalized dump records.

use super::{EdgeRecord, NodeRecord, ProjectRecord, write_graph_records};
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

unsafe extern "C" {
    fn cbm_gbuf_set_staged_writer(
        writer: unsafe extern "C" fn(
            path: *const c_char,
            project: *const c_char,
            indexed_at: *const c_char,
            root_path: *const c_char,
            nodes: *const DumpNode,
            node_count: c_int,
            edges: *const DumpEdge,
            edge_count: c_int,
        ) -> c_int,
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

unsafe extern "C" fn write(
    path: *const c_char,
    project: *const c_char,
    indexed_at: *const c_char,
    root_path: *const c_char,
    nodes: *const DumpNode,
    node_count: c_int,
    edges: *const DumpEdge,
    edge_count: c_int,
) -> c_int {
    if path.is_null()
        || node_count < 0
        || edge_count < 0
        || (node_count > 0 && nodes.is_null())
        || (edge_count > 0 && edges.is_null())
    {
        return -1;
    }
    // SAFETY: C supplies a path for its private staging file. The connection
    // borrows the native store's SQLite handle, so the store must outlive it.
    let Some(store) =
        NonNull::new(unsafe { cbm_store_open_path(path) }).map(Store)
    else {
        return -1;
    };
    let Ok(mut connection) =
        (unsafe { Connection::from_handle(cbm_store_get_db(store.0.as_ptr())) })
    else {
        return -1;
    };

    // SAFETY: the graph buffer owns the normalized arrays and their strings.
    // C invokes the callback synchronously, before freeing any record.
    let result = unsafe {
        let nodes = if node_count > 0 {
            std::slice::from_raw_parts(nodes, node_count as usize)
        } else {
            &[]
        };
        let edges = if edge_count > 0 {
            std::slice::from_raw_parts(edges, edge_count as usize)
        } else {
            &[]
        };
        write_graph_records(
            &mut connection,
            ProjectRecord {
                name: text(project),
                indexed_at: text(indexed_at),
                root_path: text(root_path),
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
