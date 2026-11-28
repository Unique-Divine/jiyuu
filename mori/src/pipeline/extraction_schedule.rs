//! Order and admit parallel extraction tasks before passing them to the native parser.

use std::ffi::{c_char, c_int, c_void};
use std::ptr;
use std::slice;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};

const BACKPRESSURE_SPINS: usize = 40;
const BACKPRESSURE_NAP_NS: u32 = 3_000_000;
const SPILL_EARLY_DIVISOR: usize = 16;

#[repr(C)]
struct Task {
    index: c_int,
    size: i64,
}

#[repr(C)]
struct ExtractContext {
    _opaque: [u8; 0],
}

type Worker = unsafe extern "C" fn(c_int, *mut c_void);

#[repr(C)]
struct Operations {
    dispatch: unsafe extern "C" fn(c_int, Worker, *mut c_void),
    start: unsafe extern "C" fn(*mut ExtractContext, c_int),
    finish: unsafe extern "C" fn(),
    claimed: unsafe extern "C" fn(*mut ExtractContext, c_int),
    extract_file: unsafe extern "C" fn(*mut ExtractContext, c_int, c_int, c_int),
    cancelled: unsafe extern "C" fn(*const ExtractContext) -> bool,
    aborted: unsafe extern "C" fn(*const ExtractContext) -> bool,
    budget: unsafe extern "C" fn() -> usize,
    charged: unsafe extern "C" fn() -> usize,
    over_budget: unsafe extern "C" fn() -> bool,
    system_under_pressure: unsafe extern "C" fn() -> bool,
    charge_probe: unsafe extern "C" fn(*mut ExtractContext, bool),
    spill_active: unsafe extern "C" fn(*const ExtractContext) -> bool,
    spill_enter: unsafe extern "C" fn(*mut ExtractContext, *const c_char),
    spill_sweep: unsafe extern "C" fn(*mut ExtractContext, c_int) -> c_int,
    spill_work_remains: unsafe extern "C" fn(*const ExtractContext) -> bool,
    begin_cycle: unsafe extern "C" fn(),
    nap: unsafe extern "C" fn(u32),
    confirm_futile: unsafe extern "C" fn(),
    fail_over_budget: unsafe extern "C" fn(*mut ExtractContext),
}

struct Schedule<'a> {
    tasks: &'a [Task],
    ops: &'a Operations,
    context: *mut ExtractContext,
    next: AtomicUsize,
    futile: AtomicBool,
}

impl Schedule<'_> {
    fn work(&self, worker: c_int) {
        // SAFETY: Native dispatch owns this worker index until the callback
        // returns. It lends distinct per-worker arenas and synchronizes its
        // shared result cache, spill store, and graph state.
        unsafe { (self.ops.start)(self.context, worker) };
        loop {
            let position = self.next.fetch_add(1, Relaxed);
            let Some(task) = self.tasks.get(position) else {
                break;
            };
            let position = position as c_int;
            unsafe { (self.ops.claimed)(self.context, position) };
            if self.stopping() {
                break;
            }
            self.admit(worker);
            // A cancellation raised during this pull takes effect on the next
            // claim. A budget failure stops admission immediately.
            if unsafe { (self.ops.aborted)(self.context) } {
                break;
            }
            unsafe {
                (self.ops.extract_file)(
                    self.context,
                    worker,
                    task.index,
                    position,
                )
            };
        }
        unsafe { (self.ops.finish)() };
    }

    fn stopping(&self) -> bool {
        unsafe {
            (self.ops.cancelled)(self.context)
                || (self.ops.aborted)(self.context)
        }
    }

    fn admit(&self, worker: c_int) {
        let ops = self.ops;
        let context = self.context;
        if unsafe { (ops.budget)() } == 0 {
            return;
        }

        let mut over = unsafe { (ops.over_budget)() };
        unsafe { (ops.charge_probe)(context, over) };
        // Enter spill before in-flight extraction pushes the charge over the
        // cap. System pressure can trigger relief but cannot fail the run.
        if !over && !unsafe { (ops.spill_active)(context) } {
            let line = cbm_rust_extract_spill_line(unsafe { (ops.budget)() });
            if unsafe { (ops.charged)() } > line {
                unsafe { (ops.spill_enter)(context, c"near_budget".as_ptr()) };
            }
        }
        if !unsafe { (ops.spill_active)(context) }
            && unsafe { (ops.charged)() > (ops.budget)() / 2 }
            && unsafe { (ops.system_under_pressure)() }
        {
            unsafe {
                (ops.spill_enter)(context, c"system_pressure".as_ptr());
                (ops.spill_sweep)(context, worker);
            }
        }

        let mut settling = false;
        if over {
            // Let the store drain cached results before deciding the
            // resident floor makes reclaiming memory futile.
            let parked = unsafe {
                (ops.spill_enter)(context, c"over_budget".as_ptr());
                (ops.spill_sweep)(context, worker)
            };
            over = unsafe { (ops.over_budget)() };
            if over
                && unsafe { (ops.spill_active)(context) }
                && (parked > 0 || unsafe { (ops.spill_work_remains)(context) })
            {
                over = false;
                settling = true;
            }
        }

        let futile = self.futile.load(Relaxed);
        if over && !futile {
            if self.cycle_still_over() && !self.futile.swap(true, Relaxed) {
                // The worker that wins the transition runs the confirmation.
                unsafe { (ops.confirm_futile)() };
                if self.cycle_still_over() {
                    unsafe { (ops.fail_over_budget)(context) };
                } else {
                    self.futile.store(false, Relaxed);
                }
            }
        } else if !over && !settling && futile {
            // Re-arm only on a real under-budget reading.
            self.futile.store(false, Relaxed);
        }
    }

    fn cycle_still_over(&self) -> bool {
        unsafe { (self.ops.begin_cycle)() };
        for _ in 0..BACKPRESSURE_SPINS {
            if !unsafe { (self.ops.over_budget)() } || self.stopping() {
                return false;
            }
            unsafe { (self.ops.nap)(BACKPRESSURE_NAP_NS) };
        }
        unsafe { (self.ops.over_budget)() }
    }
}

unsafe extern "C" fn worker(index: c_int, schedule: *mut c_void) {
    // SAFETY: The native pool invokes each index once and joins all callbacks
    // before dispatch returns. The queue and shared latch are atomic and the
    // task/operation tables are immutable for the full dispatch.
    unsafe { &*schedule.cast::<Schedule<'_>>() }.work(index);
}

#[cfg_attr(feature = "native-engine-abi", unsafe(no_mangle))]
unsafe extern "C" fn cbm_rust_extract_sort_tasks(
    tasks: *mut Task,
    count: c_int,
) {
    if count > 0 {
        // SAFETY: The coordinator owns count initialized rows until it
        // dispatches workers and gives Rust the only mutable borrow.
        let tasks = unsafe { slice::from_raw_parts_mut(tasks, count as usize) };
        tasks.sort_unstable_by(|a, b| {
            b.size.cmp(&a.size).then_with(|| a.index.cmp(&b.index))
        });
    }
}

#[cfg_attr(feature = "native-engine-abi", unsafe(no_mangle))]
unsafe extern "C" fn cbm_rust_extract_dispatch(
    tasks: *const Task,
    count: c_int,
    workers: c_int,
    context: *mut ExtractContext,
    ops: *const Operations,
) {
    if count <= 0 {
        return;
    }
    // SAFETY: The coordinator holds both borrow sources for the synchronous
    // call. No callback retains the Rust schedule or its rows.
    let (tasks, ops) =
        unsafe { (slice::from_raw_parts(tasks, count as usize), &*ops) };
    let schedule = Schedule {
        tasks,
        ops,
        context,
        next: AtomicUsize::new(0),
        futile: AtomicBool::new(false),
    };
    unsafe {
        (ops.dispatch)(
            workers,
            worker,
            ptr::from_ref(&schedule).cast_mut().cast(),
        )
    };
}

#[cfg_attr(feature = "native-engine-abi", unsafe(no_mangle))]
extern "C" fn cbm_rust_extract_spill_line(budget: usize) -> usize {
    budget - budget / SPILL_EARLY_DIVISOR
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::ffi::CStr;

    #[derive(Default)]
    struct Trace {
        started: Vec<c_int>,
        claimed: Vec<c_int>,
        files: Vec<(c_int, c_int, c_int)>,
        spills: Vec<&'static CStr>,
        aborted: bool,
    }

    thread_local! {
        static FINISHED: Cell<usize> = const { Cell::new(0) };
        static NAPS: Cell<usize> = const { Cell::new(0) };
        static CONFIRMED: Cell<usize> = const { Cell::new(0) };
    }

    unsafe extern "C" fn dispatch(
        count: c_int,
        worker: Worker,
        schedule: *mut c_void,
    ) {
        for index in 0..count {
            unsafe { worker(index, schedule) };
        }
    }

    unsafe extern "C" fn start(ctx: *mut ExtractContext, worker: c_int) {
        unsafe { &mut *ctx.cast::<Trace>() }.started.push(worker);
    }

    unsafe extern "C" fn finish() {
        FINISHED.set(FINISHED.get() + 1);
    }

    unsafe extern "C" fn claimed(ctx: *mut ExtractContext, position: c_int) {
        unsafe { &mut *ctx.cast::<Trace>() }.claimed.push(position);
    }

    unsafe extern "C" fn file(
        ctx: *mut ExtractContext,
        worker: c_int,
        file: c_int,
        pos: c_int,
    ) {
        unsafe { &mut *ctx.cast::<Trace>() }
            .files
            .push((worker, file, pos));
    }

    unsafe extern "C" fn never(_: *const ExtractContext) -> bool {
        false
    }
    unsafe extern "C" fn aborted(ctx: *const ExtractContext) -> bool {
        unsafe { (*ctx.cast::<Trace>()).aborted }
    }
    unsafe extern "C" fn cancel_after_nap(_: *const ExtractContext) -> bool {
        NAPS.get() != 0
    }
    unsafe extern "C" fn budget_disabled() -> usize {
        0
    }
    unsafe extern "C" fn budget() -> usize {
        160
    }
    unsafe extern "C" fn under() -> bool {
        false
    }
    unsafe extern "C" fn over() -> bool {
        true
    }
    unsafe extern "C" fn charge_probe(_: *mut ExtractContext, _: bool) {}
    unsafe extern "C" fn spill_enter(
        ctx: *mut ExtractContext,
        reason: *const c_char,
    ) {
        // The scheduler only passes static reason labels.
        let reason = unsafe { CStr::from_ptr(reason) };
        unsafe { &mut *ctx.cast::<Trace>() }.spills.push(reason);
    }
    unsafe extern "C" fn sweep(_: *mut ExtractContext, _: c_int) -> c_int {
        0
    }
    unsafe extern "C" fn begin_cycle() {}
    unsafe extern "C" fn nap(nanoseconds: u32) {
        assert_eq!(nanoseconds, 3_000_000);
        NAPS.set(NAPS.get() + 1);
    }
    unsafe extern "C" fn confirm() {
        CONFIRMED.set(CONFIRMED.get() + 1);
    }
    unsafe extern "C" fn fail(ctx: *mut ExtractContext) {
        unsafe { (*ctx.cast::<Trace>()).aborted = true };
    }

    const MOCK_OPS: Operations = Operations {
        dispatch,
        start,
        finish,
        claimed,
        extract_file: file,
        cancelled: never,
        aborted,
        budget: budget_disabled,
        charged: budget_disabled,
        over_budget: under,
        system_under_pressure: under,
        charge_probe,
        spill_active: never,
        spill_enter,
        spill_sweep: sweep,
        spill_work_remains: never,
        begin_cycle,
        nap,
        confirm_futile: confirm,
        fail_over_budget: fail,
    };

    fn run(tasks: &[Task], workers: c_int, ops: &Operations) -> Trace {
        FINISHED.set(0);
        NAPS.set(0);
        CONFIRMED.set(0);
        let mut trace = Trace::default();
        unsafe {
            cbm_rust_extract_dispatch(
                tasks.as_ptr(),
                tasks.len() as c_int,
                workers,
                (&raw mut trace).cast(),
                ops,
            )
        };
        trace
    }

    #[test]
    fn large_files_go_first_and_ties_keep_discovery_order() {
        let mut tasks = [
            Task { index: 0, size: 1 },
            Task { index: 1, size: 20 },
            Task { index: 2, size: 3 },
            Task { index: 3, size: 20 },
        ];
        unsafe { cbm_rust_extract_sort_tasks(tasks.as_mut_ptr(), 4) };
        let trace = run(&tasks, 2, &MOCK_OPS);
        assert_eq!(trace.claimed, [0, 1, 2, 3]);
        assert_eq!(trace.files, [(0, 1, 0), (0, 3, 1), (0, 2, 2), (0, 0, 3)]);
        assert_eq!(trace.started, [0, 1]);
        assert_eq!(FINISHED.get(), 2);
        assert!(trace.spills.is_empty());
    }

    #[test]
    fn an_active_spill_allows_new_work_while_memory_drains() {
        unsafe extern "C" fn active(_: *const ExtractContext) -> bool {
            true
        }
        unsafe extern "C" fn parked(_: *mut ExtractContext, _: c_int) -> c_int {
            1
        }
        let ops = Operations {
            budget,
            over_budget: over,
            spill_active: active,
            spill_sweep: parked,
            ..MOCK_OPS
        };
        let trace = run(&[Task { index: 0, size: 20 }], 1, &ops);
        assert!(!trace.aborted);
        assert_eq!(trace.files, [(0, 0, 0)]);
        assert_eq!(trace.spills, [c"over_budget"]);
        assert_eq!(NAPS.get(), 0);
    }

    #[test]
    fn full_confirmation_aborts_before_parsing() {
        let ops = Operations {
            budget,
            over_budget: over,
            ..MOCK_OPS
        };
        let trace = run(&[Task { index: 0, size: 20 }], 1, &ops);
        assert!(trace.aborted);
        assert!(trace.files.is_empty());
        assert_eq!(trace.spills, [c"over_budget"]);
        assert_eq!(CONFIRMED.get(), 1);
        assert_eq!(NAPS.get(), 80);
        assert_eq!(FINISHED.get(), 1);
        assert_eq!(cbm_rust_extract_spill_line(160), 150);
    }

    #[test]
    fn cancellation_during_nap_stops_the_next_claim_without_budget_failure() {
        let ops = Operations {
            budget,
            over_budget: over,
            cancelled: cancel_after_nap,
            ..MOCK_OPS
        };
        let trace = run(
            &[Task { index: 0, size: 2 }, Task { index: 1, size: 1 }],
            1,
            &ops,
        );
        assert!(!trace.aborted);
        assert_eq!(trace.claimed, [0, 1]);
        assert_eq!(trace.files, [(0, 0, 0)]);
        assert_eq!(CONFIRMED.get(), 0);
        assert_eq!(NAPS.get(), 1);
        assert_eq!(FINISHED.get(), 1);
    }
}
