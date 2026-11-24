//! Poll policy for the daemon's physical watches. The host owns registration
//! and supplies filesystem probes, index admission, and coordinated pruning.

use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, AtomicI64, Ordering::Relaxed};

#[cfg(feature = "native-engine-abi")]
mod native;

const BASE_INTERVAL_MS: i32 = 5_000;
const MAX_INTERVAL_MS: i32 = 60_000;
const MAX_BACKOFF_MS: i64 = 300_000;
const SUSTAINED_FAILURES: i32 = 10;
const MISSING_POLLS: i32 = 3;
const NS_PER_MS: i64 = 1_000_000;

fn poll_interval_ms(file_count: i32) -> i32 {
    let interval =
        i64::from(BASE_INTERVAL_MS) + i64::from(file_count / 500) * 1_000;
    interval.clamp(i64::from(i32::MIN), i64::from(MAX_INTERVAL_MS)) as i32
}

fn index_backoff_ms(interval_ms: i32, failures: i32) -> i32 {
    let interval = i64::from(interval_ms.max(0));
    let delay = interval << failures.clamp(0, 6);
    delay.min(MAX_BACKOFF_MS).max(interval) as i32
}

fn success_delay_ms(interval_ms: i32, elapsed_ms: i64) -> i32 {
    let cooldown = elapsed_ms.saturating_mul(3).clamp(0, MAX_BACKOFF_MS);
    cooldown.max(i64::from(interval_ms)) as i32
}

// Git's SHA-256 ID also fits. Compare the bytes returned by the C probe up
// to its terminator, without imposing an encoding on the probe.
type Head = [u8; 128];

#[derive(Clone, Copy)]
enum Probe<T> {
    Observed(T),
    CommandFailed,
    Uncertain,
}

#[derive(Clone, Copy)]
enum Root {
    Present,
    Missing,
    Uncertain(i32),
}

#[repr(i32)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Strategy {
    #[default]
    Pending = 0,
    Git = 1,
    None = 2,
    Tree = 3,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct WatchFlags {
    registered: bool,
    named: bool,
    catch_up: bool,
    poll_non_git: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Event {
    RootMissing(i32),
    RootRestored,
    RootStatError(i32),
    TreeWalkFailed,
    Baseline(Strategy, i32),
    IndexRetry,
    IndexError { result: i32, streak: i32 },
    SustainedFailure(i32),
    Cooldown(i32),
}

trait Watch {
    fn flags(&self) -> WatchFlags;
    fn now_ns(&self) -> i64;
    fn now_ms(&self) -> u64;
    fn root(&self) -> Root;
    // Also prepares the path from this watch's root to the Git root.
    fn git_repository(&self) -> Option<bool>;
    fn head(&self) -> Probe<Head>;
    fn dirty_signature(&self) -> Probe<u64>;
    fn git_file_count(&self) -> Probe<i32>;
    fn tree(&self) -> Option<(u64, i32)>;
    fn publish(&self, strategy: Strategy, interval_ms: i32, scanned: bool);
    fn log(&self, event: Event);
    fn set_catch_up(&self, needed: bool);
    fn index(&self, strategy: Strategy) -> Option<i32>;
    fn prune_grace_ms(&self) -> u64;
    fn prune(&self, first_missing_ms: u64);
}

struct PollState {
    // The host serializes poll snapshots. Keep diagnostics and touches off
    // this mutex so callbacks can re-enter the registry during a poll.
    data: Mutex<PollData>,
    next_poll_ns: AtomicI64,
    failures: AtomicI32,
}

struct PollData {
    strategy: Strategy,
    interval_ms: i32,
    head: Option<Head>,
    last_dirty: u64,
    last_tree: u64,
    pending_dirty: u64,
    missing_since_ms: Option<u64>,
    missing_polls: i32,
}

struct Observation {
    head: Option<Head>,
    signature: Option<u64>,
    changed: bool,
}

impl PollData {
    fn initialize(&mut self, watch: &impl Watch) -> bool {
        let Some(is_git) = watch.git_repository() else {
            return false;
        };
        let flags = watch.flags();
        let mut file_count = 0;
        if is_git {
            let head = match watch.head() {
                Probe::Observed(head) => Some(head),
                Probe::CommandFailed => None,
                Probe::Uncertain => return false,
            };
            match watch.git_file_count() {
                Probe::Observed(count) => file_count = count,
                Probe::CommandFailed => (),
                Probe::Uncertain => return false,
            }
            self.head = head;
            self.interval_ms = poll_interval_ms(file_count);
            self.strategy = Strategy::Git;
        } else if flags.named || flags.poll_non_git {
            self.strategy = Strategy::Tree;
        } else {
            self.strategy = Strategy::None;
        }
        // A dirty checkout or a tree with no prior signature still needs
        // its first index after baseline initialization.
        watch.log(Event::Baseline(self.strategy, file_count));
        true
    }

    fn observe_git(&mut self, watch: &impl Watch) -> Option<Observation> {
        let head = match watch.head() {
            Probe::Observed(head) => Some(head),
            Probe::CommandFailed => None,
            Probe::Uncertain => return None,
        };
        let head_changed = match (self.head, head) {
            (Some(old), Some(new)) => old != new,
            (None, Some(_)) => {
                // Adopt the first available HEAD, including in a formerly
                // unborn checkout. The dirty signature still gates indexing.
                self.head = head;
                false
            }
            _ => false,
        };
        match watch.dirty_signature() {
            Probe::Observed(signature) => {
                self.pending_dirty = signature;
                Some(Observation {
                    head,
                    signature: Some(signature),
                    changed: head_changed || self.last_dirty != signature,
                })
            }
            Probe::CommandFailed => {
                // Preserve the legacy named catch-up attempt on a failed
                // status command. There is no dirty observation to commit.
                Some(Observation {
                    head,
                    signature: None,
                    changed: false,
                })
            }
            Probe::Uncertain => None,
        }
    }

    fn observe_tree(&mut self, watch: &impl Watch) -> Option<Observation> {
        let (signature, count) = watch.tree()?;
        self.interval_ms = poll_interval_ms(count);
        Some(Observation {
            head: None,
            signature: Some(signature),
            changed: self.last_tree != signature,
        })
    }
}

impl PollState {
    fn new() -> Self {
        Self {
            data: Mutex::new(PollData {
                strategy: Strategy::Pending,
                interval_ms: BASE_INTERVAL_MS,
                head: None,
                last_dirty: 0,
                last_tree: 0,
                pending_dirty: 0,
                missing_since_ms: None,
                missing_polls: 0,
            }),
            next_poll_ns: AtomicI64::new(0),
            failures: AtomicI32::new(0),
        }
    }

    fn touch(&self) {
        self.next_poll_ns.store(0, Relaxed);
    }

    fn schedule(&self, anchor_ns: i64, delay_ms: i32) {
        let delay_ns = i64::from(delay_ms) * NS_PER_MS;
        self.next_poll_ns
            .store(anchor_ns.saturating_add(delay_ns), Relaxed);
    }

    fn poll(&self, watch: &impl Watch, cycle_ns: i64) -> bool {
        if !watch.flags().registered {
            return false;
        }
        let mut data = self.data.lock().unwrap();
        // Root checks run even during index backoff and for disabled trees.
        match watch.root() {
            Root::Uncertain(errno) => {
                data.missing_polls = 0;
                data.missing_since_ms = None;
                watch.log(Event::RootStatError(errno));
                return false;
            }
            Root::Missing => {
                let now_ms = watch.now_ms();
                let first = *data.missing_since_ms.get_or_insert(now_ms);
                if watch.flags().named {
                    if data.missing_polls == 0 {
                        watch.log(Event::RootMissing(0));
                    }
                    data.missing_polls = 1;
                    return false;
                }
                data.missing_polls = data.missing_polls.saturating_add(1);
                watch.log(Event::RootMissing(data.missing_polls));
                if data.missing_polls >= MISSING_POLLS
                    && now_ms.saturating_sub(first) >= watch.prune_grace_ms()
                {
                    watch.prune(first);
                }
                return false;
            }
            Root::Present => {
                if data.missing_since_ms.take().is_some() {
                    data.missing_polls = 0;
                    watch.log(Event::RootRestored);
                    if watch.flags().named {
                        data.strategy = Strategy::Pending;
                        watch.set_catch_up(true);
                    }
                }
            }
        }

        let flags = watch.flags();
        if data.strategy == Strategy::Pending
            || (data.strategy == Strategy::None && flags.named)
        {
            if data.initialize(watch) {
                self.schedule(watch.now_ns(), data.interval_ms);
                watch.publish(data.strategy, data.interval_ms, true);
            }
            return false;
        }
        if cycle_ns < self.next_poll_ns.load(Relaxed) {
            return false;
        }
        let observation = match data.strategy {
            Strategy::Git => data.observe_git(watch),
            Strategy::Tree if flags.named || flags.poll_non_git => {
                let observation = data.observe_tree(watch);
                if observation.is_none() {
                    watch.log(Event::TreeWalkFailed);
                    self.schedule(watch.now_ns(), data.interval_ms);
                }
                observation
            }
            _ => None,
        };
        let Some(observation) = observation else {
            return false;
        };
        watch.publish(data.strategy, data.interval_ms, true);

        let flags = watch.flags();
        if !observation.changed && !flags.catch_up {
            self.schedule(cycle_ns, data.interval_ms);
            return false;
        }
        if !flags.registered {
            return false;
        }
        let index_start = watch.now_ns();
        let mut failures = self.failures.load(Relaxed);
        match watch.index(data.strategy) {
            Some(0) => {
                self.failures.store(0, Relaxed);
                if flags.catch_up {
                    watch.set_catch_up(false);
                }
                // Commit only the observations that preceded the index.
                // Changes arriving during the callback remain visible.
                if let Some(head) = observation.head {
                    data.head = Some(head);
                }
                if let Some(signature) = observation.signature {
                    if data.strategy == Strategy::Git {
                        data.last_dirty = signature;
                    } else {
                        data.last_tree = signature;
                    }
                }
                if data.strategy == Strategy::Git
                    && let Probe::Observed(count) = watch.git_file_count()
                {
                    data.interval_ms = poll_interval_ms(count);
                    watch.publish(data.strategy, data.interval_ms, false);
                }
                let finished = watch.now_ns();
                let elapsed_ms =
                    finished.saturating_sub(index_start) / NS_PER_MS;
                let delay_ms = success_delay_ms(data.interval_ms, elapsed_ms);
                if delay_ms > data.interval_ms {
                    watch.log(Event::Cooldown(delay_ms));
                }
                self.schedule(finished, delay_ms);
                return true;
            }
            Some(result) if result < 0 => {
                failures = failures.saturating_add(1);
                self.failures.store(failures, Relaxed);
                watch.log(Event::IndexError {
                    result,
                    streak: failures,
                });
                if failures == SUSTAINED_FAILURES {
                    watch.log(Event::SustainedFailure(failures));
                }
            }
            Some(_) => watch.log(Event::IndexRetry),
            None => (),
        }
        self.schedule(cycle_ns, index_backoff_ms(data.interval_ms, failures));
        false
    }
}

#[cfg(test)]
mod tests;
