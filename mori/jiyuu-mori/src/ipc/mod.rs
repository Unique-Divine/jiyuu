//! Byte formats shared with the native daemon. Transport and record publication
//! are the responsibility of the caller.

mod frame;
mod socket_record;

#[cfg(feature = "c-backend")]
mod native;
#[cfg(test)]
mod tests;

pub use frame::{FRAME_HEADER_SIZE, FrameHeader, FrameKind, MAX_FRAME_PAYLOAD};
pub use socket_record::{
    ANCHOR_NAME_CAPACITY, SOCKET_RECORD_SIZE, SocketRecord, SocketRecordKind,
};

pub const INSTANCE_KEY_SIZE: usize = 16;

/// Checks the endpoint key before it can become part of a socket name.
pub fn valid_instance_key(key: &[u8]) -> bool {
    key.len() == INSTANCE_KEY_SIZE
        && key
            .iter()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}
