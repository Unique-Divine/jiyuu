use crate::ffi_output;

use super::*;
use std::{
    ffi::{OsStr, c_int},
    fs::{self, Metadata, OpenOptions},
    mem::MaybeUninit,
    os::{
        fd::{AsRawFd, FromRawFd, IntoRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        },
    },
    path::Path,
};

const PRIVATE_MODE: u32 = 0o600;
const SPECIAL_BITS: u32 = 0o7000;

#[derive(PartialEq, Eq)]
struct Identity {
    device: u64,
    inode: u64,
    mode: u32,
    links: u64,
    owner: u32,
}

impl Identity {
    fn from(meta: &Metadata) -> Self {
        Self {
            device: meta.dev(),
            inode: meta.ino(),
            mode: meta.mode(),
            links: meta.nlink(),
            owner: meta.uid(),
        }
    }
}

#[derive(PartialEq, Eq)]
pub(super) struct Snapshot {
    identity: Identity,
    group: u32,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl Snapshot {
    pub(super) fn capture(file: &File) -> Result<Self> {
        let meta = file.metadata().map_err(|_| ())?;
        require(
            meta.is_file()
                && meta.nlink() == 1
                && meta.mode() & SPECIAL_BITS == 0,
        )?;
        Ok(Self {
            identity: Identity::from(&meta),
            group: meta.gid(),
            size: meta.size(),
            modified: (meta.mtime(), meta.mtime_nsec()),
            changed: (meta.ctime(), meta.ctime_nsec()),
        })
    }

    pub(super) fn size(&self) -> u64 {
        self.size
    }
}

// Shared config_edit_path.h ABI. The helper also owns the per-thread opt-in
// scope used by the other native configuration editors.
#[repr(C)]
struct EditTarget {
    status: c_int,
    dirfd: c_int,
    fd: c_int,
    base: [c_char; 256],
    path: [c_char; 4096],
}

unsafe extern "C" {
    fn cbm_config_edit_target_open(
        path: *const c_char,
        out: *mut EditTarget,
    ) -> c_int;
    fn cbm_config_edit_target_close(target: *mut EditTarget);
    fn cbm_config_edit_target_create_temp(
        target: *const EditTarget,
        name: *const c_char,
        mode: libc::c_uint,
    ) -> c_int;
    fn cbm_config_edit_target_commit(
        target: *const EditTarget,
        name: *const c_char,
    ) -> c_int;
    fn cbm_config_edit_target_unlink(
        target: *const EditTarget,
        name: *const c_char,
    ) -> c_int;
}

pub(super) struct Target(EditTarget);

impl Target {
    pub(super) fn open(path: &CStr) -> Result<Self> {
        let mut out = MaybeUninit::zeroed();
        require(
            unsafe {
                cbm_config_edit_target_open(
                    ffi_output::c_str(path),
                    out.as_mut_ptr(),
                )
            } >= 0,
        )?;
        Ok(Self(unsafe { out.assume_init() }))
    }

    fn native(&self) -> *const EditTarget {
        // The zero-initialized arrays and scalar fields have no padding.
        unsafe { ffi_output::written(&self.0, 1) };
        &self.0
    }

    pub(super) fn path(&self) -> &CStr {
        unsafe { CStr::from_ptr(self.0.path.as_ptr()) }
    }

    fn followed(&self) -> bool {
        self.0.status == 1
    }

    pub(super) fn create(&self, temp: &CStr) -> io::Result<File> {
        if self.followed() {
            let name = base_name(temp);
            let fd = unsafe {
                cbm_config_edit_target_create_temp(
                    self.native(),
                    ffi_output::c_str(name),
                    PRIVATE_MODE,
                )
            };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            return Ok(unsafe { File::from_raw_fd(fd) });
        }
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(PRIVATE_MODE)
            .custom_flags(libc::O_NOFOLLOW)
            .open(os_path(temp))
    }

    pub(super) fn discard(&self, temp: &CStr) {
        if self.followed() {
            unsafe {
                cbm_config_edit_target_unlink(
                    self.native(),
                    ffi_output::c_str(base_name(temp)),
                )
            };
        } else {
            let _ = fs::remove_file(os_path(temp));
        }
    }

    pub(super) fn replace(&self, temp: &CStr, exists: bool) -> Result<()> {
        if self.followed() {
            return require(
                unsafe {
                    cbm_config_edit_target_commit(
                        self.native(),
                        ffi_output::c_str(base_name(temp)),
                    )
                } == 0,
            );
        }
        let path = os_path(self.path());
        let temp = os_path(temp);
        if exists {
            fs::rename(temp, path).map_err(|_| ())?;
        } else {
            fs::hard_link(temp, path).map_err(|_| ())?;
            fs::remove_file(temp).map_err(|_| ())?;
        }
        let bytes = self.path().to_bytes();
        let parent = match bytes.iter().rposition(|&b| b == b'/') {
            Some(i) => &bytes[..i.max(1)],
            None => b".",
        };
        let dir = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY)
            .open(OsStr::from_bytes(parent))
            .map_err(|_| ())?;
        dir.sync_all().map_err(|_| ())?;
        close(dir)
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        unsafe {
            ffi_output::written(&self.0, 1);
            cbm_config_edit_target_close(&mut self.0)
        };
    }
}

fn base_name(path: &CStr) -> &CStr {
    let start = path
        .to_bytes()
        .iter()
        .rposition(|&b| b == b'/')
        .map_or(0, |i| i + 1);
    CStr::from_bytes_with_nul(&path.to_bytes_with_nul()[start..])
        .expect("suffix of a C string")
}

fn os_path(path: &CStr) -> &Path {
    Path::new(OsStr::from_bytes(path.to_bytes()))
}

fn missing(err: &io::Error) -> bool {
    err.raw_os_error() == Some(libc::ENOENT)
}

pub(crate) fn absent(path: &CStr) -> bool {
    fs::symlink_metadata(os_path(path)).is_err_and(|err| missing(&err))
}

pub(super) fn open_read(path: &CStr) -> Result<Option<File>> {
    let mut target = Target::open(path)?;
    if target.followed() {
        let fd = std::mem::replace(&mut target.0.fd, -1);
        require(fd >= 0)?;
        return Ok(Some(unsafe { File::from_raw_fd(fd) }));
    }
    match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(os_path(target.path()))
    {
        Ok(file) => Ok(Some(file)),
        Err(err) if missing(&err) && absent(target.path()) => Ok(None),
        Err(_) => Err(()),
    }
}

pub(super) fn set_permissions(
    file: &File,
    snapshot: Option<&Snapshot>,
) -> Result<()> {
    let mode = if let Some(snapshot) = snapshot {
        require(
            unsafe {
                libc::fchown(
                    file.as_raw_fd(),
                    snapshot.identity.owner,
                    snapshot.group,
                )
            } == 0,
        )?;
        snapshot.identity.mode & 0o777
    } else {
        PRIVATE_MODE
    };
    file.set_permissions(fs::Permissions::from_mode(mode))
        .map_err(|_| ())
}

pub(super) fn close(file: File) -> Result<()> {
    require(unsafe { libc::close(file.into_raw_fd()) } == 0)
}

fn flock(file: &File, flags: c_int) -> Result<()> {
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), flags) } == 0 {
            return Ok(());
        }
        require(
            io::Error::last_os_error().kind() == io::ErrorKind::Interrupted,
        )?;
    }
}

fn lock_identity(meta: Metadata) -> Result<Identity> {
    require(
        meta.is_file()
            && meta.nlink() == 1
            && meta.uid() == unsafe { libc::geteuid() }
            && meta.mode() & 0o7777 == PRIVATE_MODE,
    )?;
    Ok(Identity::from(&meta))
}

pub(crate) struct Lock {
    path: CString,
    file: File,
    identity: Identity,
}

impl Lock {
    pub(crate) fn acquire(path: &CStr) -> Result<Self> {
        let path = suffix_path(path, b".cbm-yaml.lock")?;
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        let file = match options
            .clone()
            .create_new(true)
            .mode(PRIVATE_MODE)
            .open(os_path(&path))
        {
            Ok(file) => {
                file.set_permissions(fs::Permissions::from_mode(PRIVATE_MODE))
                    .map_err(|_| ())?;
                file
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                options.open(os_path(&path)).map_err(|_| ())?
            }
            Err(_) => return Err(()),
        };
        let identity = lock_identity(file.metadata().map_err(|_| ())?)?;
        hooks::LOCK_POSTCREATE.run(&path);
        flock(&file, libc::LOCK_EX | libc::LOCK_NB)?;
        let lock = Self {
            path,
            file,
            identity,
        };
        lock.verify()?;
        Ok(lock)
    }

    fn verify(&self) -> Result<()> {
        let open = lock_identity(self.file.metadata().map_err(|_| ())?)?;
        let named = lock_identity(
            fs::symlink_metadata(os_path(&self.path)).map_err(|_| ())?,
        )?;
        require(open == self.identity && named == open)
    }

    pub(crate) fn release(self) -> Result<()> {
        let same = self.verify();
        let unlocked = flock(&self.file, libc::LOCK_UN);
        let closed = close(self.file);
        same.and(unlocked).and(closed)
    }
}
