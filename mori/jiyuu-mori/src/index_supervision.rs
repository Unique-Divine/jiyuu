use std::sync::atomic::{
    AtomicU8,
    Ordering::{AcqRel, Acquire, Relaxed},
};

#[derive(Clone, Copy)]
enum FenceClaim {
    Created,
    Exists,
    Failed,
}

#[derive(Debug, PartialEq, Eq)]
#[repr(i32)]
enum WorkerPublication {
    Allowed,
    Claimed,
    RefusedLimit,
    RefusedError,
}

fn worker_publication(
    already_claimed: bool,
    claim: Option<impl FnOnce() -> FenceClaim>,
) -> WorkerPublication {
    match (already_claimed, claim) {
        (false, Some(claim)) => match claim() {
            FenceClaim::Created => WorkerPublication::Claimed,
            FenceClaim::Exists => WorkerPublication::RefusedLimit,
            FenceClaim::Failed => WorkerPublication::RefusedError,
        },
        _ => WorkerPublication::Allowed,
    }
}

const OPEN: u8 = 0;
const SUPERVISOR: u8 = 1;
const WORKER: u8 = 2;

const NONE: u8 = 0;
const CANCEL_PENDING: u8 = 1;
const CANCEL: u8 = 2;
const RESOURCE: u8 = 3;

#[derive(Default)]
struct Supervision {
    // Only the polling owner accesses fence state. Cancellation can enter from
    // another thread and only touches termination.
    fence: AtomicU8,
    termination: AtomicU8,
}

#[derive(Debug, PartialEq, Eq)]
#[repr(i32)]
enum LimitAction {
    Ignored,
    Accepted,
    AfterPublication,
    FenceError,
}

#[repr(C)]
struct TerminalFacts {
    clean: bool,
    cancellation_requested: bool,
    tree_quiesced: bool,
    supervision_failed: bool,
    resource_violation: bool,
    resource_probe_failed: bool,
}

#[derive(Debug, PartialEq, Eq)]
#[repr(i32)]
enum Disposition {
    Clean,
    WorkerFailed,
    ContainmentFailed,
    Cancelled,
    ResourceLimit,
    ResourceProbeFailed,
    ResponseMissing,
    ResponseRejected,
}

#[repr(C)]
struct TerminalDecision {
    disposition: Disposition,
    cancellation_requested: bool,
    publication_committed: bool,
    remove_fence: bool,
}

impl Supervision {
    fn checks_limits(&self) -> bool {
        self.fence.load(Relaxed) != WORKER
            && self.termination.load(Acquire) == NONE
    }

    fn claim_fence(&self, claim: impl FnOnce() -> FenceClaim) -> FenceClaim {
        debug_assert_eq!(self.fence.load(Relaxed), OPEN);
        let claimed = claim();
        match claimed {
            FenceClaim::Created => self.fence.store(SUPERVISOR, Relaxed),
            FenceClaim::Exists => self.fence.store(WORKER, Relaxed),
            FenceClaim::Failed => {}
        }
        claimed
    }

    fn limit(
        &self,
        claim: Option<impl FnOnce() -> FenceClaim>,
        cancel: impl FnOnce() -> bool,
    ) -> LimitAction {
        if let Some(claim) = claim {
            match self.fence.load(Relaxed) {
                WORKER => return LimitAction::Ignored,
                SUPERVISOR => {}
                _ => match self.claim_fence(claim) {
                    FenceClaim::Created => {}
                    FenceClaim::Exists => return LimitAction::AfterPublication,
                    FenceClaim::Failed => return LimitAction::FenceError,
                },
            }
        }
        if self
            .termination
            .compare_exchange(NONE, RESOURCE, AcqRel, Acquire)
            .is_err()
        {
            return LimitAction::Ignored;
        }
        if cancel() {
            return LimitAction::Accepted;
        }
        let _ = self
            .termination
            .compare_exchange(RESOURCE, NONE, AcqRel, Acquire);
        LimitAction::Ignored
    }

    fn cancel(&self, cancel: impl FnOnce() -> bool) -> bool {
        let first = match self.termination.compare_exchange(
            NONE,
            CANCEL_PENDING,
            AcqRel,
            Acquire,
        ) {
            Ok(_) => true,
            Err(CANCEL_PENDING) => false,
            Err(CANCEL) => return true,
            Err(_) => return false,
        };
        let accepted = cancel();
        if accepted || first {
            let next = if accepted { CANCEL } else { NONE };
            let _ = self.termination.compare_exchange(
                CANCEL_PENDING,
                next,
                AcqRel,
                Acquire,
            );
        }
        accepted
    }

    fn publication_committed(
        &self,
        claim: Option<impl FnOnce() -> FenceClaim>,
    ) -> bool {
        let Some(claim) = claim else { return false };
        match self.fence.load(Relaxed) {
            SUPERVISOR => false,
            WORKER => true,
            // Claim an open fence even after reaping. An unquiesced descendant
            // could otherwise publish after the parent reports preservation.
            _ => !matches!(self.claim_fence(claim), FenceClaim::Created),
        }
    }

    fn terminal(
        &self,
        facts: TerminalFacts,
        claim: Option<impl FnOnce() -> FenceClaim>,
    ) -> TerminalDecision {
        let cancellation_requested = match self.termination.load(Acquire) {
            CANCEL => true,
            CANCEL_PENDING => facts.cancellation_requested,
            _ => false,
        };
        let publication_committed = self.publication_committed(claim);
        let disposition = if facts.supervision_failed || !facts.tree_quiesced {
            Disposition::ContainmentFailed
        } else if facts.resource_probe_failed {
            Disposition::ResourceProbeFailed
        } else if facts.resource_violation {
            Disposition::ResourceLimit
        } else if cancellation_requested {
            Disposition::Cancelled
        } else if !facts.clean {
            Disposition::WorkerFailed
        } else {
            Disposition::Clean
        };
        TerminalDecision {
            disposition,
            cancellation_requested,
            publication_committed,
            remove_fence: facts.tree_quiesced,
        }
    }
}

// Keep the response status codes in sync with index_supervision_abi.h.
fn response_disposition(status: i32) -> Disposition {
    match status {
        0 => Disposition::Clean,
        2 => Disposition::ResponseRejected,
        _ => Disposition::ResponseMissing,
    }
}

#[cfg(feature = "native-engine-abi")]
mod ffi;

#[cfg(test)]
mod tests {
    use super::*;

    fn absent_fence() -> Option<fn() -> FenceClaim> {
        None
    }

    fn reaped(clean: bool) -> TerminalFacts {
        TerminalFacts {
            clean,
            cancellation_requested: false,
            tree_quiesced: true,
            supervision_failed: false,
            resource_violation: false,
            resource_probe_failed: false,
        }
    }

    #[test]
    fn crash_keeps_original_failure_and_never_reads_a_response() {
        let decision =
            Supervision::default().terminal(reaped(false), absent_fence());
        assert_eq!(decision.disposition, Disposition::WorkerFailed);
        assert!(!decision.cancellation_requested);
        assert!(!decision.publication_committed);
        assert!(decision.remove_fence);
    }

    #[test]
    fn only_a_contained_clean_exit_reads_the_response() {
        let state = Supervision::default();
        assert_eq!(
            state.terminal(reaped(true), absent_fence()).disposition,
            Disposition::Clean
        );
        assert_eq!(response_disposition(0), Disposition::Clean);
        assert_eq!(response_disposition(1), Disposition::ResponseMissing);
        assert_eq!(response_disposition(2), Disposition::ResponseRejected);

        let mut facts = reaped(true);
        facts.supervision_failed = true;
        assert_eq!(
            state.terminal(facts, absent_fence()).disposition,
            Disposition::ContainmentFailed
        );
    }

    #[test]
    fn cancellation_pending_at_reap_uses_subprocess_acknowledgement() {
        let state = Supervision::default();
        assert!(!state.cancel(|| {
            let terminal = |acknowledged| {
                let mut facts = reaped(true);
                facts.cancellation_requested = acknowledged;
                state.terminal(facts, absent_fence())
            };
            assert_eq!(terminal(false).disposition, Disposition::Clean);
            let cancelled = terminal(true);
            assert!(cancelled.cancellation_requested);
            assert_eq!(cancelled.disposition, Disposition::Cancelled);
            false
        }));
        assert!(state.checks_limits());
        assert!(state.cancel(|| true));
        assert!(!state.checks_limits());
        assert!(state.cancel(|| panic!("accepted cancellation is idempotent")));
    }

    #[test]
    fn supervisor_fence_blocks_worker_and_resource_stop_is_not_user_cancellation()
     {
        let state = Supervision::default();
        assert_eq!(
            state.limit(Some(|| FenceClaim::Created), || true),
            LimitAction::Accepted
        );
        assert!(!state.checks_limits());
        assert!(!state.cancel(|| panic!("resource stop already accepted")));
        assert_eq!(
            worker_publication(false, Some(|| FenceClaim::Exists)),
            WorkerPublication::RefusedLimit
        );

        let mut facts = reaped(false);
        facts.cancellation_requested = true;
        facts.resource_violation = true;
        let terminal = state.terminal(
            facts,
            Some(|| panic!("supervisor already owns the fence")),
        );
        assert_eq!(terminal.disposition, Disposition::ResourceLimit);
        assert!(!terminal.cancellation_requested);
        assert!(!terminal.publication_committed);
        assert!(terminal.remove_fence);
    }

    #[test]
    fn worker_fence_blocks_resource_stop() {
        assert_eq!(
            worker_publication(false, Some(|| FenceClaim::Created)),
            WorkerPublication::Claimed
        );
        assert_eq!(
            worker_publication(
                true,
                Some(|| panic!("worker already owns the fence"))
            ),
            WorkerPublication::Allowed
        );
        assert_eq!(
            worker_publication(false, absent_fence()),
            WorkerPublication::Allowed
        );
        let state = Supervision::default();
        assert_eq!(
            state.limit(Some(|| FenceClaim::Exists), || panic!(
                "worker won fence"
            )),
            LimitAction::AfterPublication
        );
        assert!(!state.checks_limits());
        assert!(
            state
                .terminal(
                    reaped(true),
                    Some(|| panic!("worker already owns the fence")),
                )
                .publication_committed
        );
    }

    #[test]
    fn fence_error_retries_and_terminal_reports_uncertain_publication() {
        let state = Supervision::default();
        assert_eq!(
            worker_publication(false, Some(|| FenceClaim::Failed)),
            WorkerPublication::RefusedError
        );
        assert_eq!(
            state.limit(Some(|| FenceClaim::Failed), || panic!("fence unknown")),
            LimitAction::FenceError
        );
        assert!(state.checks_limits());

        let mut facts = reaped(false);
        facts.tree_quiesced = false;
        let terminal = state.terminal(facts, Some(|| FenceClaim::Failed));
        assert_eq!(terminal.disposition, Disposition::ContainmentFailed);
        assert!(terminal.publication_committed);
        assert!(!terminal.remove_fence);
    }

    #[test]
    fn terminal_claim_stays_in_place_for_an_unquiesced_tree() {
        let state = Supervision::default();
        let mut facts = reaped(false);
        facts.tree_quiesced = false;
        let terminal = state.terminal(facts, Some(|| FenceClaim::Created));
        assert!(!terminal.publication_committed);
        assert!(!terminal.remove_fence);
    }

    #[test]
    fn rejected_resource_stop_keeps_fence_and_can_retry() {
        let state = Supervision::default();
        assert_eq!(
            state.limit(Some(|| FenceClaim::Created), || false),
            LimitAction::Ignored
        );
        assert!(state.checks_limits());
        assert_eq!(
            state.limit(Some(|| panic!("fence is still held")), || true),
            LimitAction::Accepted
        );

        let mut facts = reaped(false);
        facts.resource_violation = true;
        facts.resource_probe_failed = true;
        assert_eq!(
            state
                .terminal(facts, Some(|| panic!("fence is still held")))
                .disposition,
            Disposition::ResourceProbeFailed
        );
    }
}
