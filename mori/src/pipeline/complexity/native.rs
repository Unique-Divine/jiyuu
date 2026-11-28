//! Synchronous bridge for the native graph buffer and private staging store.

use super::{CallGraph, NodeKey, PropertyMode, stage};
use rusqlite::{Connection, ffi::sqlite3};
use std::ffi::{CStr, CString, c_char, c_int, c_void};

type ReadNode = unsafe extern "C" fn(
    *mut c_void,
    i64,
    *const c_char,
    *const c_char,
    c_int,
    *const c_char,
    bool,
);
type ReadCall = unsafe extern "C" fn(*mut c_void, i64, i64) -> bool;

unsafe extern "C" {
    fn cbm_complexity_read_buffer(
        graph: *const c_void,
        data: *mut c_void,
        node: ReadNode,
        call: ReadCall,
    );
    fn cbm_complexity_write_node(
        graph: *mut c_void,
        id: i64,
        properties: *const c_char,
    ) -> c_int;
}

unsafe fn copy_string(ptr: *const c_char, default: &[u8]) -> Vec<u8> {
    if ptr.is_null() {
        default.to_vec()
    } else {
        // SAFETY: the bridge supplies NUL-terminated buffer strings that
        // remain live for this callback. Copy them before returning.
        unsafe { CStr::from_ptr(ptr) }.to_bytes().to_vec()
    }
}

unsafe extern "C" fn read_node(
    data: *mut c_void,
    id: i64,
    qualified_name: *const c_char,
    file_path: *const c_char,
    start_line: c_int,
    properties: *const c_char,
    seed: bool,
) {
    // SAFETY: the reader lends exclusive access to data for each callback.
    unsafe {
        (*data.cast::<CallGraph>()).set_node(
            id,
            NodeKey {
                qualified_name: copy_string(qualified_name, b""),
                file_path: copy_string(file_path, b""),
                start_line,
            },
            seed.then(|| copy_string(properties, b"{}")),
        );
    }
}

unsafe extern "C" fn read_call(
    data: *mut c_void,
    source_id: i64,
    target_id: i64,
) -> bool {
    // SAFETY: the reader lends exclusive access to data for each callback.
    unsafe { (*data.cast::<CallGraph>()).add_call(source_id, target_id) }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_complexity_buffer(graph: *mut c_void) -> i64 {
    let mut calls = CallGraph::default();
    // SAFETY: the pass owns the graph for the duration of the call. The
    // bridge reads synchronously and retains neither data nor the callbacks.
    unsafe {
        cbm_complexity_read_buffer(
            graph,
            (&raw mut calls).cast(),
            read_node,
            read_call,
        )
    };
    let Ok(updates) = calls.updates(PropertyMode::Append) else {
        return -1;
    };
    let count = updates.len() as i64;
    for update in updates {
        let Ok(properties) = CString::new(update.properties) else {
            return -1;
        };
        // SAFETY: each update uses an ID from this graph. The setter copies
        // the string into the buffer's allocator before returning.
        if unsafe {
            cbm_complexity_write_node(graph, update.id, properties.as_ptr())
        } != 0
        {
            return -1;
        }
    }
    count
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_complexity_stage(
    db: *mut sqlite3,
    project: *const c_char,
) -> i64 {
    // SAFETY: the closure runner lends its private staging handle and the
    // project C string for this call. from_handle leaves the handle open.
    let Ok(mut stage) = (unsafe { Connection::from_handle(db) }) else {
        return -1;
    };
    let project = unsafe { CStr::from_ptr(project) };
    stage::recompute(&mut stage, project.to_bytes())
        .map_or(-1, |count| count as i64)
}
