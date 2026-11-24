use std::cell::Cell;
use std::ffi::{CStr, CString, c_int, c_void};
use std::fs::File;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, IntoRawHandle};
use std::ptr::{null, null_mut};
use std::sync::OnceLock;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND,
    ERROR_LOCK_VIOLATION, ERROR_PATH_NOT_FOUND, ERROR_SHARING_VIOLATION,
    ERROR_SUCCESS, GENERIC_ALL, GENERIC_READ, GENERIC_WRITE, HANDLE,
    HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, LocalFree, SetHandleInformation,
};
use windows_sys::Win32::Security::Authentication::Identity::{
    LSA_OBJECT_ATTRIBUTES, LsaClose, LsaFreeMemory, LsaOpenPolicy,
    LsaQueryInformationPolicy, POLICY_ACCOUNT_DOMAIN_INFO,
    POLICY_VIEW_LOCAL_INFORMATION, PolicyAccountDomainInformation,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidA, EXPLICIT_ACCESS_W, GetSecurityInfo, SE_FILE_OBJECT,
    SET_ACCESS, SetEntriesInAclW, TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION,
    AclSizeInformation, CreateWellKnownSid, DACL_SECURITY_INFORMATION, EqualSid,
    GetAce, GetAclInformation, GetLengthSid, GetSidLengthRequired,
    GetTokenInformation, INHERITED_ACE, InitializeSecurityDescriptor,
    IsValidAcl, IsValidSid, IsWellKnownSid, OWNER_SECURITY_INFORMATION, PSID,
    SE_DACL_PROTECTED, SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR, SID,
    SetSecurityDescriptorControl, SetSecurityDescriptorDacl,
    SetSecurityDescriptorOwner, TOKEN_INFORMATION_CLASS, TOKEN_QUERY,
    TokenOwner, TokenUser, WinAccountAdministratorSid,
    WinBuiltinAdministratorsSid, WinCreatorOwnerRightsSid, WinLocalSystemSid,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CREATE_NEW, CreateFileW, DELETE, DeleteFileW,
    FILE_ADD_FILE, FILE_ADD_SUBDIRECTORY, FILE_APPEND_DATA,
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_DELETE_CHILD, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_SEQUENTIAL_SCAN,
    FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FILE_TYPE_DISK, FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, FILE_WRITE_EA,
    FlushFileBuffers, GetFileInformationByHandle, GetFileType, GetFullPathNameW,
    MOVEFILE_DELAY_UNTIL_REBOOT, MOVEFILE_REPLACE_EXISTING,
    MOVEFILE_WRITE_THROUGH, MoveFileExW, OPEN_EXISTING, READ_CONTROL, WRITE_DAC,
    WRITE_OWNER,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcessToken,
};

use super::{Asset, Entry, copy_file, ffi, refused};

const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
const OPEN_PLAIN: u32 = FILE_FLAG_OPEN_REPARSE_POINT;
const ATTRIBUTES: u32 = FILE_READ_ATTRIBUTES | READ_CONTROL;
const FILE_READ: u32 = GENERIC_READ | ATTRIBUTES;
const SIMPLE_ALLOW: u8 = 0;
const DENY_ACE_TYPES: [u8; 4] = [0x01, 0x06, 0x0a, 0x0c];
const MUTATION_RIGHTS: u32 = GENERIC_ALL
    | GENERIC_WRITE
    | FILE_WRITE_DATA
    | FILE_APPEND_DATA
    | FILE_ADD_FILE
    | FILE_ADD_SUBDIRECTORY
    | FILE_DELETE_CHILD
    | FILE_WRITE_EA
    | FILE_WRITE_ATTRIBUTES
    | DELETE
    | WRITE_DAC
    | WRITE_OWNER;
const PREFIX: [u16; 4] = [92, 92, 63, 92];

thread_local! {
    static RENAME_FAILURES: Cell<u32> = const { Cell::new(0) };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Identity(u32, u32, u32);

pub(super) struct Directory {
    path: CString,
    identity: Identity,
}

#[derive(Clone, Copy)]
enum Owner {
    Current,
    Trusted,
}

impl Directory {
    pub(super) fn open(path: CString, source: bool) -> io::Result<Self> {
        let file = plain_directory(&path)?;
        let identity = identity(&file, false)?;
        let owner = if source {
            Owner::Trusted
        } else {
            Owner::Current
        };
        let tolerated = if source {
            FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY
        } else {
            0
        };
        security(&file, owner, tolerated, Some(&path))?;
        Ok(Self { path, identity })
    }

    pub(super) fn path(&self) -> &CStr {
        &self.path
    }

    pub(super) fn revalidate(&self) -> io::Result<()> {
        if Self::open(self.path.clone(), false)?.identity == self.identity {
            Ok(())
        } else {
            Err(refused())
        }
    }

    pub(super) fn snapshot(
        &self,
        entry: &Entry,
        links: u64,
    ) -> io::Result<Option<Identity>> {
        debug_assert_eq!(links, 1);
        self.revalidate()?;
        snapshot(&entry.path, Owner::Current)
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
        let path = file_path(&entry.path)?;
        let user = Sid::from_token(TokenUser)?;
        let access = EXPLICIT_ACCESS_W {
            grfAccessPermissions: GENERIC_ALL,
            grfAccessMode: SET_ACCESS,
            grfInheritance: 0,
            Trustee: TRUSTEE_W {
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_USER,
                ptstrName: user.as_ptr().cast(),
                ..Default::default()
            },
        };
        let mut acl = null_mut();
        // SAFETY: access owns a reference to user, and acl is an initialized out pointer.
        let result = unsafe { SetEntriesInAclW(1, &access, null(), &mut acl) };
        if result != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(result as i32));
        }
        let _acl = Local(acl.cast());
        let mut descriptor = SECURITY_DESCRIPTOR::default();
        let sd = (&mut descriptor as *mut SECURITY_DESCRIPTOR).cast();
        // SAFETY: descriptor, ACL and user stay live through the CreateFile call.
        unsafe {
            check(InitializeSecurityDescriptor(sd, 1))?;
            check(SetSecurityDescriptorDacl(sd, 1, acl, 0))?;
            check(SetSecurityDescriptorOwner(sd, user.as_ptr(), 0))?;
            check(SetSecurityDescriptorControl(
                sd,
                SE_DACL_PROTECTED,
                SE_DACL_PROTECTED,
            ))?;
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd,
            bInheritHandle: 0,
        };
        let file = open_file(
            &path,
            GENERIC_READ | GENERIC_WRITE | READ_CONTROL | DELETE,
            FILE_SHARE_READ,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL | OPEN_PLAIN,
        )
        .inspect_err(|error| {
            if error.kind() != io::ErrorKind::AlreadyExists {
                ffi::note(
                    "staged-file-create",
                    &os_detail(error),
                    Some(&entry.path),
                );
            }
        })?;
        // SAFETY: the new handle is live and exclusively owned.
        check(unsafe {
            SetHandleInformation(file.as_raw_handle(), HANDLE_FLAG_INHERIT, 0)
        })?;
        let identity = identity(&file, true)?;
        security(&file, Owner::Current, 0, Some(&entry.path))?;
        Ok((file, identity))
    }

    pub(super) fn sync(&self) -> io::Result<()> {
        Ok(())
    }

    pub(super) fn rename(
        &self,
        from: &Entry,
        to: &Entry,
        replace: bool,
    ) -> io::Result<()> {
        self.revalidate()?;
        let from = file_path(&from.path)?;
        let to = file_path(&to.path)?;
        let flags = MOVEFILE_WRITE_THROUGH
            | if replace {
                MOVEFILE_REPLACE_EXISTING
            } else {
                0
            };
        for attempt in 0..10 {
            let failure = RENAME_FAILURES.with(|remaining| {
                let count = remaining.get();
                remaining.set(count.saturating_sub(1));
                count > 0
            });
            let moved = if failure {
                Err(io::Error::from_raw_os_error(ERROR_SHARING_VIOLATION as i32))
            } else {
                // SAFETY: both paths are NUL-terminated and live.
                check(unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), flags) })
            };
            match moved {
                Ok(()) => return self.revalidate(),
                Err(error) if attempt < 9 && transient(&error) => {
                    std::thread::sleep(Duration::from_millis(500));
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!("rename attempts return on the final iteration")
    }

    pub(super) fn rename_absent(
        &self,
        from: &Entry,
        to: &Entry,
    ) -> io::Result<bool> {
        let from = file_path(&from.path)?;
        let to = file_path(&to.path)?;
        // SAFETY: both paths are NUL-terminated and live. No replacement flag is set.
        check(unsafe {
            MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_WRITE_THROUGH)
        })?;
        Ok(true)
    }

    pub(super) fn unlink_expected(
        &self,
        entry: &Entry,
        id: Identity,
        allow_deferred: bool,
    ) -> io::Result<bool> {
        match self.snapshot(entry, 1)? {
            None => return Ok(false),
            Some(current) if current == id => {}
            _ => return Err(refused()),
        }
        let path = file_path(&entry.path)?;
        // SAFETY: path is NUL-terminated and remains live for both calls.
        match check(unsafe { DeleteFileW(path.as_ptr()) }) {
            Ok(()) => Ok(false),
            Err(error) if allow_deferred && transient(&error) => {
                check(unsafe {
                    MoveFileExW(
                        path.as_ptr(),
                        null(),
                        MOVEFILE_DELAY_UNTIL_REBOOT,
                    )
                })?;
                Ok(true)
            }
            Err(error) => Err(error),
        }
    }

    pub(super) fn copy_backup(
        &self,
        entry: &Entry,
        original: Identity,
        backup: &Asset,
    ) -> io::Result<()> {
        self.revalidate()?;
        let mut input = open_existing(
            &entry.path,
            FILE_READ,
            SHARE_ALL,
            FILE_FLAG_SEQUENTIAL_SCAN,
        )?;
        let mut output = open_existing(
            &backup.entry.path,
            FILE_READ | GENERIC_WRITE,
            FILE_SHARE_READ,
            0,
        )?;
        if identity(&input, true)? != original
            || identity(&output, true)? != backup.identity
        {
            return Err(refused());
        }
        security(&input, Owner::Current, 0, None)?;
        security(&output, Owner::Current, 0, None)?;
        output.set_len(0)?;
        let copied = copy_file(&mut input, &mut output, true)
            .and_then(|()| sync_file(&output));
        let input_closed = close(input);
        let output_closed = close(output);
        copied
            .and(input_closed)
            .and(output_closed)
            .and_then(|()| self.revalidate())
    }
}

pub(super) fn source_open(path: &CStr) -> io::Result<File> {
    let (directory_path, _) = Entry::split(path).map_err(|_| refused())?;
    let directory = Directory::open(directory_path.clone(), true)?;
    let expected = snapshot(path, Owner::Trusted)?.ok_or_else(refused)?;
    let file =
        open_existing(path, FILE_READ, SHARE_ALL, FILE_FLAG_SEQUENTIAL_SCAN)?;
    if identity(&file, true)? != expected {
        return Err(refused());
    }
    security(&file, Owner::Trusted, 0, None)?;
    if Directory::open(directory_path, true)?.identity != directory.identity {
        return Err(refused());
    }
    Ok(file)
}

fn snapshot(path: &CStr, owner: Owner) -> io::Result<Option<Identity>> {
    let file = match open_existing(path, ATTRIBUTES, SHARE_ALL, 0) {
        Ok(file) => file,
        Err(error)
            if matches!(
                error.raw_os_error().map(|code| code as u32),
                Some(ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND)
            ) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let identity = identity(&file, true)?;
    security(&file, owner, 0, None)?;
    Ok(Some(identity))
}

fn check(code: i32) -> io::Result<()> {
    if code == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn transient(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error().map(|code| code as u32),
        Some(
            ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION
        )
    )
}

fn os_detail(error: &io::Error) -> String {
    format!("os {}", error.raw_os_error().unwrap_or(0))
}

pub(super) fn rename_failures(count: u32) {
    RENAME_FAILURES.with(|remaining| remaining.set(count));
}

pub(super) fn sync_file(file: &File) -> io::Result<()> {
    // SAFETY: the file owns a live handle.
    check(unsafe { FlushFileBuffers(file.as_raw_handle()) })
}

pub(super) fn close(file: File) -> io::Result<()> {
    // SAFETY: the raw handle is live and its ownership passes to CloseHandle.
    check(unsafe { CloseHandle(file.into_raw_handle()) })
}

unsafe extern "C" {
    fn _errno() -> *mut c_int;
}

pub(super) fn set_errno(error: c_int) {
    // SAFETY: _errno returns the CRT's writable thread-local errno.
    unsafe { *_errno() = error };
}

fn open_existing(
    path: &CStr,
    access: u32,
    sharing: u32,
    flags: u32,
) -> io::Result<File> {
    open_file(
        &file_path(path)?,
        access,
        sharing,
        null(),
        OPEN_EXISTING,
        flags | OPEN_PLAIN,
    )
}

fn open_file(
    path: &[u16],
    access: u32,
    sharing: u32,
    security: *const SECURITY_ATTRIBUTES,
    creation: u32,
    flags: u32,
) -> io::Result<File> {
    // SAFETY: path is NUL-terminated; security is null or points to live attributes.
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            access,
            sharing,
            security,
            creation,
            flags,
            null_mut(),
        )
    };
    owned_handle(handle)
}

fn owned_handle(handle: HANDLE) -> io::Result<File> {
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the successful API call transferred an owned handle.
    Ok(unsafe { File::from_raw_handle(handle) })
}

fn info(file: &File) -> io::Result<BY_HANDLE_FILE_INFORMATION> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    let handle = file.as_raw_handle();
    // SAFETY: handle is live and info is writable.
    unsafe {
        if GetFileType(handle) != FILE_TYPE_DISK {
            return Err(refused());
        }
        check(GetFileInformationByHandle(handle, &mut info))?;
    }
    Ok(info)
}

fn identity(file: &File, regular: bool) -> io::Result<Identity> {
    let info = info(file)?;
    if regular
        && (info.dwFileAttributes
            & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)
            != 0
            || info.nNumberOfLinks != 1)
    {
        return Err(refused());
    }
    Ok(Identity(
        info.dwVolumeSerialNumber,
        info.nFileIndexHigh,
        info.nFileIndexLow,
    ))
}

fn wide(path: &CStr) -> io::Result<Vec<u16>> {
    Ok(path
        .to_str()
        .map_err(|_| refused())?
        .encode_utf16()
        .chain([0])
        .collect())
}

fn absolute(input: &[u16]) -> io::Result<Vec<u16>> {
    // SAFETY: input is NUL-terminated. A null output queries the required length.
    let size =
        unsafe { GetFullPathNameW(input.as_ptr(), 0, null_mut(), null_mut()) };
    if size == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut path = vec![0; size as usize];
    // SAFETY: the output contains size writable UTF-16 units.
    let length = unsafe {
        GetFullPathNameW(input.as_ptr(), size, path.as_mut_ptr(), null_mut())
    };
    if length == 0 || length >= size {
        return Err(refused());
    }
    path.truncate(length as usize);
    Ok(path)
}

fn file_path(path: &CStr) -> io::Result<Vec<u16>> {
    let input = wide(path)?;
    let drive_absolute = input.len() >= 4
        && (input[0] as u8).is_ascii_alphabetic()
        && input[0] <= 127
        && input[1] == 58
        && matches!(input[2], 47 | 92);
    if drive_absolute && input.len() > 240 {
        let absolute = absolute(&input)?;
        Ok(PREFIX.into_iter().chain(absolute).chain([0]).collect())
    } else {
        Ok(input)
    }
}

fn plain_directory(path: &CStr) -> io::Result<File> {
    let input = wide(path)?;
    let mut canonical = absolute(input.strip_prefix(&PREFIX).unwrap_or(&input))?;
    if canonical.len() < 3
        || canonical[1] != 58
        || !matches!(canonical[2], 47 | 92)
    {
        return Err(refused());
    }
    for byte in &mut canonical {
        if *byte == 47 {
            *byte = 92;
        }
    }
    let mut prefix = Vec::from(PREFIX);
    for boundary in 3..=canonical.len() {
        if boundary < canonical.len() && canonical[boundary] != 92 {
            continue;
        }
        if boundary < canonical.len() && canonical[boundary - 1] == 92 {
            return Err(refused());
        }
        prefix.truncate(PREFIX.len());
        prefix.extend_from_slice(&canonical[..boundary]);
        prefix.push(0);
        let file = open_file(
            &prefix,
            ATTRIBUTES,
            SHARE_ALL,
            null(),
            OPEN_EXISTING,
            OPEN_PLAIN | FILE_FLAG_BACKUP_SEMANTICS,
        )
        .inspect_err(|error| {
            ffi::note("ancestor-component-open", &os_detail(error), Some(path))
        })?;
        if !info(&file).is_ok_and(|info| {
            info.dwFileAttributes
                & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)
                == FILE_ATTRIBUTE_DIRECTORY
        }) {
            ffi::note(
                "ancestor-component-not-plain-directory",
                "os 0",
                Some(path),
            );
            return Err(refused());
        }
        if boundary == canonical.len() {
            return Ok(file);
        }
    }
    Err(refused())
}

struct Local(*mut c_void);

impl Drop for Local {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the security API returned this allocation for LocalFree.
            unsafe { LocalFree(self.0) };
        }
    }
}

// Windows SIDs are variable-length and require DWORD alignment.
struct Sid(Vec<u32>);

impl Sid {
    fn as_ptr(&self) -> PSID {
        self.0.as_ptr().cast_mut().cast()
    }

    unsafe fn copy_from(sid: PSID) -> Option<Self> {
        if sid.is_null() || unsafe { IsValidSid(sid) } == 0 {
            return None;
        }
        let size = unsafe { GetLengthSid(sid) } as usize;
        let mut copy = vec![0; size.div_ceil(size_of::<u32>())];
        // SAFETY: the operating system owns a valid SID of this size, and copy has room.
        unsafe {
            std::ptr::copy_nonoverlapping(
                sid.cast::<u8>(),
                copy.as_mut_ptr().cast(),
                size,
            )
        };
        Some(Self(copy))
    }

    fn from_token(kind: TOKEN_INFORMATION_CLASS) -> io::Result<Self> {
        let mut token = null_mut();
        // SAFETY: GetCurrentProcess returns the process pseudo-handle, and token is writable.
        check(unsafe {
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
        })?;
        let token = owned_handle(token)?;
        let mut bytes = 0;
        // SAFETY: the null query returns the necessary size in bytes.
        unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                kind,
                null_mut(),
                0,
                &mut bytes,
            )
        };
        if bytes == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut buffer =
            vec![0usize; (bytes as usize).div_ceil(size_of::<usize>())];
        // SAFETY: buffer is aligned and holds bytes writable bytes. Both token structures
        // start with a SID pointer whose target stays live for the copy.
        unsafe {
            check(GetTokenInformation(
                token.as_raw_handle(),
                kind,
                buffer.as_mut_ptr().cast(),
                bytes,
                &mut bytes,
            ))?;
            Self::copy_from(buffer[0] as PSID).ok_or_else(refused)
        }
    }
}

fn local_admin() -> Option<&'static Sid> {
    static LOCAL_ADMIN: OnceLock<Option<Sid>> = OnceLock::new();
    LOCAL_ADMIN
        .get_or_init(|| {
            let attributes = LSA_OBJECT_ATTRIBUTES::default();
            let mut policy = 0;
            let mut information = null_mut();
            // SAFETY: output storage is writable. On success each LSA allocation is
            // released before returning a separately allocated copy of the SID.
            unsafe {
                if LsaOpenPolicy(
                    null(),
                    &attributes,
                    POLICY_VIEW_LOCAL_INFORMATION as u32,
                    &mut policy,
                ) != 0
                    || policy == 0
                {
                    return None;
                }
                let query = LsaQueryInformationPolicy(
                    policy,
                    PolicyAccountDomainInformation,
                    &mut information,
                );
                let sid = if query == 0 && !information.is_null() {
                    let domain = (*information
                        .cast::<POLICY_ACCOUNT_DOMAIN_INFO>())
                    .DomainSid;
                    let mut bytes = 0;
                    if !domain.is_null() && IsValidSid(domain) != 0 {
                        CreateWellKnownSid(
                            WinAccountAdministratorSid,
                            domain,
                            null_mut(),
                            &mut bytes,
                        );
                    }
                    let mut sid = Sid(vec![
                        0;
                        (bytes as usize)
                            .div_ceil(size_of::<u32>())
                    ]);
                    (bytes > 0
                        && CreateWellKnownSid(
                            WinAccountAdministratorSid,
                            domain,
                            sid.0.as_mut_ptr().cast(),
                            &mut bytes,
                        ) != 0
                        && IsValidSid(sid.as_ptr()) != 0)
                        .then_some(sid)
                } else {
                    None
                };
                if !information.is_null() {
                    LsaFreeMemory(information);
                }
                LsaClose(policy);
                sid
            }
        })
        .as_ref()
}

unsafe fn trust(sid: PSID, user: &Sid, acl: bool) -> bool {
    // SAFETY: the caller verified the bounds and validity of the SID.
    unsafe {
        EqualSid(sid, user.as_ptr()) != 0
            || IsWellKnownSid(sid, WinLocalSystemSid) != 0
            || IsWellKnownSid(sid, WinBuiltinAdministratorsSid) != 0
            || local_admin()
                .is_some_and(|admin| EqualSid(sid, admin.as_ptr()) != 0)
            || (acl && IsWellKnownSid(sid, WinCreatorOwnerRightsSid) != 0)
    }
}

unsafe fn sid_text(sid: PSID, fallback: &str) -> String {
    let mut string = null_mut();
    if sid.is_null()
        || unsafe { ConvertSidToStringSidA(sid, &mut string) } == 0
        || string.is_null()
    {
        return fallback.to_owned();
    }
    let _allocation = Local(string.cast());
    // SAFETY: the conversion returned a NUL-terminated string.
    unsafe { CStr::from_ptr(string.cast()) }
        .to_string_lossy()
        .into_owned()
}

fn security(
    file: &File,
    owner_rule: Owner,
    tolerated: u32,
    path: Option<&CStr>,
) -> io::Result<()> {
    let user = Sid::from_token(TokenUser)?;
    let mut owner = null_mut();
    let mut descriptor = null_mut();
    let handle = file.as_raw_handle();
    // SAFETY: the handle is live and output pointers are writable.
    let result = unsafe {
        GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            null_mut(),
            null_mut(),
            &mut descriptor,
        )
    };
    let _owner_descriptor = Local(descriptor);
    // SAFETY: owner points into the descriptor returned above, if any.
    let accepted = result == ERROR_SUCCESS
        && !owner.is_null()
        && unsafe {
            IsValidSid(owner) != 0
                && match owner_rule {
                    Owner::Trusted => trust(owner, &user, false),
                    Owner::Current => {
                        EqualSid(owner, user.as_ptr()) != 0
                            || Sid::from_token(TokenOwner).is_ok_and(|token| {
                                EqualSid(owner, token.as_ptr()) != 0
                            })
                    }
                }
        };
    if !accepted {
        let predicate = match owner_rule {
            Owner::Trusted => "owner-not-trusted",
            Owner::Current => "owner-not-current-user",
        };
        let text = if !owner.is_null() && unsafe { IsValidSid(owner) } != 0 {
            // SAFETY: the descriptor holds a valid SID.
            unsafe { sid_text(owner, "unresolved-sid") }
        } else {
            "unresolved-sid".to_owned()
        };
        ffi::note(
            &format!("{predicate}; owner={text}"),
            &format!("os {result}"),
            path,
        );
        return Err(refused());
    }
    check_acl(handle, &user, tolerated, path)
}

fn check_acl(
    handle: HANDLE,
    user: &Sid,
    tolerated: u32,
    path: Option<&CStr>,
) -> io::Result<()> {
    let mut acl: *mut ACL = null_mut();
    let mut descriptor = null_mut();
    // SAFETY: handle is live and output pointers are writable.
    let result = unsafe {
        GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &mut acl,
            null_mut(),
            &mut descriptor,
        )
    };
    let _acl_descriptor = Local(descriptor);
    let mut info = ACL_SIZE_INFORMATION::default();
    // SAFETY: the ACL remains live in its descriptor and info is writable.
    let valid = result == ERROR_SUCCESS
        && !descriptor.is_null()
        && !acl.is_null()
        && unsafe {
            IsValidAcl(acl) != 0
                && GetAclInformation(
                    acl,
                    (&mut info as *mut ACL_SIZE_INFORMATION).cast(),
                    size_of::<ACL_SIZE_INFORMATION>() as u32,
                    AclSizeInformation,
                ) != 0
        };
    if valid {
        for index in 0..info.AceCount {
            let mut pointer = null_mut();
            // SAFETY: the validated ACL owns every ACE in this range.
            if unsafe { GetAce(acl, index, &mut pointer) } == 0
                || pointer.is_null()
            {
                ffi::note(
                    "acl-unreadable-or-malformed",
                    &format!("os {result}"),
                    path,
                );
                return Err(refused());
            }
            // SAFETY: GetAce returned a live ACE within the validated ACL.
            if !unsafe { allowed_ace(pointer, user, tolerated, path) } {
                ffi::note(
                    "acl-unreadable-or-malformed",
                    &format!("os {result}"),
                    path,
                );
                return Err(refused());
            }
        }
        return Ok(());
    }
    ffi::note("acl-unreadable-or-malformed", &format!("os {result}"), path);
    Err(refused())
}

unsafe fn allowed_ace(
    pointer: *mut c_void,
    user: &Sid,
    tolerated: u32,
    path: Option<&CStr>,
) -> bool {
    let header = unsafe { &*pointer.cast::<ACE_HEADER>() };
    if DENY_ACE_TYPES.contains(&header.AceType) {
        return true;
    }
    let sid_offset = std::mem::offset_of!(ACCESS_ALLOWED_ACE, SidStart);
    let sid_header = std::mem::offset_of!(SID, SubAuthority);
    let size = usize::from(header.AceSize);
    if header.AceType != SIMPLE_ALLOW || size < sid_offset + sid_header {
        return false;
    }
    // SAFETY: the fixed allow ACE and SID header both fit in the validated ACE.
    let ace = unsafe { &*pointer.cast::<ACCESS_ALLOWED_ACE>() };
    if ace.Mask & MUTATION_RIGHTS & !tolerated == 0 {
        return true;
    }
    let sid = unsafe { pointer.cast::<u8>().add(sid_offset).cast::<SID>() };
    let length = unsafe { GetSidLengthRequired((*sid).SubAuthorityCount) };
    let sid = sid.cast();
    let valid = length as usize <= size - sid_offset
        && unsafe { IsValidSid(sid) != 0 && GetLengthSid(sid) == length };
    if !valid || !unsafe { trust(sid, user, true) } {
        let text = if valid {
            unsafe { sid_text(sid, "unparsable-sid") }
        } else {
            "unparsable-sid".to_owned()
        };
        let inherited = if u32::from(header.AceFlags) & INHERITED_ACE != 0 {
            " (inherited from a parent directory)"
        } else {
            ""
        };
        ffi::note(
            &format!("acl-grants-cross-account-mutation to {text}{inherited}"),
            "os 0",
            path,
        );
        return false;
    }
    true
}
