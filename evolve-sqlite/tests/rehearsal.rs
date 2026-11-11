use std::{cell::Cell, fs, path::Path, sync::mpsc, thread, time::Duration};

use evolve_sqlite::{Error, LegacyMigration, Migration, RehearsalPaths, Spec};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

type TestResult<T = ()> = Result<T, evolve_sqlite::CheckError>;

const INITIAL: Migration<'_> = Migration {
    name: "01_booking.sql",
    sql: "CREATE TABLE booking (id INTEGER PRIMARY KEY, status TEXT NOT NULL);
          INSERT INTO booking VALUES (1, 'pending');",
};
const UPGRADE: Migration<'_> = Migration {
    name: "02_notes.sql",
    sql: "ALTER TABLE booking ADD COLUMN notes TEXT;",
};
const MIGRATIONS: &[Migration<'_>] = &[INITIAL, UPGRADE];
const LATEST: Spec<'_> = Spec {
    application_id: 0x5248_5253,
    migrations: MIGRATIONS,
};

fn database(folder: &Path) -> TestResult<std::path::PathBuf> {
    let path = folder.join("source.sqlite");
    fs::File::create(&path)?;
    let initial = Spec {
        migrations: &MIGRATIONS[..1],
        ..LATEST
    };
    initial.apply(&initial.plan(&path)?, |_| Ok(()), |_, ()| Ok(()))?;
    Ok(path)
}

fn paths(folder: &Path) -> RehearsalPaths {
    RehearsalPaths {
        snapshot: folder.join("snapshot.sqlite"),
        copy: folder.join("rehearsal.sqlite"),
    }
}

fn rows(conn: &Connection) -> TestResult<Vec<String>> {
    conn.prepare("SELECT status FROM booking ORDER BY id")?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()
        .map_err(Into::into)
}

/// Rehearsal must include committed WAL rows while retaining an unchanged recovery snapshot.
#[test]
fn rehearsal_includes_wal_rows_and_leaves_the_source_and_snapshot_unchanged()
-> TestResult {
    let folder = TempDir::new()?;
    let source = database(folder.path())?;
    let plan = LATEST.plan(&source)?;
    let writer = Connection::open(&source)?;
    writer.pragma_update(None, "journal_mode", "WAL")?;
    writer.pragma_update(None, "wal_autocheckpoint", 0)?;
    writer.execute("INSERT INTO booking VALUES (2, 'confirmed')", [])?;
    let paths = paths(folder.path());
    let report = LATEST.rehearse(
        &plan,
        &paths,
        |conn| Ok(rows(conn)?),
        |conn, before| {
            assert_eq!(before, ["pending", "confirmed"]);
            assert_eq!(rows(conn)?, before);
            Ok(())
        },
    )?;

    assert_eq!(LATEST.plan(&source)?, plan);
    assert!(LATEST.plan(&report.copy_path)?.pending().is_empty());
    assert_eq!(LATEST.plan(&report.snapshot.path)?.pending().len(), 1);
    assert_eq!(rows(&writer)?, ["pending", "confirmed"]);
    let snapshot_bytes = fs::read(&report.snapshot.path)?;
    Connection::open(&report.copy_path)?
        .execute("UPDATE booking SET notes = 'rehearsed'", [])?;
    assert_eq!(fs::read(&report.snapshot.path)?, snapshot_bytes);
    Ok(())
}

/// A failed verifier must retain recovery files and roll back the rehearsal transaction.
#[test]
fn rejected_rehearsal_retains_recovery_files_and_rolls_back_the_copy()
-> TestResult {
    let folder = TempDir::new()?;
    let source = database(folder.path())?;
    let plan = LATEST.plan(&source)?;
    let paths = paths(folder.path());
    let error = LATEST
        .rehearse(
            &plan,
            &paths,
            |_| Ok(()),
            |_, ()| Err(std::io::Error::other("candidate rejected").into()),
        )
        .unwrap_err();
    let Error::Rehearsal {
        snapshot_path,
        copy_path,
        source: cause,
    } = error
    else {
        panic!("expected a rehearsal operation error");
    };
    assert!(matches!(*cause, Error::Verification(_)));
    assert_eq!(snapshot_path, paths.snapshot);
    assert_eq!(copy_path, paths.copy);
    assert!(snapshot_path.exists());
    assert!(copy_path.exists());
    assert_eq!(LATEST.plan(&source)?, plan);
    assert_eq!(LATEST.plan(&copy_path)?.pending().len(), 1);
    Ok(())
}

/// A stale plan must fail before creating recovery artifacts.
#[test]
fn stale_rehearsal_refuses_before_publishing_files() -> TestResult {
    let folder = TempDir::new()?;
    let source = database(folder.path())?;
    let plan = LATEST.plan(&source)?;
    Connection::open(&source)?
        .execute("CREATE TABLE changed (id INTEGER)", [])?;
    let paths = paths(folder.path());
    assert!(matches!(
        LATEST.rehearse(&plan, &paths, |_| Ok(()), |_, ()| Ok(())),
        Err(Error::StalePlan)
    ));
    assert!(!paths.snapshot.exists());
    assert!(!paths.copy.exists());
    Ok(())
}

/// Every migration entry point must refuse a lock held by another migrator.
#[test]
fn apply_and_rehearsal_share_a_migration_lock() -> TestResult {
    let folder = TempDir::new()?;
    let source = database(folder.path())?;
    let plan = LATEST.plan(&source)?;
    let (entered, observed) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let thread_plan = plan.clone();
    let running = thread::spawn(move || {
        LATEST.apply(
            &thread_plan,
            |_| {
                entered.send(())?;
                released.recv()?;
                Ok(())
            },
            |_, ()| Ok(()),
        )
    });
    observed.recv()?;
    let apply = LATEST.apply(&plan, |_| Ok(()), |_, ()| Ok(()));
    let rehearsal = LATEST.rehearse(
        &plan,
        &paths(folder.path()),
        |_| Ok(()),
        |_, ()| Ok(()),
    );
    let verified = LATEST.apply_verified(
        &plan,
        &paths(folder.path()),
        |_| Ok(()),
        |_, ()| Ok(()),
    );
    release.send(())?;
    running.join().expect("migration worker must not panic")?;
    assert!(matches!(apply, Err(Error::Locked { .. })));
    assert!(matches!(rehearsal, Err(Error::Locked { .. })));
    assert_eq!(verified.unwrap_err().code(), "migration_locked");
    Ok(())
}

#[cfg(unix)]
/// A symlink lock must not create or modify an unrelated target.
#[test]
fn a_symlink_lock_is_refused_without_creating_its_target() -> TestResult {
    use std::os::unix::fs::symlink;

    let folder = TempDir::new()?;
    let source = database(folder.path())?;
    let plan = LATEST.plan(&source)?;
    let mut lock_name = source.as_os_str().to_owned();
    lock_name.push(".evolve-sqlite.lock");
    let lock_path = std::path::PathBuf::from(lock_name);
    fs::remove_file(&lock_path)?;
    let target = folder.path().join("unrelated");
    symlink(&target, &lock_path)?;

    assert!(matches!(
        LATEST.apply(&plan, |_| Ok(()), |_, ()| Ok(())),
        Err(Error::Io(_))
    ));
    assert!(!target.exists());
    assert_eq!(LATEST.plan(&source)?, plan);
    Ok(())
}

fn writes_are_blocked(source: &Path) -> TestResult {
    let writer = Connection::open(source)?;
    writer.busy_timeout(Duration::ZERO)?;
    let error = writer
        .execute("INSERT INTO booking (id, status) VALUES (3, 'racing')", [])
        .unwrap_err();
    assert!(matches!(
        error.sqlite_error_code(),
        Some(
            rusqlite::ErrorCode::DatabaseBusy
                | rusqlite::ErrorCode::DatabaseLocked
        )
    ));
    Ok(())
}

/// Verified apply must snapshot the final committed rows and exclude writers until commit.
#[test]
fn verified_apply_snapshots_new_rows_and_excludes_writers_until_commit()
-> TestResult {
    for journal in ["DELETE", "WAL"] {
        let folder = TempDir::new()?;
        let source = database(folder.path())?;
        let writer = Connection::open(&source)?;
        writer.pragma_update(None, "journal_mode", journal)?;
        writer.pragma_update(None, "wal_autocheckpoint", 0)?;
        let plan = LATEST.plan(&source)?;
        let preview = folder.path().join("preview");
        fs::create_dir(&preview)?;
        LATEST.rehearse(&plan, &paths(&preview), |_| Ok(()), |_, ()| Ok(()))?;
        writer
            .execute("INSERT INTO booking VALUES (2, 'accepted later')", [])?;
        let capture_calls = Cell::new(0);
        let verify_calls = Cell::new(0);
        let report = LATEST.apply_verified(
            &plan,
            &paths(folder.path()),
            |conn| {
                capture_calls.set(capture_calls.get() + 1);
                Ok(rows(conn)?)
            },
            |conn, before| {
                verify_calls.set(verify_calls.get() + 1);
                assert_eq!(before, ["pending", "accepted later"]);
                assert_eq!(rows(conn)?, before);
                writes_are_blocked(&source)?;
                Ok(())
            },
        )?;
        assert_eq!(capture_calls.get(), 2);
        assert_eq!(verify_calls.get(), 2);
        assert!(LATEST.plan(&source)?.pending().is_empty());
        assert!(LATEST.plan(&report.copy_path)?.pending().is_empty());
        assert_eq!(LATEST.plan(&report.snapshot.path)?.pending().len(), 1);
        let snapshot = Connection::open(report.snapshot.path)?;
        assert_eq!(rows(&snapshot)?, ["pending", "accepted later"]);
    }
    Ok(())
}

/// Failure on either database must preserve the live migration history.
#[test]
fn a_failed_rehearsal_or_live_check_keeps_the_live_plan_unapplied() -> TestResult
{
    for reject_call in [1, 2] {
        let folder = TempDir::new()?;
        let source = database(folder.path())?;
        let plan = LATEST.plan(&source)?;
        let paths = paths(folder.path());
        let verifies = Cell::new(0);
        let result = LATEST.apply_verified(
            &plan,
            &paths,
            |conn| Ok(rows(conn)?),
            |conn, before| {
                assert_eq!(rows(conn)?, before);
                verifies.set(verifies.get() + 1);
                if verifies.get() == reject_call {
                    return Err(
                        std::io::Error::other("candidate rejected").into()
                    );
                }
                Ok(())
            },
        );
        let error = result.unwrap_err();
        assert_eq!(error.code(), "verification");
        assert!(matches!(error, Error::VerifiedApply { .. }));
        assert_eq!(verifies.get(), reject_call);
        assert_eq!(LATEST.plan(&source)?, plan);
        assert!(paths.snapshot.exists());
        assert!(paths.copy.exists());
        assert_eq!(
            LATEST.plan(&paths.copy)?.pending().len(),
            usize::from(reject_call == 1)
        );
    }
    Ok(())
}

/// Changed migration timestamps must invalidate the plan before any backup is created.
#[test]
fn verified_apply_rejects_stale_history_before_creating_a_backup() -> TestResult
{
    let folder = TempDir::new()?;
    let source = database(folder.path())?;
    let plan = LATEST.plan(&source)?;
    Connection::open(&source)?.execute(
        "UPDATE evolve_sqlite_migration SET applied_at = applied_at + 1",
        [],
    )?;
    let paths = paths(folder.path());
    let error = LATEST
        .apply_verified(&plan, &paths, |_| Ok(()), |_, ()| Ok(()))
        .unwrap_err();
    assert_eq!(error.code(), "stale_plan");
    assert!(!paths.snapshot.exists());
    assert!(!paths.copy.exists());
    Ok(())
}

/// Legacy validation must run on both databases while the live write reservation is held.
#[test]
fn verified_adoption_validates_both_databases_under_the_live_write_lock()
-> TestResult {
    let folder = TempDir::new()?;
    let source = folder.path().join("legacy.sqlite");
    let conn = Connection::open(&source)?;
    conn.execute_batch(INITIAL.sql)?;
    conn.execute_batch(
        "CREATE TABLE legacy_migration (applied_at INTEGER);
                        INSERT INTO legacy_migration VALUES (123);
                        PRAGMA user_version = 9;",
    )?;
    drop(conn);
    let read_legacy = |conn: &Connection| {
        let applied_at = conn.query_row(
            "SELECT applied_at FROM legacy_migration",
            [],
            |row| row.get(0),
        )?;
        Ok(vec![LegacyMigration {
            ordinal: 1,
            name: INITIAL.name.to_owned(),
            sha256: format!("{:x}", Sha256::digest(INITIAL.sql.as_bytes())),
            applied_at,
        }])
    };
    let plan = LATEST.plan_adoption(&source, read_legacy)?;
    let validations = Cell::new(0);
    let report = LATEST.apply_adoption_verified(
        &plan,
        &paths(folder.path()),
        |conn| {
            validations.set(validations.get() + 1);
            writes_are_blocked(&source)?;
            read_legacy(conn)
        },
        |conn| Ok(rows(conn)?),
        |conn, before| {
            assert_eq!(rows(conn)?, before);
            Ok(())
        },
    )?;
    assert_eq!(validations.get(), 2);
    assert!(LATEST.plan(&source)?.pending().is_empty());
    assert!(LATEST.plan(&report.copy_path)?.pending().is_empty());
    let conn = Connection::open(&source)?;
    assert_eq!(rows(&conn)?, ["pending"]);
    let time: i64 =
        conn.query_row("SELECT applied_at FROM legacy_migration", [], |row| {
            row.get(0)
        })?;
    assert_eq!(time, 123);
    let snapshot = Connection::open(&report.snapshot.path)?;
    assert_eq!(read_legacy(&snapshot)?, plan.adopted());
    Ok(())
}
