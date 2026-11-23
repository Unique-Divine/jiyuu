//! Adapter for the native query handler's read snapshot and response owner.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::panic;
use std::time::Instant;

use rusqlite::{Connection, ffi::sqlite3};

use crate::cypher_linear::{self, ExecutionBudget, ExecutionError, ResultRows};

#[repr(C)]
struct Cell {
    data: *const c_char,
    len: usize,
}

impl From<&[u8]> for Cell {
    fn from(value: &[u8]) -> Self {
        Self {
            data: value.as_ptr().cast(),
            len: value.len(),
        }
    }
}

#[repr(C)]
struct Sink {
    context: *mut c_void,
    begin: unsafe extern "C" fn(*mut c_void, *const Cell, usize, usize) -> bool,
    row: unsafe extern "C" fn(*mut c_void, *const Cell) -> bool,
    storage_error: unsafe extern "C" fn(*mut c_void, Cell),
}

#[repr(i32)]
enum Status {
    Ok,
    Unsupported,
    Deadline,
    Cancelled,
    RowCeiling,
    Error,
}

type Cancel = unsafe extern "C" fn(*mut c_void) -> bool;
type Evaluate = unsafe extern "C" fn(
    *mut sqlite3,
    *const c_char,
    *const c_char,
    Cancel,
    *mut c_void,
    *const Sink,
) -> c_int;

unsafe extern "C" {
    fn cbm_mcp_set_cypher_evaluator(evaluate: Evaluate);
}

pub(crate) unsafe fn register() {
    // SAFETY: startup runs before C starts request threads.
    unsafe { cbm_mcp_set_cypher_evaluator(evaluate) };
}

unsafe extern "C" fn evaluate(
    db: *mut sqlite3,
    query: *const c_char,
    project: *const c_char,
    cancelled: Cancel,
    cancel_context: *mut c_void,
    sink: *const Sink,
) -> c_int {
    panic::catch_unwind(|| {
        // SAFETY: C provides the live read-batched handle, NUL-terminated
        // strings, and callbacks with contexts valid for this invocation.
        let cancelled = || unsafe { cancelled(cancel_context) };
        let mut budget = ExecutionBudget::new(&Instant::now, &cancelled);
        let query = unsafe { CStr::from_ptr(query) };
        let project = unsafe { CStr::from_ptr(project) };
        let Ok(source) = query.to_str() else {
            return Status::Unsupported;
        };
        let Ok(project) = project.to_str() else {
            return Status::Unsupported;
        };
        let Ok(query) = cypher_linear::classify(source) else {
            return Status::Unsupported;
        };
        let sink = unsafe { &*sink };
        // from_handle borrows the native handle without taking ownership.
        let result = unsafe { Connection::from_handle(db) }
            .map_err(ExecutionError::Storage)
            .and_then(|connection| {
                query.execute_with_budget(&connection, project, &mut budget)
            });
        match result {
            Ok(rows) => sink.emit(&rows),
            Err(ExecutionError::Deadline) => Status::Deadline,
            Err(ExecutionError::Cancelled) => Status::Cancelled,
            Err(ExecutionError::RowCeiling) => Status::RowCeiling,
            Err(ExecutionError::OrderedInputCeiling) => Status::Unsupported,
            Err(ExecutionError::Storage(error)) => {
                let detail =
                    format!("query aborted: reading the graph failed ({error})");
                // SAFETY: C copies the detail during the call.
                unsafe {
                    (sink.storage_error)(sink.context, detail.as_bytes().into())
                };
                Status::Error
            }
        }
    })
    .unwrap_or(Status::Error) as c_int
}

impl Sink {
    fn emit(&self, result: &ResultRows) -> Status {
        let mut cells: Vec<_> = result
            .columns
            .iter()
            .map(|text| text.as_bytes().into())
            .collect();
        // SAFETY: callbacks copy cells synchronously and keep no Rust
        // pointers. The header determines the width of each following row.
        if !unsafe {
            (self.begin)(
                self.context,
                cells.as_ptr(),
                cells.len(),
                result.rows.len(),
            )
        } {
            return Status::Error;
        }
        for row in &result.rows {
            cells.clear();
            cells.extend(row.iter().map(|bytes| Cell::from(bytes.as_slice())));
            if !unsafe { (self.row)(self.context, cells.as_ptr()) } {
                return Status::Error;
            }
        }
        Status::Ok
    }
}
