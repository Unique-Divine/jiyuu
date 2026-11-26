//! Queue and cancellation routing for the C daemon's stdio frontend.
//! The frontend mutex serializes calls and keeps the active worker item alive.

use std::collections::VecDeque;
use std::ffi::{c_char, c_int, c_void};
use std::mem::MaybeUninit;

const CAPACITY: usize = 8;
const BYTES_MAX: usize = 12 * 1024 * 1024;

// Matches FRONTEND_CANCELLATION_* in frontend_session.h.
#[repr(i32)]
enum CancellationRoute {
    Stale = 1,
    Queued = 2,
    Active = 3,
}

type FreeBuffer = unsafe extern "C" fn(*mut c_void);
type Reserve = unsafe extern "C" fn(*mut c_void, *mut u64) -> bool;
type Matches = unsafe extern "C" fn(*const c_char, i64, *const c_char) -> bool;

#[repr(C)]
#[derive(Clone, Copy)]
struct Item {
    message: *mut c_char,
    length: usize,
    content_length_framed: bool,
    has_id: bool,
    id: i64,
    id_str: *mut c_char,
    request_token: u64,
    cancelled: bool,
}

#[derive(Clone, Copy)]
struct RequestId {
    number: i64,
    string: *const c_char,
}

impl Item {
    fn request_id(&self) -> Option<RequestId> {
        self.has_id.then_some(RequestId {
            number: self.id,
            string: self.id_str,
        })
    }
}

struct Queue {
    items: VecDeque<Item>,
    queued_bytes: usize,
    active_id: Option<RequestId>,
    free_buffer: FreeBuffer,
}

impl Drop for Queue {
    fn drop(&mut self) {
        for item in self.items.drain(..) {
            // SAFETY: Admission transferred these allocations from C.
            unsafe {
                (self.free_buffer)(item.message.cast());
                (self.free_buffer)(item.id_str.cast());
            }
        }
    }
}

#[unsafe(no_mangle)]
extern "C" fn mori_frontend_queue_new(free_buffer: FreeBuffer) -> *mut Queue {
    Box::into_raw(Box::new(Queue {
        items: VecDeque::with_capacity(CAPACITY),
        queued_bytes: 0,
        active_id: None,
        free_buffer,
    }))
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_frontend_queue_free(queue: *mut Queue) {
    // SAFETY: C passes the pointer from new exactly once after joining workers.
    drop(unsafe { Box::from_raw(queue) });
}

#[unsafe(no_mangle)]
extern "C" fn mori_frontend_queue_is_empty(queue: &Queue) -> bool {
    queue.items.is_empty()
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_frontend_queue_push(
    queue: &mut Queue,
    item: &Item,
) -> c_int {
    if item.length > BYTES_MAX {
        return -1;
    }
    if queue.items.len() == CAPACITY
        || item.length > BYTES_MAX - queue.queued_bytes
    {
        return 0;
    }
    queue.items.push_back(*item);
    queue.queued_bytes += item.length;
    1
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_frontend_queue_pop(
    queue: &mut Queue,
    client: *mut c_void,
    reserve: Reserve,
    out: &mut MaybeUninit<Item>,
) -> c_int {
    let Some(mut item) = queue.items.front().copied() else {
        return 0;
    };
    // SAFETY: The worker owns the live client and holds the frontend mutex.
    if !item.cancelled && !unsafe { reserve(client, &mut item.request_token) } {
        return -1;
    }
    queue.items.pop_front();
    queue.queued_bytes -= item.length;
    queue.active_id = item.request_id();
    out.write(item);
    1
}

#[unsafe(no_mangle)]
extern "C" fn mori_frontend_queue_finish(queue: &mut Queue) {
    queue.active_id = None;
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_frontend_queue_cancel(
    queue: &mut Queue,
    params: *const c_char,
    matches: Matches,
) -> c_int {
    // SAFETY: The reader lends params for the call. All IDs borrow live item
    // buffers through this call because the frontend mutex excludes teardown.
    let matches_id =
        |id: RequestId| unsafe { matches(params, id.number, id.string) };
    if queue.active_id.is_some_and(matches_id) {
        return CancellationRoute::Active as c_int;
    }
    for item in &mut queue.items {
        if item.request_id().is_some_and(matches_id) {
            item.cancelled = true;
            return CancellationRoute::Queued as c_int;
        }
    }
    CancellationRoute::Stale as c_int
}

#[cfg(test)]
mod tests;
