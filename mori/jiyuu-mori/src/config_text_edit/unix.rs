use std::ffi::{CStr, CString, c_char, c_int, c_uint};
use std::fs::{self, File, OpenOptions, Permissions};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use super::{MAX_BYTES, invalid};

#[derive(Debug, PartialEq, Eq)]
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
            return Err(invalid("unsafe document metadata"));
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

    pub(super) fn same_after_close(&self, other: &Self) -> bool {
        self == other
    }

    pub(super) fn mode_matches(&self, mode: u32) -> bool {
        self.mode & 0o777 == mode
    }

    pub(super) fn check_owner(&self) -> io::Result<()> {
        // SAFETY: geteuid takes no arguments.
        if self.owner != unsafe { libc::geteuid() } {
            return Err(invalid("document is owned by another user"));
        }
        Ok(())
    }
}

// Keep root registration and symlink validation shared with the other editors.
// This layout mirrors cbm_config_edit_target_t in cli/config_edit_path.h.
#[repr(C)]
struct ConfigTarget {
    status: c_int,
    dirfd: c_int,
    fd: c_int,
    base: [c_char; 256],
    path: [c_char; 4096],
}

unsafe extern "C" {
    fn cbm_config_edit_target_open(
        path: *const c_char,
        out: *mut ConfigTarget,
    ) -> c_int;
    fn cbm_config_edit_target_close(target: *mut ConfigTarget);
    fn cbm_config_edit_target_create_temp(
        target: *const ConfigTarget,
        name: *const c_char,
        mode: c_uint,
    ) -> c_int;
    fn cbm_config_edit_target_commit(
        target: *const ConfigTarget,
        name: *const c_char,
    ) -> c_int;
    fn cbm_config_edit_target_unlink(
        target: *const ConfigTarget,
        name: *const c_char,
    ) -> c_int;
}

pub(super) struct Target {
    raw: ConfigTarget,
    path: PathBuf,
}

impl Target {
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        let path = CString::new(path.as_os_str().as_bytes())?;
        let mut raw = ConfigTarget {
            status: 0,
            dirfd: -1,
            fd: -1,
            base: [0; 256],
            path: [0; 4096],
        };
        // SAFETY: path is NUL-terminated and raw has the C struct layout.
        let status =
            unsafe { cbm_config_edit_target_open(path.as_ptr(), &mut raw) };
        if status < 0 {
            return Err(invalid("configuration path was refused"));
        }
        // SAFETY: a successful open writes a NUL-terminated path.
        let resolved = unsafe { CStr::from_ptr(raw.path.as_ptr()) };
        use std::ffi::OsStr;
        Ok(Self {
            path: OsStr::from_bytes(resolved.to_bytes()).into(),
            raw,
        })
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    fn followed(&self) -> bool {
        self.raw.status == 1
    }

    pub(super) fn same_location(&self, other: &Self) -> bool {
        self.path == other.path && self.followed() == other.followed()
    }

    pub(super) fn reader(&self) -> io::Result<Option<File>> {
        if self.followed() {
            // SAFETY: the resolver owns a live descriptor. Duplication leaves it
            // available to close with the rest of the C target.
            let fd =
                unsafe { libc::fcntl(self.raw.fd, libc::F_DUPFD_CLOEXEC, 0) };
            return fd_to_file(fd).map(Some);
        }
        match open_read(&self.path) {
            Ok(file) => Ok(Some(file)),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                match fs::symlink_metadata(&self.path) {
                    Err(err) if err.kind() == io::ErrorKind::NotFound => {
                        Ok(None)
                    }
                    _ => Err(invalid("destination appeared during open")),
                }
            }
            Err(err) => Err(err),
        }
    }

    pub(super) fn create_temp(&self, path: &Path) -> io::Result<File> {
        if self.followed() {
            let name = basename(path)?;
            // SAFETY: the resolver owns the pinned dirfd and name is NUL-terminated.
            let fd = unsafe {
                cbm_config_edit_target_create_temp(
                    &self.raw,
                    name.as_ptr(),
                    0o600,
                )
            };
            fd_to_file(fd)
        } else {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)
        }
    }

    pub(super) fn reopen_temp(&self, path: &Path) -> io::Result<File> {
        if self.followed() {
            let name = basename(path)?;
            // SAFETY: dirfd is live and name is NUL-terminated.
            let fd = unsafe {
                libc::openat(
                    self.raw.dirfd,
                    name.as_ptr(),
                    READ_FLAGS | libc::O_CLOEXEC,
                )
            };
            fd_to_file(fd)
        } else {
            open_read(path)
        }
    }

    pub(super) fn discard_temp(&self, path: &Path) {
        if self.followed() {
            if let Ok(name) = basename(path) {
                // SAFETY: dirfd is live and name is NUL-terminated.
                unsafe {
                    cbm_config_edit_target_unlink(&self.raw, name.as_ptr())
                };
            }
        } else {
            let _ = fs::remove_file(path);
        }
    }

    pub(super) fn publish(
        &self,
        staged: &Path,
        destination_exists: bool,
    ) -> io::Result<()> {
        if self.followed() {
            let name = basename(staged)?;
            // SAFETY: dirfd is live and name is NUL-terminated.
            let rc = unsafe {
                cbm_config_edit_target_commit(&self.raw, name.as_ptr())
            };
            return if rc == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            };
        }
        if destination_exists {
            fs::rename(staged, &self.path)?;
        } else {
            // A concurrent creator must win even after the last snapshot check.
            fs::hard_link(staged, &self.path)?;
            fs::remove_file(staged)?;
        }
        sync_parent(&self.path)
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        // SAFETY: raw was initialized by cbm_config_edit_target_open.
        unsafe { cbm_config_edit_target_close(&mut self.raw) };
    }
}

fn fd_to_file(fd: RawFd) -> io::Result<File> {
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the successful open or dup returned a newly owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn basename(path: &Path) -> io::Result<CString> {
    let name = path
        .file_name()
        .ok_or_else(|| invalid("staging path has no filename"))?;
    Ok(CString::new(name.as_bytes())?)
}

const READ_FLAGS: c_int = libc::O_NOFOLLOW | libc::O_NONBLOCK;

fn open_read(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(READ_FLAGS)
        .open(path)
}

fn sync_parent(path: &Path) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY)
        .open(parent)?
        .sync_all()
}

pub(super) fn prepare(
    file: &File,
    old: Option<&Identity>,
    mode: Option<u32>,
) -> io::Result<()> {
    if let Some(old) = old {
        // SAFETY: file owns a live descriptor and the owner and group came from fstat.
        if unsafe { libc::fchown(file.as_raw_fd(), old.owner, old.group) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    let permissions =
        mode.unwrap_or_else(|| old.map_or(0o600, |old| old.mode & 0o777));
    file.set_permissions(Permissions::from_mode(permissions))
}

pub(super) fn close(file: File) -> io::Result<()> {
    // SAFETY: into_raw_fd transfers ownership of the live descriptor.
    if unsafe { libc::close(file.into_raw_fd()) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub(super) fn delete(path: &Path) -> io::Result<()> {
    fs::remove_file(path)?;
    sync_parent(path)
}
