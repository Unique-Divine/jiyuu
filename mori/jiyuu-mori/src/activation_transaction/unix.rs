use std::ffi::{CStr, CString, c_char, c_int};
use std::fs::{self, File, Metadata};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use crate::ffi_output;

use super::{Entry, cstring, ffi, refused};

const DIRECTORY_FLAGS: c_int =
    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW;
const READ_FLAGS: c_int =
    libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK;

unsafe extern "C" {
    fn cbm_macos_extended_acl_fd_is_empty(fd: c_int) -> bool;
    fn cbm_log_activation_ancestor_group_writable(
        path: *const c_char,
        mode: *const c_char,
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Identity(u64, u64);

pub(super) struct Directory {
    path: CString,
    file: File,
    identity: Identity,
}

impl Directory {
    pub(super) fn open(path: CString, _source: bool) -> io::Result<Self> {
        let walk = walk_path(&path)?;
        let root = if walk.to_bytes().starts_with(b"/") {
            c"/"
        } else {
            c"."
        };
        let mut file = open_at(libc::AT_FDCWD, root, DIRECTORY_FLAGS, 0)?;
        let mut parts = walk
            .to_bytes()
            .split(|&byte| byte == b'/')
            .filter(|part| !part.is_empty())
            .peekable();
        if parts.peek().is_some() && !trusted_ancestor(&file.metadata()?) {
            return Err(refused());
        }
        while let Some(part) = parts.next() {
            if part == b"." {
                continue;
            }
            if part == b".." {
                return Err(refused());
            }
            let next =
                open_at(file.as_raw_fd(), &cstring(part), DIRECTORY_FLAGS, 0)?;
            if parts.peek().is_some() {
                let metadata = next.metadata()?;
                if !trusted_ancestor(&metadata) {
                    note_mode(
                        "ancestor_directory_world_writable",
                        &walk,
                        &metadata,
                    );
                    return Err(refused());
                }
                if metadata.mode() & 0o020 != 0 && !root_sticky(&metadata) {
                    let mode = cstring(
                        format!("{:04o}", metadata.mode() & 0o7777).as_bytes(),
                    );
                    // SAFETY: both strings remain valid through the C logger.
                    unsafe {
                        cbm_log_activation_ancestor_group_writable(
                            ffi_output::c_str(&walk),
                            ffi_output::c_str(&mode),
                        )
                    };
                }
            }
            file = next;
        }
        let metadata = file.metadata()?;
        let predicate = if !metadata.is_dir() {
            Some("install_dir_not_a_directory")
        } else if metadata.uid() != current_uid() {
            Some("install_dir_not_owned_by_you")
        } else if metadata.mode() & 0o022 != 0 {
            Some("install_dir_group_or_world_writable")
        } else {
            None
        };
        if let Some(predicate) = predicate {
            note_mode(predicate, &path, &metadata);
            return Err(refused());
        }
        if !acl_empty(&file) {
            ffi::note(
                "install_dir_carries_extra_acl_entries",
                "posix acl",
                Some(&path),
            );
            return Err(refused());
        }
        Ok(Self {
            path,
            file,
            identity: Identity(metadata.dev(), metadata.ino()),
        })
    }

    pub(super) fn path(&self) -> &CStr {
        &self.path
    }

    pub(super) fn revalidate(&self) -> io::Result<()> {
        let metadata = self.file.metadata()?;
        if !metadata.is_dir()
            || metadata.uid() != current_uid()
            || metadata.mode() & 0o022 != 0
            || !acl_empty(&self.file)
            || self.identity != Identity(metadata.dev(), metadata.ino())
            || Self::open(self.path.clone(), false)?.identity != self.identity
        {
            return Err(refused());
        }
        Ok(())
    }

    fn fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }

    fn stat(&self, entry: &Entry) -> io::Result<libc::stat> {
        let mut metadata = std::mem::MaybeUninit::uninit();
        // SAFETY: fd is live, name is NUL-terminated, and the out pointer is writable.
        let result = unsafe {
            libc::fstatat(
                self.fd(),
                ffi_output::c_str(&entry.name),
                metadata.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        check(result)?;
        // SAFETY: fstatat initialized the structure on success.
        Ok(unsafe { metadata.assume_init() })
    }

    fn open_verified(
        &self,
        entry: &Entry,
        before: &libc::stat,
        links: u64,
    ) -> io::Result<(File, Identity)> {
        let id = valid_entry(before, links)?;
        let file = open_at(self.fd(), &entry.name, READ_FLAGS, 0)?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.uid() != current_uid()
            || metadata.nlink() != links
            || metadata.mode() & 0o022 != 0
            || id != Identity(metadata.dev(), metadata.ino())
            || !acl_empty(&file)
        {
            return Err(refused());
        }
        Ok((file, id))
    }

    pub(super) fn snapshot(
        &self,
        entry: &Entry,
        links: u64,
    ) -> io::Result<Option<Identity>> {
        self.revalidate()?;
        let before = match self.stat(entry) {
            Ok(stat) => stat,
            Err(error) if error.raw_os_error() == Some(libc::ENOENT) => {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let (file, id) = self.open_verified(entry, &before, links)?;
        if valid_entry(&self.stat(entry)?, links)? != id {
            return Err(refused());
        }
        close(file)?;
        Ok(Some(id))
    }

    pub(super) fn matches(
        &self,
        entry: &Entry,
        id: Identity,
        links: u64,
    ) -> bool {
        self.snapshot(entry, links)
            .is_ok_and(|snapshot| snapshot == Some(id))
    }

    pub(super) fn create(&self, entry: &Entry) -> io::Result<(File, Identity)> {
        self.revalidate()?;
        let file = open_at(
            self.fd(),
            &entry.name,
            libc::O_RDWR
                | libc::O_CREAT
                | libc::O_EXCL
                | libc::O_CLOEXEC
                | libc::O_NOFOLLOW,
            0o700,
        )?;
        // SAFETY: file owns a live descriptor.
        check(unsafe { libc::fchmod(file.as_raw_fd(), 0o700) })?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.uid() != current_uid()
            || metadata.nlink() != 1
            || metadata.mode() & 0o777 != 0o700
            || !acl_empty(&file)
        {
            return Err(refused());
        }
        let identity = Identity(metadata.dev(), metadata.ino());
        Ok((file, identity))
    }

    pub(super) fn sync(&self) -> io::Result<()> {
        sync_file(&self.file)
    }

    pub(super) fn rename(
        &self,
        from: &Entry,
        to: &Entry,
        _replace: bool,
    ) -> io::Result<()> {
        self.revalidate()?;
        // SAFETY: the directory fd and both NUL-terminated names are live.
        retry(|| unsafe {
            libc::renameat(
                self.fd(),
                ffi_output::c_str(&from.name),
                self.fd(),
                ffi_output::c_str(&to.name),
            )
        })
    }

    pub(super) fn rename_absent(
        &self,
        from: &Entry,
        to: &Entry,
    ) -> io::Result<bool> {
        #[cfg(target_os = "linux")]
        let result = retry(|| {
            // SAFETY: renameat2 takes the same descriptor-relative names as renameat.
            unsafe {
                libc::syscall(
                    libc::SYS_renameat2,
                    self.fd(),
                    ffi_output::c_str(&from.name),
                    self.fd(),
                    ffi_output::c_str(&to.name),
                    libc::RENAME_NOREPLACE,
                ) as c_int
            }
        });
        #[cfg(target_os = "macos")]
        let result = retry(|| {
            // SAFETY: both descriptors are live and the names are NUL-terminated.
            unsafe {
                libc::renameatx_np(
                    self.fd(),
                    ffi_output::c_str(&from.name),
                    self.fd(),
                    ffi_output::c_str(&to.name),
                    libc::RENAME_EXCL,
                )
            }
        });
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let result = {
            let _ = (from, to);
            Err(io::Error::from_raw_os_error(libc::ENOSYS))
        };
        match result {
            Ok(()) => Ok(true),
            Err(error)
                if matches!(
                    error.raw_os_error(),
                    Some(libc::ENOSYS | libc::EINVAL | libc::ENOTSUP)
                ) || error.raw_os_error() == Some(libc::EOPNOTSUPP) =>
            {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    pub(super) fn link(&self, from: &Entry, to: &Entry) -> io::Result<()> {
        // SAFETY: the directory fd and both NUL-terminated names are live.
        retry(|| unsafe {
            libc::linkat(
                self.fd(),
                ffi_output::c_str(&from.name),
                self.fd(),
                ffi_output::c_str(&to.name),
                0,
            )
        })
    }

    pub(super) fn unlink_strict(&self, entry: &Entry) -> io::Result<()> {
        // SAFETY: the directory fd and entry name remain live.
        retry(|| unsafe {
            libc::unlinkat(self.fd(), ffi_output::c_str(&entry.name), 0)
        })
    }

    pub(super) fn unlink(
        &self,
        entry: &Entry,
        _allow_deferred: bool,
    ) -> io::Result<bool> {
        match self.unlink_strict(entry) {
            Ok(()) => Ok(false),
            Err(error) if error.raw_os_error() == Some(libc::ENOENT) => {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    pub(super) fn unlink_expected(
        &self,
        entry: &Entry,
        id: Identity,
        allow_deferred: bool,
    ) -> io::Result<bool> {
        match self.snapshot(entry, 1)? {
            None => Ok(false),
            Some(current) if current == id => self.unlink(entry, allow_deferred),
            _ => Err(refused()),
        }
    }
}

pub(super) fn source_open(path: &CStr) -> io::Result<File> {
    let (directory, entry) = Entry::split(path).map_err(|_| refused())?;
    let directory = Directory::open(directory, true)?;
    let (file, _) =
        directory.open_verified(&entry, &directory.stat(&entry)?, 1)?;
    Ok(file)
}

fn check(code: c_int) -> io::Result<()> {
    if code == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn retry(mut call: impl FnMut() -> c_int) -> io::Result<()> {
    loop {
        match check(call()) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            result => return result,
        }
    }
}

fn open_at(
    parent: RawFd,
    name: &CStr,
    flags: c_int,
    mode: libc::mode_t,
) -> io::Result<File> {
    // SAFETY: the path is NUL-terminated. parent is a live directory fd or AT_FDCWD.
    let fd = unsafe {
        libc::openat(parent, ffi_output::c_str(name), flags, mode as c_int)
    };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: openat transferred an owned descriptor.
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

pub(super) fn sync_file(file: &File) -> io::Result<()> {
    // SAFETY: the file descriptor remains live for fsync.
    match retry(|| unsafe { libc::fsync(file.as_raw_fd()) }) {
        Err(error)
            if matches!(
                error.raw_os_error(),
                Some(libc::EINVAL | libc::ENOTSUP | libc::EROFS)
            ) =>
        {
            Ok(())
        }
        result => result,
    }
}

pub(super) fn close(file: File) -> io::Result<()> {
    // SAFETY: the raw descriptor was live and ownership passed to close.
    check(unsafe { libc::close(file.into_raw_fd()) })
}

pub(super) fn set_errno(error: c_int) {
    // SAFETY: the accessor returns this thread's writable errno.
    unsafe {
        #[cfg(target_os = "linux")]
        {
            *libc::__errno_location() = error;
        }
        #[cfg(target_os = "macos")]
        {
            *libc::__error() = error;
        }
    }
}

#[cfg(any(test, feature = "native-engine-test-seams"))]
pub(super) fn rename_failures(_count: u32) {}

fn current_uid() -> libc::uid_t {
    // SAFETY: geteuid takes no arguments.
    unsafe { libc::geteuid() }
}

fn acl_empty(file: &File) -> bool {
    // SAFETY: file owns the descriptor throughout the ACL check.
    unsafe { cbm_macos_extended_acl_fd_is_empty(file.as_raw_fd()) }
}

// The widths of these stat fields differ between Unix targets.
#[allow(clippy::unnecessary_cast)]
fn valid_entry(metadata: &libc::stat, links: u64) -> io::Result<Identity> {
    if metadata.st_mode & libc::S_IFMT != libc::S_IFREG
        || metadata.st_uid != current_uid()
        || metadata.st_nlink as u64 != links
        || metadata.st_mode & 0o022 != 0
    {
        return Err(refused());
    }
    Ok(Identity(metadata.st_dev as u64, metadata.st_ino as u64))
}

fn root_sticky(metadata: &Metadata) -> bool {
    metadata.uid() == 0 && metadata.mode() & 0o1000 != 0
}

fn trusted_ancestor(metadata: &Metadata) -> bool {
    metadata.is_dir()
        && (metadata.uid() == 0 || metadata.uid() == current_uid())
        && (metadata.mode() & 0o002 == 0 || root_sticky(metadata))
}

fn note_mode(predicate: &str, path: &CStr, metadata: &Metadata) {
    let detail = format!(
        "mode {:04o}, uid {}",
        metadata.mode() & 0o7777,
        metadata.uid()
    );
    ffi::note(predicate, &detail, Some(path));
}

fn symlink_metadata(path: &CStr) -> io::Result<Metadata> {
    fs::symlink_metadata(Path::new(std::ffi::OsStr::from_bytes(path.to_bytes())))
}

pub(super) fn map_alias(
    link: &CStr,
    link_owner: libc::uid_t,
    owner: libc::uid_t,
    accept_own_target: bool,
    rest: &[u8],
) -> io::Result<CString> {
    if link_owner != owner {
        return Err(refused());
    }
    let path = Path::new(std::ffi::OsStr::from_bytes(link.to_bytes()));
    let resolved = fs::canonicalize(path)?;
    let meta = fs::symlink_metadata(&resolved)?;
    if !meta.is_dir()
        || (meta.uid() != owner
            && !(accept_own_target && meta.uid() == current_uid()))
    {
        return Err(refused());
    }
    let mut mapped = resolved.into_os_string().into_encoded_bytes();
    if mapped.len().saturating_add(rest.len()) >= 4096 {
        return Err(refused());
    }
    mapped.extend_from_slice(rest);
    Ok(cstring(&mapped))
}

fn walk_path(path: &CStr) -> io::Result<CString> {
    #[cfg(target_os = "linux")]
    let (home, owner) = ffi::home_root();
    let aliases = [(c"/tmp", 0, false), (c"/var", 0, false)];
    #[cfg(target_os = "linux")]
    let aliases = aliases.into_iter().chain([(home.as_c_str(), owner, true)]);
    for (alias, owner, is_home) in aliases {
        let Some(rest) = path.to_bytes().strip_prefix(alias.to_bytes()) else {
            continue;
        };
        if !rest.is_empty() && rest[0] != b'/' {
            continue;
        }
        let Ok(meta) = symlink_metadata(alias) else {
            continue;
        };
        if meta.is_symlink() {
            return map_alias(alias, meta.uid(), owner, false, rest);
        }
        if !is_home
            || !meta.is_dir()
            || meta.uid() != owner
            || meta.mode() & 0o022 != 0
        {
            continue;
        }
        let Some(name) = rest.strip_prefix(b"/") else {
            continue;
        };
        let length = name
            .iter()
            .position(|&byte| byte == b'/')
            .unwrap_or(name.len());
        if length == 0 {
            continue;
        }
        let entry_len = alias.to_bytes().len() + length + 1;
        if entry_len >= 4096 {
            return Err(refused());
        }
        let entry = cstring(&path.to_bytes()[..entry_len]);
        if let Ok(metadata) = symlink_metadata(&entry)
            && metadata.is_symlink()
        {
            return map_alias(
                &entry,
                metadata.uid(),
                owner,
                true,
                &path.to_bytes()[entry_len..],
            );
        }
    }
    Ok(path.to_owned())
}
