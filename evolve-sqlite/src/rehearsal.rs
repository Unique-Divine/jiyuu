//! Rehearse migrations against verified copies before applying them.

use std::{
    fs::{File, OpenOptions, TryLockError},
    path::{Path, PathBuf},
};

use rusqlite::Connection;

use crate::{
    CheckError, Error, LegacyMigration, LegacyReader, Plan, Spec, backup,
};

/// Destinations selected by the application for one migration attempt.
///
/// Both paths must be unused. The snapshot remains an unchanged recovery
/// image, while the copy holds the rehearsal result. The application retains
/// or removes these files according to its own backup policy.
#[derive(Clone, Debug)]
pub struct RehearsalPaths {
    pub snapshot: PathBuf,
    pub copy: PathBuf,
}

/// Evidence produced by a completed rehearsal.
#[derive(Debug)]
pub struct RehearsalReport {
    pub snapshot: backup::Snapshot,
    pub copy_path: PathBuf,
}

/// An exclusive advisory lock shared by every engine migration entry point.
///
/// The file remains after unlocking. Removing it could let concurrent
/// processes acquire locks on different files under the same name.
pub(crate) struct MigrationLock {
    _file: File,
}

impl MigrationLock {
    pub(crate) fn acquire(database: &Path) -> Result<Self, Error> {
        let mut name = database.as_os_str().to_owned();
        name.push(".evolve-sqlite.lock");
        let path = PathBuf::from(name);
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path)?;
        let link_metadata = std::fs::symlink_metadata(&path)?;
        if !link_metadata.is_file() || !file.metadata()?.is_file() {
            return Err(std::io::Error::other(
                "migration lock must be a regular file",
            )
            .into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let opened = file.metadata()?;
            if opened.dev() != link_metadata.dev()
                || opened.ino() != link_metadata.ino()
            {
                return Err(std::io::Error::other(
                    "migration lock changed while opening",
                )
                .into());
            }
        }
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(TryLockError::WouldBlock) => Err(Error::Locked { path }),
            Err(TryLockError::Error(error)) => Err(error.into()),
        }
    }
}

impl Spec<'_> {
    /// Rehearse a reviewed plan on a verified copy without changing its source.
    ///
    /// Project checks run on the copy under the same transaction rules as
    /// ordinary apply. Successfully published files remain on later errors.
    pub fn rehearse<T>(
        &self,
        plan: &Plan,
        paths: &RehearsalPaths,
        capture: impl FnOnce(&Connection) -> Result<T, CheckError>,
        verify: impl FnOnce(&Connection, T) -> Result<(), CheckError>,
    ) -> Result<RehearsalReport, Error> {
        self.rehearse_plan(plan, paths, None, capture, verify)
    }

    /// Rehearse legacy adoption using the validator from planning.
    pub fn rehearse_adoption<T>(
        &self,
        plan: &Plan,
        paths: &RehearsalPaths,
        read_legacy: impl Fn(&Connection) -> Result<Vec<LegacyMigration>, CheckError>,
        capture: impl FnOnce(&Connection) -> Result<T, CheckError>,
        verify: impl FnOnce(&Connection, T) -> Result<(), CheckError>,
    ) -> Result<RehearsalReport, Error> {
        self.rehearse_plan(plan, paths, Some(&read_legacy), capture, verify)
    }

    fn rehearse_plan<T>(
        &self,
        plan: &Plan,
        paths: &RehearsalPaths,
        read_legacy: Option<&LegacyReader<'_>>,
        capture: impl FnOnce(&Connection) -> Result<T, CheckError>,
        verify: impl FnOnce(&Connection, T) -> Result<(), CheckError>,
    ) -> Result<RehearsalReport, Error> {
        let _lock = MigrationLock::acquire(&plan.path)?;
        if self.read_plan(&plan.path, read_legacy)? != *plan {
            return Err(Error::StalePlan);
        }
        self.rehearse_copy(plan, paths, read_legacy, capture, verify)
    }

    pub(crate) fn rehearse_copy<T>(
        &self,
        plan: &Plan,
        paths: &RehearsalPaths,
        read_legacy: Option<&LegacyReader<'_>>,
        capture: impl FnOnce(&Connection) -> Result<T, CheckError>,
        verify: impl FnOnce(&Connection, T) -> Result<(), CheckError>,
    ) -> Result<RehearsalReport, Error> {
        let run = || {
            let snapshot = backup::create(&plan.path, &paths.snapshot)?;
            let copy = backup::create(&snapshot.path, &paths.copy)?;
            let mut copied_plan = plan.clone();
            copied_plan.path = copy.path;
            self.apply_plan(&copied_plan, read_legacy, capture, verify)?;
            Ok(RehearsalReport {
                snapshot,
                copy_path: copied_plan.path,
            })
        };
        run().map_err(|source| Error::Rehearsal {
            snapshot_path: paths.snapshot.clone(),
            copy_path: paths.copy.clone(),
            source: Box::new(source),
        })
    }
}
