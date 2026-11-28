//! Framed exchanges over an authenticated native connection. The caller
//! serializes writers and supplies the same absolute deadline for both phases.

use std::ffi::{c_int, c_void};
use std::ptr;

use super::{FRAME_HEADER_SIZE, FrameHeader, FrameKind};

type Connection = *mut c_void;

#[repr(C)]
struct TransferOps {
    io_once:
        unsafe extern "C" fn(Connection, *mut c_void, usize, u64, bool) -> c_int,
    is_poisoned: unsafe extern "C" fn(Connection) -> bool,
    mark_written: unsafe extern "C" fn(*const c_void, usize),
    poison: unsafe extern "C" fn(Connection),
    alloc: unsafe extern "C" fn(usize) -> *mut c_void,
    free: unsafe extern "C" fn(*mut c_void),
}

#[repr(C)]
struct ReceivedFrame {
    kind: u8,
    flags: u16,
    length: u32,
    payload: *mut u8,
}

// Matches MORI_IPC_IO_RETRY in ipc_codec.h.
const IO_RETRY: c_int = -2;

struct Transfer<'a> {
    connection: Connection,
    ops: &'a TransferOps,
    deadline: u64,
}

impl Transfer<'_> {
    unsafe fn complete(
        &self,
        mut buffer: *mut u8,
        mut remaining: usize,
        writing: bool,
        allow_poisoned: bool,
    ) -> c_int {
        // SAFETY: Each native operation borrows this live connection and the
        // unread/unwritten part of the caller's buffer until it returns.
        unsafe {
            if !allow_poisoned && (self.ops.is_poisoned)(self.connection) {
                return -1;
            }
            while remaining != 0 {
                match (self.ops.io_once)(
                    self.connection,
                    buffer.cast(),
                    remaining,
                    self.deadline,
                    writing,
                ) {
                    IO_RETRY => continue,
                    moved if moved > 0 && moved as usize <= remaining => {
                        buffer = buffer.add(moved as usize);
                        remaining -= moved as usize;
                    }
                    status => {
                        // A deadline can race native progress, including a
                        // Windows overlapped transfer that was just cancelled.
                        (self.ops.poison)(self.connection);
                        return if status == 0 { 0 } else { -1 };
                    }
                }
            }
        }
        1
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_ipc_send(
    connection: Connection,
    ops: &TransferOps,
    deadline: u64,
    kind: c_int,
    flags: u16,
    payload: *const u8,
    length: u32,
) -> bool {
    let Some(kind) = u8::try_from(kind)
        .ok()
        .and_then(|kind| FrameKind::try_from(kind).ok())
    else {
        return false;
    };
    let Some(header) = (FrameHeader {
        kind,
        flags,
        length,
    })
    .encode() else {
        return false;
    };
    if length != 0 && payload.is_null() {
        return false;
    }
    let transfer = Transfer {
        connection,
        ops,
        deadline,
    };
    // SAFETY: C borrows the live connection and payload for this call. The
    // callback only reads buffers in the write direction.
    unsafe {
        (ops.mark_written)(header.as_ptr().cast(), header.len());
        transfer.complete(header.as_ptr().cast_mut(), header.len(), true, false)
            == 1
            && (length == 0
                || transfer.complete(
                    payload.cast_mut(),
                    length as usize,
                    true,
                    false,
                ) == 1)
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_ipc_receive(
    connection: Connection,
    ops: &TransferOps,
    deadline: u64,
    max_length: u32,
    after_failed_send: bool,
    out: &mut std::mem::MaybeUninit<ReceivedFrame>,
) -> c_int {
    let transfer = Transfer {
        connection,
        ops,
        deadline,
    };
    let mut header = [0; FRAME_HEADER_SIZE];
    // SAFETY: C lends the live connection and callbacks for this transfer.
    // Recovery may read already queued data while keeping sends disabled.
    unsafe {
        let status = transfer.complete(
            header.as_mut_ptr(),
            header.len(),
            false,
            after_failed_send,
        );
        if status != 1 {
            return status;
        }
        let Some(frame) = FrameHeader::decode(&header, max_length) else {
            (ops.poison)(connection);
            return -1;
        };
        let length = frame.length as usize;
        let mut payload = ptr::null_mut();
        if length != 0 {
            // The native caller releases successful payloads using its allocator.
            payload = (ops.alloc)(length).cast::<u8>();
            if payload.is_null() {
                (ops.poison)(connection);
                return -1;
            }
            let status =
                transfer.complete(payload, length, false, after_failed_send);
            if status != 1 {
                (ops.free)(payload.cast());
                return status;
            }
        }
        out.write(ReceivedFrame {
            kind: frame.kind as u8,
            flags: frame.flags,
            length: frame.length,
            payload,
        });
    }
    1
}

#[cfg(test)]
mod tests;
