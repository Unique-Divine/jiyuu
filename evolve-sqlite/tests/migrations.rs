use evolve_sqlite::{CheckError, Error, Migration, Plan, Spec};

use rusqlite::Connection;
use tempfile::NamedTempFile;

type TestResult<T = ()> = Result<T, CheckError>;

const INITIAL: Migration<'_> = Migration {
    name: "01_initial.sql",
    sql: "CREATE TABLE booking (id INTEGER PRIMARY KEY, status TEXT NOT NULL);
          INSERT INTO booking VALUES (1, 'pending');",
};
const UPGRADE: Migration<'_> = Migration {
    name: "02_notes.sql",
    sql: "ALTER TABLE booking ADD COLUMN notes TEXT;",
};
const APP_ID: i32 = 0x4544_4253;

fn spec<'a>(migrations: &'a [Migration<'a>]) -> Spec<'a> {
    Spec {
        app_id: APP_ID,
        migrations,
    }
}

fn apply(spec: Spec<'_>, plan: &Plan) -> Result<(), Error> {
    spec.apply(plan, |_| Ok(()), |_, ()| Ok(()))
}

fn rows(conn: &Connection) -> TestResult<Vec<String>> {
    conn.prepare("SELECT status FROM booking ORDER BY id")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()
        .map_err(Into::into)
}

fn schema_len(conn: &Connection) -> TestResult<i64> {
    Ok(conn.query_row("SELECT count(*) FROM sqlite_schema", [], |r| r.get(0))?)
}

/// Planning leaves an empty file untouched; apply captures rows after acquiring the write lock.
#[test]
fn planning_is_read_only_and_apply_checks_rows_under_the_write_lock()
-> TestResult {
    let db = NamedTempFile::new()?;
    let first = spec(&[INITIAL]);
    let first_plan = first.plan(db.path())?;
    assert_eq!(first_plan.pending()[0].name, INITIAL.name);
    assert_eq!(db.as_file().metadata()?.len(), 0);
    apply(first, &first_plan)?;

    let migrations = [INITIAL, UPGRADE];
    let latest = spec(&migrations);
    let plan = latest.plan(db.path())?;
    let writer = Connection::open(db.path())?;
    writer.execute("INSERT INTO booking VALUES (2, 'confirmed')", [])?;

    latest.apply(&plan, rows, |conn, before| {
        assert_eq!(before, ["pending", "confirmed"]);
        assert_eq!(rows(conn)?, before);
        Ok(())
    })?;
    assert!(latest.plan(db.path())?.pending().is_empty());
    assert!(matches!(apply(latest, &plan), Err(Error::StalePlan)));
    assert!(matches!(first.plan(db.path()), Err(Error::Ahead(_))));
    Ok(())
}

/// Existing data requires adoption, and a different application identity must be rejected.
#[test]
fn refuses_wrong_and_unmarked_nonempty_databases() -> TestResult {
    let db = NamedTempFile::new()?;
    let conn = Connection::open(db.path())?;
    conn.execute_batch("CREATE TABLE other (id INTEGER)")?;
    let target = spec(&[INITIAL]);
    assert!(matches!(
        target.plan(db.path()),
        Err(Error::AdoptionRequired)
    ));
    assert_eq!(
        target.plan(db.path()).unwrap_err().code(),
        "adoption_required"
    );
    conn.pragma_update(None, "application_id", 1)?;
    assert!(matches!(
        target.plan(db.path()),
        Err(Error::WrongDatabase { actual: 1, .. })
    ));
    assert_eq!(target.plan(db.path()).unwrap_err().code(), "wrong_database");
    Ok(())
}

/// Applied SQL checksums and the complete migration-history prefix must remain intact.
#[test]
fn refuses_changed_sql_and_a_missing_ledger_tail() -> TestResult {
    let db = NamedTempFile::new()?;
    let first = spec(&[INITIAL]);
    apply(first, &first.plan(db.path())?)?;

    let changed = [Migration {
        sql: "SELECT 1",
        ..INITIAL
    }];
    assert!(matches!(
        spec(&changed).plan(db.path()),
        Err(Error::Drift(_))
    ));

    let conn = Connection::open(db.path())?;
    conn.execute("DELETE FROM evolve_sqlite_migration", [])?;
    assert!(matches!(first.plan(db.path()), Err(Error::Drift(_))));
    Ok(())
}

/// History column changes must fail validation before planning any SQL.
#[test]
fn refuses_a_ledger_with_changed_columns() -> TestResult {
    for change in [
        "ALTER TABLE evolve_sqlite_migration DROP COLUMN applied_at",
        "ALTER TABLE evolve_sqlite_migration RENAME COLUMN sha256 TO checksum",
    ] {
        let db = NamedTempFile::new()?;
        let first = spec(&[INITIAL]);
        apply(first, &first.plan(db.path())?)?;
        let conn = Connection::open(db.path())?;
        conn.execute_batch(change)?;
        assert!(
            matches!(first.plan(db.path()), Err(Error::Drift(_))),
            "{change}"
        );
    }
    Ok(())
}

/// An applied timestamp change invalidates a plan even when its SQL checksums still match.
#[test]
fn changed_managed_ledger_timestamps_make_the_plan_stale() -> TestResult {
    let db = NamedTempFile::new()?;
    let first = spec(&[INITIAL]);
    apply(first, &first.plan(db.path())?)?;

    let migrations = [INITIAL, UPGRADE];
    let latest = spec(&migrations);
    let plan = latest.plan(db.path())?;
    let conn = Connection::open(db.path())?;
    conn.execute(
        "UPDATE evolve_sqlite_migration SET applied_at = applied_at + 1",
        [],
    )?;

    assert!(matches!(apply(latest, &plan), Err(Error::StalePlan)));
    assert_eq!(latest.plan(db.path())?.pending()[0].name, UPGRADE.name);
    Ok(())
}

/// A history-table trigger could suppress inserts, so planning and apply must reject it.
#[test]
fn rejects_a_preexisting_trigger_on_the_engine_ledger() -> TestResult {
    let db = NamedTempFile::new()?;
    let first = spec(&[INITIAL]);
    apply(first, &first.plan(db.path())?)?;

    let migrations = [INITIAL, UPGRADE];
    let latest = spec(&migrations);
    let plan = latest.plan(db.path())?;
    let conn = Connection::open(db.path())?;
    conn.execute_batch(
        "CREATE TRIGGER suppress_history BEFORE INSERT ON EVOLVE_SQLITE_MIGRATION
         BEGIN
             SELECT RAISE(IGNORE);
         END;",
    )?;

    assert!(matches!(latest.plan(db.path()), Err(Error::Drift(_))));
    assert!(matches!(apply(latest, &plan), Err(Error::Drift(_))));
    Ok(())
}

/// Pragma permissions ignore case while verification still cannot change the schema version.
#[test]
fn authorizer_recognizes_mixed_case_pragma_names() -> TestResult {
    let db = NamedTempFile::new()?;
    let migrations = [
        INITIAL,
        Migration {
            sql: "PRAGMA Page_Size; PRAGMA DATA_VERSION;",
            ..UPGRADE
        },
    ];
    let target = spec(&migrations);
    let plan = target.plan(db.path())?;
    target.apply(
        &plan,
        |_| Ok(()),
        |conn, ()| {
            let columns = conn
                .prepare("PRAGMA Table_Xinfo('booking')")?
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            assert_eq!(columns, ["id", "status"]);

            let version: i32 =
                conn.pragma_query_value(None, "User_Version", |row| row.get(0))?;
            assert_eq!(version, 2);
            assert!(conn.execute_batch("PRAGMA USER_VERSION = 10").is_err());
            Ok(())
        },
    )?;
    Ok(())
}

/// Apply must reject a plan after either the schema or its pending SQL changes.
#[test]
fn rejects_a_stale_schema_or_changed_pending_sql() -> TestResult {
    let db = NamedTempFile::new()?;
    let first = spec(&[INITIAL]);
    apply(first, &first.plan(db.path())?)?;

    let migrations = [INITIAL, UPGRADE];
    let latest = spec(&migrations);
    let plan = latest.plan(db.path())?;
    let changed = [
        INITIAL,
        Migration {
            sql: "SELECT 1",
            ..UPGRADE
        },
    ];
    assert!(matches!(
        apply(spec(&changed), &plan),
        Err(Error::StalePlan)
    ));

    let conn = Connection::open(db.path())?;
    conn.execute_batch("CREATE TABLE unexpected (id INTEGER)")?;
    assert!(matches!(apply(latest, &plan), Err(Error::StalePlan)));
    Ok(())
}

/// Invalid or forbidden SQL rolls back every pending migration and its history entries.
#[test]
fn all_pending_migrations_and_ledger_writes_roll_back_on_failure() -> TestResult
{
    for invalid_sql in [
        "INSERT INTO missing VALUES (1)",
        "COMMIT; CREATE TABLE escaped (id INTEGER)",
        "END",
        "SAVEPOINT escape",
        "ATTACH DATABASE ':memory:' AS other",
        "PRAGMA user_version = 10",
        "CREATE TEMP TABLE evolve_sqlite_migration (id INTEGER)",
        "DELETE FROM evolve_sqlite_migration",
    ] {
        let db = NamedTempFile::new()?;
        let migrations = [
            INITIAL,
            Migration {
                sql: invalid_sql,
                ..UPGRADE
            },
        ];
        let target = spec(&migrations);
        let plan = target.plan(db.path())?;
        assert!(
            matches!(apply(target, &plan), Err(Error::Migration { .. })),
            "{invalid_sql}"
        );
        let conn = Connection::open(db.path())?;
        assert_eq!(schema_len(&conn)?, 0, "{invalid_sql}");
        assert_eq!(target.plan(db.path())?, plan, "{invalid_sql}");
    }
    Ok(())
}

/// Dropping a referenced table must not silently delete children during a rebuild.
#[test]
fn a_table_rebuild_cannot_cascade_delete_children() -> TestResult {
    let db = NamedTempFile::new()?;
    let initial = Migration {
        name: INITIAL.name,
        sql: "CREATE TABLE parent (id INTEGER PRIMARY KEY);
              CREATE TABLE child (id INTEGER REFERENCES parent(id) ON DELETE CASCADE);
              INSERT INTO parent VALUES (1);
              INSERT INTO child VALUES (1);",
    };
    let first = spec(std::slice::from_ref(&initial));
    apply(first, &first.plan(db.path())?)?;

    let migrations = [
        initial,
        Migration {
            name: UPGRADE.name,
            sql: "CREATE TABLE replacement (id INTEGER PRIMARY KEY, name TEXT);
                  INSERT INTO replacement (id) SELECT id FROM parent;
                  DROP TABLE parent;
                  ALTER TABLE replacement RENAME TO parent;",
        },
    ];
    let target = spec(&migrations);
    let plan = target.plan(db.path())?;
    assert!(matches!(apply(target, &plan), Err(Error::Migration { .. })));
    assert_eq!(target.plan(db.path())?, plan);

    let conn = Connection::open(db.path())?;
    let child: i64 =
        conn.query_row("SELECT id FROM child", [], |row| row.get(0))?;
    assert_eq!(child, 1);
    Ok(())
}

/// Verification sees migrated data but cannot write or commit; rejection rolls back the batch.
#[test]
fn rejected_verification_rolls_back_and_cannot_commit_or_write() -> TestResult {
    let db = NamedTempFile::new()?;
    let target = spec(&[INITIAL]);
    let plan = target.plan(db.path())?;
    let error = target
        .apply(
            &plan,
            |_| Ok(()),
            |conn, ()| {
                assert_eq!(rows(conn)?, ["pending"]);
                assert!(conn.execute_batch("COMMIT").is_err());
                assert!(conn.execute("DELETE FROM booking", []).is_err());
                Err(std::io::Error::other("rejected by project check").into())
            },
        )
        .unwrap_err();
    assert!(matches!(error, Error::Verification(_)));
    assert_eq!(target.plan(db.path())?, plan);
    Ok(())
}

/// Ordinary trigger bodies and FTS indexing must remain usable under migration permissions.
#[test]
fn trigger_bodies_and_fts_work_with_the_sql_authorizer() -> TestResult {
    let db = NamedTempFile::new()?;
    let migrations = [
        INITIAL,
        Migration {
            name: "02_search.sql",
            sql: "CREATE VIRTUAL TABLE booking_fts USING fts5(status);
                  CREATE TRIGGER booking_search AFTER INSERT ON booking BEGIN
                      INSERT INTO booking_fts (rowid, status) VALUES (new.id, new.status);
                  END;
                  INSERT INTO booking VALUES (2, 'confirmed');",
        },
    ];
    let target = spec(&migrations);
    let plan = target.plan(db.path())?;
    target.apply(&plan, |_| Ok(()), |conn, ()| {
        let count: i64 = conn.query_row(
            "SELECT count(*) FROM booking_fts WHERE booking_fts MATCH 'confirmed'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(count, 1);
        Ok(())
    })?;
    Ok(())
}

mod adoption {
    use super::*;
    use evolve_sqlite::LegacyMigration;
    use sha2::{Digest, Sha256};

    fn legacy_database() -> TestResult<NamedTempFile> {
        let db = NamedTempFile::new()?;
        let conn = Connection::open(db.path())?;
        conn.execute_batch(INITIAL.sql)?;
        conn.execute_batch(
            "CREATE TABLE schema_migration (
                version INTEGER PRIMARY KEY, name TEXT NOT NULL,
                sha256 TEXT NOT NULL, applied_at INTEGER NOT NULL
             ) STRICT;
             PRAGMA user_version = 7;",
        )?;
        conn.execute(
            "INSERT INTO schema_migration VALUES (7, 'legacy_initial', ?1, 123)",
            [format!("{:x}", Sha256::digest(INITIAL.sql.as_bytes()))],
        )?;
        Ok(db)
    }

    fn read_legacy(conn: &Connection) -> TestResult<Vec<LegacyMigration>> {
        let version: i32 =
            conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        let count: i64 = conn.query_row(
            "SELECT count(*) FROM schema_migration",
            [],
            |row| row.get(0),
        )?;
        if version != 7 || count != 1 {
            return Err(std::io::Error::other(
                "unexpected legacy version or row count",
            )
            .into());
        }
        let (sha256, applied_at) = conn.query_row(
            "SELECT sha256, applied_at FROM schema_migration
             WHERE version = 7 AND name = 'legacy_initial'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok(vec![LegacyMigration {
            ordinal: 1,
            name: INITIAL.name.to_owned(),
            sha256,
            applied_at,
        }])
    }

    /// Adoption maps legacy version numbers without replaying SQL or replacing original timestamps.
    #[test]
    fn adopts_a_renumbered_prefix_without_replaying_sql() -> TestResult {
        let db = legacy_database()?;
        let migrations = [INITIAL, UPGRADE];
        let target = spec(&migrations);
        assert!(matches!(
            target.plan(db.path()),
            Err(Error::AdoptionRequired)
        ));

        let plan = target.plan_adoption(db.path(), read_legacy)?;
        assert_eq!(plan.adopted()[0].applied_at, 123);
        assert_eq!(plan.pending()[0].name, UPGRADE.name);
        target.apply_adoption(&plan, read_legacy, rows, |conn, before| {
            assert_eq!(rows(conn)?, before);
            Ok(())
        })?;

        assert!(target.plan(db.path())?.pending().is_empty());
        let conn = Connection::open(db.path())?;
        let evidence: (i64, i64) = conn.query_row(
            "SELECT (SELECT applied_at FROM evolve_sqlite_migration WHERE ordinal = 1),
                    (SELECT version FROM schema_migration)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        assert_eq!(evidence, (123, 7));
        Ok(())
    }

    /// Adoption must stamp the application identity even when the entire SQL prefix already exists.
    #[test]
    fn adoption_stamps_the_identity_when_no_sql_is_pending() -> TestResult {
        let db = legacy_database()?;
        let target = spec(&[INITIAL]);
        let plan = target.plan_adoption(db.path(), read_legacy)?;
        assert!(plan.pending().is_empty());
        target.apply_adoption(&plan, read_legacy, |_| Ok(()), |_, ()| Ok(()))?;
        assert!(target.plan(db.path())?.pending().is_empty());
        assert!(matches!(
            target.plan_adoption(db.path(), read_legacy),
            Err(Error::NotLegacy)
        ));
        Ok(())
    }

    /// Changed legacy timestamps invalidate plans, and changed checksums prevent adoption.
    #[test]
    fn rejects_drift_and_stale_legacy_evidence() -> TestResult {
        let db = legacy_database()?;
        let target = spec(&[INITIAL]);
        let plan = target.plan_adoption(db.path(), read_legacy)?;
        let conn = Connection::open(db.path())?;
        conn.execute("UPDATE schema_migration SET applied_at = 456", [])?;
        assert!(matches!(
            target.apply_adoption(
                &plan,
                read_legacy,
                |_| Ok(()),
                |_, ()| Ok(())
            ),
            Err(Error::StalePlan)
        ));
        conn.execute("UPDATE schema_migration SET sha256 = 'changed'", [])?;
        assert!(matches!(
            target.plan_adoption(db.path(), read_legacy),
            Err(Error::Drift(_))
        ));
        Ok(())
    }

    /// Rejected adoption preserves legacy history and leaves the database requiring adoption.
    #[test]
    fn failed_adoption_leaves_original_history_and_identity_unchanged()
    -> TestResult {
        let db = legacy_database()?;
        let migrations = [INITIAL, UPGRADE];
        let target = spec(&migrations);
        let plan = target.plan_adoption(db.path(), read_legacy)?;
        let result = target.apply_adoption(
            &plan,
            read_legacy,
            |_| Ok(()),
            |_, ()| Err(std::io::Error::other("rejected adoption").into()),
        );
        assert!(matches!(result, Err(Error::Verification(_))));
        assert_eq!(target.plan_adoption(db.path(), read_legacy)?, plan);
        assert!(matches!(
            target.plan(db.path()),
            Err(Error::AdoptionRequired)
        ));
        Ok(())
    }
}
