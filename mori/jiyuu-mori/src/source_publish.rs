//! Drive the outer source publication decision after indexing into a stage.

use std::ffi::c_int;

#[repr(C)]
struct Publish {
    _opaque: [u8; 0],
}

type Check = unsafe extern "C" fn(*mut Publish) -> bool;
type Action = unsafe extern "C" fn(*mut Publish) -> c_int;

#[repr(C)]
struct Ops {
    abort_preserve_db: c_int,
    persist_failed: c_int,
    cancelled: Check,
    finish_noop: Action,
    relink: Check,
    seal: Check,
    before_publish: unsafe extern "C" fn(*mut Publish),
    source_matches: Check,
    claim_lease: Check,
    commit: Action,
}

unsafe extern "C" fn publish(
    run: *mut Publish,
    ops: *const Ops,
    staged_rc: c_int,
    live_noop: bool,
    final_existed: bool,
) -> c_int {
    if staged_rc != 0 {
        return staged_rc;
    }
    // SAFETY: C lends the context and matching operations for this call.
    // The native commit operation owns both rename and any needed rollback.
    unsafe {
        let ops = &*ops;
        if (ops.cancelled)(run) {
            return ops.abort_preserve_db;
        }
        if live_noop {
            return (ops.finish_noop)(run);
        }
        if final_existed && !(ops.relink)(run) {
            return if (ops.cancelled)(run) {
                ops.abort_preserve_db
            } else {
                ops.persist_failed
            };
        }
        if !(ops.seal)(run) {
            return ops.persist_failed;
        }
        (ops.before_publish)(run);
        if (ops.cancelled)(run) {
            return ops.abort_preserve_db;
        }
        if !(ops.seal)(run) {
            return ops.persist_failed;
        }
        if !(ops.source_matches)(run) || !(ops.claim_lease)(run) {
            return ops.abort_preserve_db;
        }
        (ops.commit)(run)
    }
}

unsafe extern "C" {
    fn cbm_pipeline_set_source_publish(
        driver: unsafe extern "C" fn(
            *mut Publish,
            *const Ops,
            c_int,
            bool,
            bool,
        ) -> c_int,
    );
}

/// Install Rust source publication for this process.
///
/// # Safety
///
/// Call before any pipeline starts or any indexing thread reads the callback.
pub unsafe fn register() {
    // SAFETY: the caller guarantees startup registration, and the callback
    // lives for the life of the program.
    unsafe { cbm_pipeline_set_source_publish(publish) };
}
