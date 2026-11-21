//! Forward-row decisions and stale-row deletion for GraphQL cross-repo linking.

#[cfg(feature = "c-backend")]
use rusqlite::{Connection, Result};

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

#[cfg(test)]
mod tests {
    use super::{Action, plan_link};

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
