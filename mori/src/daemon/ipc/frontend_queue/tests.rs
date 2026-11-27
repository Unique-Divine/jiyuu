use std::cell::Cell;
use std::ffi::{CStr, CString};
use std::ptr;

use super::*;

thread_local! {
    static FREED: Cell<usize> = const { Cell::new(0) };
}

unsafe extern "C" fn release(buffer: *mut c_void) {
    if !buffer.is_null() {
        // SAFETY: Every test payload and string ID is allocated with into_raw.
        drop(unsafe { CString::from_raw(buffer.cast()) });
        FREED.with(|count| count.set(count.get() + 1));
    }
}

unsafe extern "C" fn reserve(context: *mut c_void, out: *mut u64) -> bool {
    // SAFETY: The fixture supplies pointers to the token counter and item slot.
    unsafe {
        let next = &mut *context.cast::<u64>();
        if *next == u64::MAX {
            return false;
        }
        out.write(*next);
        *next += 1;
    }
    true
}

unsafe extern "C" fn matches(
    params: *const c_char,
    id: i64,
    id_str: *const c_char,
) -> bool {
    // This fixture injects typed cancellation IDs in place of the C JSON parser.
    let params = unsafe { CStr::from_ptr(params) }.to_bytes();
    if let Some(string) = params.strip_prefix(b"s:") {
        return !id_str.is_null()
            && string == unsafe { CStr::from_ptr(id_str) }.to_bytes();
    }
    id_str.is_null()
        && params.strip_prefix(b"n:") == Some(id.to_string().as_bytes())
}

fn item(id: i64, length: usize, id_str: Option<&CStr>) -> Item {
    Item {
        message: CString::from(c"request").into_raw(),
        length,
        content_length_framed: id_str.is_some(),
        has_id: true,
        id,
        id_str: id_str
            .map_or(ptr::null_mut(), |value| value.to_owned().into_raw()),
        request_token: 0,
        cancelled: false,
    }
}

unsafe fn free_item(item: Item) {
    // SAFETY: A popped or unadmitted item is ours to free.
    unsafe {
        release(item.message.cast());
        release(item.id_str.cast());
    }
}

#[test]
fn full_queue_routes_exact_ids_and_discards_unforwarded_items_on_disconnect() {
    FREED.with(|count| count.set(0));
    let mut next_token = 101_u64;
    let mut out = MaybeUninit::uninit();
    // SAFETY: The queue remains live until free. Item buffers stay alive
    // while admitted or active, and the callback context is the token counter.
    unsafe {
        let queue = mori_frontend_queue_new(release);
        assert_eq!(mori_frontend_queue_push(&mut *queue, &item(7, 1, None)), 1);
        assert_eq!(
            mori_frontend_queue_pop(
                &mut *queue,
                ptr::from_mut(&mut next_token).cast(),
                reserve,
                &mut out
            ),
            1,
        );
        let active = out.assume_init_read();
        assert_eq!(active.request_token, 101);

        assert_eq!(
            mori_frontend_queue_push(&mut *queue, &item(-1, 1, Some(c"7"))),
            1
        );
        for id in 8..8 + (CAPACITY as i64 - 1) {
            assert_eq!(
                mori_frontend_queue_push(&mut *queue, &item(id, 1, None)),
                1
            );
        }
        let extra = item(42, 1, None);
        assert_eq!(mori_frontend_queue_push(&mut *queue, &extra), 0);
        assert_eq!(
            mori_frontend_queue_cancel(&mut *queue, c"n:7".as_ptr(), matches),
            3
        );
        assert_eq!(
            mori_frontend_queue_cancel(&mut *queue, c"s:7".as_ptr(), matches),
            2
        );
        assert_eq!(
            mori_frontend_queue_cancel(&mut *queue, c"n:42".as_ptr(), matches),
            1
        );
        mori_frontend_queue_finish(&mut *queue);
        free_item(active);
        assert_eq!(
            mori_frontend_queue_cancel(&mut *queue, c"n:7".as_ptr(), matches),
            1
        );

        assert_eq!(
            mori_frontend_queue_pop(
                &mut *queue,
                ptr::from_mut(&mut next_token).cast(),
                reserve,
                &mut out
            ),
            1,
        );
        let cancelled = out.assume_init_read();
        assert!(cancelled.cancelled && cancelled.content_length_framed);
        assert_eq!(CStr::from_ptr(cancelled.id_str), c"7");
        assert_eq!(cancelled.request_token, 0);
        assert_eq!(next_token, 102);
        assert_eq!(mori_frontend_queue_push(&mut *queue, &extra), 1);
        mori_frontend_queue_finish(&mut *queue);
        free_item(cancelled);

        assert_eq!(
            mori_frontend_queue_pop(
                &mut *queue,
                ptr::from_mut(&mut next_token).cast(),
                reserve,
                &mut out
            ),
            1,
        );
        let next = out.assume_init_read();
        assert_eq!(
            (next.id, next.request_token, next.cancelled),
            (8, 102, false)
        );
        mori_frontend_queue_finish(&mut *queue);
        free_item(next);
        mori_frontend_queue_free(queue);
    }
    assert_eq!(FREED.with(Cell::get), CAPACITY + 3);
}

#[test]
fn byte_budget_and_failed_reservation_leave_queue_ownership_intact() {
    FREED.with(|count| count.set(0));
    let mut next_token = u64::MAX;
    let mut out = MaybeUninit::uninit();
    // SAFETY: The fixture retains buffers until successful admission. The
    // failed pop leaves the original item in the queue for teardown.
    unsafe {
        let queue = mori_frontend_queue_new(release);
        let oversized = item(1, BYTES_MAX + 1, None);
        assert_eq!(mori_frontend_queue_push(&mut *queue, &oversized), -1);
        free_item(oversized);
        assert!(mori_frontend_queue_is_empty(&*queue));
        assert_eq!(
            mori_frontend_queue_push(&mut *queue, &item(2, BYTES_MAX, None)),
            1
        );

        let extra = item(3, 1, None);
        assert_eq!(mori_frontend_queue_push(&mut *queue, &extra), 0);
        free_item(extra);
        assert_eq!(
            mori_frontend_queue_pop(
                &mut *queue,
                ptr::from_mut(&mut next_token).cast(),
                reserve,
                &mut out
            ),
            -1,
        );
        assert!(!mori_frontend_queue_is_empty(&*queue));
        assert_eq!(
            mori_frontend_queue_cancel(&mut *queue, c"n:2".as_ptr(), matches),
            2
        );
        mori_frontend_queue_free(queue);
    }
    assert_eq!(FREED.with(Cell::get), 3);
}
