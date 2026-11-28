//! Executes search queries against the caller's live SQLite connection.
//!
//! The native planner supplies the predicates and bound values. That preserves
//! the indexed name ranges and the connection's custom regex functions. The
//! native output callbacks keep allocation and rendering on the C side.

use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void};
use std::{panic, ptr};

use rusqlite::ffi::{self, sqlite3, sqlite3_stmt};
use rusqlite::{Connection, ToSql, params_from_iter};

#[path = "search_code.rs"]
mod search_code;

const STORE_OK: c_int = 0;
const STORE_ERR: c_int = -1;

type EmitRow = unsafe extern "C" fn(*mut c_void, *mut sqlite3_stmt);
type ReportError = unsafe extern "C" fn(*mut c_void, *const c_char);
#[cfg(not(test))]
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

#[repr(C)]
struct RankFilters {
    fts_query: *const c_char,
    project: *const c_char,
    file_like: *const c_char,
    label: *const c_char,
    kind: *const c_char,
    namespace: *const c_char,
    label_weight: f64,
}

#[repr(C)]
struct RankRequest {
    rank_sql: *const c_char,
    count_sql: *const c_char,
    probe_sql: *const c_char,
    filters: *const RankFilters,
    exact_query: *const c_char,
    limit: c_int,
    offset: c_int,
    candidate_limit: c_int,
}

#[cfg(not(test))]
type RankReader = unsafe extern "C" fn(
    *mut sqlite3,
    *const RankRequest,
    *mut c_int,
    *mut bool,
    *mut c_void,
    EmitRow,
) -> bool;

#[cfg(not(test))]
type CodeDegreeReader = unsafe extern "C" fn(
    *mut sqlite3,
    *const i64,
    c_int,
    *mut c_int,
    *mut c_int,
) -> bool;

fn count_call_degrees(
    connection: &Connection,
    ids: &[i64],
    inbound: &mut [c_int],
    outbound: &mut [c_int],
) -> rusqlite::Result<()> {
    let mut positions = HashMap::new();
    for (index, &id) in ids.iter().enumerate() {
        // Match the native reader if an ID occurs twice.
        positions.entry(id).or_insert(index);
    }
    let placeholders = vec!["?"; ids.len()].join(",");
    let edge_type = "CALLS";
    for (column, counts) in [("target_id", inbound), ("source_id", outbound)] {
        let sql = format!(
            "SELECT {column}, COUNT(*) FROM edges \
             WHERE {column} IN ({placeholders}) AND type = ? GROUP BY {column}"
        );
        let parameters = ids
            .iter()
            .map(|id| id as &dyn ToSql)
            .chain(std::iter::once(&edge_type as &dyn ToSql));
        let mut statement = connection.prepare(&sql)?;
        let mut rows = statement.query(params_from_iter(parameters))?;
        while let Some(row) = rows.next()? {
            let node_id = row.get::<_, i64>(0)?;
            if let Some(&index) = positions.get(&node_id) {
                // sqlite3_column_int returns the low 32 bits of the count.
                counts[index] = row.get::<_, i64>(1)? as c_int;
            }
        }
    }
    Ok(())
}

unsafe extern "C" fn read_code_degrees(
    db: *mut sqlite3,
    ids: *const i64,
    count: c_int,
    inbound: *mut c_int,
    outbound: *mut c_int,
) -> bool {
    let Ok(count) = usize::try_from(count) else {
        return false;
    };
    if count == 0
        || db.is_null()
        || ids.is_null()
        || inbound.is_null()
        || outbound.is_null()
    {
        return false;
    }
    panic::catch_unwind(|| {
        // SAFETY: C lends a live connection, initialized IDs and separate
        // writable output buffers. from_handle does not close the connection.
        unsafe {
            let connection = Connection::from_handle(db)?;
            let ids = std::slice::from_raw_parts(ids, count);
            inbound.write_bytes(0, count);
            outbound.write_bytes(0, count);
            count_call_degrees(
                &connection,
                ids,
                std::slice::from_raw_parts_mut(inbound, count),
                std::slice::from_raw_parts_mut(outbound, count),
            )
        }
    })
    .is_ok_and(|result| result.is_ok())
}

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

    // The caller lends a NUL-terminated string, or NULL for a SQL NULL.
    unsafe fn bind_text(&self, index: c_int, value: *const c_char) -> c_int {
        // SAFETY: SQLite copies the native plan's string before returning.
        unsafe {
            ffi::sqlite3_bind_text(
                self.0,
                index,
                value,
                -1,
                ffi::SQLITE_TRANSIENT(),
            )
        }
    }

    unsafe fn bind_texts(&self, binds: &[*const c_char]) {
        for (index, value) in binds.iter().enumerate() {
            // SAFETY: The native plan lends its bound values for the query.
            unsafe { self.bind_text(index as c_int + 1, *value) };
        }
    }

    fn bind_int(&self, index: c_int, value: c_int) {
        // SAFETY: Self owns a live SQLite statement.
        unsafe { ffi::sqlite3_bind_int(self.0, index, value) };
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

// Parameter positions shared by the native BM25 statements. The count uses
// slot 3 for its candidate window, while the page uses slot 5.
const QUERY: c_int = 1;
const PROJECT: c_int = 2;
const LIMIT: c_int = 3;
const OFFSET: c_int = 4;
const WINDOW: c_int = 5;
const FILE: c_int = 6;
const LABEL: c_int = 7;
const EXACT: c_int = 8;
const KIND: c_int = 9;
const NAMESPACE: c_int = 10;
const LABEL_WEIGHT: c_int = 11;
const ALL_CANDIDATES: c_int = -1;

unsafe fn present(value: *const c_char) -> bool {
    // SAFETY: The native plan stores either NULL or a valid C string.
    !value.is_null() && unsafe { *value != 0 }
}

impl RankFilters {
    unsafe fn bind(&self, stmt: &Statement) {
        // SAFETY: The request lends these strings through finalization.
        unsafe {
            stmt.bind_text(QUERY, self.fts_query);
            stmt.bind_text(PROJECT, self.project);
            for (index, value) in [
                (FILE, self.file_like),
                (LABEL, self.label),
                (KIND, self.kind),
                (NAMESPACE, self.namespace),
            ] {
                stmt.bind_text(
                    index,
                    if present(value) { value } else { ptr::null() },
                );
            }
            ffi::sqlite3_bind_double(stmt.0, LABEL_WEIGHT, self.label_weight);
        }
    }
}

unsafe fn count_candidates(
    db: *mut sqlite3,
    sql: *const c_char,
    filters: &RankFilters,
    window: c_int,
) -> c_int {
    // SAFETY: The native request lends the connection, SQL, and filters.
    unsafe {
        let Some(count) = Statement::prepare(db, sql) else {
            return 0;
        };
        filters.bind(&count);
        count.bind_int(LIMIT, window);
        if count.step() == ffi::SQLITE_ROW {
            ffi::sqlite3_column_int(count.0, 0)
        } else {
            0
        }
    }
}

unsafe extern "C" fn read_ranked(
    db: *mut sqlite3,
    request: *const RankRequest,
    total: *mut c_int,
    window_saturated: *mut bool,
    context: *mut c_void,
    emit: EmitRow,
) -> bool {
    // SAFETY: The native caller provides a live connection, request, output
    // pointers, and bounded row collector. We finalize before returning.
    unsafe {
        let request = &*request;
        let filters = &*request.filters;
        let Some(ranked) = Statement::prepare(db, request.rank_sql) else {
            // Older or lean databases can lack the FTS table.
            return false;
        };
        let saturated =
            Statement::prepare(db, request.probe_sql).is_none_or(|probe| {
                probe.bind_text(QUERY, filters.fts_query);
                probe.bind_int(2, request.candidate_limit);
                probe.step() != ffi::SQLITE_DONE
            });
        let has_post_filter = !filters.file_like.is_null()
            || present(filters.kind)
            || present(filters.namespace);
        let mut window = if saturated && has_post_filter {
            ALL_CANDIDATES
        } else {
            request.candidate_limit
        };
        *total = count_candidates(db, request.count_sql, filters, window);
        if saturated && *total == 0 && window != ALL_CANDIDATES {
            window = ALL_CANDIDATES;
            *total = count_candidates(db, request.count_sql, filters, window);
        }
        *window_saturated = saturated && window != ALL_CANDIDATES;

        filters.bind(&ranked);
        ranked.bind_int(LIMIT, request.limit);
        ranked.bind_int(OFFSET, request.offset);
        ranked.bind_int(WINDOW, window);
        ranked.bind_text(EXACT, request.exact_query);
        for _ in 0..request.limit {
            if ranked.step() != ffi::SQLITE_ROW {
                break;
            }
            emit(context, ranked.0);
        }
        true
    }
}

#[cfg(not(test))]
unsafe extern "C" {
    fn cbm_store_set_pattern_search_reader(reader: PatternReader);
    fn cbm_mcp_set_rank_search_reader(reader: RankReader);
    fn cbm_mcp_set_search_code_degrees_reader(reader: CodeDegreeReader);
}

/// Register during process startup, before native worker threads can start.
#[cfg(not(test))]
pub(crate) unsafe fn register() {
    // SAFETY: The registered function lives for the process lifetime.
    unsafe {
        cbm_store_set_pattern_search_reader(read_pattern);
        cbm_mcp_set_rank_search_reader(read_ranked);
        cbm_mcp_set_search_code_degrees_reader(read_code_degrees);
        search_code::register();
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::CStr;

    use rusqlite::Connection;

    use super::*;

    #[test]
    fn search_code_counts_calls_in_input_order_and_keeps_zero_degrees() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE edges(source_id INTEGER, target_id INTEGER, type TEXT);
                 INSERT INTO edges VALUES (1,2,'CALLS'), (1,3,'CALLS'),
                 (2,1,'CALLS'), (1,2,'USAGE'), (4,2,'CALLS');",
            )
            .unwrap();
        let ids = [2_i64, 1, 2, 5];
        let mut inbound = [-1; 4];
        let mut outbound = [-1; 4];
        // SAFETY: All buffers and the borrowed handle live through the call.
        let handled = unsafe {
            read_code_degrees(
                connection.handle(),
                ids.as_ptr(),
                4,
                inbound.as_mut_ptr(),
                outbound.as_mut_ptr(),
            )
        };
        assert!(handled);
        assert_eq!(inbound, [2, 1, 0, 0]);
        assert_eq!(outbound, [1, 2, 0, 0]);
    }

    #[derive(Default)]
    struct ResultRows {
        ids: Vec<i64>,
        error: String,
    }

    unsafe extern "C" fn capture(context: *mut c_void, row: *mut sqlite3_stmt) {
        // SAFETY: Each test passes an exclusive ResultRows pointer, and the
        // reader lends one current row for this invocation.
        unsafe {
            let output = &mut *context.cast::<ResultRows>();
            output.ids.push(ffi::sqlite3_column_int64(row, 0));
        }
    }

    unsafe extern "C" fn error(context: *mut c_void, operation: *const c_char) {
        // SAFETY: Each test passes an exclusive ResultRows pointer. The
        // operation is a NUL-terminated diagnostic from read_pattern.
        unsafe {
            let output = &mut *context.cast::<ResultRows>();
            output.error =
                CStr::from_ptr(operation).to_string_lossy().into_owned();
        }
    }

    #[test]
    fn pattern_reader_counts_and_pages_on_the_borrowed_handle() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE nodes (id INTEGER, name TEXT);
             INSERT INTO nodes VALUES (3,'a'), (2,'a'), (4,'b'), (1,'a');",
        )
        .unwrap();
        let bind = [c"a".as_ptr()];
        let mut total = 0;
        let mut output = ResultRows::default();
        // SAFETY: The connection, plan, and outputs stay live until return.
        let status = unsafe {
            read_pattern(
                db.handle(),
                c"SELECT count(*) FROM nodes WHERE name=?1".as_ptr(),
                c"SELECT id FROM nodes WHERE name=?1 ORDER BY name,id LIMIT 1 OFFSET 1".as_ptr(),
                bind.as_ptr(),
                1,
                &mut total,
                ptr::from_mut(&mut output).cast(),
                capture,
                error,
            )
        };
        assert_eq!(status, STORE_OK);
        assert_eq!(total, 3);
        assert_eq!(output.ids, [2]);
        assert!(output.error.is_empty());
    }

    #[test]
    fn ranked_reader_reports_a_window_and_expands_it_for_file_filters() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE VIRTUAL TABLE needle USING fts5(name, path UNINDEXED);
             INSERT INTO needle VALUES ('needle','a'), ('needle','b'), ('needle','c');",
        )
        .unwrap();
        let mut filters = RankFilters {
            fts_query: c"needle".as_ptr(),
            project: c"test".as_ptr(),
            file_like: ptr::null(),
            label: ptr::null(),
            kind: ptr::null(),
            namespace: ptr::null(),
            label_weight: 1.0,
        };
        for (path, expected_ids, expected_total, expected_window) in [
            (ptr::null(), &[1, 2][..], 2, true),
            (c"c".as_ptr(), &[3][..], 1, false),
        ] {
            filters.file_like = path;
            let request = RankRequest {
                rank_sql: c"SELECT q.rowid FROM                     (SELECT rowid, path FROM needle WHERE needle MATCH ?1                      ORDER BY bm25(needle), rowid LIMIT ?5) q                      WHERE (?6 IS NULL OR path LIKE ?6) LIMIT ?3 OFFSET ?4"
                    .as_ptr(),
                count_sql: c"SELECT count(*) FROM                     (SELECT rowid, path FROM needle WHERE needle MATCH ?1                      ORDER BY bm25(needle), rowid LIMIT ?3) q                      WHERE (?6 IS NULL OR path LIKE ?6)"
                    .as_ptr(),
                probe_sql: c"SELECT rowid FROM needle WHERE needle MATCH ?1                      ORDER BY bm25(needle), rowid LIMIT 1 OFFSET ?2"
                    .as_ptr(),
                filters: &filters,
                exact_query: c"needle".as_ptr(),
                limit: 5,
                offset: 0,
                candidate_limit: 2,
            };
            let mut total = 0;
            let mut saturated = false;
            let mut output = ResultRows::default();
            // SAFETY: The connection, request, and outputs stay live until return.
            let handled = unsafe {
                read_ranked(
                    db.handle(),
                    &request,
                    &mut total,
                    &mut saturated,
                    ptr::from_mut(&mut output).cast(),
                    capture,
                )
            };
            assert!(handled);
            assert_eq!(output.ids, expected_ids);
            assert_eq!(total, expected_total);
            assert_eq!(saturated, expected_window);
        }
    }
}
