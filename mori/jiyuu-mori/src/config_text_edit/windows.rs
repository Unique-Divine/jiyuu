use std::ffi::c_void;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, IntoRawHandle};
use std::path::{Path, PathBuf};

use super::{MAX_BYTES, invalid};

const FILE_TYPE_DISK: u32 = 1;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const REPLACEFILE_WRITE_THROUGH: u32 = 1;
const MOVEFILE_WRITE_THROUGH: u32 = 8;

#[repr(C)]
#[derive(PartialEq, Eq, Debug)]
struct FileTime {
    low: u32,
    high: u32,
}

#[repr(C)]
#[derive(PartialEq, Eq, Debug)]
pub(super) struct Identity {
    attributes: u32,
    creation: FileTime,
    access: FileTime,
    write: FileTime,
    volume: u32,
    size_high: u32,
    size_low: u32,
    links: u32,
    index_high: u32,
    index_low: u32,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetFileType(handle: *mut c_void) -> u32;
    fn CloseHandle(handle: *mut c_void) -> i32;
    fn GetFileInformationByHandle(
        handle: *mut c_void,
        out: *mut Identity,
    ) -> i32;
    fn ReplaceFileW(
        path: *const u16,
        replacement: *const u16,
        backup: *const u16,
        flags: u32,
        exclude: *mut c_void,
        reserved: *mut c_void,
    ) -> i32;
    fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
}

impl Identity {
    pub(super) fn read(file: &File) -> io::Result<Self> {
        let handle = file.as_raw_handle();
        let mut identity = std::mem::MaybeUninit::uninit();
        // SAFETY: the file owns the handle and the out pointer has the Win32 layout.
        let mut identity = unsafe {
            if GetFileType(handle) != FILE_TYPE_DISK {
                return Err(invalid("document is not a disk file"));
            }
            if GetFileInformationByHandle(handle, identity.as_mut_ptr()) == 0 {
                return Err(io::Error::last_os_error());
            }
            identity.assume_init()
        };
        if identity.attributes
            & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)
            != 0
            || identity.links != 1
            || (identity.index_high == 0 && identity.index_low == 0)
            || identity.size_high != 0
            || identity.len() > MAX_BYTES
        {
            return Err(invalid("unsafe document metadata"));
        }
        // Reading a descriptor can change atime.
        identity.access = FileTime { low: 0, high: 0 };
        Ok(identity)
    }

    pub(super) fn len(&self) -> usize {
        self.size_low as usize
    }

    pub(super) fn same_after_close(&self, other: &Self) -> bool {
        // Windows may update write times when closing the writer.
        self.volume == other.volume
            && self.index_high == other.index_high
            && self.index_low == other.index_low
            && self.attributes == other.attributes
            && self.links == other.links
            && self.len() == other.len()
    }

    pub(super) fn mode_matches(&self, _mode: u32) -> bool {
        true
    }

    pub(super) fn check_owner(&self) -> io::Result<()> {
        Ok(())
    }
}

pub(super) struct Target(PathBuf);

impl Target {
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        Ok(Self(path.to_owned()))
    }

    pub(super) fn path(&self) -> &Path {
        &self.0
    }

    pub(super) fn same_location(&self, other: &Self) -> bool {
        self.0 == other.0
    }

    pub(super) fn reader(&self) -> io::Result<Option<File>> {
        match open_read(&self.0) {
            Ok(file) => Ok(Some(file)),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err),
        }
    }

    pub(super) fn create_temp(&self, path: &Path) -> io::Result<File> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)
    }

    pub(super) fn reopen_temp(&self, path: &Path) -> io::Result<File> {
        open_read(path)
    }

    pub(super) fn discard_temp(&self, path: &Path) {
        let _ = fs::remove_file(path);
    }

    pub(super) fn publish(
        &self,
        staged: &Path,
        destination_exists: bool,
    ) -> io::Result<()> {
        let staged = wide(staged);
        let path = wide(&self.0);
        // SAFETY: both paths are NUL-terminated and the optional pointers are null.
        let result = unsafe {
            if destination_exists {
                ReplaceFileW(
                    path.as_ptr(),
                    staged.as_ptr(),
                    std::ptr::null(),
                    REPLACEFILE_WRITE_THROUGH,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            } else {
                MoveFileExW(
                    staged.as_ptr(),
                    path.as_ptr(),
                    MOVEFILE_WRITE_THROUGH,
                )
            }
        };
        if result == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain([0]).collect()
}

fn open_read(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

pub(super) fn prepare(
    _file: &File,
    _old: Option<&Identity>,
    _mode: Option<u32>,
) -> io::Result<()> {
    Ok(())
}

pub(super) fn close(file: File) -> io::Result<()> {
    // SAFETY: into_raw_handle transfers ownership of the live handle.
    if unsafe { CloseHandle(file.into_raw_handle()) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub(super) fn delete(path: &Path) -> io::Result<()> {
    fs::remove_file(path)
}
