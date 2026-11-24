use std::ffi::{CStr, OsStr};
use std::fs::{self, File, OpenOptions, Permissions};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use super::{
    MAX_BYTES, Target, Temp, cbm_config_edit_target_commit,
    cbm_config_edit_target_create_temp, cbm_config_edit_target_unlink,
    invalid_path, parent,
};

#[derive(PartialEq, Eq)]
pub(super) struct Identity {
    device: u64,
    inode: u64,
    mode: u32,
    owner: u32,
    group: u32,
    len: usize,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl Identity {
    pub(super) fn read(file: &File) -> io::Result<Self> {
        let meta = file.metadata()?;
        if !meta.is_file()
            || meta.ino() == 0
            || meta.nlink() != 1
            || meta.len() > MAX_BYTES as u64
            || meta.mode() & 0o7000 != 0
        {
            return Err(invalid_path());
        }
        Ok(Self {
            device: meta.dev(),
            inode: meta.ino(),
            mode: meta.mode(),
            owner: meta.uid(),
            group: meta.gid(),
            len: meta.len() as usize,
            modified: (meta.mtime(), meta.mtime_nsec()),
            changed: (meta.ctime(), meta.ctime_nsec()),
        })
    }

    pub(super) fn len(&self) -> usize {
        self.len
    }
}

fn borrowed_path(path: &CStr) -> &Path {
    Path::new(OsStr::from_bytes(path.to_bytes()))
}

pub(super) fn path(name: &CStr) -> io::Result<PathBuf> {
    Ok(borrowed_path(name).to_owned())
}

fn basename(name: &CStr) -> &CStr {
    let offset = name
        .to_bytes()
        .iter()
        .rposition(|&byte| byte == b'/')
        .map_or(0, |i| i + 1);
    CStr::from_bytes_with_nul(&name.to_bytes_with_nul()[offset..])
        .expect("suffix of C string")
}

fn file_from_fd(fd: RawFd) -> io::Result<File> {
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the caller transferred ownership of this descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}

pub(super) fn reader(name: &CStr) -> io::Result<Option<File>> {
    let mut target = Target::open(name)?;
    if target.followed() {
        return file_from_fd(target.take_fd()).map(Some);
    }
    let path = borrowed_path(target.path());
    match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match fs::symlink_metadata(path) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    Ok(None)
                }
                _ => Err(invalid_path()),
            }
        }
        Err(error) => Err(error),
    }
}

pub(super) fn create_temp(
    target: &Target,
    name: &CStr,
    path: &Path,
) -> io::Result<File> {
    if target.followed() {
        // SAFETY: the target owns the pinned dirfd and basename is terminated.
        let fd = unsafe {
            cbm_config_edit_target_create_temp(
                &target.0,
                basename(name).as_ptr(),
                0o600,
            )
        };
        file_from_fd(fd)
    } else {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
    }
}

pub(super) fn discard(target: &Target, name: &CStr, path: &Path) {
    if target.followed() {
        // SAFETY: the target and the terminated staging basename are live.
        unsafe {
            cbm_config_edit_target_unlink(&target.0, basename(name).as_ptr())
        };
    } else {
        let _ = fs::remove_file(path);
    }
}

pub(super) fn publish(
    target: &Target,
    temp: &Temp<'_>,
    exists: bool,
) -> io::Result<()> {
    if target.followed() {
        // SAFETY: the target and the terminated staging basename are live.
        return status(unsafe {
            cbm_config_edit_target_commit(
                &target.0,
                basename(&temp.name).as_ptr(),
            )
        });
    }
    let path = borrowed_path(target.path());
    if exists {
        fs::rename(&temp.path, path)?;
    } else {
        // A concurrent creator wins even after final snapshot verification.
        fs::hard_link(&temp.path, path)?;
        fs::remove_file(&temp.path)?;
    }
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY)
        .open(borrowed_path(&parent(target.path())))?;
    directory.sync_all()?;
    close(directory)
}

fn status(result: i32) -> io::Result<()> {
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub(super) fn prepare(file: &File, old: Option<&Identity>) -> io::Result<()> {
    if let Some(old) = old {
        // SAFETY: the live descriptor and owner/group came from the two files.
        status(unsafe { libc::fchown(file.as_raw_fd(), old.owner, old.group) })?;
    }
    file.set_permissions(Permissions::from_mode(
        old.map_or(0o600, |old| old.mode & 0o777),
    ))
}

pub(super) fn close(file: File) -> io::Result<()> {
    // SAFETY: into_raw_fd transfers ownership to this close.
    status(unsafe { libc::close(file.into_raw_fd()) })
}
