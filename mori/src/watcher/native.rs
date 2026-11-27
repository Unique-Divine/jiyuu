//! Private ABI for the watcher host. Names and paths stay in C.

use super::*;
use std::alloc::{Layout, alloc};
use std::ffi::c_void;
use std::ptr;

unsafe extern "C" {
    fn cbm_watcher_host_flags(host: *mut c_void) -> WatchFlags;
    fn cbm_watcher_host_now_ns() -> i64;
    fn cbm_watcher_host_now_ms() -> u64;
    fn cbm_watcher_host_root(host: *mut c_void, errno: *mut i32) -> i32;
    fn cbm_watcher_host_git_repository(host: *mut c_void) -> i32;
    fn cbm_watcher_host_head(
        host: *mut c_void,
        head: *mut u8,
        size: usize,
    ) -> i32;
    fn cbm_watcher_host_dirty_signature(
        host: *mut c_void,
        signature: *mut u64,
    ) -> i32;
    fn cbm_watcher_host_git_file_count(
        host: *mut c_void,
        count: *mut i32,
    ) -> i32;
    fn cbm_watcher_host_tree(
        host: *mut c_void,
        signature: *mut u64,
        count: *mut i32,
    ) -> bool;
    fn cbm_watcher_host_publish(
        host: *mut c_void,
        strategy: i32,
        interval_ms: i32,
        scanned: bool,
    );
    fn cbm_watcher_host_log(
        host: *mut c_void,
        event: i32,
        value: i32,
        extra: i32,
    );
    fn cbm_watcher_host_set_catch_up(host: *mut c_void, needed: bool);
    fn cbm_watcher_host_index(
        host: *mut c_void,
        strategy: i32,
        result: *mut i32,
    ) -> bool;
    fn cbm_watcher_host_prune_grace_s() -> u64;
    fn cbm_watcher_host_prune(host: *mut c_void, first_missing_ms: u64);
}

// The caller pins the host and project for the entire synchronous poll.
struct Host(*mut c_void);

fn probe<T>(status: i32, value: T) -> Probe<T> {
    match status {
        0 => Probe::Observed(value),
        1 => Probe::CommandFailed,
        _ => Probe::Uncertain,
    }
}

impl Watch for Host {
    fn flags(&self) -> WatchFlags {
        unsafe { cbm_watcher_host_flags(self.0) }
    }

    fn now_ns(&self) -> i64 {
        unsafe { cbm_watcher_host_now_ns() }
    }

    fn now_ms(&self) -> u64 {
        unsafe { cbm_watcher_host_now_ms() }
    }

    fn root(&self) -> Root {
        let mut errno = 0;
        match unsafe { cbm_watcher_host_root(self.0, &mut errno) } {
            0 => Root::Present,
            1 => Root::Missing,
            _ => Root::Uncertain(errno),
        }
    }

    fn git_repository(&self) -> Option<bool> {
        match unsafe { cbm_watcher_host_git_repository(self.0) } {
            0 => Some(true),
            1 => Some(false),
            _ => None,
        }
    }

    fn head(&self) -> Probe<Head> {
        let mut head = [0; 128];
        let status = unsafe {
            cbm_watcher_host_head(self.0, head.as_mut_ptr(), head.len())
        };
        if let Some(end) = head.iter().position(|byte| *byte == 0) {
            head[end..].fill(0);
        }
        probe(status, head)
    }

    fn dirty_signature(&self) -> Probe<u64> {
        let mut signature = 0;
        let status =
            unsafe { cbm_watcher_host_dirty_signature(self.0, &mut signature) };
        probe(status, signature)
    }

    fn git_file_count(&self) -> Probe<i32> {
        let mut count = 0;
        let status =
            unsafe { cbm_watcher_host_git_file_count(self.0, &mut count) };
        probe(status, count)
    }

    fn tree(&self) -> Option<(u64, i32)> {
        let mut signature = 0;
        let mut count = 0;
        unsafe { cbm_watcher_host_tree(self.0, &mut signature, &mut count) }
            .then_some((signature, count))
    }

    fn publish(&self, strategy: Strategy, interval_ms: i32, scanned: bool) {
        unsafe {
            cbm_watcher_host_publish(
                self.0,
                strategy as i32,
                interval_ms,
                scanned,
            )
        }
    }

    fn log(&self, event: Event) {
        // The event numbers are declared in watcher/rust_poll.h.
        let (kind, value, extra) = match event {
            Event::RootMissing(polls) => (0, polls, 0),
            Event::RootRestored => (1, 0, 0),
            Event::RootStatError(errno) => (2, errno, 0),
            Event::TreeWalkFailed => (3, 0, 0),
            Event::Baseline(strategy, files) => (4, strategy as i32, files),
            Event::IndexRetry => (5, 0, 0),
            Event::IndexError { result, streak } => (6, result, streak),
            Event::SustainedFailure(streak) => (7, streak, 0),
            Event::Cooldown(delay_ms) => (8, delay_ms, 0),
        };
        unsafe { cbm_watcher_host_log(self.0, kind, value, extra) }
    }

    fn set_catch_up(&self, needed: bool) {
        unsafe { cbm_watcher_host_set_catch_up(self.0, needed) }
    }

    fn index(&self, strategy: Strategy) -> Option<i32> {
        let mut result = 0;
        unsafe { cbm_watcher_host_index(self.0, strategy as i32, &mut result) }
            .then_some(result)
    }

    fn prune_grace_ms(&self) -> u64 {
        unsafe { cbm_watcher_host_prune_grace_s() }.saturating_mul(1_000)
    }

    fn prune(&self, first_missing_ms: u64) {
        unsafe { cbm_watcher_host_prune(self.0, first_missing_ms) };
    }
}

#[unsafe(no_mangle)]
extern "C" fn cbm_rust_watcher_new() -> *mut c_void {
    // Use fallible allocation so C can keep its existing registration on OOM.
    let state = unsafe { alloc(Layout::new::<PollState>()).cast::<PollState>() };
    if !state.is_null() {
        unsafe { state.write(PollState::new()) };
    }
    state.cast()
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_watcher_free(state: *mut c_void) {
    if !state.is_null() {
        // C drops the box once the poll snapshot can no longer reach it.
        drop(unsafe { Box::from_raw(state.cast::<PollState>()) });
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_watcher_poll(
    state: *const c_void,
    host: *mut c_void,
    cycle_ns: i64,
) -> bool {
    let state = unsafe { &*state.cast::<PollState>() };
    state.poll(&Host(host), cycle_ns)
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_watcher_touch(state: *const c_void) {
    unsafe { &*state.cast::<PollState>() }.touch();
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_watcher_failures(state: *const c_void) -> i32 {
    unsafe { &*state.cast::<PollState>() }
        .failures
        .load(Relaxed)
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_watcher_next_poll_ns(state: *const c_void) -> i64 {
    unsafe { &*state.cast::<PollState>() }
        .next_poll_ns
        .load(Relaxed)
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_watcher_dirty_sigs(
    state: *const c_void,
    last: *mut u64,
    pending: *mut u64,
) -> i32 {
    let state = unsafe { &*state.cast::<PollState>() };
    let Ok(data) = state.data.try_lock() else {
        return -1;
    };
    if !last.is_null() {
        unsafe { ptr::write(last, data.last_dirty) };
    }
    if !pending.is_null() {
        unsafe { ptr::write(pending, data.pending_dirty) };
    }
    0
}

#[unsafe(no_mangle)]
extern "C" fn cbm_watcher_poll_interval_ms(file_count: i32) -> i32 {
    poll_interval_ms(file_count)
}

#[unsafe(no_mangle)]
extern "C" fn cbm_watcher_index_backoff_ms(
    interval_ms: i32,
    failures: i32,
) -> i32 {
    index_backoff_ms(interval_ms, failures)
}

#[unsafe(no_mangle)]
extern "C" fn cbm_watcher_success_delay_ms(
    interval_ms: i32,
    elapsed_ms: i64,
) -> i32 {
    success_delay_ms(interval_ms, elapsed_ms)
}
