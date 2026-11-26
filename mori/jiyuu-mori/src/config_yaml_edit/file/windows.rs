use super::*;
use std::{
    mem::MaybeUninit,
    os::windows::io::{AsRawHandle, FromRawHandle, IntoRawHandle},
    ptr,
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, GENERIC_READ,
        GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    },
    Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CREATE_NEW, CreateDirectoryW, CreateFileW,
        DELETE, DeleteFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_INFO,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FILE_TYPE_DISK, FileDispositionInfo,
        GetFileInformationByHandle, GetFileType, MOVEFILE_WRITE_THROUGH,
        MoveFileExW, OPEN_EXISTING, REPLACEFILE_WRITE_THROUGH, RemoveDirectoryW,
        ReplaceFileW, SetFileInformationByHandle,
    },
};

fn wide(path: &CStr) -> Result<Vec<u16>> {
    let text = path.to_str().map_err(|_| ())?;
    let mut wide = Vec::new();
    wide.try_reserve_exact(text.len() + 1).map_err(|_| ())?;
    wide.extend(text.encode_utf16());
    wide.push(0);
    Ok(wide)
}

fn open(
    path: &[u16],
    access: u32,
    share: u32,
    disposition: u32,
    flags: u32,
) -> io::Result<File> {
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            access,
            share,
            ptr::null(),
            disposition,
            flags,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_handle(handle) })
    }
}

fn info(handle: HANDLE) -> Result<BY_HANDLE_FILE_INFORMATION> {
    let mut info = MaybeUninit::uninit();
    require(
        unsafe { GetFileInformationByHandle(handle, info.as_mut_ptr()) } != 0,
    )?;
    Ok(unsafe { info.assume_init() })
}

fn identity(info: &BY_HANDLE_FILE_INFORMATION) -> (u32, u32, u32) {
    (
        info.dwVolumeSerialNumber,
        info.nFileIndexHigh,
        info.nFileIndexLow,
    )
}

#[derive(PartialEq, Eq)]
pub(super) struct Snapshot {
    identity: (u32, u32, u32),
    attributes: u32,
    links: u32,
    created: (u32, u32),
    written: (u32, u32),
    size: u64,
}

impl Snapshot {
    pub(super) fn capture(file: &File) -> Result<Self> {
        let handle = file.as_raw_handle();
        require(unsafe { GetFileType(handle) } == FILE_TYPE_DISK)?;
        let info = info(handle)?;
        require(
            info.dwFileAttributes
                & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)
                == 0
                && info.nNumberOfLinks == 1,
        )?;
        Ok(Self {
            identity: identity(&info),
            attributes: info.dwFileAttributes,
            links: info.nNumberOfLinks,
            created: (
                info.ftCreationTime.dwHighDateTime,
                info.ftCreationTime.dwLowDateTime,
            ),
            written: (
                info.ftLastWriteTime.dwHighDateTime,
                info.ftLastWriteTime.dwLowDateTime,
            ),
            size: (u64::from(info.nFileSizeHigh) << 32)
                | u64::from(info.nFileSizeLow),
        })
    }

    pub(super) fn size(&self) -> u64 {
        self.size
    }
}

pub(super) struct Target(CString);

impl Target {
    pub(super) fn open(path: &CStr) -> Result<Self> {
        // config_edit_path uses a fixed buffer for the requested path.
        require(!path.to_bytes().is_empty() && path.to_bytes().len() < 4096)?;
        Ok(Self(path.to_owned()))
    }

    pub(super) fn path(&self) -> &CStr {
        &self.0
    }

    pub(super) fn create(&self, temp: &CStr) -> io::Result<File> {
        let wide = wide(temp).map_err(|_| io::ErrorKind::InvalidInput)?;
        open(
            &wide,
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
        )
    }

    pub(super) fn discard(&self, temp: &CStr) {
        if let Ok(path) = wide(temp) {
            unsafe { DeleteFileW(path.as_ptr()) };
        }
    }

    pub(super) fn replace(&self, temp: &CStr, exists: bool) -> Result<()> {
        let path = wide(self.path())?;
        let temp = wide(temp)?;
        let replaced = unsafe {
            if exists {
                ReplaceFileW(
                    path.as_ptr(),
                    temp.as_ptr(),
                    ptr::null(),
                    REPLACEFILE_WRITE_THROUGH,
                    ptr::null(),
                    ptr::null(),
                )
            } else {
                MoveFileExW(temp.as_ptr(), path.as_ptr(), MOVEFILE_WRITE_THROUGH)
            }
        };
        require(replaced != 0)
    }
}

fn open_existing(path: &CStr) -> Result<Option<File>> {
    let wide = wide(path)?;
    match open(
        &wide,
        GENERIC_READ,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
    ) {
        Ok(file) => Ok(Some(file)),
        Err(err)
            if err.raw_os_error().is_some_and(|errno| {
                matches!(
                    errno as u32,
                    ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND
                )
            }) =>
        {
            Ok(None)
        }
        Err(_) => Err(()),
    }
}

pub(crate) fn absent(path: &CStr) -> bool {
    matches!(open_existing(path), Ok(None))
}

pub(super) fn open_read(path: &CStr) -> Result<Option<File>> {
    open_existing(path)
}

pub(super) fn set_permissions(
    _file: &File,
    _snapshot: Option<&Snapshot>,
) -> Result<()> {
    Ok(())
}

pub(super) fn close(file: File) -> Result<()> {
    require(unsafe { CloseHandle(file.into_raw_handle()) } != 0)
}

fn directory_identity(file: &File) -> Result<(u32, u32, u32)> {
    let info = info(file.as_raw_handle())?;
    require(
        info.dwFileAttributes
            & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)
            == FILE_ATTRIBUTE_DIRECTORY,
    )?;
    Ok(identity(&info))
}

fn delete_on_close(file: &File) -> Result<()> {
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    require(
        unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle(),
                FileDispositionInfo,
                (&raw const disposition).cast(),
                size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        } != 0,
    )
}

pub(crate) struct Lock {
    file: File,
    identity: (u32, u32, u32),
}

impl Lock {
    pub(crate) fn acquire(path: &CStr) -> Result<Self> {
        let path = suffix_path(path, b".cbm-yaml.lock")?;
        let wide = wide(&path)?;
        require(unsafe { CreateDirectoryW(wide.as_ptr(), ptr::null()) } != 0)?;
        let file = match open(
            &wide,
            DELETE | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
        ) {
            Ok(file) => file,
            Err(_) => {
                unsafe { RemoveDirectoryW(wide.as_ptr()) };
                return Err(());
            }
        };
        let verified = (|| {
            let identity = directory_identity(&file)?;
            hooks::LOCK_POSTCREATE.run(&path);
            require(directory_identity(&file)? == identity)?;
            Ok(identity)
        })();
        match verified {
            Ok(identity) => Ok(Self { file, identity }),
            Err(()) => {
                let _ = delete_on_close(&file);
                Err(())
            }
        }
    }

    pub(crate) fn release(self) -> Result<()> {
        let removed = directory_identity(&self.file).and_then(|identity| {
            require(identity == self.identity)?;
            delete_on_close(&self.file)
        });
        let closed = close(self.file);
        removed.and(closed)
    }
}
