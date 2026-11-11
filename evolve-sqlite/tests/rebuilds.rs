use evolve_sqlite::{
    CheckError, Error, Migration, RebuildPolicy, RehearsalPaths, Spec,
};
use rusqlite::Connection;
use tempfile::{NamedTempFile, tempdir};

type TestResult<T = ()> = Result<T, CheckError>;

const INITIAL: Migration<'_> = Migration {
    name: "01_initial.sql",
    sql: "CREATE TABLE parent (id INTEGER PRIMARY KEY, label TEXT NOT NULL);
          CREATE TABLE child (id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES parent(id) ON DELETE CASCADE);
          INSERT INTO parent VALUES (1, 'custom description');
          INSERT INTO child VALUES (10, 1);",
};
const REBUILD: Migration<'_> = Migration {
    name: "02_rebuild.sql",
    sql: "PRAGMA legacy_alter_table=ON;
          CREATE TABLE replacement (id INTEGER PRIMARY KEY, label TEXT NOT NULL, note TEXT NOT NULL DEFAULT '');
          INSERT INTO replacement(id, label) SELECT id, label FROM parent;
          DROP TABLE parent;
          ALTER TABLE replacement RENAME TO parent;
          PRAGMA legacy_alter_table=OFF;",
};

fn spec<'a>(migrations: &'a [Migration<'a>]) -> Spec<'a> {
    Spec {
        app_id: 0x4556_4f4c,
        migrations,
    }
}

fn children(conn: &Connection) -> TestResult<Vec<i64>> {
    Ok(conn
        .prepare("SELECT id FROM child ORDER BY id")?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

/// Rebuilding with FK enforcement on can silently cascade-delete child rows.
/// A reviewed rebuild preserves children and customized parent values on both
/// the rehearsal copy and live transaction.
#[test]
fn verified_rebuild_preserves_children_and_customizations() -> TestResult {
    let db = NamedTempFile::new()?;
    let initial = [INITIAL];
    let first = spec(&initial);
    first.apply(&first.plan(db.path())?, |_| Ok(()), |_, ()| Ok(()))?;
    let migrations = [INITIAL, REBUILD];
    let latest = spec(&migrations);
    let policy = RebuildPolicy::new(&["PARENT", "parent"])?;
    let plan = latest.plan_with_rebuilds(db.path(), &policy)?;
    assert_eq!(plan.rebuilds().tables(), &["parent"]);
    assert_ne!(plan, latest.plan(db.path())?);
    let dir = tempdir()?;
    let paths = RehearsalPaths {
        snapshot: dir.path().join("snapshot.db"),
        copy: dir.path().join("copy.db"),
    };
    latest.apply_verified(&plan, &paths, children, |conn, before| {
        assert_eq!(children(conn)?, before);
        let label: String =
            conn.query_row("SELECT label FROM parent WHERE id=1", [], |r| {
                r.get(0)
            })?;
        assert_eq!(label, "custom description");
        Ok(())
    })?;
    for path in [db.path(), paths.copy.as_path()] {
        let conn = Connection::open(path)?;
        assert_eq!(children(&conn)?, [10]);
        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
            2
        );
    }
    Ok(())
}

/// A generic strict plan and an allowlist for another table must both reject
/// the drop, rolling back replacement DDL and the migration-history row.
#[test]
fn unreviewed_drop_rolls_back_every_statement() -> TestResult {
    for policy in [RebuildPolicy::default(), RebuildPolicy::new(&["child"])?] {
        let db = NamedTempFile::new()?;
        let migrations = [INITIAL, REBUILD];
        let target = spec(&migrations);
        let plan = target.plan_with_rebuilds(db.path(), &policy)?;
        assert!(matches!(
            target.apply(&plan, |_| Ok(()), |_, ()| Ok(())),
            Err(Error::Migration { .. })
        ));
        let conn = Connection::open(db.path())?;
        let count: i64 =
            conn.query_row("SELECT count(*) FROM sqlite_schema", [], |r| {
                r.get(0)
            })?;
        assert_eq!(count, 0);
    }
    Ok(())
}

/// Disabling FK enforcement is safe only when the complete transaction is
/// rejected for broken references, including engine identity and history.
#[test]
fn broken_references_reject_rebuild_before_commit() -> TestResult {
    let db = NamedTempFile::new()?;
    let invalid = Migration {
        name: "03_orphan.sql",
        sql: "DELETE FROM parent;",
    };
    let migrations = [INITIAL, REBUILD, invalid];
    let target = spec(&migrations);
    let plan = target
        .plan_with_rebuilds(db.path(), &RebuildPolicy::new(&["parent"])?)?;
    assert!(matches!(
        target.apply(&plan, |_| Ok(()), |_, ()| Ok(())),
        Err(Error::ForeignKeyViolation)
    ));
    assert!(target.plan(db.path())?.adopted().is_empty());
    assert_eq!(target.plan(db.path())?.pending().len(), 3);
    Ok(())
}

/// Rebuild permission cannot authorize engine metadata writes or transaction
/// escape, and project verifiers remain read-only even in rebuild mode.
#[test]
fn rebuild_permission_keeps_other_sql_boundaries() -> TestResult {
    for sql in [
        "PRAGMA foreign_keys=ON;",
        "DELETE FROM evolve_sqlite_migration;",
        "COMMIT;",
        "DROP TABLE child;",
    ] {
        let db = NamedTempFile::new()?;
        let migrations = [
            INITIAL,
            Migration {
                name: "02_forbidden.sql",
                sql,
            },
        ];
        let target = spec(&migrations);
        let plan = target
            .plan_with_rebuilds(db.path(), &RebuildPolicy::new(&["parent"])?)?;
        assert!(matches!(
            target.apply(&plan, |_| Ok(()), |_, ()| Ok(())),
            Err(Error::Migration { .. })
        ));
    }
    let db = NamedTempFile::new()?;
    let migrations = [INITIAL, REBUILD];
    let target = spec(&migrations);
    let plan = target
        .plan_with_rebuilds(db.path(), &RebuildPolicy::new(&["parent"])?)?;
    assert!(matches!(
        target.apply(
            &plan,
            |_| Ok(()),
            |conn, ()| {
                conn.execute("DELETE FROM parent", [])?;
                Ok(())
            }
        ),
        Err(Error::Verification(_))
    ));
    assert_eq!(target.plan(db.path())?.pending().len(), 2);
    Ok(())
}

/// A policy cannot name attached databases, SQLite internals, or engine history.
#[test]
fn rejects_reserved_and_qualified_rebuild_names() -> TestResult {
    for name in [
        "",
        "main.parent",
        "sqlite_schema",
        "EVOLVE_SQLITE_MIGRATION",
        "a b",
        "1table",
    ] {
        assert!(matches!(
            RebuildPolicy::new(&[name]),
            Err(Error::InvalidSpec(_))
        ));
    }
    Ok(())
}

/// Historical ALTER ADD CHECK uses SQLite's internal read-only quick_check.
/// Permitting that inspection must not grant temporary object or data access.
#[test]
fn checked_column_addition_keeps_temp_database_inaccessible() -> TestResult {
    let db = NamedTempFile::new()?;
    let migrations = [
        INITIAL,
        REBUILD,
        Migration {
            name: "03_checked_column.sql",
            sql: "ALTER TABLE parent ADD COLUMN rank INTEGER NOT NULL DEFAULT 0 CHECK(rank >= 0);",
        },
    ];
    let target = spec(&migrations);
    let policy = RebuildPolicy::new(&["parent"])?;
    target.apply(
        &target.plan_with_rebuilds(db.path(), &policy)?,
        |_| Ok(()),
        |_, ()| Ok(()),
    )?;
    for sql in [
        "CREATE TEMP TABLE leaked(id);",
        "UPDATE temp.sqlite_temp_master SET sql='bad';",
    ] {
        let migrations = [
            INITIAL,
            REBUILD,
            Migration {
                name: "03_temp_escape.sql",
                sql,
            },
        ];
        let invalid = spec(&migrations);
        let fresh = NamedTempFile::new()?;
        assert!(matches!(
            invalid.apply(
                &invalid.plan_with_rebuilds(fresh.path(), &policy)?,
                |_| Ok(()),
                |_, ()| Ok(())
            ),
            Err(Error::Migration { .. })
        ));
    }
    Ok(())
}

/// Legacy adoption records the old prefix without replay, then applies the
/// rebuild while preserving the original history table and timestamps.
#[test]
fn adoption_and_rebuild_share_one_reviewed_plan() -> TestResult {
    use evolve_sqlite::LegacyMigration;
    use sha2::{Digest, Sha256};
    let db = NamedTempFile::new()?;
    let conn = Connection::open(db.path())?;
    conn.execute_batch(INITIAL.sql)?;
    conn.execute_batch("CREATE TABLE old_history(name TEXT, applied_at INTEGER); INSERT INTO old_history VALUES ('initial',123);")?;
    let migrations = [INITIAL, REBUILD];
    let target = spec(&migrations);
    let read_legacy =
        |conn: &Connection| -> Result<Vec<LegacyMigration>, CheckError> {
            let (name, applied_at): (String, i64) = conn.query_row(
                "SELECT name, applied_at FROM old_history",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            assert_eq!(name, "initial");
            Ok(vec![LegacyMigration {
                ordinal: 1,
                name: INITIAL.name.into(),
                sha256: format!("{:x}", Sha256::digest(INITIAL.sql)),
                applied_at,
            }])
        };
    let policy = RebuildPolicy::new(&["parent"])?;
    let plan =
        target.plan_adoption_with_rebuilds(db.path(), &policy, read_legacy)?;
    assert_eq!(plan.pending().len(), 1);
    target.apply_adoption(&plan, read_legacy, children, |conn, before| {
        assert_eq!(children(conn)?, before);
        assert_eq!(
            conn.query_row("SELECT applied_at FROM old_history", [], |r| r
                .get::<_, i64>(0))?,
            123
        );
        Ok(())
    })?;
    assert_eq!(
        conn.query_row(
            "SELECT applied_at FROM evolve_sqlite_migration WHERE ordinal=1",
            [],
            |r| r.get::<_, i64>(0)
        )?,
        123
    );
    Ok(())
}
