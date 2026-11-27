//! Daemon HTTP lifecycle around the platform thread and native route callbacks.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr;

const CONFIG_POLL_MS: u64 = 1_000;
const RETRY_INITIAL_MS: u32 = 1_000;
const RETRY_MAX_MS: u32 = 30_000;

#[derive(Clone, Copy, Default, Eq, PartialEq)]
#[repr(C)]
struct Config {
    enabled: bool,
    port: c_int,
}

#[repr(C)]
struct Host {
    server: *mut c_void,
    started: bool,
    retiring: bool,
    config_loaded: bool,
    assets_available: bool,
    config: Config,
    next_config_load_ms: u64,
    retry_at_ms: u64,
    retry_delay_ms: u32,
    largest_scheduled_retry_ms: u32,
}

#[repr(C)]
struct Ops {
    context: *mut c_void,
    config_load: unsafe extern "C" fn(*mut c_void, *mut Config),
    server_new: unsafe extern "C" fn(*mut c_void, c_int) -> *mut c_void,
    server_configure:
        unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void),
    server_stop: unsafe extern "C" fn(*mut c_void, *mut c_void),
    server_free: unsafe extern "C" fn(*mut c_void, *mut c_void) -> bool,
    thread_start:
        unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> c_int,
    thread_join: unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int,
    log_retry: unsafe extern "C" fn(*const c_char, u32),
    log_no_assets: unsafe extern "C" fn(),
}

/// All operations run on the daemon loop. The owner and callback table stay
/// live for each call. The server thread borrows the server and the daemon's
/// dependency contexts, so a failed join or free must retain the server.
impl Host {
    unsafe fn stop(&self, ops: &Ops) {
        if self.started && !self.server.is_null() {
            unsafe { (ops.server_stop)(ops.context, self.server) };
        }
    }

    unsafe fn join(&mut self, ops: &Ops, owner: *mut c_void) -> bool {
        if self.started {
            if unsafe { (ops.thread_join)(ops.context, owner) } != 0 {
                return false;
            }
            self.started = false;
        }
        true
    }

    unsafe fn dispose(&mut self, ops: &Ops, owner: *mut c_void) -> bool {
        unsafe { self.stop(ops) };
        if !unsafe { self.join(ops, owner) }
            || !unsafe { (ops.server_free)(ops.context, self.server) }
        {
            return false;
        }
        self.server = ptr::null_mut();
        true
    }

    unsafe fn schedule_retry(&mut self, ops: &Ops, now_ms: u64, reason: &CStr) {
        let delay = if self.retry_delay_ms == 0 {
            RETRY_INITIAL_MS
        } else {
            self.retry_delay_ms
        };
        self.retry_at_ms = now_ms.saturating_add(u64::from(delay));
        self.largest_scheduled_retry_ms =
            self.largest_scheduled_retry_ms.max(delay);
        self.retry_delay_ms = if delay >= RETRY_MAX_MS / 2 {
            RETRY_MAX_MS
        } else {
            delay * 2
        };
        unsafe { (ops.log_retry)(crate::ffi_output::c_str(reason), delay) };
    }

    unsafe fn reconcile(
        &mut self,
        ops: &Ops,
        owner: *mut c_void,
        now_ms: u64,
        force_config_load: bool,
    ) {
        if !force_config_load
            && self.config_loaded
            && now_ms < self.next_config_load_ms
        {
            return;
        }
        self.next_config_load_ms = now_ms.saturating_add(CONFIG_POLL_MS);

        let mut desired = Config::default();
        unsafe { (ops.config_load)(ops.context, &mut desired) };
        let config_changed = !self.config_loaded || desired != self.config;

        if config_changed && !self.retiring {
            self.retiring = true;
            self.retry_at_ms = now_ms;
            self.retry_delay_ms = RETRY_INITIAL_MS;
        }
        if self.retiring {
            if now_ms < self.retry_at_ms {
                return;
            }
            if !unsafe { self.dispose(ops, owner) } {
                unsafe { self.schedule_retry(ops, now_ms, c"server_retire") };
                return;
            }
            self.retiring = false;
        }
        if config_changed {
            self.config_loaded = true;
            self.config = desired;
            self.retry_at_ms = now_ms;
            self.retry_delay_ms = RETRY_INITIAL_MS;
        }
        if !desired.enabled || self.started || !self.server.is_null() {
            return;
        }
        if !self.assets_available {
            if config_changed {
                unsafe { (ops.log_no_assets)() };
            }
            self.retry_at_ms = u64::MAX;
            return;
        }
        if now_ms < self.retry_at_ms {
            return;
        }

        self.server = unsafe { (ops.server_new)(ops.context, desired.port) };
        if self.server.is_null() {
            unsafe { self.schedule_retry(ops, now_ms, c"server_create") };
            return;
        }
        unsafe { (ops.server_configure)(ops.context, self.server, owner) };
        if unsafe { (ops.thread_start)(ops.context, owner, self.server) } == 0 {
            self.started = true;
            self.retry_at_ms = 0;
            self.retry_delay_ms = RETRY_INITIAL_MS;
        } else {
            self.retiring = !unsafe { self.dispose(ops, owner) };
            unsafe { self.schedule_retry(ops, now_ms, c"thread_start") };
        }
    }
}

// SAFETY for the exported calls: host.c provides an initialized state, an
// immutable table of live C callbacks, and exclusive access on the daemon
// thread. The callbacks may use the owner's other fields but never this state.
#[unsafe(no_mangle)]
unsafe extern "C" fn mori_http_host_reconcile(
    state: *mut Host,
    ops: *const Ops,
    owner: *mut c_void,
    now_ms: u64,
    force_config_load: bool,
) {
    unsafe { (*state).reconcile(&*ops, owner, now_ms, force_config_load) };
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_http_host_stop(state: *const Host, ops: *const Ops) {
    unsafe { (*state).stop(&*ops) };
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_http_host_join(
    state: *mut Host,
    ops: *const Ops,
    owner: *mut c_void,
) -> bool {
    unsafe { (*state).join(&*ops, owner) }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_http_host_dispose(
    state: *mut Host,
    ops: *const Ops,
    owner: *mut c_void,
) -> bool {
    unsafe { (*state).dispose(&*ops, owner) }
}
