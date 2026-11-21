//! Executes search queries against the caller's live SQLite connection.
//!
//! The native planner supplies the predicates and bound values. That preserves
//! the indexed name ranges and the connection's custom regex functions. The
//! native output callbacks keep allocation and rendering on the C side.

use std::ffi::{c_char, c_int, c_void};
use std::{panic, ptr};

use rusqlite::ffi::{self, sqlite3, sqlite3_stmt};

const STORE_OK: c_int = 0;
const STORE_ERR: c_int = -1;

type EmitRow = unsafe extern "C" fn(*mut c_void, *mut sqlite3_stmt);
type ReportError = unsafe extern "C" fn(*mut c_void, *const c_char);
type PatternReader = unsafe extern "C" fn(
    *mut sqlite3,
    *const c_char,
    *const c_char,
    *const *const c_char,
    c_int,
    *mut c_int,
    *mut c_void,
    EmitRow,
    ReportError,
) -> c_int;

struct Statement(*mut sqlite3_stmt);

impl Statement {
    // The caller lends the DB for the lifetime of the statement and supplies
    // NUL-terminated SQL.
    unsafe fn prepare(db: *mut sqlite3, sql: *const c_char) -> Option<Self> {
        let mut stmt = ptr::null_mut();
        // SAFETY: The arguments follow the contract above.
        let status = unsafe {
            ffi::sqlite3_prepare_v2(db, sql, -1, &mut stmt, ptr::null_mut())
        };
        (status == ffi::SQLITE_OK && !stmt.is_null()).then(|| Self(stmt))
    }

    // The caller owns each value until the prepared statement is finalized.
    unsafe fn bind_texts(&self, values: &[*const c_char]) {
        for (index, value) in values.iter().enumerate() {
            // SAFETY: SQLite reads NUL-terminated strings from the native plan
            // and copies them. Binding null has the same semantics as native.
            unsafe {
                ffi::sqlite3_bind_text(
                    self.0,
                    index as c_int + 1,
                    *value,
                    -1,
                    ffi::SQLITE_TRANSIENT(),
                );
            }
        }
    }

    fn step(&self) -> c_int {
        // SAFETY: Self owns a live SQLite statement.
        unsafe { ffi::sqlite3_step(self.0) }
    }
}

impl Drop for Statement {
    fn drop(&mut self) {
        // SAFETY: Statement finalization belongs exclusively to Self.
        unsafe { ffi::sqlite3_finalize(self.0) };
    }
}

unsafe extern "C" fn read_pattern(
    db: *mut sqlite3,
    count_sql: *const c_char,
    page_sql: *const c_char,
    binds: *const *const c_char,
    bind_count: c_int,
    total: *mut c_int,
    context: *mut c_void,
    emit: EmitRow,
    report: ReportError,
) -> c_int {
    let result = panic::catch_unwind(|| {
        // SAFETY: The C caller lends the connection, binds, and callbacks
        // for this invocation. Statement handles never outlive the call.
        unsafe {
            let binds = std::slice::from_raw_parts(binds, bind_count as usize);
            if let Some(count) = Statement::prepare(db, count_sql) {
                count.bind_texts(binds);
                if count.step() == ffi::SQLITE_ROW {
                    *total = ffi::sqlite3_column_int(count.0, 0);
                }
            }
            let Some(page) = Statement::prepare(db, page_sql) else {
                report(context, c"search prepare".as_ptr());
                return STORE_ERR;
            };
            page.bind_texts(binds);
            loop {
                match page.step() {
                    ffi::SQLITE_DONE => return STORE_OK,
                    ffi::SQLITE_ROW => emit(context, page.0),
                    _ => {
                        report(context, c"row scan aborted".as_ptr());
                        return STORE_ERR;
                    }
                }
            }
        }
    });
    result.unwrap_or_else(|_| {
        // SAFETY: The only caller provides a valid error callback and context.
        unsafe { report(context, c"Rust search reader failed".as_ptr()) };
        STORE_ERR
    })
}

unsafe extern "C" {
    fn cbm_store_set_pattern_search_reader(reader: PatternReader);
}

/// Register during process startup, before native worker threads can start.
pub(crate) unsafe fn register() {
    // SAFETY: The registered function lives for the process lifetime.
    unsafe { cbm_store_set_pattern_search_reader(read_pattern) };
}
