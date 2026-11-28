//! Execute the extraction and resolution stages of an incremental index run.

use std::ffi::c_int;

const PARALLEL_THRESHOLD: c_int = 50;

#[repr(C)]
struct FileRun {
    _opaque: [u8; 0],
}

#[derive(Clone, Copy)]
#[cfg_attr(test, derive(Debug, PartialEq, Eq))]
#[repr(C)]
// Keep the discriminants aligned with incremental_file_phase_t.
enum Phase {
    Extract,
    Registry,
    Resolve,
    Definitions,
    CrossFile,
    Calls,
    Usages,
    Semantic,
    DocLinks,
    Tests,
    DecoratorTags,
    ConfigLink,
    Operator,
    ProjectDocLinks,
    Similarity,
    SemanticEdges,
    Importance,
}

#[repr(C)]
struct Operations {
    begin: unsafe extern "C" fn(*mut FileRun, c_int, bool) -> c_int,
    step: unsafe extern "C" fn(*mut FileRun, Phase) -> c_int,
    check_cancelled: unsafe extern "C" fn(*mut FileRun) -> c_int,
    log_timing: unsafe extern "C" fn(*mut FileRun, Phase),
    sync_ids: unsafe extern "C" fn(*mut FileRun),
    pass_registry_ids: unsafe extern "C" fn(*mut FileRun),
    needs_cross_file: unsafe extern "C" fn(*const FileRun) -> bool,
    doc_candidates_enabled: unsafe extern "C" fn(*const FileRun) -> bool,
    finish: unsafe extern "C" fn(*mut FileRun, bool),
}

struct Execute<'a> {
    run: *mut FileRun,
    ops: &'a Operations,
}

impl Execute<'_> {
    fn step(&self, phase: Phase) -> c_int {
        // Native state belongs to the coordinator until this call returns.
        unsafe { (self.ops.step)(self.run, phase) }
    }

    fn check_cancelled(&self, rc: c_int) -> c_int {
        if rc != 0 {
            rc
        } else {
            unsafe { (self.ops.check_cancelled)(self.run) }
        }
    }

    fn parallel(&self) -> c_int {
        for phase in [Phase::Extract, Phase::Registry, Phase::Resolve] {
            let rc = self.check_cancelled(self.step(phase));
            unsafe { (self.ops.log_timing)(self.run, phase) };
            // Resolve may have allocated IDs before it failed. Sync the
            // watermark for the enclosing attempt before releasing its cache.
            if matches!(phase, Phase::Resolve) {
                unsafe { (self.ops.sync_ids)(self.run) };
            }
            if rc != 0 {
                return rc;
            }
            match phase {
                Phase::Extract => unsafe { (self.ops.sync_ids)(self.run) },
                Phase::Registry => unsafe {
                    (self.ops.pass_registry_ids)(self.run)
                },
                _ => {}
            }
        }
        0
    }

    fn sequential(&self) -> c_int {
        let rc = self.check_cancelled(self.step(Phase::Definitions));
        if rc != 0 {
            return rc;
        }
        if unsafe { (self.ops.needs_cross_file)(self.run) } {
            let rc = self.step(Phase::CrossFile);
            if rc != 0 {
                return rc;
            }
        }
        for phase in [Phase::Calls, Phase::Usages, Phase::Semantic] {
            let rc = self.check_cancelled(self.step(phase));
            if rc != 0 {
                return rc;
            }
        }
        self.step(Phase::DocLinks);
        0
    }
    fn postpass(&self, phase: Phase, cancelled: c_int) -> c_int {
        let rc = self.step(phase);
        unsafe { (self.ops.log_timing)(self.run, phase) };
        let failed = if matches!(phase, Phase::Tests) {
            rc != 0
        } else {
            rc < 0
        };
        if failed {
            return rc;
        }
        if unsafe { (self.ops.check_cancelled)(self.run) } != 0 {
            return cancelled;
        }
        0
    }

    fn postpasses(
        &self,
        moderate_or_full: bool,
        score_importance: bool,
        cancelled: c_int,
    ) -> c_int {
        for phase in [
            Phase::Tests,
            Phase::DecoratorTags,
            Phase::ConfigLink,
            Phase::Operator,
            Phase::ProjectDocLinks,
        ] {
            let rc = self.postpass(phase, cancelled);
            if rc != 0 {
                return rc;
            }
        }
        if moderate_or_full {
            let rc = self.postpass(Phase::Similarity, cancelled);
            if rc != 0 {
                return rc;
            }
        }
        if moderate_or_full
            || unsafe { (self.ops.doc_candidates_enabled)(self.run) }
        {
            let rc = self.postpass(Phase::SemanticEdges, cancelled);
            if rc != 0 {
                return rc;
            }
        }
        if score_importance {
            self.step(Phase::Importance);
            unsafe { (self.ops.log_timing)(self.run, Phase::Importance) };
        }
        if unsafe { (self.ops.check_cancelled)(self.run) } != 0 {
            cancelled
        } else {
            0
        }
    }
}

#[cfg_attr(feature = "native-engine-abi", unsafe(no_mangle))]
unsafe extern "C" fn cbm_rust_incremental_execute(
    run: *mut FileRun,
    ops: *const Operations,
    file_count: c_int,
    workers: c_int,
    closure_repair: bool,
) -> c_int {
    // SAFETY: The C coordinator lends the operation table and all per-run
    // state until this function returns. Each operation is synchronous.
    let ops = unsafe { &*ops };
    let parallel =
        closure_repair || (workers > 1 && file_count > PARALLEL_THRESHOLD);
    let rc = unsafe { (ops.begin)(run, workers, parallel) };
    if rc != 0 {
        return rc;
    }

    let execute = Execute { run, ops };
    let rc = if parallel {
        execute.parallel()
    } else {
        execute.sequential()
    };
    unsafe { (ops.finish)(run, parallel) };
    rc
}

#[cfg_attr(feature = "native-engine-abi", unsafe(no_mangle))]
unsafe extern "C" fn cbm_rust_incremental_postpasses(
    run: *mut FileRun,
    ops: *const Operations,
    moderate_or_full: bool,
    score_importance: bool,
    cancelled: c_int,
) -> c_int {
    // SAFETY: The native coordinator borrows this run and operation table for
    // all graph mutations and checks in this synchronous call.
    let ops = unsafe { &*ops };
    Execute { run, ops }.postpasses(
        moderate_or_full,
        score_importance,
        cancelled,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    enum Event {
        Begin(c_int, bool),
        Step(Phase),
        Cancel,
        Log(Phase),
        Sync,
        RegistryIds,
        Finish(bool),
    }

    #[derive(Default)]
    struct Trace {
        events: Vec<Event>,
        begin_status: c_int,
        override_status: Option<(Phase, c_int)>,
        cancel_at: Option<Phase>,
        cancelled: bool,
        cross_file: bool,
        doc_candidates: bool,
    }

    impl Trace {
        fn steps(&self) -> Vec<Phase> {
            self.events
                .iter()
                .filter_map(|event| match event {
                    Event::Step(phase) => Some(*phase),
                    _ => None,
                })
                .collect()
        }

        fn run(&mut self, count: c_int, workers: c_int, closure: bool) -> c_int {
            unsafe {
                cbm_rust_incremental_execute(
                    std::ptr::from_mut(self).cast(),
                    &OPS,
                    count,
                    workers,
                    closure,
                )
            }
        }

        fn post(&mut self, moderate: bool, importance: bool) -> c_int {
            unsafe {
                cbm_rust_incremental_postpasses(
                    std::ptr::from_mut(self).cast(),
                    &OPS,
                    moderate,
                    importance,
                    -1,
                )
            }
        }
    }

    unsafe extern "C" fn begin(
        ctx: *mut FileRun,
        workers: c_int,
        parallel: bool,
    ) -> c_int {
        let trace = unsafe { &mut *ctx.cast::<Trace>() };
        trace.events.push(Event::Begin(workers, parallel));
        trace.begin_status
    }

    unsafe extern "C" fn step(ctx: *mut FileRun, phase: Phase) -> c_int {
        let trace = unsafe { &mut *ctx.cast::<Trace>() };
        trace.events.push(Event::Step(phase));
        trace.cancelled |= trace.cancel_at == Some(phase);
        trace
            .override_status
            .filter(|&(at, _)| at == phase)
            .map_or(0, |(_, status)| status)
    }

    unsafe extern "C" fn cancelled(ctx: *mut FileRun) -> c_int {
        let trace = unsafe { &mut *ctx.cast::<Trace>() };
        trace.events.push(Event::Cancel);
        if trace.cancelled { -1 } else { 0 }
    }

    unsafe extern "C" fn log(ctx: *mut FileRun, phase: Phase) {
        unsafe { &mut *ctx.cast::<Trace>() }
            .events
            .push(Event::Log(phase));
    }

    unsafe extern "C" fn sync(ctx: *mut FileRun) {
        unsafe { &mut *ctx.cast::<Trace>() }
            .events
            .push(Event::Sync);
    }

    unsafe extern "C" fn registry_ids(ctx: *mut FileRun) {
        unsafe { &mut *ctx.cast::<Trace>() }
            .events
            .push(Event::RegistryIds);
    }

    unsafe extern "C" fn owners(ctx: *const FileRun) -> bool {
        let trace = unsafe { &*ctx.cast::<Trace>() };
        trace.cross_file
    }

    unsafe extern "C" fn candidates(ctx: *const FileRun) -> bool {
        let trace = unsafe { &*ctx.cast::<Trace>() };
        trace.doc_candidates
    }

    unsafe extern "C" fn finish(ctx: *mut FileRun, parallel: bool) {
        unsafe { &mut *ctx.cast::<Trace>() }
            .events
            .push(Event::Finish(parallel));
    }

    const OPS: Operations = Operations {
        begin,
        step,
        check_cancelled: cancelled,
        log_timing: log,
        sync_ids: sync,
        pass_registry_ids: registry_ids,
        needs_cross_file: owners,
        doc_candidates_enabled: candidates,
        finish,
    };

    #[test]
    fn closure_repair_uses_parallel_phases_with_one_worker() {
        use Event::*;
        let mut trace = Trace::default();
        assert_eq!(trace.run(1, 1, true), 0);
        assert_eq!(
            trace.events,
            [
                Begin(1, true),
                Step(Phase::Extract),
                Cancel,
                Log(Phase::Extract),
                Sync,
                Step(Phase::Registry),
                Cancel,
                Log(Phase::Registry),
                RegistryIds,
                Step(Phase::Resolve),
                Cancel,
                Log(Phase::Resolve),
                Sync,
                Finish(true),
            ]
        );
    }

    #[test]
    fn sequential_legacy_keeps_optional_owner_cross_walk_at_the_same_boundary() {
        for owners in [true, false] {
            let mut trace = Trace {
                cross_file: owners,
                ..Trace::default()
            };
            assert_eq!(trace.run(50, 2, false), 0);
            let mut expected = vec![Phase::Definitions];
            if owners {
                expected.push(Phase::CrossFile);
            }
            expected.extend([
                Phase::Calls,
                Phase::Usages,
                Phase::Semantic,
                Phase::DocLinks,
            ]);
            assert_eq!(trace.steps(), expected);
            assert_eq!(trace.events.first(), Some(&Event::Begin(2, false)));
            assert_eq!(trace.events.last(), Some(&Event::Finish(false)));
        }
    }

    #[test]
    fn failure_or_cancellation_stops_before_later_steps_and_releases_state() {
        let mut begin = Trace {
            begin_status: -7,
            ..Trace::default()
        };
        assert_eq!(begin.run(51, 2, false), -7);
        assert_eq!(begin.events, [Event::Begin(2, true)]);

        let mut cancel = Trace {
            cancelled: true,
            ..Trace::default()
        };
        assert_eq!(cancel.run(51, 2, false), -1);
        assert_eq!(cancel.steps(), [Phase::Extract]);
        assert_eq!(cancel.events.last(), Some(&Event::Finish(true)));

        let mut resolve = Trace {
            override_status: Some((Phase::Resolve, -17)),
            ..Trace::default()
        };
        assert_eq!(resolve.run(51, 2, false), -17);
        assert!(resolve.events.ends_with(&[
            Event::Step(Phase::Resolve),
            Event::Log(Phase::Resolve),
            Event::Sync,
            Event::Finish(true),
        ]));
    }

    #[test]
    fn fast_mode_keeps_document_candidates_and_scores_importance_last() {
        let mut trace = Trace {
            doc_candidates: true,
            override_status: Some((Phase::DecoratorTags, 12)),
            ..Trace::default()
        };
        assert_eq!(trace.post(false, true), 0);
        assert_eq!(
            trace.steps(),
            [
                Phase::Tests,
                Phase::DecoratorTags,
                Phase::ConfigLink,
                Phase::Operator,
                Phase::ProjectDocLinks,
                Phase::SemanticEdges,
                Phase::Importance,
            ]
        );

        let mut tests = Trace {
            override_status: Some((Phase::Tests, 12)),
            ..Trace::default()
        };
        assert_eq!(tests.post(true, false), 12);
        assert_eq!(tests.steps(), [Phase::Tests]);
    }

    #[test]
    fn postpasses_stop_on_cancellation_before_scoring() {
        let mut trace = Trace {
            cancel_at: Some(Phase::Similarity),
            ..Trace::default()
        };
        assert_eq!(trace.post(true, true), -1);
        assert!(trace.events.ends_with(&[
            Event::Step(Phase::Similarity),
            Event::Log(Phase::Similarity),
            Event::Cancel,
        ]));
        assert!(!trace.steps().contains(&Phase::SemanticEdges));
        assert!(!trace.steps().contains(&Phase::Importance));
    }

    #[test]
    fn closure_defers_importance_to_the_staged_graph() {
        let mut trace = Trace::default();
        assert_eq!(trace.post(true, false), 0);
        assert!(
            trace
                .steps()
                .ends_with(&[Phase::Similarity, Phase::SemanticEdges])
        );

        let mut fast = Trace::default();
        assert_eq!(fast.post(false, false), 0);
        assert_eq!(fast.steps().last(), Some(&Phase::ProjectDocLinks));
    }
}
