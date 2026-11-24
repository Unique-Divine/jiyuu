//! Retry contained index worker failures without releasing the daemon's job reservation.

#[cfg(not(test))]
use std::ffi::OsStr;
use std::ffi::c_int;

const CANCELLED: u8 = 1;
const RETRY: u8 = 2;

#[repr(C)]
struct JobRecovery {
    _opaque: [u8; 0],
}

#[repr(C)]
struct Summary {
    has_response: bool,
    unsafe_terminal: bool,
    quarantined: bool,
}

#[repr(C)]
struct RecoveryOps {
    cancel_requested: unsafe extern "C" fn(*mut JobRecovery) -> bool,
    mark_cancelled: unsafe extern "C" fn(*mut JobRecovery),
    clear_marker: unsafe extern "C" fn(*mut JobRecovery) -> bool,
    attempt: unsafe extern "C" fn(*mut JobRecovery) -> u8,
    record_suspects: unsafe extern "C" fn(*mut JobRecovery, c_int) -> bool,
    finish_attempts: unsafe extern "C" fn(*mut JobRecovery) -> Summary,
    try_partial: unsafe extern "C" fn(*mut JobRecovery),
}

unsafe extern "C" fn recover(
    run: *mut JobRecovery,
    ops: *const RecoveryOps,
    max_attempts: c_int,
) {
    // SAFETY: C lends the live job context and the matching operations on
    // its supervisor thread. C owns the worker, files, and cancellation.
    unsafe {
        let ops = &*ops;
        for index in 0..max_attempts {
            if (ops.cancel_requested)(run) {
                (ops.mark_cancelled)(run);
                break;
            }
            if !(ops.clear_marker)(run) {
                break;
            }
            match (ops.attempt)(run) {
                RETRY if (ops.record_suspects)(run, index) => {}
                CANCELLED => {
                    (ops.mark_cancelled)(run);
                    break;
                }
                _ => break,
            }
        }
        let summary = (ops.finish_attempts)(run);
        if !summary.has_response
            && !summary.unsafe_terminal
            && summary.quarantined
        {
            (ops.try_partial)(run);
        }
    }
}

#[cfg(not(test))]
unsafe extern "C" {
    fn cbm_daemon_application_set_job_recovery_driver(
        driver: unsafe extern "C" fn(
            *mut JobRecovery,
            *const RecoveryOps,
            c_int,
        ),
    );
}

/// Select the Rust driver at process startup.
///
/// # Safety
///
/// Call before any daemon thread can use the callback.
#[cfg(not(test))]
pub unsafe fn register() {
    if std::env::var_os("MORI_IMPL_JOB_RECOVERY").as_deref()
        == Some(OsStr::new("rust"))
    {
        // SAFETY: The caller guarantees startup registration.
        unsafe { cbm_daemon_application_set_job_recovery_driver(recover) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[derive(Debug, PartialEq, Eq)]
    enum Call {
        CheckCancellation,
        ClearMarker,
        Attempt,
        Record(c_int),
        MarkCancelled,
        FinishAttempts,
        Partial,
    }

    struct Harness {
        cancel_at_check: Option<usize>,
        checks: usize,
        attempts: VecDeque<u8>,
        summary: Summary,
        calls: Vec<Call>,
    }

    unsafe extern "C" fn cancel_requested(run: *mut JobRecovery) -> bool {
        // SAFETY: The test lends a live Harness to every operation.
        let harness = unsafe { &mut *run.cast::<Harness>() };
        harness.calls.push(Call::CheckCancellation);
        let check = harness.checks;
        harness.checks += 1;
        harness.cancel_at_check == Some(check)
    }

    unsafe extern "C" fn mark_cancelled(run: *mut JobRecovery) {
        // SAFETY: The test lends a live Harness to every operation.
        let harness = unsafe { &mut *run.cast::<Harness>() };
        harness.calls.push(Call::MarkCancelled);
        harness.summary.unsafe_terminal = true;
    }

    unsafe extern "C" fn clear_marker(run: *mut JobRecovery) -> bool {
        // SAFETY: The test lends a live Harness to every operation.
        unsafe { (*run.cast::<Harness>()).calls.push(Call::ClearMarker) };
        true
    }

    unsafe extern "C" fn attempt(run: *mut JobRecovery) -> u8 {
        // SAFETY: The test lends a live Harness to every operation.
        let harness = unsafe { &mut *run.cast::<Harness>() };
        harness.calls.push(Call::Attempt);
        harness.attempts.pop_front().unwrap_or(0)
    }

    unsafe extern "C" fn record_suspects(
        run: *mut JobRecovery,
        index: c_int,
    ) -> bool {
        // SAFETY: The test lends a live Harness to every operation.
        unsafe { (*run.cast::<Harness>()).calls.push(Call::Record(index)) };
        true
    }

    unsafe extern "C" fn finish_attempts(run: *mut JobRecovery) -> Summary {
        // SAFETY: The test lends a live Harness to every operation.
        let harness = unsafe { &mut *run.cast::<Harness>() };
        harness.calls.push(Call::FinishAttempts);
        Summary { ..harness.summary }
    }

    unsafe extern "C" fn try_partial(run: *mut JobRecovery) {
        // SAFETY: The test lends a live Harness to every operation.
        unsafe { (*run.cast::<Harness>()).calls.push(Call::Partial) };
    }

    fn run(
        attempts: impl Into<VecDeque<u8>>,
        cap: c_int,
        summary: Summary,
        cancel_at_check: Option<usize>,
    ) -> Vec<Call> {
        let mut harness = Harness {
            cancel_at_check,
            checks: 0,
            attempts: attempts.into(),
            summary,
            calls: Vec::new(),
        };
        let ops = RecoveryOps {
            cancel_requested,
            mark_cancelled,
            clear_marker,
            attempt,
            record_suspects,
            finish_attempts,
            try_partial,
        };
        // SAFETY: All mock operations receive a valid Harness for this call.
        unsafe { recover((&mut harness as *mut Harness).cast(), &ops, cap) };
        harness.calls
    }

    #[test]
    fn retry_budget_finishes_attempts_before_partial_index() {
        let calls = run(
            [RETRY, RETRY],
            1,
            Summary {
                has_response: false,
                unsafe_terminal: false,
                quarantined: true,
            },
            None,
        );
        assert_eq!(
            calls,
            [
                Call::CheckCancellation,
                Call::ClearMarker,
                Call::Attempt,
                Call::Record(0),
                Call::FinishAttempts,
                Call::Partial,
            ]
        );
    }

    #[test]
    fn cancelled_worker_cannot_publish_a_partial_index() {
        let calls = run(
            [CANCELLED, RETRY],
            2,
            Summary {
                has_response: false,
                unsafe_terminal: false,
                quarantined: true,
            },
            None,
        );
        assert_eq!(
            calls,
            [
                Call::CheckCancellation,
                Call::ClearMarker,
                Call::Attempt,
                Call::MarkCancelled,
                Call::FinishAttempts,
            ]
        );
    }

    #[test]
    fn cancellation_between_attempts_prevents_another_spawn() {
        let calls = run(
            [RETRY, RETRY],
            2,
            Summary {
                has_response: false,
                unsafe_terminal: false,
                quarantined: true,
            },
            Some(1),
        );
        assert_eq!(
            calls,
            [
                Call::CheckCancellation,
                Call::ClearMarker,
                Call::Attempt,
                Call::Record(0),
                Call::CheckCancellation,
                Call::MarkCancelled,
                Call::FinishAttempts,
            ],
        );
    }
}
