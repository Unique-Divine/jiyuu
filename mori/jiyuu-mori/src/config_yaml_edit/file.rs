use super::{INPUT_MAX, Result, require, validate_bytes};
use std::{
    ffi::{CStr, CString, c_char},
    fs::File,
    io::{self, Read, Write},
    sync::atomic::{AtomicU32, Ordering},
};

#[cfg(unix)]
#[path = "file/unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "file/windows.rs"]
mod platform;

pub(super) use platform::{Lock, absent};
use platform::{Snapshot, Target, close};

pub(super) struct Version {
    pub data: Vec<u8>,
    snapshot: Option<Snapshot>,
}

pub(super) fn read(path: &CStr) -> Result<Version> {
    let Some(mut file) = platform::open_read(path)? else {
        return Ok(Version {
            data: Vec::new(),
            snapshot: None,
        });
    };
    let snapshot = Snapshot::capture(&file)?;
    let size = snapshot.size();
    require(size <= INPUT_MAX as u64)?;
    let size = size as usize;
    let mut data = Vec::new();
    data.try_reserve_exact(size).map_err(|_| ())?;
    data.resize(size, 0);
    file.read_exact(&mut data).map_err(|_| ())?;
    require(Snapshot::capture(&file)? == snapshot)?;
    close(file)?;
    validate_bytes(&data, true)?;
    Ok(Version {
        data,
        snapshot: Some(snapshot),
    })
}

fn verify(
    path: &CStr,
    expected_data: &[u8],
    expected: &Option<Snapshot>,
) -> Result<()> {
    let current = read(path)?;
    require(current.snapshot == *expected && current.data == expected_data)
}

static TEMP_SEQUENCE: AtomicU32 = AtomicU32::new(0);

struct Staged<'a> {
    target: &'a Target,
    path: CString,
    published: bool,
}

impl Drop for Staged<'_> {
    fn drop(&mut self) {
        if !self.published {
            self.target.discard(&self.path);
        }
    }
}

fn stage(target: &Target) -> Result<(Staged<'_>, File)> {
    for _ in 0..100 {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let suffix = format!(".cbm-yaml-{}-{sequence}.tmp", std::process::id());
        let path = suffix_path(target.path(), suffix.as_bytes())?;
        match target.create(&path) {
            Ok(file) => {
                return Ok((
                    Staged {
                        target,
                        path,
                        published: false,
                    },
                    file,
                ));
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err(()),
        }
    }
    Err(())
}

pub(super) fn publish(
    path: &CStr,
    original: &Version,
    data: &[u8],
) -> Result<()> {
    let target = Target::open(path)?;
    let (mut staged, mut file) = stage(&target)?;
    file.write_all(data).map_err(|_| ())?;
    platform::set_permissions(&file, original.snapshot.as_ref())?;
    file.sync_all().map_err(|_| ())?;
    close(file)?;

    let temp = read(&staged.path)?;
    require(temp.snapshot.is_some() && temp.data == data)?;
    hooks::PRECOMMIT.run(target.path());
    verify(target.path(), &original.data, &original.snapshot)?;
    hooks::PREPUBLISH.run(target.path());
    verify(target.path(), &original.data, &original.snapshot)?;
    verify(&staged.path, data, &temp.snapshot)?;
    target.replace(&staged.path, original.snapshot.is_some())?;
    staged.published = true;
    Ok(())
}

fn suffix_path(path: &CStr, suffix: &[u8]) -> Result<CString> {
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(path.to_bytes().len() + suffix.len() + 1)
        .map_err(|_| ())?;
    bytes.extend_from_slice(path.to_bytes());
    bytes.extend_from_slice(suffix);
    CString::new(bytes).map_err(|_| ())
}

// The C fixture runner enables this feature on the same Rust archive it links.
// Release binaries have no mutable hooks or setter symbols.
mod hooks {
    use super::*;
    #[cfg(feature = "native-engine-test-hooks")]
    use std::{ffi::c_void, sync::Mutex};

    #[cfg(feature = "native-engine-test-hooks")]
    type Callback = unsafe extern "C" fn(*const c_char, *mut c_void);

    #[cfg(feature = "native-engine-test-hooks")]
    pub(super) struct Hook(Mutex<Option<(Callback, usize)>>);

    #[cfg(not(feature = "native-engine-test-hooks"))]
    pub(super) struct Hook;

    impl Hook {
        const fn new() -> Self {
            #[cfg(feature = "native-engine-test-hooks")]
            return Self(Mutex::new(None));
            #[cfg(not(feature = "native-engine-test-hooks"))]
            Self
        }

        pub(super) fn run(&self, _path: &CStr) {
            #[cfg(feature = "native-engine-test-hooks")]
            {
                let hook = *self.0.lock().unwrap_or_else(|err| err.into_inner());
                if let Some((callback, context)) = hook {
                    // Call outside the mutex. A callback may start another edit.
                    unsafe { callback(_path.as_ptr(), context as *mut c_void) };
                }
            }
        }

        #[cfg(feature = "native-engine-test-hooks")]
        fn set(&self, callback: Option<Callback>, context: *mut c_void) {
            *self.0.lock().unwrap_or_else(|err| err.into_inner()) =
                callback.map(|callback| (callback, context as usize));
        }
    }

    pub(super) static PRECOMMIT: Hook = Hook::new();
    pub(super) static PREPUBLISH: Hook = Hook::new();
    pub(super) static LOCK_POSTCREATE: Hook = Hook::new();

    #[cfg(feature = "native-engine-test-hooks")]
    #[unsafe(no_mangle)]
    extern "C" fn cbm_yaml_set_precommit_hook_for_testing(
        hook: Option<Callback>,
        context: *mut c_void,
    ) {
        PRECOMMIT.set(hook, context);
    }

    #[cfg(feature = "native-engine-test-hooks")]
    #[unsafe(no_mangle)]
    extern "C" fn cbm_yaml_set_prepublish_hook_for_testing(
        hook: Option<Callback>,
        context: *mut c_void,
    ) {
        PREPUBLISH.set(hook, context);
    }

    #[cfg(feature = "native-engine-test-hooks")]
    #[unsafe(no_mangle)]
    extern "C" fn cbm_yaml_set_lock_postcreate_hook_for_testing(
        hook: Option<Callback>,
        context: *mut c_void,
    ) {
        LOCK_POSTCREATE.set(hook, context);
    }
}
