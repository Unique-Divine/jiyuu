//! Reconcile a complete named watch snapshot under the daemon application mutex.

use std::ffi::{OsStr, c_void};

#[repr(C)]
struct NamedWatchRun {
    _opaque: [u8; 0],
}

#[repr(C)]
struct NamedWatchStage {
    present: bool,
    deferred: bool,
    added: *mut c_void,
}

#[repr(C)]
struct NamedWatchOps {
    check: unsafe extern "C" fn(
        *mut NamedWatchRun,
        usize,
        *mut bool,
        *mut bool,
    ) -> bool,
    add: unsafe extern "C" fn(
        *mut NamedWatchRun,
        usize,
        *mut NamedWatchStage,
    ) -> bool,
    undo: unsafe extern "C" fn(*mut NamedWatchRun, *mut c_void),
    commit: unsafe extern "C" fn(*mut NamedWatchRun, *const NamedWatchStage),
}

unsafe extern "C" fn reconcile(
    run: *mut NamedWatchRun,
    ops: *const NamedWatchOps,
    stage: *mut NamedWatchStage,
    count: usize,
) -> bool {
    // SAFETY: C holds the application mutex, lends the context and operations,
    // and supplies a separate, initialized staging buffer sized for count.
    unsafe {
        let ops = &*ops;
        let stage = std::slice::from_raw_parts_mut(stage, count);
        for (index, entry) in stage.iter_mut().enumerate() {
            let mut was_pending = false;
            if !(ops.check)(run, index, &mut entry.present, &mut was_pending) {
                if !was_pending {
                    return false;
                }
                entry.deferred = true;
            }
        }

        for index in 0..stage.len() {
            let entry = &mut stage[index];
            if !entry.deferred && !(ops.add)(run, index, entry) {
                for applied in stage[..index].iter().rev() {
                    if !applied.added.is_null() {
                        (ops.undo)(run, applied.added);
                    }
                }
                return false;
            }
        }
        (ops.commit)(run, stage.as_ptr());
        true
    }
}

type WatchDriver = unsafe extern "C" fn(
    *mut NamedWatchRun,
    *const NamedWatchOps,
    *mut NamedWatchStage,
    usize,
) -> bool;

unsafe extern "C" {
    fn cbm_daemon_application_set_named_watch_driver(driver: WatchDriver);
}

/// Select the Rust reconciler at process startup.
///
/// # Safety
///
/// Register before any daemon thread can use the callback.
pub unsafe fn register() {
    if std::env::var_os("MORI_IMPL_NAMED_WATCH").as_deref()
        == Some(OsStr::new("rust"))
    {
        // SAFETY: the caller guarantees startup registration.
        unsafe { cbm_daemon_application_set_named_watch_driver(reconcile) };
    }
}
