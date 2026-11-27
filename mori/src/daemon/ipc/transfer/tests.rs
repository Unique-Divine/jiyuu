use std::mem::MaybeUninit;
use std::slice;

use super::*;

unsafe extern "C" {
    fn malloc(size: usize) -> *mut c_void;
    fn free(block: *mut c_void);
}

const DEADLINE: u64 = 173;

#[derive(Default)]
struct Stream {
    input: Vec<u8>,
    output: Vec<u8>,
    consumed: usize,
    poisoned: bool,
    retry: bool,
    blocked_write: bool,
    read_end: c_int,
}

impl Stream {
    fn connection(&mut self) -> Connection {
        ptr::from_mut(self).cast()
    }

    fn receive(
        &mut self,
        limit: u32,
        after_failed_send: bool,
    ) -> Result<ReceivedFrame, c_int> {
        let mut result = MaybeUninit::uninit();
        // SAFETY: The callbacks only access this stream during the call. The
        // receive function initializes result on success.
        unsafe {
            match mori_ipc_receive(
                self.connection(),
                &OPS,
                DEADLINE,
                limit,
                after_failed_send,
                &mut result,
            ) {
                1 => Ok(result.assume_init()),
                status => Err(status),
            }
        }
    }
}

unsafe extern "C" fn io(
    state: Connection,
    bytes: *mut c_void,
    length: usize,
    deadline: u64,
    writing: bool,
) -> c_int {
    // SAFETY: The test passes an exclusive stream and a buffer of length bytes.
    let state = unsafe { &mut *state.cast::<Stream>() };
    assert_eq!(deadline, DEADLINE);
    if std::mem::take(&mut state.retry) {
        return IO_RETRY;
    }
    let count = length.min(3);
    if writing {
        if state.blocked_write && state.output.len() == FRAME_HEADER_SIZE {
            return 0;
        }
        state.output.extend_from_slice(unsafe {
            slice::from_raw_parts(bytes.cast(), count)
        });
        return count as c_int;
    }
    let unread = &state.input[state.consumed..];
    let count = count.min(unread.len());
    if count == 0 {
        return state.read_end;
    }
    unsafe { ptr::copy_nonoverlapping(unread.as_ptr(), bytes.cast(), count) };
    state.consumed += count;
    count as c_int
}

unsafe extern "C" fn poisoned(state: Connection) -> bool {
    unsafe { (*state.cast::<Stream>()).poisoned }
}

unsafe extern "C" fn poison(state: Connection) {
    unsafe { (*state.cast::<Stream>()).poisoned = true };
}

unsafe extern "C" fn mark_written(_: *const c_void, _: usize) {}

const OPS: TransferOps = TransferOps {
    io_once: io,
    is_poisoned: poisoned,
    mark_written,
    poison,
    alloc: malloc,
    free,
};

fn wire(kind: FrameKind, flags: u16, payload: &[u8]) -> Vec<u8> {
    let header = FrameHeader {
        kind,
        flags,
        length: payload.len() as u32,
    };
    [header.encode().unwrap().as_slice(), payload].concat()
}

#[test]
fn short_transfers_share_one_deadline_and_preserve_runtime_frames() {
    let token = u64::MAX.to_be_bytes();
    let mut stream = Stream {
        retry: true,
        ..Stream::default()
    };
    // SAFETY: All buffers and the connection remain live for the call.
    unsafe {
        assert!(mori_ipc_send(
            stream.connection(),
            &OPS,
            DEADLINE,
            1,
            7,
            token.as_ptr(),
            8
        ));
        assert!(mori_ipc_send(
            stream.connection(),
            &OPS,
            DEADLINE,
            1,
            5,
            ptr::null(),
            0
        ));
    }
    assert_eq!(
        stream.output,
        [
            wire(FrameKind::Request, 7, &token),
            wire(FrameKind::Request, 5, &[])
        ]
        .concat()
    );
    stream.input = std::mem::take(&mut stream.output);
    let cancel = stream.receive(8, false).ok().unwrap();
    assert_eq!((cancel.kind, cancel.flags, cancel.length), (1, 7, 8));
    // SAFETY: receive returned a live payload allocated by malloc.
    unsafe {
        assert_eq!(slice::from_raw_parts(cancel.payload, 8), token);
        free(cancel.payload.cast());
    }
    let disconnect = stream.receive(0, false).ok().unwrap();
    assert_eq!((disconnect.flags, disconnect.length), (5, 0));
    assert!(disconnect.payload.is_null());
}

#[test]
fn bounded_receive_rejects_before_payload_and_disconnect_poisons_partial_frames()
{
    let mut stream = Stream {
        input: wire(FrameKind::Request, 6, b"body"),
        read_end: -1,
        ..Stream::default()
    };
    assert_eq!(stream.receive(3, false).err(), Some(-1));
    assert_eq!(stream.consumed, FRAME_HEADER_SIZE);
    assert!(stream.poisoned);

    stream.poisoned = false;
    stream.consumed = 0;
    stream.input.pop();
    assert_eq!(stream.receive(4, false).err(), Some(-1));
    assert_eq!(stream.consumed, FRAME_HEADER_SIZE + 3);
    assert!(stream.poisoned);
}

#[test]
fn failed_send_can_read_admission_reply_without_reviving_writes() {
    let mut stream = Stream {
        input: wire(FrameKind::Response, 1, b"busy"),
        blocked_write: true,
        ..Stream::default()
    };
    let payload = b"hello";
    // SAFETY: All buffers and the connection remain live for each call.
    unsafe {
        assert!(!mori_ipc_send(
            stream.connection(),
            &OPS,
            DEADLINE,
            1,
            1,
            payload.as_ptr(),
            5
        ));
        assert!(stream.poisoned);
        assert_eq!(stream.receive(4, false).err(), Some(-1));
        let reply = stream.receive(4, true).ok().unwrap();
        assert_eq!(slice::from_raw_parts(reply.payload, 4), b"busy");
        free(reply.payload.cast());
        assert!(stream.poisoned);
        assert!(!mori_ipc_send(
            stream.connection(),
            &OPS,
            DEADLINE,
            1,
            5,
            ptr::null(),
            0
        ));
    }
    assert_eq!(stream.output.len(), FRAME_HEADER_SIZE);
}
