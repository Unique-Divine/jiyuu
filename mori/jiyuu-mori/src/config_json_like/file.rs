use std::ffi::{CStr, CString, c_char, c_int, c_uint};
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::ffi_output;

use super::{Invalid, MAX_BYTES, Result, parser::validate_document};

#[cfg(unix)]
#[path = "unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "windows.rs"]
mod platform;

pub(super) fn invalid_path() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "unsafe configuration path or file",
    )
}

// Layout from cli/config_edit_path.h. The C resolver keeps the opt-in roots
// shared across every editor and owns the followed target's descriptors.
#[repr(C)]
pub(super) struct ConfigTarget {
    status: c_int,
    dirfd: c_int,
    fd: c_int,
    base: [c_char; 256],
    path: [c_char; 4096],
}

unsafe extern "C" {
    fn cbm_config_edit_target_open(
        path: *const c_char,
        target: *mut ConfigTarget,
    ) -> c_int;
    fn cbm_config_edit_target_close(target: *mut ConfigTarget);

    #[cfg(unix)]
    fn cbm_config_edit_target_create_temp(
        target: *const ConfigTarget,
        name: *const c_char,
        mode: c_uint,
    ) -> c_int;
    #[cfg(unix)]
    fn cbm_config_edit_target_commit(
        target: *const ConfigTarget,
        name: *const c_char,
    ) -> c_int;
    #[cfg(unix)]
    fn cbm_config_edit_target_unlink(
        target: *const ConfigTarget,
        name: *const c_char,
    ) -> c_int;

    fn cbm_mkdir_p_ex(path: *const c_char, mode: c_int, policy: c_uint) -> bool;
}

struct Target(ConfigTarget);

impl Target {
    fn open(path: &CStr) -> io::Result<Self> {
        let mut raw = ConfigTarget {
            status: 0,
            dirfd: -1,
            fd: -1,
            base: [0; 256],
            path: [0; 4096],
        };
        // SAFETY: path is terminated and raw has the C struct layout.
        if unsafe {
            cbm_config_edit_target_open(ffi_output::c_str(path), &mut raw)
        } < 0
        {
            return Err(invalid_path());
        }
        Ok(Self(raw))
    }

    fn path(&self) -> &CStr {
        // SAFETY: the successful resolver terminated the path.
        unsafe { CStr::from_ptr(self.0.path.as_ptr()) }
    }

    #[cfg(unix)]
    fn native(&self) -> *const ConfigTarget {
        // The zero-initialized arrays and scalar fields have no padding.
        unsafe { ffi_output::written(&self.0, 1) };
        &self.0
    }

    fn followed(&self) -> bool {
        self.0.status == 1
    }

    #[cfg(unix)]
    fn take_fd(&mut self) -> c_int {
        std::mem::replace(&mut self.0.fd, -1)
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        // SAFETY: the successful resolver initialized this target.
        unsafe {
            ffi_output::written(&self.0, 1);
            cbm_config_edit_target_close(&mut self.0)
        };
    }
}

#[derive(PartialEq, Eq)]
pub(super) struct Snapshot {
    identity: Option<platform::Identity>,
    bytes: Vec<u8>,
}

impl Snapshot {
    pub(super) fn read(path: &CStr) -> io::Result<Self> {
        let Some(mut file) = platform::reader(path)? else {
            return Ok(Self {
                identity: None,
                bytes: Vec::new(),
            });
        };
        let before = platform::Identity::read(&file)?;
        let mut bytes = vec![0; before.len()];
        file.read_exact(&mut bytes)?;
        if platform::Identity::read(&file)? != before {
            return Err(invalid_path());
        }
        platform::close(file)?;
        Ok(Self {
            identity: Some(before),
            bytes,
        })
    }

    pub(super) fn content(&self) -> Option<&[u8]> {
        self.identity.as_ref().map(|_| self.bytes.as_slice())
    }
}

fn parent(path: &CStr) -> CString {
    let bytes = path.to_bytes();
    let end = bytes
        .iter()
        .rposition(|&byte| byte == b'/' || (cfg!(windows) && byte == b'\\'));
    let bytes = match end {
        None => b".",
        Some(0) => b"/",
        Some(i) => &bytes[..i],
    };
    CString::new(bytes).expect("a C string prefix contains no NUL")
}

pub(super) fn write(path: &CStr, old: &Snapshot, updated: &[u8]) -> Result<()> {
    validate_document(updated)?;
    publish(path, old, updated).map_err(|_| Invalid)
}

fn publish(path: &CStr, old: &Snapshot, updated: &[u8]) -> io::Result<()> {
    let target = Target::open(path)?;
    let resolved = target.path();
    if !target.followed() {
        let parent = parent(resolved);
        // SAFETY: parent is NUL-terminated. Bit 0 is CBM_MKDIR_FOLLOW_OWNED.
        if parent.to_bytes() != b"."
            && !unsafe { cbm_mkdir_p_ex(ffi_output::c_str(&parent), 0o755, 1) }
        {
            return Err(invalid_path());
        }
    }

    let (mut temp, mut file) = Temp::create(&target)?;
    file.write_all(updated)?;
    platform::prepare(&file, old.identity.as_ref())?;
    file.sync_all()?;
    platform::close(file)?;
    let staged = Snapshot::read(&temp.name)?;
    if staged.content() != Some(updated) {
        return Err(invalid_path());
    }

    #[cfg(feature = "native-engine-test-seams")]
    super::abi::run_precommit(resolved);
    if Snapshot::read(resolved)? != *old {
        return Err(invalid_path());
    }
    #[cfg(feature = "native-engine-test-seams")]
    super::abi::run_prepublish(resolved);
    if Snapshot::read(resolved)? != *old || Snapshot::read(&temp.name)? != staged
    {
        return Err(invalid_path());
    }

    platform::publish(&target, &temp, old.identity.is_some())?;
    temp.published = true;
    Ok(())
}

struct Temp<'a> {
    target: &'a Target,
    name: CString,
    path: PathBuf,
    published: bool,
}

impl<'a> Temp<'a> {
    fn create(target: &'a Target) -> io::Result<(Self, File)> {
        static SEQUENCE: AtomicU32 = AtomicU32::new(0);
        for _ in 0..64 {
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let mut bytes = target.path().to_bytes().to_vec();
            bytes.extend_from_slice(
                format!(".cbm.tmp.{}.{sequence}", std::process::id()).as_bytes(),
            );
            let name = CString::new(bytes)
                .expect("a C path plus suffix contains no NUL");
            let path = platform::path(&name)?;
            match platform::create_temp(target, &name, &path) {
                Ok(file) => {
                    return Ok((
                        Self {
                            target,
                            name,
                            path,
                            published: false,
                        },
                        file,
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Err(io::ErrorKind::AlreadyExists.into())
    }
}

impl Drop for Temp<'_> {
    fn drop(&mut self) {
        if !self.published {
            platform::discard(self.target, &self.name, &self.path);
        }
    }
}
