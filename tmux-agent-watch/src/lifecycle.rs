use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use anyhow::{Context, Result};
use fs2::FileExt;

use crate::snapshot::prepare_runtime_directory;
use crate::tmux::ServerIdentity;

pub struct WatcherLock {
    _file: File,
    path: PathBuf,
}

impl WatcherLock {
    pub fn try_acquire(identity: &ServerIdentity) -> Result<Option<Self>> {
        let path = prepare_runtime_directory()?
            .join(format!("server-{}.lock", identity.pid));
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .with_context(|| {
                format!("failed to open lock {}", path.display())
            })?;

        match FileExt::try_lock_exclusive(&file) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                return Ok(None);
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to lock {}", path.display())
                });
            }
        }

        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        writeln!(
            file,
            "watcher_pid={}\nserver_pid={}\nsocket_path={}",
            std::process::id(),
            identity.pid,
            identity.socket_path.display(),
        )?;
        file.sync_all()?;

        Ok(Some(Self { _file: file, path }))
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(pid: u32) -> ServerIdentity {
        ServerIdentity {
            socket_path: PathBuf::from(format!("/tmp/tmux-test-{pid}")),
            pid,
            id: format!("server-{pid}"),
        }
    }

    #[test]
    fn lock_is_singleton_and_recovers_after_drop() {
        let server = identity(std::process::id());
        let first = WatcherLock::try_acquire(&server)
            .expect("first lock should succeed")
            .expect("first watcher should own lock");
        assert!(
            WatcherLock::try_acquire(&server)
                .expect("duplicate check should succeed")
                .is_none()
        );

        drop(first);
        assert!(
            WatcherLock::try_acquire(&server)
                .expect("released lock should be reusable")
                .is_some()
        );
    }

    #[test]
    fn different_server_pids_use_independent_locks() {
        let first = WatcherLock::try_acquire(&identity(std::process::id() + 1))
            .expect("first lock should succeed")
            .expect("first server should own lock");
        let second = WatcherLock::try_acquire(&identity(std::process::id() + 2))
            .expect("second lock should succeed")
            .expect("second server should own an independent lock");
        drop((first, second));
    }
}
