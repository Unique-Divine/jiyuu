//! Verified, standalone SQLite snapshots published without replacing a file.

use std::{
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use rusqlite::{Connection, OpenFlags, backup::StepResult};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

const BACKUP_TIMEOUT: Duration = Duration::from_secs(30);
const RETRY_DELAY: Duration = Duration::from_millis(10);

/// Metadata for the verified bytes published at the caller's destination.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Snapshot {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BackupError {
    #[error("backup source must be an existing regular file")]
    InvalidSource,
    #[error("backup destination must have a filename in an existing directory")]
    InvalidDestination,
    #[error("backup destination already exists")]
    DestinationExists,
    #[error("SQLite backup did not complete within its time limit")]
    Timeout,
    #[error("snapshot failed SQLite integrity_check")]
    Integrity,
    #[error("snapshot failed SQLite foreign_key_check")]
    ForeignKeys,
    #[error(
        "verified snapshot was published at {path}, but directory synchronization failed: {source}"
    )]
    Durability {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl BackupError {
    /// Stable identifiers for callers that present structured errors.
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidSource => "backup_invalid_source",
            Self::InvalidDestination => "backup_invalid_destination",
            Self::DestinationExists => "backup_destination_exists",
            Self::Timeout => "backup_timeout",
            Self::Integrity => "backup_integrity",
            Self::ForeignKeys => "backup_foreign_keys",
            Self::Durability { .. } => "backup_durability",
            Self::Sqlite(_) => "backup_sqlite",
            Self::Io(_) => "backup_io",
        }
    }
}

/// Copy an existing SQLite database, verify it, and publish a private snapshot.
///
/// SQLite's backup API includes committed WAL contents while other connections
/// remain open. The destination is a standalone database with a SHA-256 digest
/// of its complete bytes. A file, directory, or symlink already occupying the
/// destination or any SQLite sidecar path causes an error. The source is
/// opened read-only. The caller must control the destination directory while
/// creating and retaining a snapshot.
///
/// The caller chooses the destination directory and owns retention. The copy
/// has a 30-second completion limit. Failures before publication leave the
/// destination absent and clean up the temporary file. A directory-sync
/// failure after publication returns [`BackupError::Durability`] with the
/// published path, whose survival across a crash is uncertain.
pub fn create(
    source: impl AsRef<Path>,
    destination: impl AsRef<Path>,
) -> Result<Snapshot, BackupError> {
    create_with_timeout(
        source.as_ref(),
        destination.as_ref(),
        BACKUP_TIMEOUT,
        sync_directory,
    )
}

fn create_with_timeout(
    source: &Path,
    destination: &Path,
    timeout: Duration,
    sync_parent: impl FnOnce(&Path) -> io::Result<()>,
) -> Result<Snapshot, BackupError> {
    let source = source.canonicalize()?;
    if !source.is_file() {
        return Err(BackupError::InvalidSource);
    }
    let filename = destination
        .file_name()
        .ok_or(BackupError::InvalidDestination)?;
    let parent = destination.parent().filter(|p| !p.as_os_str().is_empty());
    let parent = parent.unwrap_or_else(|| Path::new(".")).canonicalize()?;
    if !parent.is_dir() {
        return Err(BackupError::InvalidDestination);
    }
    let destination = parent.join(filename);
    require_unused_destination(&destination)?;

    let input = Connection::open_with_flags(
        source,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    input.busy_timeout(Duration::ZERO)?;
    let mut temporary = NamedTempFile::new_in(&parent)?;
    let mut output = Connection::open_with_flags(
        temporary.path(),
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    output.busy_timeout(Duration::ZERO)?;
    {
        let backup = rusqlite::backup::Backup::new(&input, &mut output)?;
        let deadline = Instant::now() + timeout;
        loop {
            if Instant::now() >= deadline {
                return Err(BackupError::Timeout);
            }
            match backup.step(128)? {
                StepResult::Done => break,
                StepResult::More => {}
                StepResult::Busy | StepResult::Locked => {
                    thread::sleep(RETRY_DELAY.min(
                        deadline.saturating_duration_since(Instant::now()),
                    ));
                }
                _ => unreachable!("SQLite returned an unknown backup state"),
            }
        }
    }
    // The copied header may select WAL. Switching to DELETE moves every
    // committed page into the main file and removes dependency on sidecars.
    let mode: String =
        output.query_row("PRAGMA journal_mode=DELETE", [], |r| r.get(0))?;
    if !mode.eq_ignore_ascii_case("delete") {
        return Err(BackupError::Integrity);
    }
    verify(&output)?;
    output.close().map_err(|(_, error)| error)?;
    input.close().map_err(|(_, error)| error)?;

    temporary.as_file_mut().seek(SeekFrom::Start(0))?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = temporary.as_file_mut().read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    let size_bytes = temporary.as_file().metadata()?.len();
    temporary.as_file().sync_all()?;
    require_unused_destination(&destination)?;
    let published =
        temporary.persist_noclobber(&destination).map_err(|error| {
            if error.error.kind() == io::ErrorKind::AlreadyExists {
                BackupError::DestinationExists
            } else {
                BackupError::Io(error.error)
            }
        })?;
    drop(published);
    sync_parent(&parent).map_err(|source| BackupError::Durability {
        path: destination.clone(),
        source,
    })?;
    Ok(Snapshot {
        path: destination,
        size_bytes,
        sha256: format!("{:x}", digest.finalize()),
    })
}

fn require_unused_destination(destination: &Path) -> Result<(), BackupError> {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut candidate = destination.as_os_str().to_owned();
        candidate.push(suffix);
        match fs::symlink_metadata(Path::new(&candidate)) {
            Ok(_) => return Err(BackupError::DestinationExists),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn sync_directory(parent: &Path) -> io::Result<()> {
    File::open(parent)?.sync_all()
}

fn verify(conn: &Connection) -> Result<(), BackupError> {
    let mut statement = conn.prepare("PRAGMA integrity_check")?;
    let results = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if results != ["ok"] {
        return Err(BackupError::Integrity);
    }
    if conn.prepare("PRAGMA foreign_key_check")?.exists([])? {
        return Err(BackupError::ForeignKeys);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_busy_source_times_out_and_leaves_no_destination() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.sqlite");
        let destination = directory.path().join("snapshot.sqlite");
        let writer = Connection::open(&source).unwrap();
        writer
            .execute_batch("CREATE TABLE item (id INTEGER); BEGIN EXCLUSIVE;")
            .unwrap();
        let error = create_with_timeout(
            &source,
            &destination,
            Duration::from_millis(20),
            sync_directory,
        )
        .unwrap_err();
        assert_eq!(error.code(), "backup_timeout");
        assert!(!destination.exists());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }
    #[test]
    fn a_directory_sync_failure_retains_the_verified_published_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.sqlite");
        let destination = directory.path().join("snapshot.sqlite");
        Connection::open(&source).unwrap().execute_batch(
            "CREATE TABLE record (id INTEGER); INSERT INTO record VALUES (42);",
        ).unwrap();
        let error =
            create_with_timeout(&source, &destination, BACKUP_TIMEOUT, |_| {
                Err(io::Error::other("injected directory sync failure"))
            })
            .unwrap_err();
        assert_eq!(error.code(), "backup_durability");
        match error {
            BackupError::Durability { path, source } => {
                assert_eq!(path, destination.canonicalize().unwrap());
                assert_eq!(
                    source.to_string(),
                    "injected directory sync failure"
                );
            }
            other => panic!("unexpected publication failure: {other:?}"),
        }
        let snapshot = Connection::open_with_flags(
            &destination,
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        verify(&snapshot).unwrap();
        assert_eq!(
            snapshot
                .query_row("SELECT id FROM record", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            42
        );
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
    }
}
