use super::*;
use std::cell::{Cell, RefCell};

struct FakeWatch {
    flags: Cell<WatchFlags>,
    root: Cell<Root>,
    clock_ns: Cell<i64>,
    git: Cell<Option<bool>>,
    head: Cell<Probe<Head>>,
    dirty: Cell<Probe<u64>>,
    count: Cell<Probe<i32>>,
    tree: Cell<Option<(u64, i32)>>,
    index_result: Cell<Option<i32>>,
    index_time_ms: Cell<i64>,
    dirty_during_index: Cell<Option<u64>>,
    detach_on_publish: Cell<bool>,
    scans: RefCell<Vec<(Strategy, i32, bool)>>,
    events: RefCell<Vec<Event>>,
    indexes: Cell<i32>,
    prunes: Cell<i32>,
    grace_ms: u64,
}

impl FakeWatch {
    fn new(git: bool, named: bool, poll_non_git: bool) -> Self {
        Self {
            flags: Cell::new(WatchFlags {
                registered: true,
                named,
                catch_up: named,
                poll_non_git,
            }),
            root: Cell::new(Root::Present),
            clock_ns: Cell::new(0),
            git: Cell::new(Some(git)),
            head: Cell::new(Probe::Observed(head(b'a'))),
            dirty: Cell::new(Probe::Observed(0)),
            count: Cell::new(Probe::Observed(500)),
            tree: Cell::new(Some((1, 200))),
            index_result: Cell::new(Some(0)),
            index_time_ms: Cell::new(0),
            dirty_during_index: Cell::new(None),
            detach_on_publish: Cell::new(false),
            scans: RefCell::new(Vec::new()),
            events: RefCell::new(Vec::new()),
            indexes: Cell::new(0),
            prunes: Cell::new(0),
            grace_ms: 10_000,
        }
    }

    fn poll_at_ms(&self, state: &PollState, ms: i64) -> bool {
        self.clock_ns.set(ms * NS_PER_MS);
        state.poll(self, self.now_ns())
    }

    fn poll_when_due(&self, state: &PollState) -> bool {
        self.poll_at_ms(state, state.next_poll_ns.load(Relaxed) / NS_PER_MS)
    }
}

fn head(byte: u8) -> Head {
    let mut head = [0; 128];
    head[..64].fill(byte);
    head
}

impl Watch for FakeWatch {
    fn flags(&self) -> WatchFlags {
        self.flags.get()
    }

    fn now_ns(&self) -> i64 {
        self.clock_ns.get()
    }

    fn now_ms(&self) -> u64 {
        (self.now_ns() / NS_PER_MS) as u64
    }

    fn root(&self) -> Root {
        self.root.get()
    }

    fn git_repository(&self) -> Option<bool> {
        self.git.get()
    }

    fn head(&self) -> Probe<Head> {
        self.head.get()
    }

    fn dirty_signature(&self) -> Probe<u64> {
        self.dirty.get()
    }

    fn git_file_count(&self) -> Probe<i32> {
        self.count.get()
    }

    fn tree(&self) -> Option<(u64, i32)> {
        self.tree.get()
    }

    fn publish(&self, strategy: Strategy, interval_ms: i32, scanned: bool) {
        self.scans
            .borrow_mut()
            .push((strategy, interval_ms, scanned));
        if self.detach_on_publish.get() {
            self.flags.set(WatchFlags {
                registered: false,
                ..self.flags.get()
            });
        }
    }

    fn log(&self, event: Event) {
        self.events.borrow_mut().push(event);
    }

    fn set_catch_up(&self, needed: bool) {
        self.flags.set(WatchFlags {
            catch_up: needed,
            ..self.flags.get()
        });
    }

    fn index(&self, _: Strategy) -> Option<i32> {
        self.indexes.set(self.indexes.get() + 1);
        self.clock_ns
            .set(self.now_ns() + self.index_time_ms.get() * NS_PER_MS);
        if let Some(dirty) = self.dirty_during_index.take() {
            self.dirty.set(Probe::Observed(dirty));
        }
        self.index_result.get()
    }

    fn prune_grace_ms(&self) -> u64 {
        self.grace_ms
    }

    fn prune(&self, _: u64) {
        self.prunes.set(self.prunes.get() + 1);
    }
}

#[test]
fn adaptive_cadence_matches_the_reference_bounds() {
    assert_eq!(poll_interval_ms(-500), 4_000);
    assert_eq!(poll_interval_ms(499), 5_000);
    assert_eq!(poll_interval_ms(500), 6_000);
    assert_eq!(poll_interval_ms(i32::MAX), 60_000);
    assert_eq!(index_backoff_ms(-1, i32::MAX), 0);
    assert_eq!(index_backoff_ms(100, 32), 6_400);
    assert_eq!(index_backoff_ms(5_000, 32), 300_000);
    assert_eq!(index_backoff_ms(i32::MAX, 32), i32::MAX);
    assert_eq!(success_delay_ms(-1, 0), 0);
    assert_eq!(success_delay_ms(5_000, 3000), 9_000);
    assert_eq!(success_delay_ms(5_000, i64::MAX), 300_000);
}

#[test]
fn git_commits_only_the_pre_index_observation() {
    let watch = FakeWatch::new(true, false, false);
    let state = PollState::new();
    watch.dirty.set(Probe::Observed(1));
    assert!(!watch.poll_at_ms(&state, 0));
    assert!(!watch.poll_at_ms(&state, 5_999));

    watch.dirty_during_index.set(Some(2));
    assert!(watch.poll_when_due(&state));
    assert_eq!(state.data.lock().unwrap().last_dirty, 1);
    assert!(watch.poll_when_due(&state));
    assert_eq!(state.data.lock().unwrap().last_dirty, 2);
    assert!(!watch.poll_when_due(&state));

    watch.dirty.set(Probe::Observed(0));
    assert!(watch.poll_when_due(&state));
    watch.head.set(Probe::Observed(head(b'b')));
    assert!(watch.poll_when_due(&state));
    assert!(!watch.poll_when_due(&state));
    assert_eq!(watch.indexes.get(), 4);
}

#[test]
fn busy_preserves_failure_streak_and_success_anchors_cooldown() {
    let watch = FakeWatch::new(true, false, false);
    let state = PollState::new();
    assert!(!watch.poll_at_ms(&state, 0));
    watch.dirty.set(Probe::Observed(42));
    watch.index_result.set(Some(-3));
    assert!(!watch.poll_when_due(&state));
    assert_eq!(state.failures.load(Relaxed), 1);
    assert_eq!(state.next_poll_ns.load(Relaxed) / NS_PER_MS, 18_000);
    assert_eq!(state.data.lock().unwrap().pending_dirty, 42);
    assert_eq!(state.data.lock().unwrap().last_dirty, 0);

    watch.index_result.set(Some(1));
    assert!(!watch.poll_at_ms(&state, 17_999));
    assert_eq!(watch.indexes.get(), 1);
    assert!(!watch.poll_when_due(&state));
    assert_eq!(state.failures.load(Relaxed), 1);
    assert_eq!(state.next_poll_ns.load(Relaxed) / NS_PER_MS, 30_000);

    watch.index_result.set(Some(0));
    watch.index_time_ms.set(4_000);
    state.touch();
    assert!(watch.poll_at_ms(&state, 19_000));
    assert_eq!(state.failures.load(Relaxed), 0);
    assert_eq!(state.data.lock().unwrap().last_dirty, 42);
    assert_eq!(state.next_poll_ns.load(Relaxed) / NS_PER_MS, 35_000);
    assert!(watch.events.borrow().contains(&Event::Cooldown(12_000)));
}

#[test]
fn uncertain_git_probes_preserve_the_last_indexed_state() {
    let watch = FakeWatch::new(true, true, false);
    let state = PollState::new();
    watch.head.set(Probe::Uncertain);
    assert!(!watch.poll_at_ms(&state, 0));
    assert!(watch.scans.borrow().is_empty());
    watch.head.set(Probe::CommandFailed);
    assert!(!watch.poll_at_ms(&state, 3));
    watch.dirty.set(Probe::Uncertain);
    assert!(!watch.poll_when_due(&state));
    assert!(watch.flags().catch_up);

    watch.dirty.set(Probe::Observed(8));
    assert!(watch.poll_when_due(&state));
    watch.head.set(Probe::Observed(head(b'b')));
    assert!(!watch.poll_when_due(&state)); // First HEAD is baseline.
    watch.head.set(Probe::Observed(head(b'c')));
    watch.dirty.set(Probe::CommandFailed);
    assert!(!watch.poll_when_due(&state)); // A failed status probe masks HEAD movement.
    assert_eq!(state.data.lock().unwrap().last_dirty, 8);
    watch.dirty.set(Probe::Observed(8));
    assert!(watch.poll_when_due(&state));
    assert_eq!(watch.indexes.get(), 2);
}

#[test]
fn tree_polling_retries_failed_walks_and_respects_config() {
    let watch = FakeWatch::new(false, false, true);
    let state = PollState::new();
    assert!(!watch.poll_at_ms(&state, 0));
    watch.tree.set(None);
    assert!(!watch.poll_when_due(&state));
    assert_eq!(state.next_poll_ns.load(Relaxed) / NS_PER_MS, 10_000);
    watch.tree.set(Some((2, 1_500)));
    assert!(watch.poll_when_due(&state));
    assert_eq!(state.next_poll_ns.load(Relaxed) / NS_PER_MS, 18_000);
    assert!(!watch.poll_when_due(&state));
    watch.flags.set(WatchFlags {
        poll_non_git: false,
        ..watch.flags()
    });
    watch.tree.set(Some((3, 1_500)));
    assert!(!watch.poll_when_due(&state));
    assert_eq!(watch.indexes.get(), 1);
}

#[test]
fn named_checkout_recovers_and_catches_up_after_restoration() {
    let watch = FakeWatch::new(false, false, false);
    let state = PollState::new();
    assert!(!watch.poll_at_ms(&state, 0));
    assert!(!watch.poll_when_due(&state));
    assert_eq!(
        watch.scans.borrow().as_slice(),
        &[(Strategy::None, 5_000, true)]
    );
    watch.flags.set(WatchFlags {
        named: true,
        catch_up: true,
        ..watch.flags()
    });
    assert!(!watch.poll_at_ms(&state, 5_001));
    assert!(watch.poll_when_due(&state));
    assert!(!watch.flags().catch_up);

    watch.root.set(Root::Missing);
    for ms in [20_000, 30_000, 40_000] {
        assert!(!watch.poll_at_ms(&state, ms));
    }
    assert_eq!(watch.prunes.get(), 0);
    assert_eq!(
        watch
            .events
            .borrow()
            .iter()
            .filter(|event| **event == Event::RootMissing(0))
            .count(),
        1
    );
    watch.root.set(Root::Present);
    assert!(!watch.poll_at_ms(&state, 50_000));
    assert!(watch.flags().catch_up);
    assert!(watch.poll_when_due(&state));
    assert!(!watch.poll_when_due(&state));
}

#[test]
fn uncertain_and_restored_roots_restart_prune_grace() {
    let watch = FakeWatch::new(false, false, false);
    let state = PollState::new();
    watch.root.set(Root::Missing);
    for ms in [0, 1_000, 2_000] {
        assert!(!watch.poll_at_ms(&state, ms));
    }
    assert_eq!(watch.prunes.get(), 0);
    watch.root.set(Root::Uncertain(5));
    assert!(!watch.poll_at_ms(&state, 15_000));
    watch.root.set(Root::Missing);
    for ms in [20_000, 25_000, 29_000] {
        assert!(!watch.poll_at_ms(&state, ms));
    }
    assert_eq!(watch.prunes.get(), 0);
    assert!(!watch.poll_at_ms(&state, 30_000));
    assert_eq!(watch.prunes.get(), 1);
    watch.root.set(Root::Present);
    assert!(!watch.poll_at_ms(&state, 35_000));
    watch.root.set(Root::Missing);
    assert!(!watch.poll_at_ms(&state, 50_000));
    assert_eq!(watch.prunes.get(), 1);
}

#[test]
fn detached_projects_skip_index_admission_after_scanning() {
    let watch = FakeWatch::new(true, true, false);
    let state = PollState::new();
    assert!(!watch.poll_at_ms(&state, 0));
    watch.detach_on_publish.set(true);
    assert!(!watch.poll_when_due(&state));
    assert_eq!(watch.indexes.get(), 0);
    assert!(watch.flags().catch_up);
}

#[test]
fn clean_named_git_checkout_retries_catch_up_until_success() {
    let watch = FakeWatch::new(true, true, false);
    let state = PollState::new();
    assert!(!watch.poll_at_ms(&state, 0));
    watch.index_result.set(Some(1));
    assert!(!watch.poll_when_due(&state));
    assert!(watch.flags().catch_up);
    watch.index_result.set(Some(0));
    assert!(watch.poll_when_due(&state));
    assert!(!watch.flags().catch_up);
    assert!(!watch.poll_when_due(&state));
    assert_eq!(watch.indexes.get(), 2);
}

#[test]
fn sustained_failure_emits_once_until_the_streak_resets() {
    let watch = FakeWatch::new(true, false, false);
    let state = PollState::new();
    assert!(!watch.poll_at_ms(&state, 0));
    watch.dirty.set(Probe::Observed(1));
    watch.index_result.set(Some(-1));
    for _ in 0..12 {
        assert!(!watch.poll_when_due(&state));
    }
    assert_eq!(state.failures.load(Relaxed), 12);
    assert_eq!(
        watch
            .events
            .borrow()
            .iter()
            .filter(|event| **event == Event::SustainedFailure(10))
            .count(),
        1
    );
    assert_eq!(
        state.next_poll_ns.load(Relaxed) - watch.now_ns(),
        300_000 * NS_PER_MS
    );
}
