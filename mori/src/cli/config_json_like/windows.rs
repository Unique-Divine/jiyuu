use std::ffi::{CStr, c_void};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, IntoRawHandle};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};

use super::{MAX_BYTES, Target, Temp, invalid_path};

const FILE_TYPE_DISK: u32 = 1;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const REPLACEFILE_WRITE_THROUGH: u32 = 1;
const MOVEFILE_WRITE_THROUGH: u32 = 8;

#[repr(C)]
#[derive(PartialEq, Eq)]
struct FileTime {
    low: u32,
    high: u32,
}

// BY_HANDLE_FILE_INFORMATION, with access time cleared for comparisons.
#[repr(C)]
#[derive(PartialEq, Eq)]
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
    fn GetFileInformationByHandle(
        handle: *mut c_void,
        out: *mut Identity,
    ) -> i32;
    fn CloseHandle(handle: *mut c_void) -> i32;
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
        let mut info = std::mem::MaybeUninit::uninit();
        // SAFETY: the file owns the handle and info has the Win32 layout.
        let mut info = unsafe {
            if GetFileType(handle) != FILE_TYPE_DISK {
                return Err(invalid_path());
            }
            status(GetFileInformationByHandle(handle, info.as_mut_ptr()))?;
            info.assume_init()
        };
        if info.attributes
            & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)
            != 0
            || info.links != 1
            || (info.index_high == 0 && info.index_low == 0)
            || info.size_high != 0
            || info.len() > MAX_BYTES
        {
            return Err(invalid_path());
        }
        info.access = FileTime { low: 0, high: 0 };
        Ok(info)
    }

    pub(super) fn len(&self) -> usize {
        self.size_low as usize
    }
}

pub(super) fn path(name: &CStr) -> io::Result<PathBuf> {
    Ok(name.to_str().map_err(|_| invalid_path())?.into())
}

pub(super) fn reader(name: &CStr) -> io::Result<Option<File>> {
    match OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path(name)?)
    {
        Ok(file) => Ok(Some(file)),
        Err(error) if matches!(error.raw_os_error(), Some(2 | 3)) => Ok(None),
        Err(error) => Err(error),
    }
}

pub(super) fn create_temp(
    _: &Target,
    _: &CStr,
    path: &Path,
) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

pub(super) fn discard(_: &Target, _: &CStr, path: &Path) {
    let _ = fs::remove_file(path);
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain([0]).collect()
}

pub(super) fn publish(
    target: &Target,
    temp: &Temp<'_>,
    exists: bool,
) -> io::Result<()> {
    let replacement = wide(&temp.path);
    let destination = wide(&path(target.path())?);
    // SAFETY: both paths are terminated and optional pointers are null.
    status(unsafe {
        if exists {
            ReplaceFileW(
                destination.as_ptr(),
                replacement.as_ptr(),
                null(),
                REPLACEFILE_WRITE_THROUGH,
                null_mut(),
                null_mut(),
            )
        } else {
            MoveFileExW(
                replacement.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_WRITE_THROUGH,
            )
        }
    })
}

pub(super) fn prepare(_: &File, _: Option<&Identity>) -> io::Result<()> {
    Ok(())
}

pub(super) fn close(file: File) -> io::Result<()> {
    // SAFETY: into_raw_handle transfers ownership to CloseHandle.
    status(unsafe { CloseHandle(file.into_raw_handle()) })
}

fn status(result: i32) -> io::Result<()> {
    if result != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
