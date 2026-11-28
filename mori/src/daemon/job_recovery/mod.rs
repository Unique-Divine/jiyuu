//! Retry contained index worker failures without releasing the daemon's job reservation.

mod provider_repair;

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

#[unsafe(export_name = "mori_daemon_job_recover")]
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

    #[derive(PartialEq, Eq)]
    enum Fault {
        CancelBeforeAttempt(usize),
        ClearMarker,
        RecordSuspects,
    }

    struct Harness {
        fault: Option<Fault>,
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
        harness.fault == Some(Fault::CancelBeforeAttempt(check))
    }

    unsafe extern "C" fn mark_cancelled(run: *mut JobRecovery) {
        // SAFETY: The test lends a live Harness to every operation.
        let harness = unsafe { &mut *run.cast::<Harness>() };
        harness.calls.push(Call::MarkCancelled);
        harness.summary.unsafe_terminal = true;
    }

    unsafe extern "C" fn clear_marker(run: *mut JobRecovery) -> bool {
        // SAFETY: The test lends a live Harness to every operation.
        let harness = unsafe { &mut *run.cast::<Harness>() };
        harness.calls.push(Call::ClearMarker);
        harness.fault != Some(Fault::ClearMarker)
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
        let harness = unsafe { &mut *run.cast::<Harness>() };
        harness.calls.push(Call::Record(index));
        harness.fault != Some(Fault::RecordSuspects)
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
        fault: Option<Fault>,
    ) -> Vec<Call> {
        let mut harness = Harness {
            fault,
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
            Some(Fault::CancelBeforeAttempt(1)),
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

    #[test]
    fn marker_failure_prevents_worker_spawn() {
        let calls = run(
            [RETRY],
            2,
            Summary {
                has_response: false,
                unsafe_terminal: false,
                quarantined: false,
            },
            Some(Fault::ClearMarker),
        );
        assert_eq!(
            calls,
            [
                Call::CheckCancellation,
                Call::ClearMarker,
                Call::FinishAttempts
            ]
        );
    }

    #[test]
    fn retries_stop_when_the_worker_yields_no_more_suspects() {
        let calls = run(
            [RETRY, RETRY],
            2,
            Summary {
                has_response: false,
                unsafe_terminal: false,
                quarantined: true,
            },
            Some(Fault::RecordSuspects),
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
    fn terminal_worker_results_bypass_retries_and_partial_indexing() {
        for (has_response, unsafe_terminal) in [(true, false), (false, true)] {
            let calls = run(
                [0, RETRY],
                2,
                Summary {
                    has_response,
                    unsafe_terminal,
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
                    Call::FinishAttempts,
                ]
            );
        }
    }
}
