//! Descriptor snapshots and same-directory publication for text edits.

use std::fs::File;
use std::io::{self, Read, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use super::{MAX_BYTES, invalid, single_line, text};

#[cfg(unix)]
#[path = "unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "windows.rs"]
mod platform;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Snapshot {
    metadata: Option<platform::Identity>,
    bytes: Vec<u8>,
}

impl Snapshot {
    fn from_file(file: &mut File) -> io::Result<Self> {
        let before = platform::Identity::read(file)?;
        let mut bytes = Vec::with_capacity(before.len());
        file.take(before.len() as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() != before.len()
            || before != platform::Identity::read(file)?
        {
            return Err(invalid("document changed while it was being read"));
        }
        Ok(Self {
            metadata: Some(before),
            bytes,
        })
    }

    pub(crate) fn content(&self) -> Option<&[u8]> {
        self.metadata.as_ref().map(|_| self.bytes.as_slice())
    }

    pub(super) fn text(&self) -> io::Result<&str> {
        text(&self.bytes, true)
    }

    pub(super) fn check_mode_owner(&self, mode: Option<u32>) -> io::Result<()> {
        if let (Some(_), Some(metadata)) = (mode, &self.metadata) {
            metadata.check_owner()?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum WriteKind {
    Text(Option<u32>),
    Toml,
}

pub(crate) enum WritePhase {
    Precommit,
    Prepublish,
}

pub(crate) struct Target<'a> {
    requested: &'a Path,
    resolved: platform::Target,
}

impl<'a> Target<'a> {
    pub(super) fn open(requested: &'a Path) -> io::Result<Self> {
        single_line(requested.as_os_str().as_encoded_bytes(), 32768)?;
        Self::open_validated(requested)
    }

    pub(crate) fn open_validated(requested: &'a Path) -> io::Result<Self> {
        Ok(Self {
            requested,
            resolved: platform::Target::open(requested)?,
        })
    }

    pub(crate) fn read(&self) -> io::Result<Snapshot> {
        match self.resolved.reader()? {
            Some(mut file) => Snapshot::from_file(&mut file),
            None => Ok(Snapshot {
                metadata: None,
                bytes: Vec::new(),
            }),
        }
    }

    fn revalidate(&self, old: &Snapshot) -> io::Result<()> {
        let current = Self::open_validated(self.requested)?;
        if !self.resolved.same_location(&current.resolved)
            || current.read()? != *old
        {
            return Err(invalid("destination changed before publication"));
        }
        Ok(())
    }

    pub(super) fn write(
        &self,
        old: &Snapshot,
        updated: &[u8],
        mode: Option<u32>,
    ) -> io::Result<()> {
        self.write_staged(old, updated, WriteKind::Text(mode), |_, _| {})
    }

    pub(crate) fn write_toml(
        &self,
        old: &Snapshot,
        updated: &[u8],
        hooks: impl FnMut(WritePhase, &Path),
    ) -> io::Result<()> {
        #[cfg(windows)]
        {
            const PATH_CAPACITY: usize = 4096;
            const SUFFIX_LEN: usize = 7;
            let len = self.requested.as_os_str().as_encoded_bytes().len();
            if len >= PATH_CAPACITY
                || (updated != old.bytes && len + SUFFIX_LEN >= PATH_CAPACITY)
            {
                return Err(invalid("configuration path is too long"));
            }
        }
        self.write_staged(old, updated, WriteKind::Toml, hooks)
    }

    fn write_staged(
        &self,
        old: &Snapshot,
        updated: &[u8],
        kind: WriteKind,
        mut hooks: impl FnMut(WritePhase, &Path),
    ) -> io::Result<()> {
        if updated.len() > MAX_BYTES {
            return Err(invalid("replacement exceeds the document limit"));
        }
        let mode = match kind {
            WriteKind::Text(mode) => mode,
            WriteKind::Toml => None,
        };
        if old.bytes == updated
            && mode.is_none_or(|mode| {
                old.metadata
                    .as_ref()
                    .is_some_and(|metadata| metadata.mode_matches(mode))
            })
        {
            return Ok(());
        }
        let (mut temp, mut file) = Temp::create(&self.resolved, kind)?;
        file.write_all(updated)?;
        platform::prepare(&file, old.metadata.as_ref(), mode)?;
        file.sync_all()?;
        let trusted = platform::Identity::read(&file)?;
        if let Some(mode) = mode {
            trusted.check_owner()?;
            if !trusted.mode_matches(mode) {
                return Err(invalid("staged replacement has the wrong mode"));
            }
        }
        platform::close(file)?;
        #[cfg(any(test, feature = "native-engine-test-seams"))]
        if matches!(kind, WriteKind::Text(_)) {
            super::test_hooks::run_hook(
                super::test_hooks::Phase::TempClosed,
                &temp.path,
            );
        }
        let reopened = temp.read()?;
        if !reopened
            .metadata
            .as_ref()
            .is_some_and(|identity| trusted.same_after_close(identity))
            || reopened.bytes != updated
        {
            return Err(invalid("staged replacement changed after close"));
        }
        #[cfg(any(test, feature = "native-engine-test-seams"))]
        if matches!(kind, WriteKind::Text(_)) {
            super::test_hooks::run_hook(
                super::test_hooks::Phase::Precommit,
                self.requested,
            );
        }
        hooks(WritePhase::Precommit, self.resolved.path());
        self.revalidate(old)?;
        #[cfg(any(test, feature = "native-engine-test-seams"))]
        if matches!(kind, WriteKind::Text(_)) {
            super::test_hooks::run_hook(
                super::test_hooks::Phase::Prepublish,
                self.requested,
            );
        }
        hooks(WritePhase::Prepublish, self.resolved.path());
        self.revalidate(old)?;
        if temp.read()? != reopened {
            return Err(invalid(
                "staged replacement changed before publication",
            ));
        }
        self.resolved.publish(&temp.path, old.metadata.is_some())?;
        temp.published = true;
        Ok(())
    }

    pub(super) fn delete(&self, old: &Snapshot) -> io::Result<()> {
        #[cfg(any(test, feature = "native-engine-test-seams"))]
        super::test_hooks::run_hook(
            super::test_hooks::Phase::Precommit,
            self.requested,
        );
        self.revalidate(old)?;
        #[cfg(any(test, feature = "native-engine-test-seams"))]
        super::test_hooks::run_hook(
            super::test_hooks::Phase::Prepublish,
            self.requested,
        );
        self.revalidate(old)?;
        // Legacy exact deletion removes the requested entry. For an opted-in
        // symlink, its referent provides the ownership proof and stays in place.
        platform::delete(self.requested)
    }
}

struct Temp<'a> {
    target: &'a platform::Target,
    path: PathBuf,
    published: bool,
}

impl<'a> Temp<'a> {
    fn create(
        target: &'a platform::Target,
        kind: WriteKind,
    ) -> io::Result<(Self, File)> {
        static SEQUENCE: AtomicU32 = AtomicU32::new(1);
        for attempt in 0..64 {
            let mut name = target.path().as_os_str().to_owned();
            match kind {
                WriteKind::Text(_) => {
                    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
                    name.push(format!(
                        ".cbm-text-{}-{sequence}.tmp",
                        std::process::id()
                    ));
                }
                #[cfg(unix)]
                WriteKind::Toml if target.followed() => {
                    name.push(format!(".cbm-toml-{attempt}.tmp"));
                }
                WriteKind::Toml => {
                    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
                    let id =
                        std::process::id().wrapping_mul(0x9e37_79b9) ^ sequence;
                    name.push(format!(".{:06x}", id & 0x00ff_ffff));
                }
            }
            #[cfg(windows)]
            let _ = attempt;
            let path = PathBuf::from(name);
            match target.create_temp(&path) {
                Ok(file) => {
                    return Ok((
                        Self {
                            target,
                            path,
                            published: false,
                        },
                        file,
                    ));
                }
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
                Err(err) => return Err(err),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "staging names are occupied",
        ))
    }

    fn read(&self) -> io::Result<Snapshot> {
        Snapshot::from_file(&mut self.target.reopen_temp(&self.path)?)
    }
}

impl Drop for Temp<'_> {
    fn drop(&mut self) {
        if !self.published {
            self.target.discard_temp(&self.path);
        }
    }
}
