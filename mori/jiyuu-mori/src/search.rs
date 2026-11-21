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

type RankReader = unsafe extern "C" fn(
    *mut sqlite3,
    *const RankRequest,
    *mut c_int,
    *mut bool,
    *mut c_void,
    EmitRow,
) -> bool;

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
    unsafe fn bind_text(&self, index: c_int, value: *const c_char) {
        // SAFETY: SQLite copies the native plan's string before returning.
        unsafe {
            ffi::sqlite3_bind_text(
                self.0,
                index,
                value,
                -1,
                ffi::SQLITE_TRANSIENT(),
            );
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

unsafe extern "C" {
    fn cbm_store_set_pattern_search_reader(reader: PatternReader);
    fn cbm_mcp_set_rank_search_reader(reader: RankReader);
}

/// Register during process startup, before native worker threads can start.
pub(crate) unsafe fn register() {
    // SAFETY: The registered function lives for the process lifetime.
    unsafe {
        cbm_store_set_pattern_search_reader(read_pattern);
        cbm_mcp_set_rank_search_reader(read_ranked);
    }
}
