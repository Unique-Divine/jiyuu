//! Schedule startup and deferred provider repair on the daemon recovery thread.

#[cfg(not(test))]
use std::ffi::OsStr;

const STOPPING: u8 = 1 << 0;
const REQUESTED: u8 = 1 << 1;
const SETTLED: u8 = 1 << 2;

#[derive(Debug, PartialEq, Eq)]
enum Decision {
    Finish,
    Rescan,
    Wait,
}

fn decide(flags: u8) -> Decision {
    if flags & STOPPING != 0 {
        Decision::Finish
    } else if flags & REQUESTED != 0 {
        Decision::Rescan
    } else if flags & SETTLED != 0 {
        Decision::Finish
    } else {
        Decision::Wait
    }
}

#[repr(C)]
struct Recovery {
    _opaque: [u8; 0],
}

#[repr(C)]
struct RecoveryOps {
    stopping: unsafe extern "C" fn(*const Recovery) -> bool,
    run_batch: unsafe extern "C" fn(*mut Recovery, bool) -> bool,
    retry_deadline: unsafe extern "C" fn() -> u64,
    begin_check: unsafe extern "C" fn(*mut Recovery) -> u8,
    end_check: unsafe extern "C" fn(*mut Recovery, bool),
    wait_tick: unsafe extern "C" fn(u64) -> bool,
    finish: unsafe extern "C" fn(*mut Recovery),
}

unsafe extern "C" fn run_recovery(run: *mut Recovery, ops: *const RecoveryOps) {
    // SAFETY: C lends the context and operations on its recovery thread.
    // Each begin_check locks the application until its matching end_check.
    // No application operation follows a finished check.
    unsafe {
        let ops = &*ops;
        let mut rescan = true;
        let mut have_projects = false;
        while !(ops.stopping)(run) {
            have_projects = (ops.run_batch)(run, rescan || !have_projects);
            let retry_at = (ops.retry_deadline)();
            loop {
                let action = decide((ops.begin_check)(run));
                (ops.end_check)(run, action == Decision::Finish);
                match action {
                    Decision::Finish => return,
                    Decision::Rescan => {
                        rescan = true;
                        break;
                    }
                    Decision::Wait if (ops.wait_tick)(retry_at) => {
                        rescan = false;
                        break;
                    }
                    Decision::Wait => {}
                }
            }
        }
        (ops.finish)(run);
    }
}

#[cfg(not(test))]
unsafe extern "C" {
    fn cbm_daemon_application_set_recovery_driver(
        driver: unsafe extern "C" fn(*mut Recovery, *const RecoveryOps),
    );
}

/// Select the Rust scheduler before starting any daemon threads.
///
/// # Safety
///
/// The C setter must run before any thread reads the callback.
#[cfg(not(test))]
pub unsafe fn register() {
    if std::env::var_os("MORI_IMPL_REPAIR_RECOVERY")
        .as_deref()
        .is_none_or(|value| value == OsStr::new("rust"))
    {
        // SAFETY: the caller guarantees startup registration.
        unsafe { cbm_daemon_application_set_recovery_driver(run_recovery) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::cell::{Cell, RefCell};

    #[derive(Debug, PartialEq, Eq)]
    enum Call {
        Stopping,
        Scan(bool),
        BeginCheck,
        EndCheck(bool),
        Finish,
    }

    #[derive(Default)]
    struct Harness {
        checks: Cell<usize>,
        calls: RefCell<Vec<Call>>,
    }

    unsafe extern "C" fn stopping(run: *const Recovery) -> bool {
        // SAFETY: The test lends a live Harness to each operation.
        let harness = unsafe { &*run.cast::<Harness>() };
        harness.calls.borrow_mut().push(Call::Stopping);
        harness.checks.get() >= 2
    }

    unsafe extern "C" fn run_batch(run: *mut Recovery, full_scan: bool) -> bool {
        // SAFETY: The test lends a live Harness to each operation.
        let harness = unsafe { &*run.cast::<Harness>() };
        harness.calls.borrow_mut().push(Call::Scan(full_scan));
        true
    }

    unsafe extern "C" fn begin_check(run: *mut Recovery) -> u8 {
        // SAFETY: The test lends a live Harness to each operation.
        let harness = unsafe { &*run.cast::<Harness>() };
        harness.calls.borrow_mut().push(Call::BeginCheck);
        let check = harness.checks.get();
        harness.checks.set(check + 1);
        if check == 0 {
            REQUESTED | SETTLED
        } else {
            STOPPING | REQUESTED | SETTLED
        }
    }

    unsafe extern "C" fn end_check(run: *mut Recovery, done: bool) {
        // SAFETY: The test lends a live Harness to each operation.
        let harness = unsafe { &*run.cast::<Harness>() };
        harness.calls.borrow_mut().push(Call::EndCheck(done));
    }

    unsafe extern "C" fn finish(run: *mut Recovery) {
        // SAFETY: The test lends a live Harness to each operation.
        let harness = unsafe { &*run.cast::<Harness>() };
        harness.calls.borrow_mut().push(Call::Finish);
    }

    unsafe extern "C" fn retry_deadline() -> u64 {
        1
    }

    thread_local! {
        static WAIT_TICKS: Cell<usize> = const { Cell::new(0) };
    }

    unsafe extern "C" fn wait_tick(_: u64) -> bool {
        WAIT_TICKS.with(|ticks| ticks.set(ticks.get() + 1));
        true
    }

    #[test]
    fn driver_rescans_settled_batch_then_finishes_under_the_final_check() {
        let mut harness = Harness::default();
        let ops = RecoveryOps {
            stopping,
            run_batch,
            retry_deadline,
            begin_check,
            end_check,
            wait_tick,
            finish,
        };
        WAIT_TICKS.with(|ticks| ticks.set(0));
        // SAFETY: All operations receive the live Harness for this call.
        unsafe { run_recovery((&mut harness as *mut Harness).cast(), &ops) };
        assert_eq!(WAIT_TICKS.with(Cell::get), 0);
        assert_eq!(
            harness.calls.into_inner(),
            [
                Call::Stopping,
                Call::Scan(true),
                Call::BeginCheck,
                Call::EndCheck(false),
                Call::Stopping,
                Call::Scan(true),
                Call::BeginCheck,
                Call::EndCheck(true),
            ]
        );
    }

    #[test]
    fn shutdown_takes_priority_over_a_pending_rescan() {
        assert_eq!(decide(STOPPING | REQUESTED), Decision::Finish);
        assert_eq!(decide(STOPPING | REQUESTED | SETTLED), Decision::Finish);
    }

    #[test]
    fn a_new_request_rescans_even_after_the_pending_list_settles() {
        assert_eq!(decide(REQUESTED), Decision::Rescan);
        assert_eq!(decide(REQUESTED | SETTLED), Decision::Rescan);
    }

    #[test]
    fn retry_waits_only_while_unsettled_and_without_a_new_request() {
        assert_eq!(decide(0), Decision::Wait);
        assert_eq!(decide(SETTLED), Decision::Finish);
    }
}
