//! GraphQL cross-repo link decisions and edge writes.

#[cfg(feature = "c-backend")]
use std::ffi::CStr;

#[cfg(feature = "c-backend")]
use rusqlite::{Connection, Result, ffi};

/// ABI mirror of `cbm_cross_route_action_t` in `pass_cross_repo.h`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub(crate) enum Action {
    Keep,
    Drop,
    LinkAmbiguous,
    LinkResolved,
}

/// Provider-side calls reach this function only for an existing row owned by
/// that provider. An ambiguous caller-side candidate can replace another
/// ambiguous row, but never a resolved row owned by another target.
pub(crate) extern "C" fn plan_link(
    provider_side: bool,
    has_handler: bool,
    ambiguous: bool,
    names_other: bool,
    existing_ambiguous: bool,
) -> Action {
    match (has_handler, ambiguous) {
        (false, _) if provider_side => Action::Drop,
        (false, _) => Action::Keep,
        (true, false) => Action::LinkResolved,
        (true, true) if !provider_side && names_other && !existing_ambiguous => {
            Action::Keep
        }
        (true, true) => Action::LinkAmbiguous,
    }
}

/// Use the caller's connection so this delete keeps its SQLite transaction
/// and progress handler. The link pass has already checked provider ownership.
#[cfg(feature = "c-backend")]
pub(crate) fn delete_stale_link(
    db: &Connection,
    caller: i64,
    route: i64,
) -> Result<bool> {
    db.execute(
        "DELETE FROM edges WHERE source_id = ?1 AND target_id = ?2 AND type = 'CROSS_GRAPHQL_CALLS'",
        (caller, route),
    )
    .map(|count| count > 0)
}

/// Write one GraphQL edge using the graph store's cached upsert statement.
///
/// The caller lends a live, reset edge-upsert statement and retains exclusive
/// ownership for the whole call. This function resets it before returning.
#[cfg(feature = "c-backend")]
pub(crate) unsafe fn upsert_link(
    stmt: *mut ffi::sqlite3_stmt,
    project: &CStr,
    source: i64,
    target: i64,
    properties: &CStr,
) -> bool {
    let properties = if properties.is_empty() {
        c"{}"
    } else {
        properties
    };
    // SAFETY: The store owns the live statement. SQLite copies these
    // NUL-terminated strings because the cached statement outlives the call.
    unsafe {
        let bind_text = |index, text: &CStr| {
            ffi::sqlite3_bind_text(
                stmt,
                index,
                text.as_ptr(),
                -1,
                ffi::SQLITE_TRANSIENT(),
            ) == ffi::SQLITE_OK
        };
        let inserted = bind_text(1, project)
            && ffi::sqlite3_bind_int64(stmt, 2, source) == ffi::SQLITE_OK
            && ffi::sqlite3_bind_int64(stmt, 3, target) == ffi::SQLITE_OK
            && bind_text(4, c"CROSS_GRAPHQL_CALLS")
            && bind_text(5, properties)
            && ffi::sqlite3_step(stmt) == ffi::SQLITE_ROW
            && ffi::sqlite3_column_int64(stmt, 0) > 0;
        // The C writer also resets after reading RETURNING rather than keeping
        // the statement active across a transaction boundary.
        ffi::sqlite3_reset(stmt);
        inserted
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, plan_link};

    #[cfg(feature = "c-backend")]
    #[test]
    fn upsert_reuses_one_edge_and_keeps_both_callers() {
        use rusqlite::{Connection, ffi};
        use std::ptr::null_mut;

        let mut db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE edges(
                id INTEGER PRIMARY KEY, project TEXT, source_id INT, target_id INT,
                type TEXT, properties TEXT, local_name_gen TEXT GENERATED ALWAYS AS (''),
                UNIQUE (source_id, target_id, type, local_name_gen)
            );",
        )
        .unwrap();
        let txn = db.transaction().unwrap();
        let mut stmt = null_mut();
        let sql = c"INSERT INTO edges (project, source_id, target_id, type, properties)
                    VALUES (?1, ?2, ?3, ?4, ?5)
                    ON CONFLICT(source_id, target_id, type, local_name_gen) DO UPDATE SET
                    properties = json_patch(properties, ?5) RETURNING id;";
        // SAFETY: The transaction owns a live handle. Finalize the prepared
        // statement before querying or rolling back the result.
        let writes = unsafe {
            assert_eq!(
                ffi::sqlite3_prepare_v2(
                    txn.handle(),
                    sql.as_ptr(),
                    -1,
                    &mut stmt,
                    null_mut()
                ),
                ffi::SQLITE_OK
            );
            let first = c"{\"callers\":{\"client:first\":{\"line\":1}}}";
            let second = c"{\"callers\":{\"client:second\":{\"line\":2}}}";
            let writes = [first, second, second].map(|properties| {
                super::upsert_link(stmt, c"server", 1, 2, properties)
            });
            assert_eq!(ffi::sqlite3_finalize(stmt), ffi::SQLITE_OK);
            writes
        };
        assert_eq!(writes, [true; 3]);
        let edge: (i64, String, String, String) = txn
            .query_row(
                "SELECT count(*), project, type, properties FROM edges",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(edge.0, 1);
        assert_eq!(edge.1, "server");
        assert_eq!(edge.2, "CROSS_GRAPHQL_CALLS");
        assert_eq!(
            edge.3,
            r#"{"callers":{"client:first":{"line":1},"client:second":{"line":2}}}"#
        );
        txn.rollback().unwrap();
        let count: i64 = db
            .query_row("SELECT count(*) FROM edges", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[cfg(feature = "c-backend")]
    #[test]
    fn deletes_only_the_requested_graphql_edge_in_the_existing_transaction() {
        let mut db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE edges(source_id, target_id, type);
             INSERT INTO edges VALUES
                 (1, 2, 'CROSS_GRAPHQL_CALLS'),
                 (1, 3, 'CROSS_GRAPHQL_CALLS'),
                 (3, 2, 'CROSS_GRAPHQL_CALLS'),
                 (1, 2, 'CROSS_GRPC_CALLS');",
        )
        .unwrap();

        let txn = db.transaction().unwrap();
        assert!(super::delete_stale_link(&txn, 1, 2).unwrap());
        assert!(!super::delete_stale_link(&txn, 1, 2).unwrap());
        let remaining: (i64, i64) = txn
            .query_row(
                "SELECT count(*), sum(source_id = 1 AND target_id = 2 AND type = 'CROSS_GRAPHQL_CALLS')
                 FROM edges",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(remaining, (3, 0));
        txn.rollback().unwrap();
        let restored: i64 = db
            .query_row("SELECT count(*) FROM edges", [], |row| row.get(0))
            .unwrap();
        assert_eq!(restored, 4);
    }

    #[test]
    fn caller_preserves_a_resolved_row_when_another_target_is_ambiguous() {
        assert_eq!(plan_link(false, true, true, true, false), Action::Keep);
        assert_eq!(
            plan_link(false, true, true, true, true),
            Action::LinkAmbiguous
        );
        assert_eq!(
            plan_link(false, true, false, true, true),
            Action::LinkResolved
        );
    }

    #[test]
    fn only_provider_relinks_drop_a_forward_row_without_a_handler() {
        assert_eq!(plan_link(true, false, false, false, false), Action::Drop);
        assert_eq!(plan_link(false, false, false, false, false), Action::Keep);
        assert_eq!(
            plan_link(true, true, true, false, false),
            Action::LinkAmbiguous
        );
    }
}
