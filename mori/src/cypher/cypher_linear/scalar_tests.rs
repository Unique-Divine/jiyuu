use std::cell::Cell;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr;
use std::slice;
use std::time::Instant;

use rusqlite::ffi::sqlite3;

use super::{
    Connection, ENGINE_LIMIT, ExecutionBudget, ExecutionError, classify,
};

#[repr(C)]
#[derive(Default)]
struct NativeResult {
    columns: *const *const c_char,
    col_count: c_int,
    rows: *const *const *const c_char,
    row_count: c_int,
    truncated: bool,
    error: *const c_char,
    warning: *const c_char,
}

#[repr(C)]
struct NativeBudget {
    deadline_ms: u64,
    cancelled: unsafe extern "C" fn(*mut c_void) -> bool,
    context: *mut c_void,
}

unsafe extern "C" {
    fn cbm_store_open_memory() -> *mut c_void;
    fn cbm_store_get_db(store: *mut c_void) -> *mut sqlite3;
    fn cbm_store_close(store: *mut c_void);
    fn cbm_cypher_execute(
        store: *mut c_void,
        query: *const c_char,
        project: *const c_char,
        max_rows: c_int,
        out: *mut NativeResult,
    ) -> c_int;
    fn cbm_cypher_budget_start(
        cancelled: unsafe extern "C" fn(*mut c_void) -> bool,
        context: *mut c_void,
    ) -> NativeBudget;
    fn cbm_cypher_execute_with_budget(
        store: *mut c_void,
        query: *const c_char,
        project: *const c_char,
        max_rows: c_int,
        out: *mut NativeResult,
        budget: *const NativeBudget,
    ) -> c_int;
    fn cbm_cypher_result_free(out: *mut NativeResult);
}

impl Drop for NativeResult {
    fn drop(&mut self) {
        unsafe { cbm_cypher_result_free(self) }
    }
}

impl NativeResult {
    fn error(&self) -> Option<&[u8]> {
        (!self.error.is_null())
            .then(|| unsafe { CStr::from_ptr(self.error) }.to_bytes())
    }

    fn columns(&self) -> Vec<&[u8]> {
        unsafe { cells(self.columns, self.col_count) }
    }

    fn rows(&self) -> Vec<Vec<&[u8]>> {
        if self.row_count == 0 {
            return vec![];
        }
        unsafe { slice::from_raw_parts(self.rows, self.row_count as usize) }
            .iter()
            .map(|&row| unsafe { cells(row, self.col_count) })
            .collect()
    }
}

// SAFETY: Every entry in the native result's length-delimited array is a
// NUL-terminated cell owned by the result.
unsafe fn cells<'a>(
    values: *const *const c_char,
    count: c_int,
) -> Vec<&'a [u8]> {
    if count == 0 {
        return vec![];
    }
    unsafe { slice::from_raw_parts(values, count as usize) }
        .iter()
        .map(|&value| unsafe { CStr::from_ptr(value) }.to_bytes())
        .collect()
}

struct NativeStore(*mut c_void);

impl Drop for NativeStore {
    fn drop(&mut self) {
        unsafe { cbm_store_close(self.0) }
    }
}

struct Fixture {
    // Drop the borrowed connection before the native store that owns it.
    connection: Connection,
    store: NativeStore,
}

impl Fixture {
    fn new() -> Self {
        let store = NativeStore(unsafe { cbm_store_open_memory() });
        assert!(!store.0.is_null());
        let fixture = Self {
            connection: unsafe {
                Connection::from_handle(cbm_store_get_db(store.0))
            }
            .unwrap(),
            store,
        };
        fixture.connection.execute_batch(
            "INSERT INTO projects VALUES ('p', 'now', '/tmp');
             INSERT INTO nodes(project, label, name, qualified_name, properties) VALUES
                ('p', 'Probe', 'éclair', 'first', '{\"offset\":\" 1suffix\"}'),
                ('p', 'Probe', 'éclat', 'second', '{}'),
                ('p', 'Probe', 'école', 'third', '{}'),
                ('p', 'Binary', CAST(X'FF6120620063' AS TEXT), 'binary', '{}');
             INSERT INTO edges(project, source_id, target_id, type, properties)
                VALUES ('p', 1, 2, 'CALLS', '{\"suffix\":\"air\"}');",
        ).unwrap();
        fixture
    }

    fn native(&self, source: &str) -> NativeResult {
        let source = CString::new(source).unwrap();
        let mut result = NativeResult::default();
        let status = unsafe {
            cbm_cypher_execute(
                self.store.0,
                source.as_ptr(),
                c"p".as_ptr(),
                0,
                &mut result,
            )
        };
        assert_eq!(status == 0, result.error().is_none());
        result
    }

    fn compare(&self, source: &str, expected: &[&[&[u8]]]) {
        let native = self.native(source);
        assert_eq!(native.error(), None, "{source}");
        assert_eq!(native.rows(), expected, "{source}");
        let rust = classify(source)
            .unwrap_or_else(|error| panic!("{source}: {error:?}"))
            .execute_on(&self.connection, "p")
            .unwrap();
        assert_eq!(rust.rows, native.rows(), "{source}");
        assert_eq!(
            rust.columns
                .iter()
                .map(|name| name.as_bytes())
                .collect::<Vec<_>>(),
            native.columns(),
            "{source}"
        );
        assert_eq!(rust.truncated, native.truncated, "{source}");
    }
}

#[test]
fn scalar_bytes_arity_and_stable_limit() {
    let fixture = Fixture::new();
    fixture.compare(
        "MATCH (n:Probe {qn: 'first'}) RETURN \
         LeFt(n.name, n.offset), right(n.name, 4, 'ignored') AS suffix, \
         replace(n.name, 'air', 'at') AS replacement, substring(n.name, 1, 1) AS split, \
         coalesce(n.missing, n.name) AS original",
        &[&[b"\xc3", b"lair", "éclat".as_bytes(), b"\xa9", "éclair".as_bytes()]],
    );
    fixture.compare(
        "MATCH (n:Probe {qn: 'first'}) RETURN replace(n.name, '') AS missing, \
         left(n.name, ' -1') AS negative, coalesce() AS empty",
        &[&[b"", b"", b""]],
    );
    fixture.compare(
        "MATCH (n:Binary) RETURN replace(n.name, 'a', '') AS removed, \
         replace(n.name, '', 'ignored') AS unchanged, \
         right(n.name, '9999999999999999999999999') AS all_bytes, \
         coalesce(n.name, 'fallback') AS original",
        &[&[b"\xff b", b"\xffa b", b"\xffa b", b"\xffa b"]],
    );
    fixture.compare(
        "MATCH (n:Probe) RETURN left(n.name, 2) AS prefix, n.qn AS qn \
         ORDER BY prefix DESC SKIP 1 LIMIT 1",
        &[&["é".as_bytes(), b"second"]],
    );
    fixture.compare(
        "MATCH (a:Probe)-[r:CALLS]->(b) WHERE replace(a.name, r.suffix, 'at') = b.name \
         WITH right(b.name, 2) AS tail WHERE left(tail, 1) = 'a' RETURN tail",
        &[&[b"at"]],
    );
    let long = "abcdef".repeat(200);
    let query = format!(
        "MATCH (n:Binary) RETURN coalesce('{long}') AS full, \
         right('{long}', 700) AS suffix, replace('{long}', 'a', 'AA') AS replacement"
    );
    let expanded = long.replace('a', "AA");
    fixture.compare(
        &query,
        &[&[
            long.as_bytes(),
            &long.as_bytes()[long.len() - 511..],
            &expanded.as_bytes()[..511],
        ]],
    );
}

#[test]
fn scalar_native_fallback_and_parser_offset() {
    let fixture = Fixture::new();
    let fallback = "MATCH (n:Probe {qn: 'first'}) RETURN \
        CASE WHEN right(n.name, 3) = 'air' THEN 'yes' ELSE 'no' END AS result";
    assert!(classify(fallback).is_err());
    assert_eq!(fixture.native(fallback).rows(), [vec![&b"yes"[..]]]);

    let wide = format!(
        "MATCH (a)-[r:CALLS]->(b) RETURN r.absent, {}",
        (0..8)
            .map(|i| format!("left(r.suffix, 1) AS col{i}"))
            .collect::<Vec<_>>()
            .join(", "),
    );
    assert!(classify(&wide).is_err());
    let expected: Vec<&[u8]> = [&b"air"[..]]
        .into_iter()
        .chain(std::iter::repeat_n(&b"a"[..], 8))
        .collect();
    assert_eq!(fixture.native(&wide).rows(), [expected]);

    let malformed = "MATCH (n:Probe {name: 'éclair'}) RETURN left(n.name,)";
    assert!(classify(malformed).unwrap_err().diagnostic().is_none());
    let native = fixture.native(malformed);
    let error = format!(
        "expected token type 85, got 67 at pos {}",
        malformed.len() - 1
    );
    assert_eq!(native.error(), Some(error.as_bytes()));
}

#[test]
fn scalar_engine_ceiling_and_cancellation() {
    let fixture = Fixture::new();
    fixture.connection.execute(
        "WITH RECURSIVE seq(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM seq WHERE x < ?1)
         INSERT INTO nodes(project, label, name, qualified_name)
         SELECT 'p', 'Many', 'value', 'many-' || x FROM seq",
        [ENGINE_LIMIT as i64],
    ).unwrap();
    let source = "MATCH (n:Many) RETURN right(n.name, 3) AS tail";
    let query = classify(source).unwrap();
    let native = fixture.native(source);
    assert!(native.truncated);
    assert_eq!(
        native.error(),
        Some(
            "result exceeded 100k rows — use narrower filters or add LIMIT"
                .as_bytes()
        )
    );
    assert!(matches!(
        query.execute_on(&fixture.connection, "p"),
        Err(ExecutionError::RowCeiling)
    ));

    unsafe extern "C" fn cancel(context: *mut c_void) -> bool {
        let checks = unsafe { &*context.cast::<Cell<usize>>() };
        checks.set(checks.get() + 1);
        checks.get() >= 4
    }
    let checks = Cell::new(0usize);
    let context = ptr::from_ref(&checks).cast_mut().cast();
    let mut native = NativeResult::default();
    let source = CString::new(source).unwrap();
    unsafe {
        let budget = cbm_cypher_budget_start(cancel, context);
        cbm_cypher_execute_with_budget(
            fixture.store.0,
            source.as_ptr(),
            c"p".as_ptr(),
            0,
            &mut native,
            &budget,
        );
    }
    assert_eq!(
        native.error(),
        Some(&b"query_graph cancelled for this request"[..])
    );
    assert_eq!(native.row_count, 0);

    checks.set(0);
    let cancelled = || unsafe { cancel(context) };
    let mut budget = ExecutionBudget::new(&Instant::now, &cancelled);
    assert!(matches!(
        query.execute_with_budget(&fixture.connection, "p", &mut budget),
        Err(ExecutionError::Cancelled)
    ));
}
