use std::{fs, path::Path, sync::mpsc, thread};

use evolve_sqlite::{Error, Migration, RehearsalPaths, Spec};
use rusqlite::Connection;
use tempfile::TempDir;

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

fn database(folder: &Path) -> std::path::PathBuf {
    let path = folder.join("source.sqlite");
    fs::File::create(&path).unwrap();
    let initial = Spec {
        migrations: &MIGRATIONS[..1],
        ..LATEST
    };
    initial
        .apply(&initial.plan(&path).unwrap(), |_| Ok(()), |_, ()| Ok(()))
        .unwrap();
    path
}

fn paths(folder: &Path) -> RehearsalPaths {
    RehearsalPaths {
        snapshot: folder.join("snapshot.sqlite"),
        copy: folder.join("rehearsal.sqlite"),
    }
}

fn rows(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    conn.prepare("SELECT status FROM booking ORDER BY id")?
        .query_map([], |row| row.get(0))?
        .collect()
}

#[test]
fn rehearsal_includes_wal_rows_and_leaves_the_source_and_snapshot_unchanged() {
    let folder = TempDir::new().unwrap();
    let source = database(folder.path());
    let plan = LATEST.plan(&source).unwrap();
    let writer = Connection::open(&source).unwrap();
    writer.pragma_update(None, "journal_mode", "WAL").unwrap();
    writer.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
    writer
        .execute("INSERT INTO booking VALUES (2, 'confirmed')", [])
        .unwrap();
    let paths = paths(folder.path());
    let report = LATEST
        .rehearse(
            &plan,
            &paths,
            |conn| Ok(rows(conn)?),
            |conn, before| {
                assert_eq!(before, ["pending", "confirmed"]);
                assert_eq!(rows(conn)?, before);
                Ok(())
            },
        )
        .unwrap();

    assert_eq!(LATEST.plan(&source).unwrap(), plan);
    assert!(LATEST.plan(&report.copy_path).unwrap().pending().is_empty());
    assert_eq!(
        LATEST.plan(&report.snapshot.path).unwrap().pending().len(),
        1
    );
    assert_eq!(rows(&writer).unwrap(), ["pending", "confirmed"]);
    let snapshot_bytes = fs::read(&report.snapshot.path).unwrap();
    Connection::open(&report.copy_path)
        .unwrap()
        .execute("UPDATE booking SET notes = 'rehearsed'", [])
        .unwrap();
    assert_eq!(fs::read(&report.snapshot.path).unwrap(), snapshot_bytes);
}

#[test]
fn rejected_rehearsal_retains_recovery_files_and_rolls_back_the_copy() {
    let folder = TempDir::new().unwrap();
    let source = database(folder.path());
    let plan = LATEST.plan(&source).unwrap();
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
        panic!("expected a rehearsal phase error");
    };
    assert!(matches!(*cause, Error::Verification(_)));
    assert_eq!(snapshot_path, paths.snapshot);
    assert_eq!(copy_path, paths.copy);
    assert!(snapshot_path.exists());
    assert!(copy_path.exists());
    assert_eq!(LATEST.plan(&source).unwrap(), plan);
    assert_eq!(LATEST.plan(&copy_path).unwrap().pending().len(), 1);
}

#[test]
fn stale_rehearsal_refuses_before_publishing_files() {
    let folder = TempDir::new().unwrap();
    let source = database(folder.path());
    let plan = LATEST.plan(&source).unwrap();
    Connection::open(&source)
        .unwrap()
        .execute("CREATE TABLE changed (id INTEGER)", [])
        .unwrap();
    let paths = paths(folder.path());
    assert!(matches!(
        LATEST.rehearse(&plan, &paths, |_| Ok(()), |_, ()| Ok(())),
        Err(Error::StalePlan)
    ));
    assert!(!paths.snapshot.exists());
    assert!(!paths.copy.exists());
}

#[test]
fn apply_and_rehearsal_share_a_migration_lock() {
    let folder = TempDir::new().unwrap();
    let source = database(folder.path());
    let plan = LATEST.plan(&source).unwrap();
    let (entered, observed) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let thread_plan = plan.clone();
    let running = thread::spawn(move || {
        LATEST.apply(
            &thread_plan,
            |_| {
                entered.send(()).unwrap();
                released.recv().unwrap();
                Ok(())
            },
            |_, ()| Ok(()),
        )
    });
    observed.recv().unwrap();
    let apply = LATEST.apply(&plan, |_| Ok(()), |_, ()| Ok(()));
    let rehearsal = LATEST.rehearse(
        &plan,
        &paths(folder.path()),
        |_| Ok(()),
        |_, ()| Ok(()),
    );
    release.send(()).unwrap();
    running.join().unwrap().unwrap();
    assert!(matches!(apply, Err(Error::Locked { .. })));
    assert!(matches!(rehearsal, Err(Error::Locked { .. })));
}
