//! Recompute complexity after the closure patch restores the complete graph.

use super::{CallGraph, NodeKey, PropertyMode};
use rusqlite::{
    Connection, TransactionBehavior, params,
    types::{ToSqlOutput, ValueRef},
};

const PROJECT_MAX_ID: &str =
    "SELECT COALESCE(MAX(id), 0) FROM nodes WHERE project = ?1";
const NODES: &str = "
    SELECT id, label IN ('Function','Method'), CAST(qualified_name AS TEXT),
           CAST(file_path AS TEXT), start_line, CAST(properties AS TEXT)
    FROM nodes WHERE project = ?1
    AND (label IN ('Function','Method') OR id IN (
        SELECT target_id FROM edges WHERE project = ?1 AND type = 'CALLS'
    ))";
const CALLS: &str = "SELECT source_id, target_id FROM edges WHERE project = ?1 AND type = 'CALLS' ORDER BY id";

fn text(value: ValueRef<'_>, default: &[u8]) -> Vec<u8> {
    match value {
        ValueRef::Text(bytes) => bytes
            .split(|byte| *byte == 0)
            .next()
            .unwrap_or_default()
            .to_vec(),
        _ => default.to_vec(),
    }
}

pub(super) fn recompute(
    stage: &mut Connection,
    project: &[u8],
) -> rusqlite::Result<usize> {
    // Keep the snapshot and updates in the same transaction. Closure
    // publication has not begun and no other writer should touch this stage.
    let tx = stage.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let project = ToSqlOutput::Borrowed(ValueRef::Text(project));
    let max_id: i64 =
        tx.query_row(PROJECT_MAX_ID, [&project], |row| row.get(0))?;
    if max_id < 1 {
        return Ok(0);
    }

    let mut graph = CallGraph::default();
    {
        let mut stmt = tx.prepare(NODES)?;
        let mut rows = stmt.query([&project])?;
        while let Some(row) = rows.next()? {
            let id = row.get(0)?;
            if id < 1 {
                return Err(rusqlite::Error::InvalidQuery);
            }
            let seed: bool = row.get(1)?;
            graph.set_node(
                id,
                NodeKey {
                    qualified_name: text(row.get_ref(2)?, b""),
                    file_path: text(row.get_ref(3)?, b""),
                    start_line: row.get::<_, Option<i64>>(4)?.unwrap_or(0)
                        as i32,
                },
                if seed {
                    Some(text(row.get_ref(5)?, b"{}"))
                } else {
                    None
                },
            );
        }
    }
    {
        let mut stmt = tx.prepare(CALLS)?;
        let mut rows = stmt.query([&project])?;
        while let Some(row) = rows.next()? {
            let source = row.get(0)?;
            let target = row.get(1)?;
            if !(1..=max_id).contains(&source) {
                return Err(rusqlite::Error::InvalidQuery);
            }
            if (1..=max_id).contains(&target) {
                graph.add_call(source, target);
            }
        }
    }

    let updates = graph
        .updates(PropertyMode::Rewrite)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    {
        let mut stmt = tx.prepare(
            "UPDATE nodes SET properties = ?1 WHERE id = ?2 AND project = ?3",
        )?;
        for update in &updates {
            if stmt.execute(params![
                ToSqlOutput::Borrowed(ValueRef::Text(&update.properties)),
                update.id,
                project
            ])? != 1
            {
                return Err(rusqlite::Error::QueryReturnedNoRows);
            }
        }
    }
    tx.commit()?;
    Ok(updates.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Connection {
        let stage = Connection::open_in_memory().unwrap();
        stage.execute_batch(
            r#"
            CREATE TABLE nodes (
                id INTEGER PRIMARY KEY, project TEXT, label TEXT, qualified_name TEXT,
                file_path TEXT, start_line INTEGER, properties TEXT
            );
            CREATE TABLE edges (
                id INTEGER PRIMARY KEY, project TEXT, source_id INTEGER, target_id INTEGER, type TEXT
            );
            INSERT INTO nodes VALUES
                (50, 'repo', 'Function', 'outer', 'outer.py', 1,
                 '{"loop_depth":1,"transitive_loop_depth":2,"recursive":false,"importance":7.0}'),
                (900000000, 'repo', 'Method', 'leaf', NULL, NULL, '{"loop_depth":3}'),
                (60, 'other', 'Function', 'unrelated', 'other.py', 1, '{}');
            INSERT INTO edges VALUES
                (1, 'repo', 50, 900000000, 'CALLS'),
                (2, 'repo', 50, 50, 'USAGE');
            "#,
        )
        .unwrap();
        stage
    }

    fn properties(stage: &Connection, id: i64) -> String {
        stage
            .query_row(
                "SELECT properties FROM nodes WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[test]
    fn closure_updates_an_unchanged_caller_and_the_reextracted_callee() {
        let mut stage = fixture();
        for _ in 0..2 {
            assert_eq!(recompute(&mut stage, b"repo").unwrap(), 2);
            assert_eq!(
                properties(&stage, 50),
                r#"{"loop_depth":1,"transitive_loop_depth":4,"recursive":false,"importance":7.0}"#
            );
            assert_eq!(
                properties(&stage, 900000000),
                r#"{"loop_depth":3,"transitive_loop_depth":3,"recursive":false}"#
            );
        }
        assert_eq!(properties(&stage, 60), "{}");
        assert_eq!(recompute(&mut stage, b"empty").unwrap(), 0);
        assert!(stage.is_autocommit());
    }

    #[test]
    fn closure_rolls_back_if_a_later_update_fails() {
        let mut stage = fixture();
        stage.execute_batch(
            "CREATE TRIGGER fail_outer BEFORE UPDATE ON nodes WHEN OLD.id = 50
             BEGIN SELECT RAISE(ABORT, 'outer update failed'); END;",
        ).unwrap();
        assert!(recompute(&mut stage, b"repo").is_err());
        assert_eq!(properties(&stage, 900000000), r#"{"loop_depth":3}"#);
        assert!(stage.is_autocommit());
    }
}
