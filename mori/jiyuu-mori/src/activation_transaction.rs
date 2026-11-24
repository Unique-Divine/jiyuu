//! Same-directory staging and reversible publication for CLI activation.

use std::ffi::{CStr, CString};
use std::fs::File;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};

#[path = "activation_transaction/ffi.rs"]
mod ffi;
#[cfg(unix)]
#[path = "activation_transaction/unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "activation_transaction/windows.rs"]
mod platform;

use platform::{Directory, Identity};

#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Ok = 0,
    Deferred = 1,
    InvalidArgument = -1,
    NoMemory = -2,
    Io = -3,
    InvalidState = -4,
    ValidationFailed = -5,
    RollbackFailed = -6,
}

#[derive(PartialEq, Eq)]
enum State {
    Staged,
    Committed,
    RolledBack,
    Finalized,
    Deferred,
    Recovery,
}

struct Entry {
    path: CString,
    #[cfg(unix)]
    name: CString,
}

impl Entry {
    fn split(path: &CStr) -> Result<(CString, Self), Status> {
        let bytes = path.to_bytes();
        if bytes.is_empty() || matches!(bytes.last(), Some(b'/' | b'\\')) {
            return Err(Status::InvalidArgument);
        }
        let separator = bytes
            .iter()
            .rposition(|&c| c == b'/' || (cfg!(windows) && c == b'\\'));
        let (directory, name) = match separator {
            None => (b".".as_slice(), bytes),
            Some(at) => {
                let root =
                    at == 0 || (cfg!(windows) && at == 2 && bytes[1] == b':');
                (&bytes[..at + usize::from(root)], &bytes[at + 1..])
            }
        };
        if matches!(name, b"." | b"..") {
            return Err(Status::InvalidArgument);
        }
        Ok((
            cstring(directory),
            Self {
                path: path.to_owned(),
                #[cfg(unix)]
                name: cstring(name),
            },
        ))
    }
}

fn cstring(bytes: &[u8]) -> CString {
    CString::new(bytes).expect("substring of a C string has no interior NUL")
}

struct Asset {
    entry: Entry,
    identity: Identity,
    exists: bool,
}

struct Transaction {
    directory: Directory,
    target: Entry,
    original: Option<Identity>,
    replace: bool,
    state: State,
    staged: Option<Asset>,
    backup: Option<Asset>,
    backup_holds_original: bool,
}

enum Candidate<'a> {
    Bytes(&'a [u8]),
    File(File),
}

enum Publication {
    Unchanged,
    Changed,
}

fn refused() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "activation filesystem check refused",
    )
}

impl Transaction {
    fn prepare(path: &CStr, replace: bool) -> Result<Self, Status> {
        let (directory, target) = Entry::split(path)?;
        let directory =
            Directory::open(directory, false).map_err(|_| Status::Io)?;
        let original = directory.snapshot(&target, 1).map_err(|_| {
            ffi::note("target-entry-snapshot", "os 0", None);
            Status::Io
        })?;
        let mut tx = Self {
            directory,
            target,
            original,
            replace,
            state: State::Staged,
            staged: None,
            backup: None,
            backup_holds_original: false,
        };
        if original.is_some() {
            let (backup, reservation) = tx.create_unique("backup")?;
            tx.backup = Some(backup);
            let sync = platform::sync_file(&reservation);
            let close = platform::close(reservation);
            if sync.and(close).and_then(|()| tx.directory.sync()).is_err() {
                let _ = tx.discard();
                return Err(Status::Io);
            }
        }
        Ok(tx)
    }

    fn create_unique(&self, tag: &str) -> Result<(Asset, File), Status> {
        static SEQUENCE: AtomicU64 = AtomicU64::new(1);
        let directory = self.directory.path().to_bytes();
        for _ in 0..1024 {
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let name = format!(".cbm-{tag}-{}-{sequence}", std::process::id());
            let mut path = directory.to_vec();
            if !matches!(path.last(), Some(b'/' | b'\\')) {
                path.push(if cfg!(windows) { b'\\' } else { b'/' });
            }
            path.extend_from_slice(name.as_bytes());
            let entry = Entry {
                path: cstring(&path),
                #[cfg(unix)]
                name: cstring(name.as_bytes()),
            };
            match self.directory.create(&entry) {
                Ok((file, identity)) => {
                    return Ok((
                        Asset {
                            entry,
                            identity,
                            exists: true,
                        },
                        file,
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err(Status::Io),
            }
        }
        Err(Status::Io)
    }

    fn stage(&mut self, mut source: Candidate<'_>) -> Result<(), Status> {
        let (asset, mut file) = match self.create_unique("stage") {
            Ok(created) => created,
            Err(status) => {
                let _ = self.discard();
                return Err(status);
            }
        };
        self.staged = Some(asset);
        let write = match &mut source {
            Candidate::Bytes(bytes) => write_candidate(&mut file, bytes),
            Candidate::File(source) => copy_file(source, &mut file, false),
        };
        let written = write.and_then(|()| platform::sync_file(&file));
        let source_closed = match source {
            Candidate::Bytes(_) => Ok(()),
            Candidate::File(source) => platform::close(source),
        };
        let staged_closed = platform::close(file);
        let sync = written
            .and(source_closed)
            .and(staged_closed)
            .and_then(|()| self.directory.sync());
        if let Err(error) = sync {
            let _ = self.discard();
            platform::set_errno(error.raw_os_error().unwrap_or(0));
            return Err(Status::Io);
        }
        Ok(())
    }

    fn staged_file(&mut self, source: &CStr) -> Result<(), Status> {
        match platform::source_open(source) {
            Ok(file) => self.stage(Candidate::File(file)),
            Err(_) => {
                let _ = self.discard();
                Err(Status::Io)
            }
        }
    }

    fn discard(&mut self) -> Result<(), Status> {
        let mut clean = true;
        for asset in [
            self.staged.as_mut(),
            self.backup.as_mut().filter(|_| !self.backup_holds_original),
        ]
        .into_iter()
        .flatten()
        .filter(|asset| asset.exists)
        {
            if self
                .directory
                .unlink_expected(&asset.entry, asset.identity, false)
                .is_ok()
            {
                asset.exists = false;
            } else {
                clean = false;
            }
        }
        let synced = self.directory.sync().is_ok();
        if clean && synced {
            Ok(())
        } else {
            Err(Status::Io)
        }
    }

    fn commit(&mut self) -> Result<(), Status> {
        if self.state != State::Staged {
            return Err(Status::InvalidState);
        }
        if self
            .directory
            .snapshot(&self.target, 1)
            .map_err(|_| Status::Io)?
            != self.original
        {
            return Err(Status::Io);
        }
        let result = if self.replace {
            let staged = self.staged.as_ref().ok_or(Status::Io)?;
            if !staged.exists
                || !self.directory.matches(&staged.entry, staged.identity, 1)
            {
                return Err(Status::Io);
            }
            if self.original.is_some() {
                self.publish_existing()
            } else {
                #[cfg(any(test, feature = "native-engine-test-seams"))]
                ffi::before_absent_publish(&self.target.path);
                self.publish_absent()
            }
        } else if let Some(original) = self.original {
            let backup = self.backup.as_mut().ok_or(Status::Io)?;
            if !backup.exists
                || !self.directory.matches(&backup.entry, backup.identity, 1)
            {
                return Err(Status::Io);
            }
            self.directory
                .rename(&self.target, &backup.entry, true)
                .map_err(|_| Status::Io)?;
            backup.identity = original;
            self.backup_holds_original = true;
            Ok(())
        } else {
            Ok(())
        };
        match result {
            Ok(()) => {}
            Err(Publication::Unchanged) => return Err(Status::Io),
            Err(Publication::Changed) => {
                self.state = State::Recovery;
                return Err(self.restore_after(Status::Io));
            }
        }
        self.state = State::Committed;
        if self
            .directory
            .revalidate()
            .and_then(|()| self.directory.sync())
            .is_err()
        {
            return Err(self.restore_after(Status::Io));
        }
        Ok(())
    }

    fn publish_absent(&mut self) -> Result<(), Publication> {
        let staged = self.staged.as_mut().ok_or(Publication::Unchanged)?;
        if !self.directory.matches(&staged.entry, staged.identity, 1) {
            return Err(Publication::Unchanged);
        }
        let renamed = self
            .directory
            .rename_absent(&staged.entry, &self.target)
            .map_err(|_| Publication::Unchanged)?;
        #[cfg(unix)]
        if !renamed {
            self.directory
                .link(&staged.entry, &self.target)
                .map_err(|_| Publication::Unchanged)?;
            if !self.directory.matches(&staged.entry, staged.identity, 2)
                || !self.directory.matches(&self.target, staged.identity, 2)
            {
                return Err(Publication::Changed);
            }
            self.directory
                .unlink(&staged.entry, false)
                .map_err(|_| Publication::Changed)?;
        }
        #[cfg(windows)]
        debug_assert!(renamed);
        staged.exists = false;
        if self.directory.matches(&self.target, staged.identity, 1) {
            Ok(())
        } else {
            Err(Publication::Changed)
        }
    }

    #[cfg(unix)]
    fn publish_existing(&mut self) -> Result<(), Publication> {
        let (Some(staged), Some(backup), Some(original)) =
            (self.staged.as_mut(), self.backup.as_mut(), self.original)
        else {
            return Err(Publication::Unchanged);
        };
        self.directory
            .unlink_expected(&backup.entry, backup.identity, false)
            .map_err(|_| Publication::Unchanged)?;
        backup.exists = false;
        self.directory
            .revalidate()
            .and_then(|()| self.directory.link(&self.target, &backup.entry))
            .map_err(|_| Publication::Unchanged)?;
        backup.exists = true;
        backup.identity = original;
        self.backup_holds_original = true;
        let linked_pair = || {
            self.directory.matches(&self.target, original, 2)
                && self.directory.matches(&backup.entry, original, 2)
        };
        if !linked_pair()
            || !self.directory.matches(&staged.entry, staged.identity, 1)
            || self.directory.sync().is_err()
            || self
                .directory
                .rename(&staged.entry, &self.target, true)
                .is_err()
        {
            if linked_pair()
                && self.directory.unlink_strict(&backup.entry).is_ok()
            {
                backup.exists = false;
                self.backup_holds_original = false;
                return Err(Publication::Unchanged);
            }
            return Err(Publication::Changed);
        }
        staged.exists = false;
        Ok(())
    }

    #[cfg(windows)]
    fn publish_existing(&mut self) -> Result<(), Publication> {
        let (Some(staged), Some(backup), Some(original)) =
            (self.staged.as_mut(), self.backup.as_ref(), self.original)
        else {
            return Err(Publication::Unchanged);
        };
        self.backup_holds_original = false;
        self.directory
            .copy_backup(&self.target, original, backup)
            .and_then(|()| {
                self.directory.rename(&staged.entry, &self.target, true)
            })
            .map_err(|_| Publication::Unchanged)?;
        staged.exists = false;
        self.backup_holds_original = true;
        if self.directory.matches(&self.target, staged.identity, 1) {
            Ok(())
        } else {
            Err(Publication::Changed)
        }
    }

    fn restore_after(&mut self, reason: Status) -> Status {
        if self.restore().is_ok() {
            reason
        } else {
            Status::RollbackFailed
        }
    }

    fn rollback(&mut self) -> Result<(), Status> {
        if !matches!(self.state, State::Committed | State::Recovery) {
            return Err(Status::InvalidState);
        }
        self.restore()
    }

    fn restore(&mut self) -> Result<(), Status> {
        if self.restore_entries().is_err() {
            self.state = State::Recovery;
            return Err(Status::RollbackFailed);
        }
        self.state = State::RolledBack;
        self.directory.sync().map_err(|_| Status::Io)
    }

    fn restore_entries(&mut self) -> io::Result<()> {
        if self.original.is_some() {
            let backup = self
                .backup
                .as_mut()
                .filter(|_| self.backup_holds_original)
                .ok_or_else(refused)?;
            if !self.directory.matches(&backup.entry, backup.identity, 1) {
                return Err(refused());
            }
            let target = self.directory.snapshot(&self.target, 1)?;
            if let Some(id) = target
                && (!self.replace
                    || self
                        .staged
                        .as_ref()
                        .is_none_or(|asset| asset.identity != id))
            {
                return Err(refused());
            }
            self.directory.rename(
                &backup.entry,
                &self.target,
                target.is_some(),
            )?;
            backup.exists = false;
            self.backup_holds_original = false;
        } else if let Some(staged) = &self.staged {
            match self.directory.snapshot(&self.target, 1) {
                Ok(None) => {}
                Ok(Some(id)) if id == staged.identity => {
                    self.directory.unlink_expected(&self.target, id, false)?;
                }
                #[cfg(unix)]
                _ if staged.exists
                    && self.directory.matches(
                        &self.target,
                        staged.identity,
                        2,
                    )
                    && self.directory.matches(
                        &staged.entry,
                        staged.identity,
                        2,
                    ) =>
                {
                    self.directory.unlink_strict(&self.target)?;
                    if !self.directory.matches(&staged.entry, staged.identity, 1)
                        || self.directory.snapshot(&self.target, 1)?.is_some()
                    {
                        return Err(refused());
                    }
                }
                _ => return Err(refused()),
            }
        }
        Ok(())
    }

    fn finalize(&mut self) -> Result<Status, Status> {
        if self.state != State::Committed {
            return Err(Status::InvalidState);
        }
        self.directory.revalidate().map_err(|_| Status::Io)?;
        if self.backup_holds_original {
            let backup = self.backup.as_mut().ok_or(Status::Io)?;
            if self
                .directory
                .unlink_expected(&backup.entry, backup.identity, true)
                .map_err(|_| Status::Io)?
            {
                self.state = State::Deferred;
                return Ok(Status::Deferred);
            }
            backup.exists = false;
            self.backup_holds_original = false;
        }
        self.directory.sync().map_err(|_| Status::Io)?;
        self.state = State::Finalized;
        Ok(Status::Ok)
    }

    fn close(&mut self) -> Result<(), Status> {
        if matches!(self.state, State::Committed | State::Recovery) {
            self.restore()?;
        }
        if matches!(self.state, State::Staged | State::RolledBack) {
            self.discard()?;
        }
        Ok(())
    }
}

fn write_candidate(file: &mut File, bytes: &[u8]) -> io::Result<()> {
    ffi::check_write_failure()?;
    file.write_all(bytes)
}

fn copy_file(
    source: &mut File,
    target: &mut File,
    empty_ok: bool,
) -> io::Result<()> {
    let mut buffer = [0; 64 * 1024];
    let mut total = 0usize;
    loop {
        let count = match source.read(&mut buffer) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            break;
        }
        total = total.checked_add(count).ok_or_else(refused)?;
        write_candidate(target, &buffer[..count])?;
    }
    if total == 0 && !empty_ok {
        Err(refused())
    } else {
        Ok(())
    }
}
