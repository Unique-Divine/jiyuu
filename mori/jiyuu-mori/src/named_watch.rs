//! Reconcile a complete named watch snapshot under the daemon application mutex.

#[cfg(not(test))]
use std::ffi::OsStr;
use std::ffi::c_void;

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

#[cfg(not(test))]
type WatchDriver = unsafe extern "C" fn(
    *mut NamedWatchRun,
    *const NamedWatchOps,
    *mut NamedWatchStage,
    usize,
) -> bool;

#[cfg(not(test))]
unsafe extern "C" {
    fn cbm_daemon_application_set_named_watch_driver(driver: WatchDriver);
}

/// Select the Rust reconciler at process startup.
///
/// # Safety
///
/// Register before any daemon thread can use the callback.
#[cfg(not(test))]
pub unsafe fn register() {
    if std::env::var_os("MORI_IMPL_NAMED_WATCH")
        .as_deref()
        .is_none_or(|value| value == OsStr::new("rust"))
    {
        // SAFETY: the caller guarantees startup registration.
        unsafe { cbm_daemon_application_set_named_watch_driver(reconcile) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    enum Call {
        Check(usize),
        Add(usize),
        Undo(usize),
        Commit,
    }

    #[derive(Default)]
    struct Harness {
        calls: Vec<Call>,
    }

    unsafe extern "C" fn check(
        run: *mut NamedWatchRun,
        index: usize,
        present: *mut bool,
        was_pending: *mut bool,
    ) -> bool {
        // SAFETY: The test lends the harness and the driver lends both flags.
        unsafe {
            (*run.cast::<Harness>()).calls.push(Call::Check(index));
            *present = index != 1;
            *was_pending = index == 1;
        }
        index != 1
    }

    unsafe extern "C" fn add(
        run: *mut NamedWatchRun,
        index: usize,
        entry: *mut NamedWatchStage,
    ) -> bool {
        // SAFETY: The test lends the harness and the driver lends this slot.
        unsafe { (*run.cast::<Harness>()).calls.push(Call::Add(index)) };
        if index == 3 {
            return false;
        }
        // SAFETY: The callback only returns a non-null token. No code dereferences it.
        unsafe { (*entry).added = std::ptr::without_provenance_mut(index + 1) };
        true
    }

    unsafe extern "C" fn undo(run: *mut NamedWatchRun, added: *mut c_void) {
        // SAFETY: The test lends a live Harness to every operation.
        let harness = unsafe { &mut *run.cast::<Harness>() };
        harness
            .calls
            .push(Call::Undo(added.addr().saturating_sub(1)));
    }

    unsafe extern "C" fn commit(
        run: *mut NamedWatchRun,
        _: *const NamedWatchStage,
    ) {
        // SAFETY: The test lends a live Harness to every operation.
        unsafe { (*run.cast::<Harness>()).calls.push(Call::Commit) };
    }

    #[test]
    fn failed_registration_rolls_back_in_reverse_without_committing() {
        let mut harness = Harness::default();
        let mut stage: [_; 4] = std::array::from_fn(|_| NamedWatchStage {
            present: false,
            deferred: false,
            added: std::ptr::null_mut(),
        });
        let ops = NamedWatchOps {
            check,
            add,
            undo,
            commit,
        };
        // SAFETY: All operations receive a valid harness and the stage has four slots.
        let accepted = unsafe {
            reconcile(
                (&mut harness as *mut Harness).cast(),
                &ops,
                stage.as_mut_ptr(),
                stage.len(),
            )
        };
        assert!(!accepted);
        assert_eq!(
            harness.calls,
            [
                Call::Check(0),
                Call::Check(1),
                Call::Check(2),
                Call::Check(3),
                Call::Add(0),
                Call::Add(2),
                Call::Add(3),
                Call::Undo(2),
                Call::Undo(0),
            ]
        );
    }
}
