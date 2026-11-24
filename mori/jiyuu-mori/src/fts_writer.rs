//! Maintain the FTS side table on the indexer's current SQLite connection.

use std::ffi::{CString, c_char, c_int};
use std::ptr::null_mut;

use rusqlite::ffi::{
    SQLITE_DONE, SQLITE_INTERRUPT, SQLITE_OK, SQLITE_ROW, SQLITE_TRANSIENT,
    sqlite3, sqlite3_bind_int64, sqlite3_bind_text, sqlite3_errcode,
    sqlite3_errmsg, sqlite3_exec, sqlite3_finalize, sqlite3_free,
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

type ReportError =
    unsafe extern "C" fn(*mut Store, *const c_char, *const c_char);

// Legacy rows can contain malformed properties. Check validity before decoding
// docstrings and skip JSON parsing for rows without the docstring key.
const BODY_EXPR: &str = r#", CASE WHEN properties LIKE '%"docstring"%' AND json_valid(properties) THEN json_extract(properties, '$.docstring') END"#;

// The Macro label already supplies the token "macro". Strip the qualified-name
// suffix to keep macros from taking over BM25 results for that token.
const QN_EXPR: &str = "CASE WHEN label = 'Macro' AND substr(qualified_name, -6) = '#macro' \
     THEN substr(qualified_name, 1, length(qualified_name) - 6) \
     ELSE qualified_name END";

fn insert_sql(partial: bool, body: bool, camel: bool) -> CString {
    let body_column = if body { ", body" } else { "" };
    let body_expression = if body { BODY_EXPR } else { "" };
    let name = if camel {
        "cbm_camel_split(name)"
    } else {
        "name"
    };
    let filter = if partial {
        " WHERE project = ?1 AND id > ?2"
    } else {
        ""
    };
    CString::new(format!(
        "INSERT INTO nodes_fts (rowid, name, qualified_name, label, file_path{body_column}) \
         SELECT id, {name}, {QN_EXPR}, label, file_path{body_expression} FROM nodes{filter};"
    ))
    .expect("FTS backfill SQL consists of static fragments without NUL bytes")
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

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_fts_write(
    store: *mut Store,
    db: *mut sqlite3,
    project: *const c_char,
    after_id: i64,
    closure_refresh: bool,
    report_error: ReportError,
) -> c_int {
    // SAFETY: The native entry point lends the live store, its connection, and
    // an optional NUL-terminated project name. The caller owns the transaction.
    unsafe {
        // An absent optional table is distinct from an existing virtual
        // table whose SQLite constructor fails.
        if closure_refresh {
            let catalog = c"SELECT 1 FROM main.sqlite_schema WHERE name = 'nodes_fts' COLLATE NOCASE;";
            let Some(statement) = Statement::prepare(db, catalog.as_ptr())
            else {
                report_error(store, c"fts_catalog".as_ptr(), sqlite3_errmsg(db));
                return ERROR;
            };
            match sqlite3_step(statement.0) {
                SQLITE_ROW => {}
                SQLITE_DONE => return NOT_FOUND,
                _ => {
                    report_error(
                        store,
                        c"fts_catalog".as_ptr(),
                        sqlite3_errmsg(db),
                    );
                    return ERROR;
                }
            }
        }
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
                report_error(store, c"exec".as_ptr(), message);
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
            let sql = insert_sql(partial, body, camel);
            let Some(statement) = Statement::prepare(db, sql.as_ptr()) else {
                if sqlite3_errcode(db) == SQLITE_INTERRUPT {
                    return ERROR;
                }
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
            let step_status = sqlite3_step(statement.0);
            if step_status == SQLITE_DONE {
                return OK;
            }
            // Capture the error before SQLite finalizes the statement.
            report_error(store, c"fts_backfill".as_ptr(), sqlite3_errmsg(db));
            if step_status == SQLITE_INTERRUPT {
                return ERROR;
            }
            status = ERROR;
        }
        // Once delete-all succeeded, a malformed schema must fail the stage.
        if closure_refresh && status == NOT_FOUND {
            report_error(store, c"fts_prepare".as_ptr(), sqlite3_errmsg(db));
            return ERROR;
        }
        status
    }
}
