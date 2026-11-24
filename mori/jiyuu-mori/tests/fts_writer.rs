//! Exercise the linked FTS writer on the store's live SQLite connection.
#![cfg(feature = "c-backend")]

use std::ffi::{CStr, c_char, c_int};
use std::ptr::NonNull;

use jiyuu_mori as _;
use rusqlite::Connection;
use rusqlite::ffi::sqlite3;

const STORE_OK: c_int = 0;
const STORE_ERR: c_int = -1;
const STORE_NOT_FOUND: c_int = -2;

#[repr(C)]
struct Store {
    _opaque: [u8; 0],
}

unsafe extern "C" {
    fn cbm_alloc_init();
    fn cbm_store_open_memory() -> *mut Store;
    fn cbm_store_get_db(store: *mut Store) -> *mut sqlite3;
    fn cbm_store_close(store: *mut Store);
    fn cbm_store_fts_rebuild(
        store: *mut Store,
        project: *const c_char,
        after_id: i64,
    ) -> c_int;
    fn cbm_store_error(store: *mut Store) -> *const c_char;
}

struct StoreGuard(NonNull<Store>);

impl StoreGuard {
    fn new() -> Self {
        // SAFETY: The test initializes the native runtime before opening a store.
        Self(NonNull::new(unsafe { cbm_store_open_memory() }).unwrap())
    }

    fn rebuild(&self, project: Option<&CStr>, after_id: i64) -> c_int {
        let project = project.map_or(std::ptr::null(), CStr::as_ptr);
        // SAFETY: The store and project string remain live during the call.
        unsafe { cbm_store_fts_rebuild(self.0.as_ptr(), project, after_id) }
    }

    fn error(&self) -> String {
        // SAFETY: C retains the terminated error text on the live store.
        unsafe { CStr::from_ptr(cbm_store_error(self.0.as_ptr())) }
            .to_string_lossy()
            .into_owned()
    }
}

impl Drop for StoreGuard {
    fn drop(&mut self) {
        // SAFETY: This guard uniquely owns the store.
        unsafe { cbm_store_close(self.0.as_ptr()) };
    }
}

fn matches(connection: &Connection, term: &str) -> i64 {
    connection
        .query_row(
            "SELECT count(*) FROM nodes_fts WHERE nodes_fts MATCH ?1",
            [term],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn fts_writer_preserves_legacy_schema_and_failure_statuses() {
    // SAFETY: This is the sole native test in this process and no stores exist yet.
    unsafe { cbm_alloc_init() };
    let store = StoreGuard::new();
    // SAFETY: The borrowed connection is dropped before the native store.
    // from_handle does not close the connection.
    let db =
        unsafe { Connection::from_handle(cbm_store_get_db(store.0.as_ptr())) }
            .unwrap();
    db.execute_batch(
        r#"INSERT INTO projects (name, indexed_at, root_path) VALUES
              ('p', '2026-01-01', '/tmp/p'), ('q', '2026-01-01', '/tmp/q');
           INSERT INTO nodes (id, project, label, name, qualified_name, properties) VALUES
             (1, 'p', 'Function', 'camelLedger', 'p.camelLedger',
              '{"docstring":"retention proof"}'),
             (2, 'p', 'Macro', 'ASSIST', 'p.ASSIST#macro', '{}'),
             (3, 'p', 'Function', 'broken', 'p.broken', '{"docstring":');"#,
    )
    .unwrap();
    assert_eq!(store.rebuild(None, 0), STORE_OK);
    assert_eq!(matches(&db, "body:retention"), 1);
    assert_eq!(matches(&db, "name:ledger"), 1);
    assert_eq!(matches(&db, "qualified_name:macro"), 0);
    assert_eq!(matches(&db, "name:broken"), 1);

    db.execute_batch(
        r#"BEGIN;
           UPDATE nodes SET properties = '{"docstring":"rebuildproof"}' WHERE id = 1;"#,
    )
    .unwrap();
    assert_eq!(store.rebuild(None, 0), STORE_OK);
    assert!(!db.is_autocommit());
    assert_eq!(matches(&db, "body:rebuildproof"), 1);
    assert_eq!(matches(&db, "body:retention"), 0);
    db.execute_batch("ROLLBACK").unwrap();
    assert_eq!(matches(&db, "body:rebuildproof"), 0);
    assert_eq!(matches(&db, "body:retention"), 1);

    db.execute_batch(
        "DROP TABLE nodes_fts;
         CREATE VIRTUAL TABLE nodes_fts USING fts5(
             name, qualified_name, label, file_path, content='');",
    )
    .unwrap();
    assert_eq!(store.rebuild(None, 0), STORE_OK);
    db.execute_batch(
        "BEGIN;
         INSERT INTO nodes (id, project, label, name, qualified_name) VALUES
            (4, 'p', 'Function', 'uniqueappend', 'p.uniqueappend'),
            (5, 'q', 'Function', 'otherappend', 'q.otherappend');",
    )
    .unwrap();
    assert_eq!(store.rebuild(Some(c"p"), 3), STORE_OK);
    assert!(!db.is_autocommit());
    assert_eq!(matches(&db, "name:ledger"), 1);
    assert_eq!(matches(&db, "name:uniqueappend"), 1);
    assert_eq!(matches(&db, "name:otherappend"), 0);
    db.execute_batch("ROLLBACK").unwrap();
    assert_eq!(matches(&db, "name:uniqueappend"), 0);

    db.execute_batch("DROP TABLE nodes_fts").unwrap();
    assert_eq!(store.rebuild(Some(c"p"), 0), STORE_NOT_FOUND);
    assert_eq!(store.rebuild(None, 0), STORE_ERR);
    assert_eq!(store.error(), "exec: no such table: nodes_fts");

    db.execute_batch(
        "CREATE TABLE nodes_fts (
             name TEXT CHECK (name = 'never matches'),
             qualified_name TEXT, label TEXT, file_path TEXT, body TEXT);
         BEGIN;",
    )
    .unwrap();
    assert_eq!(store.rebuild(Some(c"p"), 0), STORE_ERR);
    assert!(!db.is_autocommit());
    assert_eq!(
        store.error(),
        "fts_backfill: CHECK constraint failed: name = 'never matches'"
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM nodes_fts", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    db.execute_batch("ROLLBACK").unwrap();
}
