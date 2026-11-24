//! Maintain the FTS side table on the indexer's current SQLite connection.

use std::ffi::{c_char, c_int};
use std::ptr::null_mut;

use rusqlite::ffi::{
    SQLITE_DONE, SQLITE_OK, SQLITE_TRANSIENT, sqlite3, sqlite3_bind_int64,
    sqlite3_bind_text, sqlite3_exec, sqlite3_finalize, sqlite3_free,
    sqlite3_prepare_v2, sqlite3_step, sqlite3_stmt,
};

// Mirror the native store result codes. Both pipeline callers distinguish
// prepare-time unavailability from failures while writing.
const OK: c_int = 0;
const ERROR: c_int = -1;
const NOT_FOUND: c_int = -2;

#[repr(C)]
struct Store {
    _opaque: [u8; 0],
}

#[repr(C)]
struct Operations {
    build_query:
        unsafe extern "C" fn(*mut c_char, usize, bool, bool, bool) -> bool,
    report_exec_error: unsafe extern "C" fn(*mut Store, *const c_char),
    report_step_error: unsafe extern "C" fn(*mut Store),
}

type Writer = unsafe extern "C" fn(
    *mut Store,
    *mut sqlite3,
    *const c_char,
    i64,
    *const Operations,
    *mut c_char,
    usize,
) -> c_int;

unsafe extern "C" {
    fn cbm_store_set_fts_writer(writer: Writer);
}

struct Statement(*mut sqlite3_stmt);

impl Statement {
    /// The SQL must be terminated and the connection must outlive the result.
    unsafe fn prepare(db: *mut sqlite3, sql: *const c_char) -> Option<Self> {
        let mut statement = null_mut();
        // SAFETY: The caller lends its open connection and terminated SQL.
        let status = unsafe {
            sqlite3_prepare_v2(db, sql, -1, &mut statement, null_mut())
        };
        if status == SQLITE_OK {
            Some(Self(statement))
        } else {
            None
        }
    }
}

impl Drop for Statement {
    fn drop(&mut self) {
        // SAFETY: This guard owns the statement and the connection is still open.
        unsafe { sqlite3_finalize(self.0) };
    }
}

unsafe extern "C" fn rebuild(
    store: *mut Store,
    db: *mut sqlite3,
    project: *const c_char,
    after_id: i64,
    operations: *const Operations,
    sql: *mut c_char,
    sql_capacity: usize,
) -> c_int {
    // SAFETY: The native entry point lends the live connection and writable
    // query buffer for this synchronous call. SQLite retains neither the query
    // buffer nor the project string. The caller owns the transaction.
    unsafe {
        let operations = &*operations;
        let partial = !project.is_null();
        if !partial {
            let mut message = null_mut();
            let rc = sqlite3_exec(
                db,
                c"INSERT INTO nodes_fts(nodes_fts) VALUES('delete-all');"
                    .as_ptr(),
                None,
                null_mut(),
                &mut message,
            );
            if rc != SQLITE_OK {
                (operations.report_exec_error)(store, message);
                sqlite3_free(message.cast());
                return ERROR;
            }
        }

        // Legacy caches may have a four-column contentless FTS table.
        let has_body = Statement::prepare(
            db,
            c"SELECT body FROM nodes_fts LIMIT 0;".as_ptr(),
        )
        .is_some();
        let mut status = NOT_FOUND;
        for (body, camel) in
            [(true, true), (true, false), (false, true), (false, false)]
        {
            if body && !has_body {
                continue;
            }
            if !(operations.build_query)(sql, sql_capacity, partial, body, camel)
            {
                status = ERROR;
                continue;
            }
            let Some(statement) = Statement::prepare(db, sql) else {
                status = NOT_FOUND;
                continue;
            };
            if partial {
                sqlite3_bind_text(
                    statement.0,
                    1,
                    project,
                    -1,
                    SQLITE_TRANSIENT(),
                );
                sqlite3_bind_int64(statement.0, 2, after_id);
            }
            if sqlite3_step(statement.0) == SQLITE_DONE {
                return OK;
            }
            // Capture the error before SQLite finalizes the statement.
            (operations.report_step_error)(store);
            status = ERROR;
        }
        status
    }
}

/// Install the native FTS writer callback.
///
/// # Safety
/// No other thread may read or register the native writer during registration.
pub unsafe fn register() {
    // SAFETY: The caller guarantees exclusive startup registration.
    unsafe { cbm_store_set_fts_writer(rebuild) };
}
