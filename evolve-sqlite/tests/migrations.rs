use evolve_sqlite::{Error, Migration, Plan, Spec};
use rusqlite::Connection;
use tempfile::NamedTempFile;

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
        application_id: APP_ID,
        migrations,
    }
}

fn apply(spec: Spec<'_>, plan: &Plan) -> Result<(), Error> {
    spec.apply(plan, |_| Ok(()), |_, ()| Ok(()))
}

fn rows(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    conn.prepare("SELECT status FROM booking ORDER BY id")?
        .query_map([], |r| r.get(0))?
        .collect()
}

fn schema_len(conn: &Connection) -> i64 {
    conn.query_row("SELECT count(*) FROM sqlite_schema", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn planning_is_read_only_and_apply_checks_rows_under_the_write_lock() {
    let db = NamedTempFile::new().unwrap();
    let first = spec(&[INITIAL]);
    let first_plan = first.plan(db.path()).unwrap();
    assert_eq!(first_plan.pending()[0].name, INITIAL.name);
    assert_eq!(db.as_file().metadata().unwrap().len(), 0);
    apply(first, &first_plan).unwrap();

    let migrations = [INITIAL, UPGRADE];
    let latest = spec(&migrations);
    let plan = latest.plan(db.path()).unwrap();
    let writer = Connection::open(db.path()).unwrap();
    writer
        .execute("INSERT INTO booking VALUES (2, 'confirmed')", [])
        .unwrap();

    latest
        .apply(
            &plan,
            |conn| Ok(rows(conn)?),
            |conn, before| {
                assert_eq!(before, ["pending", "confirmed"]);
                assert_eq!(rows(conn)?, before);
                Ok(())
            },
        )
        .unwrap();
    assert!(latest.plan(db.path()).unwrap().pending().is_empty());
    assert!(matches!(apply(latest, &plan), Err(Error::StalePlan)));
    assert!(matches!(first.plan(db.path()), Err(Error::Ahead(_))));
}

#[test]
fn refuses_wrong_and_unmarked_nonempty_databases() {
    let db = NamedTempFile::new().unwrap();
    let conn = Connection::open(db.path()).unwrap();
    conn.execute_batch("CREATE TABLE other (id INTEGER)")
        .unwrap();
    let target = spec(&[INITIAL]);
    assert!(matches!(
        target.plan(db.path()),
        Err(Error::AdoptionRequired)
    ));
    conn.pragma_update(None, "application_id", 1).unwrap();
    assert!(matches!(
        target.plan(db.path()),
        Err(Error::WrongDatabase { actual: 1, .. })
    ));
}

#[test]
fn refuses_changed_sql_and_a_missing_ledger_tail() {
    let db = NamedTempFile::new().unwrap();
    let first = spec(&[INITIAL]);
    apply(first, &first.plan(db.path()).unwrap()).unwrap();

    let changed = [Migration {
        sql: "SELECT 1",
        ..INITIAL
    }];
    assert!(matches!(
        spec(&changed).plan(db.path()),
        Err(Error::Drift(_))
    ));

    let conn = Connection::open(db.path()).unwrap();
    conn.execute("DELETE FROM evolve_sqlite_migration", [])
        .unwrap();
    assert!(matches!(first.plan(db.path()), Err(Error::Drift(_))));
}

#[test]
fn rejects_a_stale_schema_or_changed_pending_sql() {
    let db = NamedTempFile::new().unwrap();
    let first = spec(&[INITIAL]);
    apply(first, &first.plan(db.path()).unwrap()).unwrap();

    let migrations = [INITIAL, UPGRADE];
    let latest = spec(&migrations);
    let plan = latest.plan(db.path()).unwrap();
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

    let conn = Connection::open(db.path()).unwrap();
    conn.execute_batch("CREATE TABLE unexpected (id INTEGER)")
        .unwrap();
    assert!(matches!(apply(latest, &plan), Err(Error::StalePlan)));
}

#[test]
fn all_pending_migrations_and_ledger_writes_roll_back_on_failure() {
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
        let db = NamedTempFile::new().unwrap();
        let migrations = [
            INITIAL,
            Migration {
                sql: invalid_sql,
                ..UPGRADE
            },
        ];
        let target = spec(&migrations);
        let plan = target.plan(db.path()).unwrap();
        assert!(
            matches!(apply(target, &plan), Err(Error::Migration { .. })),
            "{invalid_sql}"
        );
        let conn = Connection::open(db.path()).unwrap();
        assert_eq!(schema_len(&conn), 0, "{invalid_sql}");
        assert_eq!(target.plan(db.path()).unwrap(), plan, "{invalid_sql}");
    }
}

#[test]
fn rejected_verification_rolls_back_and_cannot_commit_or_write() {
    let db = NamedTempFile::new().unwrap();
    let target = spec(&[INITIAL]);
    let plan = target.plan(db.path()).unwrap();
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
    assert_eq!(target.plan(db.path()).unwrap(), plan);
}

#[test]
fn trigger_bodies_and_fts_work_with_the_sql_authorizer() {
    let db = NamedTempFile::new().unwrap();
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
    let plan = target.plan(db.path()).unwrap();
    target.apply(&plan, |_| Ok(()), |conn, ()| {
        let count: i64 = conn.query_row(
            "SELECT count(*) FROM booking_fts WHERE booking_fts MATCH 'confirmed'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(count, 1);
        Ok(())
    }).unwrap();
}
