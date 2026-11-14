use std::{
    fs,
    sync::{Arc, Barrier},
    thread,
};

use evolve_sqlite::backup::{self, BackupError};
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};

type TestResult<T = ()> = Result<T, evolve_sqlite::CheckError>;

/// A live WAL backup must include committed rows and remain readable without sidecars.
#[test]
fn a_live_wal_source_becomes_a_verified_standalone_snapshot() -> TestResult {
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("source.sqlite");
    let destination = directory.path().join("snapshot.sqlite");
    let writer = Connection::open(&source)?;
    writer.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
         PRAGMA application_id=1234;
         CREATE TABLE record (id INTEGER PRIMARY KEY, content TEXT);
         INSERT INTO record VALUES (1, 'committed in WAL');",
    )?;
    assert!(fs::metadata(source.with_extension("sqlite-wal"))?.len() > 0);
    let source_bytes = fs::read(&source)?;

    let snapshot = backup::create(&source, &destination)?;
    assert_eq!(fs::read(&source)?, source_bytes);
    assert_eq!(snapshot.path, destination.canonicalize()?);
    let contents = fs::read(&snapshot.path)?;
    assert_eq!(snapshot.size_bytes, contents.len() as u64);
    assert_eq!(snapshot.sha256, format!("{:x}", Sha256::digest(&contents)));
    assert!(!destination.with_extension("sqlite-wal").exists());
    assert!(!destination.with_extension("sqlite-shm").exists());
    let copy = Connection::open_with_flags(
        &destination,
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let content: String =
        copy.query_row("SELECT content FROM record", [], |r| r.get(0))?;
    assert_eq!(content, "committed in WAL");
    let app_id: i32 =
        copy.pragma_query_value(None, "application_id", |r| r.get(0))?;
    assert_eq!(app_id, 1234);
    let mode: String =
        copy.pragma_query_value(None, "journal_mode", |r| r.get(0))?;
    assert_eq!(mode, "delete");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&destination)?.permissions().mode() & 0o777,
            0o600
        );
    }
    Ok(())
}

/// Refusing occupied paths must preserve caller files and aliases of the source.
#[test]
fn existing_destinations_and_source_aliases_are_preserved() -> TestResult {
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("source.sqlite");
    Connection::open(&source)?
        .execute_batch("CREATE TABLE record (id INTEGER)")?;
    let existing = directory.path().join("existing");
    fs::write(&existing, b"keep this file")?;
    let original = fs::read(&source)?;
    for destination in [&existing, &source] {
        assert!(matches!(
            backup::create(&source, destination),
            Err(BackupError::DestinationExists)
        ));
    }
    let alias = directory.path().join("hardlink");
    fs::hard_link(&source, &alias)?;
    assert!(matches!(
        backup::create(&source, &alias),
        Err(BackupError::DestinationExists)
    ));
    #[cfg(unix)]
    {
        let alias = directory.path().join("symlink");
        std::os::unix::fs::symlink(&source, &alias)?;
        assert!(matches!(
            backup::create(&source, &alias),
            Err(BackupError::DestinationExists)
        ));
        let dangling = directory.path().join("dangling");
        std::os::unix::fs::symlink(directory.path().join("absent"), &dangling)?;
        assert!(matches!(
            backup::create(&source, &dangling),
            Err(BackupError::DestinationExists)
        ));
    }
    assert_eq!(fs::read(&source)?, original);
    assert_eq!(fs::read(&existing)?, b"keep this file");
    Ok(())
}

/// Foreign-key violations must prevent publication and leave no temporary snapshot.
#[test]
fn failed_verification_publishes_nothing_and_removes_its_temporary_file()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("source.sqlite");
    let destination = directory.path().join("snapshot.sqlite");
    Connection::open(&source)?.execute_batch(
        "PRAGMA foreign_keys=OFF;
         CREATE TABLE parent (id INTEGER PRIMARY KEY);
         CREATE TABLE child (parent_id INTEGER REFERENCES parent(id));
         INSERT INTO child VALUES (9);",
    )?;
    let original = fs::read(&source)?;
    let error = backup::create(&source, &destination).unwrap_err();
    assert_eq!(error.code(), "backup_foreign_keys");
    assert!(!destination.exists());
    assert_eq!(fs::read_dir(directory.path())?.count(), 1);
    assert_eq!(fs::read(&source)?, original);
    Ok(())
}

/// Invalid SQLite bytes must fail before any destination is published.
#[test]
fn a_corrupt_source_publishes_nothing() -> TestResult {
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("source.sqlite");
    let destination = directory.path().join("snapshot.sqlite");
    fs::write(&source, b"this is not a SQLite database")?;
    assert!(matches!(
        backup::create(&source, &destination),
        Err(BackupError::Sqlite(_))
    ));
    assert!(!destination.exists());
    assert_eq!(fs::read_dir(directory.path())?.count(), 1);
    Ok(())
}

/// Concurrent publication must keep one complete copy without replacing the winner.
#[test]
fn concurrent_creators_publish_one_complete_snapshot() -> TestResult {
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("source.sqlite");
    let destination = directory.path().join("snapshot.sqlite");
    Connection::open(&source)?.execute_batch(
        "CREATE TABLE record (id INTEGER); INSERT INTO record VALUES (7);",
    )?;
    let barrier = Arc::new(Barrier::new(2));
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let barrier = Arc::clone(&barrier);
            let source = source.clone();
            let destination = destination.clone();
            thread::spawn(move || {
                barrier.wait();
                backup::create(source, destination)
            })
        })
        .collect();
    let mut successful = 0;
    for worker in workers {
        match worker.join().expect("snapshot worker must not panic") {
            Ok(_) => successful += 1,
            Err(BackupError::DestinationExists) => {}
            other => panic!("unexpected snapshot result: {other:?}"),
        }
    }
    assert_eq!(successful, 1);
    let copy = Connection::open(&destination)?;
    assert_eq!(
        copy.query_row("SELECT id FROM record", [], |r| r.get::<_, i64>(0))?,
        7
    );
    assert_eq!(fs::read_dir(directory.path())?.count(), 2);
    Ok(())
}

/// A reserved write transaction must permit a backup of committed data.
#[test]
fn a_reserved_writer_can_snapshot_before_live_changes() -> TestResult {
    for mode in ["DELETE", "WAL"] {
        let directory = tempfile::tempdir()?;
        let source = directory.path().join("source.sqlite");
        let destination = directory.path().join("snapshot.sqlite");
        let writer = Connection::open(&source)?;
        writer.execute_batch(&format!("PRAGMA journal_mode={mode}; CREATE TABLE record (id INTEGER); INSERT INTO record VALUES (8); BEGIN IMMEDIATE;"))?;
        backup::create(&source, &destination)?;
        writer.execute_batch("ROLLBACK")?;
        let copy = Connection::open(&destination)?;
        assert_eq!(
            copy.query_row("SELECT id FROM record", [], |r| r.get::<_, i64>(0))?,
            8
        );
    }
    Ok(())
}

/// A retained WAL can replace copied rows on open, so publication must reject it.
#[test]
fn retained_destination_wal_cannot_override_a_published_snapshot() -> TestResult
{
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("source.sqlite");
    let destination = directory.path().join("snapshot.sqlite");
    Connection::open(&source)?.execute_batch(
        "CREATE TABLE record (id INTEGER); INSERT INTO record VALUES (42);",
    )?;
    let old_database = directory.path().join("old.sqlite");
    let old_writer = Connection::open(&old_database)?;
    old_writer.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
         CREATE TABLE record (id INTEGER); INSERT INTO record VALUES (666);",
    )?;
    let wal_bytes = fs::read(old_database.with_extension("sqlite-wal"))?;
    assert!(!wal_bytes.is_empty());
    drop(old_writer);
    let wal = destination.with_extension("sqlite-wal");
    fs::write(&wal, &wal_bytes)?;

    let error = backup::create(&source, &destination).unwrap_err();
    assert_eq!(error.code(), "backup_destination_exists");
    assert!(!destination.exists());
    assert_eq!(fs::read(&wal)?, wal_bytes);
    Ok(())
}

/// Reserved sidecar paths must remain untouched, including dangling symlinks.
#[test]
fn destination_sidecars_and_dangling_sidecar_symlinks_are_preserved()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("source.sqlite");
    Connection::open(&source)?
        .execute_batch("CREATE TABLE record (id INTEGER)")?;
    for suffix in ["-wal", "-shm", "-journal"] {
        let destination =
            directory.path().join(format!("snapshot{suffix}.sqlite"));
        let mut sidecar = destination.as_os_str().to_owned();
        sidecar.push(suffix);
        let sidecar = std::path::PathBuf::from(sidecar);
        fs::write(&sidecar, b"retain sidecar evidence")?;
        assert!(matches!(
            backup::create(&source, &destination),
            Err(BackupError::DestinationExists)
        ));
        assert!(!destination.exists());
        assert_eq!(fs::read(&sidecar)?, b"retain sidecar evidence");

        #[cfg(unix)]
        {
            let destination =
                directory.path().join(format!("symlink{suffix}.sqlite"));
            let mut sidecar = destination.as_os_str().to_owned();
            sidecar.push(suffix);
            let sidecar = std::path::PathBuf::from(sidecar);
            std::os::unix::fs::symlink(
                directory.path().join("absent"),
                &sidecar,
            )?;
            assert!(matches!(
                backup::create(&source, &destination),
                Err(BackupError::DestinationExists)
            ));
            assert!(!destination.exists());
            assert!(fs::symlink_metadata(sidecar)?.file_type().is_symlink());
        }
    }
    Ok(())
}
