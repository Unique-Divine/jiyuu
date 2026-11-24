use std::cell::RefCell;
use std::ffi::CStr;

use super::*;

const BUFFER_SIZE: usize = 32;

thread_local! {
    static TRACE: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn record(event: impl Into<String>) {
    TRACE.with_borrow_mut(|events| events.push(event.into()));
}

unsafe extern "C" fn alloc_message(size: usize) -> *mut c_void {
    if size > BUFFER_SIZE {
        return ptr::null_mut();
    }
    let mut buffer = Box::new([b'!'; BUFFER_SIZE]);
    buffer[BUFFER_SIZE - 1] = 0;
    Box::into_raw(buffer).cast()
}

unsafe extern "C" fn free_message(message: *mut c_void) {
    // SAFETY: The loop passes a live NUL-terminated copy from alloc_message.
    let text = unsafe { CStr::from_ptr(message.cast()) }.to_string_lossy();
    record(format!("free {text}"));
    unsafe { drop(Box::from_raw(message.cast::<[u8; BUFFER_SIZE]>())) };
}

unsafe extern "C" fn route_cancellation(
    _: *mut FrontendState,
    message: *const c_char,
    active: *mut bool,
) -> bool {
    // SAFETY: The loop supplies its live message copy and a writable flag.
    let text = unsafe { CStr::from_ptr(message) }.to_string_lossy();
    record(format!("route {text}"));
    unsafe { *active = text == "active" };
    matches!(text.as_ref(), "active" | "queued" | "stale")
}

unsafe extern "C" fn cancel_active(state: *mut FrontendState) -> bool {
    record("cancel active");
    // SAFETY: run lends this flag as the opaque state for the whole call.
    unsafe { *state.cast::<bool>() }
}

unsafe extern "C" fn enqueue(
    _: *mut FrontendState,
    message: *mut c_char,
    framed: bool,
) -> bool {
    // SAFETY: The loop supplies a live allocation from alloc_message.
    let text = unsafe { CStr::from_ptr(message) }.to_string_lossy();
    record(format!("enqueue {text} framed={framed}"));
    if text == "reject" {
        return false;
    }
    // Simulate ownership transfer to the queue, then release its allocation.
    unsafe { drop(Box::from_raw(message.cast::<[u8; BUFFER_SIZE]>())) };
    true
}

unsafe extern "C" fn stop_begin(_: *mut FrontendState) -> bool {
    record("begin stop");
    true
}

fn run(mut input: &[u8], mut cancel_ok: bool) -> (c_int, bool, Vec<String>) {
    let ops = ReceiveOps {
        alloc_message,
        free_message,
        route_cancellation,
        cancel_active,
        enqueue,
        stop_begin,
    };
    let mut stop_begun = false;
    // SAFETY: The test operations all borrow cancel_ok for this call and
    // agree on message allocation and ownership.
    let status = unsafe {
        receive_loop(
            &mut input,
            (&raw mut cancel_ok).cast(),
            &ops,
            &mut stop_begun,
        )
    };
    (status, stop_begun, TRACE.with_borrow_mut(std::mem::take))
}

#[test]
fn receives_cancellations_before_admission_and_continues_to_eof() {
    let (status, stop_begun, events) = run(
        b"line\nContent-Length: 6\r\n\r\nqueuedstale\nactive\nContent-Length: 6\r\n\r\nframed",
        true,
    );

    assert_eq!((status, stop_begun), (0, false));
    assert_eq!(
        events,
        [
            "route line",
            "enqueue line framed=false",
            "route queued",
            "free queued",
            "route stale",
            "free stale",
            "route active",
            "free active",
            "cancel active",
            "route framed",
            "enqueue framed framed=true",
        ],
    );
}

#[test]
fn receive_failures_preserve_message_ownership_and_close_state() {
    let (status, stop_begun, events) = run(b"active\nunread\n", false);
    assert_eq!((status, stop_begun), (-1, true));
    assert_eq!(
        events,
        ["route active", "free active", "cancel active", "begin stop"]
    );

    let (status, stop_begun, events) = run(b"reject\nunread\n", true);
    assert_eq!((status, stop_begun), (-1, false));
    assert_eq!(
        events,
        ["route reject", "enqueue reject framed=false", "free reject"]
    );

    let (status, stop_begun, events) = run(b"Content-Length: 4\r\n\r\nx", true);
    assert_eq!((status, stop_begun), (-1, false));
    assert!(events.is_empty(), "{events:?}");
}
