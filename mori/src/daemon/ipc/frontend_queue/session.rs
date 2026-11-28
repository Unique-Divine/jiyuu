//! Lifetime of a daemon-backed MCP stdio session. C retains the platform
//! streams, thread handles, and authenticated IPC client implementation.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::mem::MaybeUninit;
use std::ptr::{null, null_mut};
use std::slice;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use super::{
    CancellationRoute, Item, Queue, mori_frontend_queue_cancel,
    mori_frontend_queue_finish, mori_frontend_queue_is_empty,
    mori_frontend_queue_pop, mori_frontend_queue_push,
};
use crate::stdio::{
    c_transport::{FileStream, write_frame},
    frontend_receive::{FrontendState, ReceiveOps, receive_loop},
};

const REQUEST_TIMEOUT_MS: u32 = 24 * 60 * 60 * 1000;
const CLOSE_TIMEOUT_MS: u32 = 5000;
const JOIN_TIMEOUT: Duration = Duration::from_millis(CLOSE_TIMEOUT_MS as u64);
const EOF_DRAIN_TIMEOUT: Duration = Duration::from_secs(15);

const INVALID_TOKEN: u64 = 0;
const TRANSPORT_ERROR: c_int = 0;
const APPLICATION_OK: c_int = 1;
const APPLICATION_CANCELLED: c_int = 6;
const CANCEL_ERROR: c_int = -1;
const CANCEL_ACCEPTED: c_int = 1;

type Native = *mut c_void;
type ThreadFn = unsafe extern "C" fn(Native) -> Native;
type CancelFn = unsafe extern "C" fn(Native) -> bool;

unsafe extern "C" {
    fn mori_frontend_thread_start(run: ThreadFn, context: Native) -> Native;
    fn mori_frontend_thread_join(thread: Native) -> bool;
    fn mori_frontend_fail_stop() -> !;
    fn mori_frontend_monitor_start(
        manager: Native,
        cancel: CancelFn,
        context: Native,
        exit_code: c_int,
        participant: *const c_char,
        exit_when_cancel_not_needed: bool,
    ) -> Native;
    fn cbm_daemon_maintenance_monitor_stop(monitor: *mut Native) -> bool;

    fn mori_frontend_note_idle();
    fn mori_frontend_note_cancellation();
    fn mori_frontend_alloc(size: usize) -> Native;
    fn mori_frontend_free(buffer: Native);
    fn mori_frontend_read_id(
        message: *const c_char,
        has_id: *mut bool,
        id: *mut i64,
        id_str: *mut *mut c_char,
    ) -> bool;
    fn mori_frontend_cancel_params(
        message: *const c_char,
        params: *mut *const c_char,
    ) -> bool;
    fn cbm_mcp_cancel_request_matches(
        params: *const c_char,
        id: i64,
        id_str: *const c_char,
    ) -> bool;
    fn mori_frontend_cancelled_response(
        message: *const c_char,
        id: i64,
        id_str: *const c_char,
    ) -> *mut c_char;

    fn mori_frontend_bootstrap(config: *const c_void) -> Native;
    fn mori_frontend_configure(client: Native, config: *const c_void) -> bool;
    fn cbm_daemon_runtime_client_application_token_reserve(
        client: Native,
        token: *mut u64,
    ) -> bool;
    fn cbm_daemon_runtime_client_application_cancel(
        client: Native,
        token: u64,
    ) -> c_int;
    fn cbm_daemon_application_client_mcp_tagged(
        client: Native,
        token: u64,
        message: *const c_char,
        response: *mut *mut u8,
        length: *mut u32,
        sent: *mut bool,
        timeout_ms: u32,
    ) -> c_int;
    fn cbm_daemon_runtime_client_close_begin(client: Native) -> bool;
    fn cbm_daemon_runtime_client_close_finish(
        client: Native,
        timeout_ms: u32,
    ) -> bool;
    fn cbm_daemon_runtime_client_close(client: Native, timeout_ms: u32) -> bool;
}

struct NativeBuffer(*mut u8);

impl Drop for NativeBuffer {
    fn drop(&mut self) {
        // Native responses and parsed params use the native allocator.
        unsafe { mori_frontend_free(self.0.cast()) };
    }
}

struct Active {
    token: u64,
    cancelled: bool,
}

struct State {
    client: Native,
    queue: Queue,
    active: Option<Active>,
    input_closed: bool,
    stopping: bool,
    failed: bool,
    worker_done: bool,
    completed: u64,
}

impl State {
    fn should_stop(&self) -> bool {
        self.stopping
            || (self.input_closed
                && self.active.is_none()
                && mori_frontend_queue_is_empty(&self.queue))
    }

    fn fail_admission(&mut self) {
        self.failed = true;
        self.stopping = true;
    }

    // Call with the state lock held to keep the exact client and token paired.
    fn cancel_active(&self) -> Option<c_int> {
        let active = self.active.as_ref().filter(|active| active.cancelled)?;
        if self.client.is_null() || active.token == INVALID_TOKEN {
            return None;
        }
        Some(unsafe {
            cbm_daemon_runtime_client_application_cancel(
                self.client,
                active.token,
            )
        })
    }
}

// The queue owns its buffers. Its active ID borrows the sole worker item
// until finish, under the same lock as all queue operations. The native
// client's exchange, cancel, and close-begin APIs allow concurrent calls.
unsafe impl Send for State {}

struct Session {
    state: Mutex<State>,
    changed: Condvar,
    config: *const c_void,
    output: Native,
}

// C lends immutable config and exclusive access to the output stream. Only
// the worker uses output. All callbacks join before these borrows end.
unsafe impl Sync for Session {}

impl Session {
    fn context(&self) -> Native {
        (self as *const Self).cast_mut().cast()
    }

    fn next_item(&self) -> Option<(Item, Native)> {
        let mut state = self.state.lock().unwrap();
        while !state.should_stop() {
            let client = state.client;
            let mut item = MaybeUninit::uninit();
            // Publish the active ID and token under the same lock as dequeue.
            let popped = unsafe {
                mori_frontend_queue_pop(
                    &mut state.queue,
                    client,
                    cbm_daemon_runtime_client_application_token_reserve,
                    &mut item,
                )
            };
            match popped {
                1 => {
                    let item = unsafe { item.assume_init() };
                    state.active = Some(Active {
                        token: item.request_token,
                        cancelled: false,
                    });
                    self.changed.notify_all();
                    return Some((item, client));
                }
                0 => {
                    unsafe { mori_frontend_note_idle() };
                    state = self.changed.wait(state).unwrap();
                }
                _ => {
                    state.fail_admission();
                    self.changed.notify_all();
                    break;
                }
            }
        }
        None
    }

    fn recover(&self) -> Option<(Native, u64)> {
        let previous = {
            let mut state = self.state.lock().unwrap();
            if state.stopping || state.client.is_null() {
                return None;
            }
            state.active.as_mut().unwrap().token = INVALID_TOKEN;
            // Transfer client ownership to this worker. Teardown will see
            // NULL until this worker publishes a usable replacement.
            std::mem::replace(&mut state.client, null_mut())
        };
        unsafe { cbm_daemon_runtime_client_close(previous, CLOSE_TIMEOUT_MS) };
        let replacement = unsafe { mori_frontend_bootstrap(self.config) };
        if replacement.is_null() {
            return None;
        }

        if unsafe { mori_frontend_configure(replacement, self.config) } {
            let mut state = self.state.lock().unwrap();
            if !state.stopping && !state.failed {
                let active = state.active.as_mut().unwrap();
                let mut token = INVALID_TOKEN;
                // A cancellation during replay keeps the replacement for
                // subsequent requests but skips the retry of this item.
                if active.cancelled
                    || unsafe {
                        cbm_daemon_runtime_client_application_token_reserve(
                            replacement,
                            &mut token,
                        )
                    }
                {
                    active.token = token;
                    state.client = replacement;
                    return Some((replacement, token));
                }
            }
        }
        unsafe {
            cbm_daemon_runtime_client_close(replacement, CLOSE_TIMEOUT_MS)
        };
        None
    }

    fn write_cancelled(&self, item: &Item) -> bool {
        let reply = NativeBuffer(
            unsafe {
                mori_frontend_cancelled_response(
                    item.message,
                    item.id,
                    item.id_str,
                )
            }
            .cast(),
        );
        if reply.0.is_null() {
            return false;
        }
        let bytes = unsafe { CStr::from_ptr(reply.0.cast()) }.to_bytes();
        write_frame(self.output, bytes, item.content_length_framed)
    }

    fn forward(&self, item: &Item, client: Native) -> bool {
        if item.cancelled {
            return self.write_cancelled(item);
        }
        let mut reply = Reply::exchange(client, item.request_token, item);
        if reply.status == TRANSPORT_ERROR && !reply.sent {
            drop(reply);
            let Some((replacement, token)) = self.recover() else {
                return false;
            };
            if token == INVALID_TOKEN {
                return self.write_cancelled(item);
            }
            // Only the first failure before a frame was sent may retry.
            reply = Reply::exchange(replacement, token, item);
        }
        match reply.status {
            APPLICATION_CANCELLED => self.write_cancelled(item),
            APPLICATION_OK => {
                let bytes = reply.bytes();
                bytes.is_empty()
                    || write_frame(
                        self.output,
                        bytes,
                        item.content_length_framed,
                    )
            }
            _ => false,
        }
    }

    fn work(&self) {
        while let Some((item, client)) = self.next_item() {
            let ok = self.forward(&item, client);
            let mut state = self.state.lock().unwrap();
            // Check while the request is still active. EOF alone cannot
            // excuse a failed final request.
            if !ok && !state.should_stop() {
                fail_stop();
            }
            mori_frontend_queue_finish(&mut state.queue);
            state.active = None;
            unsafe {
                mori_frontend_free(item.message.cast());
                mori_frontend_free(item.id_str.cast());
            }
            state.completed += 1;
            self.changed.notify_all();
            if !ok {
                break;
            }
        }
        let mut state = self.state.lock().unwrap();
        state.worker_done = true;
        self.changed.notify_all();
    }

    fn drain_eof(&self) {
        let mut state = self.state.lock().unwrap();
        state.input_closed = true;
        self.changed.notify_all();

        // Every completed item earns a fresh window. Only a stalled drain
        // reaches the deadline, even if a batch takes longer in total.
        let mut completed = state.completed;
        let mut deadline = Instant::now() + EOF_DRAIN_TIMEOUT;
        while !state.worker_done {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            (state, _) = self.changed.wait_timeout(state, remaining).unwrap();
            if state.completed != completed {
                completed = state.completed;
                deadline = Instant::now() + EOF_DRAIN_TIMEOUT;
            }
        }
    }

    fn stop_begin(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        state.stopping = true;
        self.changed.notify_all();
        state.client.is_null()
            || unsafe { cbm_daemon_runtime_client_close_begin(state.client) }
    }

    fn run(&self, manager: Native, input: Native) -> c_int {
        let mut monitor = unsafe {
            mori_frontend_monitor_start(
                manager,
                cancel_maintenance,
                self.context(),
                0,
                c"MCP frontend".as_ptr(),
                true,
            )
        };
        if monitor.is_null() {
            self.close_before_work();
            return -1;
        }
        let worker = unsafe { mori_frontend_thread_start(work, self.context()) };
        if worker.is_null() {
            stop_monitor(&mut monitor);
            self.close_before_work();
            return -1;
        }

        let mut stop_begun = false;
        let mut status = unsafe {
            receive_loop(
                &mut FileStream(input),
                self.context().cast(),
                &RECEIVE_OPS,
                &mut stop_begun,
            )
        };
        if status == 0 && !stop_begun {
            self.drain_eof();
        }
        if !stop_begun {
            stop_begun = self.stop_begin();
        }
        self.join_worker(worker);
        stop_monitor(&mut monitor);

        let state = self.state.lock().unwrap();
        if stop_begun && !state.client.is_null() {
            unsafe {
                cbm_daemon_runtime_client_close_finish(
                    state.client,
                    CLOSE_TIMEOUT_MS,
                )
            };
        }
        if !stop_begun || state.failed {
            status = -1;
        }
        status
    }

    fn join_worker(&self, worker: Native) {
        if self.state.lock().unwrap().worker_done {
            join(worker);
            return;
        }

        // The watchdog must remain runnable even if a native call wedges
        // while holding the session mutex.
        let watchdog = JoinWatchdog::default();
        let context = (&raw const watchdog).cast_mut().cast();
        let thread = unsafe { mori_frontend_thread_start(watch_join, context) };
        if thread.is_null() {
            fail_stop();
        }
        join(worker);
        *watchdog.joined.lock().unwrap() = true;
        watchdog.changed.notify_one();
        join(thread);
    }

    fn close_before_work(&self) {
        let client = self.state.lock().unwrap().client;
        unsafe { cbm_daemon_runtime_client_close(client, CLOSE_TIMEOUT_MS) };
    }
}

struct Reply {
    buffer: NativeBuffer,
    length: u32,
    sent: bool,
    status: c_int,
}

impl Reply {
    fn exchange(client: Native, token: u64, item: &Item) -> Self {
        let mut reply = Self {
            buffer: NativeBuffer(null_mut()),
            length: 0,
            sent: false,
            status: TRANSPORT_ERROR,
        };
        reply.status = unsafe {
            cbm_daemon_application_client_mcp_tagged(
                client,
                token,
                item.message,
                &mut reply.buffer.0,
                &mut reply.length,
                &mut reply.sent,
                REQUEST_TIMEOUT_MS,
            )
        };
        reply
    }

    fn bytes(&self) -> &[u8] {
        if self.buffer.0.is_null() {
            return &[];
        }
        // The runtime lends the initialized payload until the reply drops.
        unsafe { slice::from_raw_parts(self.buffer.0, self.length as usize) }
    }
}

fn fail_stop() -> ! {
    // The reader or worker can be blocked inside FILE. Native immediate exit
    // interrupts the exact IPC session without flushing agent-owned streams.
    unsafe { mori_frontend_fail_stop() }
}

fn join(thread: Native) {
    if !unsafe { mori_frontend_thread_join(thread) } {
        fail_stop();
    }
}

fn stop_monitor(monitor: &mut Native) {
    if !unsafe { cbm_daemon_maintenance_monitor_stop(monitor) } {
        fail_stop();
    }
}

unsafe extern "C" fn work(context: Native) -> Native {
    // Session stays live until the caller joins this native thread.
    unsafe { &*context.cast::<Session>() }.work();
    null_mut()
}

#[derive(Default)]
struct JoinWatchdog {
    joined: Mutex<bool>,
    changed: Condvar,
}

unsafe extern "C" fn watch_join(context: Native) -> Native {
    let watchdog = unsafe { &*context.cast::<JoinWatchdog>() };
    let joined = watchdog.joined.lock().unwrap();
    let (joined, _) = watchdog
        .changed
        .wait_timeout_while(joined, JOIN_TIMEOUT, |joined| !*joined)
        .unwrap();
    if !*joined {
        fail_stop();
    }
    null_mut()
}

unsafe extern "C" fn cancel_maintenance(context: Native) -> bool {
    let session = unsafe { &*context.cast::<Session>() };
    let mut state = session.state.lock().unwrap();
    state.stopping = true;
    session.changed.notify_all();
    if let Some(active) = &mut state.active {
        active.cancelled = true;
    }
    state.cancel_active() == Some(CANCEL_ACCEPTED)
}

unsafe extern "C" fn stop_begin(context: *mut FrontendState) -> bool {
    unsafe { &*context.cast::<Session>() }.stop_begin()
}

unsafe extern "C" fn route_cancellation(
    context: *mut FrontendState,
    message: *const c_char,
    active: *mut bool,
) -> bool {
    let mut params = null();
    if !unsafe { mori_frontend_cancel_params(message, &mut params) } {
        return false;
    }
    let params = NativeBuffer(params.cast_mut().cast());
    let session = unsafe { &*context.cast::<Session>() };
    let mut state = session.state.lock().unwrap();
    let route = unsafe {
        mori_frontend_queue_cancel(
            &mut state.queue,
            params.0.cast(),
            cbm_mcp_cancel_request_matches,
        )
    };
    let targets_active = route == CancellationRoute::Active as c_int;
    if targets_active {
        state.active.as_mut().unwrap().cancelled = true;
    }
    if targets_active || route == CancellationRoute::Queued as c_int {
        unsafe { mori_frontend_note_cancellation() };
    }
    unsafe { active.write(targets_active) };
    true
}

unsafe extern "C" fn cancel_active(context: *mut FrontendState) -> bool {
    let session = unsafe { &*context.cast::<Session>() };
    // A replacement may be replaying context with no token to cancel yet.
    // The worker checks the cancellation flag before publishing a retry.
    session.state.lock().unwrap().cancel_active() != Some(CANCEL_ERROR)
}

unsafe extern "C" fn enqueue(
    context: *mut FrontendState,
    message: *mut c_char,
    framed: bool,
) -> bool {
    let session = unsafe { &*context.cast::<Session>() };
    let mut item = Item {
        message,
        length: unsafe { CStr::from_ptr(message) }.to_bytes().len(),
        content_length_framed: framed,
        has_id: false,
        id: 0,
        id_str: null_mut(),
        request_token: INVALID_TOKEN,
        cancelled: false,
    };
    if !unsafe {
        mori_frontend_read_id(
            message,
            &mut item.has_id,
            &mut item.id,
            &mut item.id_str,
        )
    } {
        return false;
    }

    let mut state = session.state.lock().unwrap();
    while !state.stopping && !state.failed {
        match unsafe { mori_frontend_queue_push(&mut state.queue, &item) } {
            1 => {
                session.changed.notify_all();
                return true; // The queue now owns both native allocations.
            }
            0 => state = session.changed.wait(state).unwrap(),
            _ => {
                state.fail_admission();
                session.changed.notify_all();
                break;
            }
        }
    }
    // The reader loop still owns the message on rejected admission.
    unsafe { mori_frontend_free(item.id_str.cast()) };
    false
}

static RECEIVE_OPS: ReceiveOps = ReceiveOps {
    alloc_message: mori_frontend_alloc,
    free_message: mori_frontend_free,
    route_cancellation,
    cancel_active,
    enqueue,
    stop_begin,
};

#[unsafe(no_mangle)]
extern "C" fn mori_frontend_session_run(
    client: Native,
    manager: Native,
    config: *const c_void,
    input: Native,
    output: Native,
) -> c_int {
    // The C entry validates all arguments and transfers client ownership.
    Session {
        state: Mutex::new(State {
            client,
            queue: Queue::new(mori_frontend_free),
            active: None,
            input_closed: false,
            stopping: false,
            failed: false,
            worker_done: false,
            completed: 0,
        }),
        changed: Condvar::new(),
        config,
        output,
    }
    .run(manager, input)
}
