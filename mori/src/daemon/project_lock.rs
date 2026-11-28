//! Project mutation leases shared by the daemon, local CLI, and index worker.

use std::{
    alloc::{Layout, alloc},
    ffi::{CStr, c_char, c_int, c_void},
    ptr,
};

// Match the mode and status values in foundation/private_file_lock.h.
const SHARED: c_int = 1;
const EXCLUSIVE: c_int = 2;
const OK: c_int = 0;
const UNSAFE: c_int = 2;
const IO: c_int = 3;

const PROJECT_SET_KEY: &CStr = c"cbm-project-set-v1";
const PROJECT_KEY_PREFIX: &[u8] = b"cbm-project-v1:";
const PROJECT_KEY_CAP: usize = 4096;

#[repr(C)]
struct Endpoint {
    _opaque: [u8; 0],
}

#[repr(C)]
struct Directory {
    _opaque: [u8; 0],
}

#[repr(C)]
struct Registry {
    _opaque: [u8; 0],
}

#[repr(C)]
struct NativeLease {
    _opaque: [u8; 0],
}

// C's atomic cancellation token stays opaque and is only accessed by the registry.
type CancelToken = c_void;

unsafe extern "C" {
    fn cbm_daemon_ipc_private_lock_directory_new(
        endpoint: *const Endpoint,
        out: *mut *mut Directory,
    ) -> c_int;
    fn cbm_private_lock_directory_close(directory: *mut Directory);
    fn cbm_lock_registry_new(directory: *mut Directory) -> *mut Registry;
    fn cbm_lock_registry_free(registry: *mut *mut Registry) -> c_int;
    fn cbm_lock_registry_request_cancel(
        registry: *mut Registry,
        token: *mut CancelToken,
    ) -> c_int;
    fn cbm_lock_registry_acquire(
        registry: *mut Registry,
        key: *const c_char,
        mode: c_int,
        deadline_ms: u64,
        token: *const CancelToken,
        out: *mut *mut NativeLease,
    ) -> c_int;
    fn cbm_lock_registry_try_acquire(
        registry: *mut Registry,
        key: *const c_char,
        mode: c_int,
        out: *mut *mut NativeLease,
    ) -> c_int;
    fn cbm_lock_lease_release(lease: *mut *mut NativeLease) -> c_int;
}

struct Manager {
    directory: *mut Directory,
    registry: *mut Registry,
}

struct Lease {
    project: *mut NativeLease,
    project_set: *mut NativeLease,
}

// These two handle types are nonempty. Preserve recoverable allocation failure
// at the C boundary. The caller can dispose a successful allocation with Box.
fn allocate_handle<T>(value: T) -> *mut T {
    let handle = unsafe { alloc(Layout::new::<T>()).cast::<T>() };
    if !handle.is_null() {
        unsafe { handle.write(value) };
    }
    handle
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_project_lock_manager_new(
    endpoint: *const Endpoint,
) -> *mut Manager {
    let mut directory = ptr::null_mut();
    // SAFETY: The caller lends the endpoint. The output is owned on return.
    let status = unsafe {
        cbm_daemon_ipc_private_lock_directory_new(endpoint, &mut directory)
    };
    if directory.is_null() {
        return ptr::null_mut();
    }
    if status == OK {
        let manager = allocate_handle(Manager {
            directory,
            registry: ptr::null_mut(),
        });
        if !manager.is_null() {
            let registry = unsafe { cbm_lock_registry_new(directory) };
            if !registry.is_null() {
                unsafe { (*manager).registry = registry };
                return manager;
            }
            unsafe { drop(Box::from_raw(manager)) };
        }
    }
    unsafe { cbm_private_lock_directory_close(directory) };
    ptr::null_mut()
}

impl Lease {
    // Keep the set lease while a project lease still needs cleanup. A native
    // release can report IO and consume the handle, so check both results.
    fn release(&mut self) -> c_int {
        let mut status = OK;
        for lease in [&mut self.project, &mut self.project_set] {
            if !lease.is_null() {
                let released = unsafe { cbm_lock_lease_release(lease) };
                if !lease.is_null() {
                    return IO;
                }
                if released != OK {
                    status = released;
                }
            }
        }
        status
    }

    fn is_released(&self) -> bool {
        self.project.is_null() && self.project_set.is_null()
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_project_lock_lease_release(
    lease_io: *mut *mut Lease,
) -> c_int {
    // SAFETY: A non-null output slot is writable and owns any lease it contains.
    let Some(slot) = (unsafe { lease_io.as_mut() }) else {
        return IO;
    };
    let Some(lease) = (unsafe { slot.as_mut() }) else {
        return IO;
    };
    let status = lease.release();
    if lease.is_released() {
        unsafe { drop(Box::from_raw(*slot)) };
        *slot = ptr::null_mut();
    }
    status
}

#[derive(Clone, Copy)]
enum Acquire {
    Try,
    Wait {
        deadline_ms: u64,
        token: *const CancelToken,
    },
}

impl Acquire {
    unsafe fn lock(
        self,
        registry: *mut Registry,
        key: &CStr,
        mode: c_int,
        out: &mut *mut NativeLease,
    ) -> c_int {
        // SAFETY: The caller lends the registry and any token for the call.
        unsafe {
            match self {
                Self::Try => cbm_lock_registry_try_acquire(
                    registry,
                    key.as_ptr(),
                    mode,
                    out,
                ),
                Self::Wait { deadline_ms, token } => cbm_lock_registry_acquire(
                    registry,
                    key.as_ptr(),
                    mode,
                    deadline_ms,
                    token,
                    out,
                ),
            }
        }
    }
}

unsafe fn acquire(
    manager: *mut Manager,
    project: *const c_char,
    request: Acquire,
    lease_out: *mut *mut Lease,
) -> c_int {
    // SAFETY: The caller supplies an exclusive output slot and lends the
    // manager, project name, and optional token for the call.
    let Some(out) = (unsafe { lease_out.as_mut() }) else {
        return UNSAFE;
    };
    *out = ptr::null_mut();
    if manager.is_null() || project.is_null() {
        return UNSAFE;
    }
    let registry = unsafe { (*manager).registry };
    if registry.is_null() {
        return UNSAFE;
    }
    let mut key = [0; PROJECT_KEY_CAP];
    let prefix_len = PROJECT_KEY_PREFIX.len();
    key[..prefix_len].copy_from_slice(PROJECT_KEY_PREFIX);
    // Read only until the terminator or until the key can no longer fit.
    for (index, slot) in key[prefix_len..].iter_mut().enumerate() {
        let byte = unsafe { project.cast::<u8>().add(index).read() };
        if byte == 0 {
            break;
        }
        *slot = byte.to_ascii_lowercase();
    }
    let Ok(key) = CStr::from_bytes_until_nul(&key) else {
        return UNSAFE;
    };
    let name = &key.to_bytes()[prefix_len..];
    if name.is_empty() {
        return UNSAFE;
    }
    let wildcard = name == b"*";

    let handle = allocate_handle(Lease {
        project: ptr::null_mut(),
        project_set: ptr::null_mut(),
    });
    if handle.is_null() {
        return IO;
    }
    let lease = unsafe { &mut *handle };
    let set_mode = if wildcard { EXCLUSIVE } else { SHARED };
    let mut status = unsafe {
        request.lock(registry, PROJECT_SET_KEY, set_mode, &mut lease.project_set)
    };
    if status == OK && !wildcard {
        status = unsafe {
            request.lock(registry, key, EXCLUSIVE, &mut lease.project)
        };
    }
    if status == OK {
        *out = handle;
        return OK;
    }

    let cleanup = lease.release();
    if lease.is_released() {
        unsafe { drop(Box::from_raw(handle)) };
    } else {
        *out = handle;
        return IO;
    }
    if cleanup == OK { status } else { IO }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_project_lock_acquire(
    manager: *mut Manager,
    project: *const c_char,
    deadline_ms: u64,
    token: *const CancelToken,
    lease_out: *mut *mut Lease,
) -> c_int {
    unsafe {
        acquire(
            manager,
            project,
            Acquire::Wait { deadline_ms, token },
            lease_out,
        )
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_project_lock_try_acquire(
    manager: *mut Manager,
    project: *const c_char,
    lease_out: *mut *mut Lease,
) -> c_int {
    unsafe { acquire(manager, project, Acquire::Try, lease_out) }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_project_lock_request_cancel(
    manager: *mut Manager,
    token: *mut CancelToken,
) -> c_int {
    if manager.is_null() {
        return IO;
    }
    unsafe { cbm_lock_registry_request_cancel((*manager).registry, token) }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_project_lock_manager_free(
    manager_io: *mut *mut Manager,
) -> c_int {
    // SAFETY: The caller owns the slot and must coordinate teardown with users
    // of the manager. The registry refuses teardown with live work.
    let Some(slot) = (unsafe { manager_io.as_mut() }) else {
        return IO;
    };
    let manager = *slot;
    if manager.is_null() {
        return IO;
    }
    let mut registry = unsafe { (*manager).registry };
    let status = unsafe { cbm_lock_registry_free(&mut registry) };
    if status == OK {
        unsafe {
            cbm_private_lock_directory_close((*manager).directory);
            drop(Box::from_raw(manager));
        }
        *slot = ptr::null_mut();
    }
    status
}
