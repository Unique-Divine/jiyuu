use std::{
    fs,
    sync::{Arc, Barrier},
    thread,
};

use evolve_sqlite::backup::{self, BackupError};
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};

#[test]
fn a_live_wal_source_becomes_a_verified_standalone_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.sqlite");
    let destination = directory.path().join("snapshot.sqlite");
    let writer = Connection::open(&source).unwrap();
    writer
        .execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
         PRAGMA application_id=1234;
         CREATE TABLE record (id INTEGER PRIMARY KEY, content TEXT);
         INSERT INTO record VALUES (1, 'committed in WAL');",
        )
        .unwrap();
    assert!(
        fs::metadata(source.with_extension("sqlite-wal"))
            .unwrap()
            .len()
            > 0
    );
    let source_bytes = fs::read(&source).unwrap();

    let snapshot = backup::create(&source, &destination).unwrap();
    assert_eq!(fs::read(&source).unwrap(), source_bytes);
    assert_eq!(snapshot.path, destination.canonicalize().unwrap());
    let contents = fs::read(&snapshot.path).unwrap();
    assert_eq!(snapshot.size_bytes, contents.len() as u64);
    assert_eq!(snapshot.sha256, format!("{:x}", Sha256::digest(&contents)));
    assert!(!destination.with_extension("sqlite-wal").exists());
    assert!(!destination.with_extension("sqlite-shm").exists());
    let copy = Connection::open_with_flags(
        &destination,
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let content: String = copy
        .query_row("SELECT content FROM record", [], |r| r.get(0))
        .unwrap();
    assert_eq!(content, "committed in WAL");
    let application_id: i32 = copy
        .pragma_query_value(None, "application_id", |r| r.get(0))
        .unwrap();
    assert_eq!(application_id, 1234);
    let mode: String = copy
        .pragma_query_value(None, "journal_mode", |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "delete");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn existing_destinations_and_source_aliases_are_preserved() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.sqlite");
    Connection::open(&source)
        .unwrap()
        .execute_batch("CREATE TABLE record (id INTEGER)")
        .unwrap();
    let existing = directory.path().join("existing");
    fs::write(&existing, b"keep this file").unwrap();
    let original = fs::read(&source).unwrap();
    for destination in [&existing, &source] {
        assert!(matches!(
            backup::create(&source, destination),
            Err(BackupError::DestinationExists)
        ));
    }
    let alias = directory.path().join("hardlink");
    fs::hard_link(&source, &alias).unwrap();
    assert!(matches!(
        backup::create(&source, &alias),
        Err(BackupError::DestinationExists)
    ));
    #[cfg(unix)]
    {
        let alias = directory.path().join("symlink");
        std::os::unix::fs::symlink(&source, &alias).unwrap();
        assert!(matches!(
            backup::create(&source, &alias),
            Err(BackupError::DestinationExists)
        ));
        let dangling = directory.path().join("dangling");
        std::os::unix::fs::symlink(directory.path().join("absent"), &dangling)
            .unwrap();
        assert!(matches!(
            backup::create(&source, &dangling),
            Err(BackupError::DestinationExists)
        ));
    }
    assert_eq!(fs::read(&source).unwrap(), original);
    assert_eq!(fs::read(&existing).unwrap(), b"keep this file");
}

#[test]
fn failed_verification_publishes_nothing_and_removes_its_temporary_file() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.sqlite");
    let destination = directory.path().join("snapshot.sqlite");
    Connection::open(&source)
        .unwrap()
        .execute_batch(
            "PRAGMA foreign_keys=OFF;
         CREATE TABLE parent (id INTEGER PRIMARY KEY);
         CREATE TABLE child (parent_id INTEGER REFERENCES parent(id));
         INSERT INTO child VALUES (9);",
        )
        .unwrap();
    let original = fs::read(&source).unwrap();
    let error = backup::create(&source, &destination).unwrap_err();
    assert_eq!(error.code(), "backup_foreign_keys");
    assert!(!destination.exists());
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    assert_eq!(fs::read(&source).unwrap(), original);
}

#[test]
fn a_corrupt_source_publishes_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.sqlite");
    let destination = directory.path().join("snapshot.sqlite");
    fs::write(&source, b"this is not a SQLite database").unwrap();
    assert!(matches!(
        backup::create(&source, &destination),
        Err(BackupError::Sqlite(_))
    ));
    assert!(!destination.exists());
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn concurrent_creators_publish_one_complete_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.sqlite");
    let destination = directory.path().join("snapshot.sqlite");
    Connection::open(&source)
        .unwrap()
        .execute_batch(
            "CREATE TABLE record (id INTEGER); INSERT INTO record VALUES (7);",
        )
        .unwrap();
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
        match worker.join().unwrap() {
            Ok(_) => successful += 1,
            Err(BackupError::DestinationExists) => {}
            other => panic!("unexpected snapshot result: {other:?}"),
        }
    }
    assert_eq!(successful, 1);
    let copy = Connection::open(&destination).unwrap();
    assert_eq!(
        copy.query_row("SELECT id FROM record", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        7
    );
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
}

#[test]
fn a_reserved_writer_can_snapshot_before_live_changes() {
    for mode in ["DELETE", "WAL"] {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.sqlite");
        let destination = directory.path().join("snapshot.sqlite");
        let writer = Connection::open(&source).unwrap();
        writer.execute_batch(&format!("PRAGMA journal_mode={mode}; CREATE TABLE record (id INTEGER); INSERT INTO record VALUES (8); BEGIN IMMEDIATE;")).unwrap();
        backup::create(&source, &destination).unwrap();
        writer.execute_batch("ROLLBACK").unwrap();
        let copy = Connection::open(&destination).unwrap();
        assert_eq!(
            copy.query_row("SELECT id FROM record", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            8
        );
    }
}

#[test]
fn retained_destination_wal_cannot_override_a_published_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.sqlite");
    let destination = directory.path().join("snapshot.sqlite");
    Connection::open(&source)
        .unwrap()
        .execute_batch(
            "CREATE TABLE record (id INTEGER); INSERT INTO record VALUES (42);",
        )
        .unwrap();
    let old_database = directory.path().join("old.sqlite");
    let old_writer = Connection::open(&old_database).unwrap();
    old_writer
        .execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
         CREATE TABLE record (id INTEGER); INSERT INTO record VALUES (666);",
        )
        .unwrap();
    let wal_bytes = fs::read(old_database.with_extension("sqlite-wal")).unwrap();
    assert!(!wal_bytes.is_empty());
    drop(old_writer);
    let wal = destination.with_extension("sqlite-wal");
    fs::write(&wal, &wal_bytes).unwrap();

    let error = backup::create(&source, &destination).unwrap_err();
    assert_eq!(error.code(), "backup_destination_exists");
    assert!(!destination.exists());
    assert_eq!(fs::read(&wal).unwrap(), wal_bytes);
}

#[test]
fn destination_sidecars_and_dangling_sidecar_symlinks_are_preserved() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.sqlite");
    Connection::open(&source)
        .unwrap()
        .execute_batch("CREATE TABLE record (id INTEGER)")
        .unwrap();
    for suffix in ["-wal", "-shm", "-journal"] {
        let destination =
            directory.path().join(format!("snapshot{suffix}.sqlite"));
        let mut sidecar = destination.as_os_str().to_owned();
        sidecar.push(suffix);
        let sidecar = std::path::PathBuf::from(sidecar);
        fs::write(&sidecar, b"retain sidecar evidence").unwrap();
        assert!(matches!(
            backup::create(&source, &destination),
            Err(BackupError::DestinationExists)
        ));
        assert!(!destination.exists());
        assert_eq!(fs::read(&sidecar).unwrap(), b"retain sidecar evidence");

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
            )
            .unwrap();
            assert!(matches!(
                backup::create(&source, &destination),
                Err(BackupError::DestinationExists)
            ));
            assert!(!destination.exists());
            assert!(
                fs::symlink_metadata(sidecar)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
        }
    }
}
