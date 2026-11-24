//! Adapters for the C socket and named-pipe transports.

use std::ffi::c_int;
use std::slice;

use super::{
    ANCHOR_NAME_CAPACITY, FRAME_HEADER_SIZE, FrameHeader, FrameKind,
    INSTANCE_KEY_SIZE, SOCKET_RECORD_SIZE, SocketRecord, SocketRecordKind,
    valid_instance_key,
};

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_ipc_instance_key_valid(
    key: *const [u8; INSTANCE_KEY_SIZE],
) -> bool {
    if key.is_null() {
        return false;
    }
    // SAFETY: The native caller checks the string length before calling.
    valid_instance_key(unsafe { &*key })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_ipc_frame_header_encode(
    out: *mut [u8; FRAME_HEADER_SIZE],
    kind: c_int,
    flags: u16,
    length: u32,
) -> bool {
    if out.is_null() {
        return false;
    }
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
    // SAFETY: C supplies writable space for one header.
    unsafe { out.write(header) };
    true
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_ipc_frame_header_decode(
    header: *const [u8; FRAME_HEADER_SIZE],
    max_payload_length: u32,
    kind_out: *mut u8,
    flags_out: *mut u16,
    length_out: *mut u32,
) -> bool {
    if header.is_null()
        || kind_out.is_null()
        || flags_out.is_null()
        || length_out.is_null()
    {
        return false;
    }
    // SAFETY: C supplies a complete initialized header and valid output slots.
    let Some(frame) =
        FrameHeader::decode(unsafe { &*header }, max_payload_length)
    else {
        return false;
    };
    unsafe {
        kind_out.write(frame.kind as u8);
        flags_out.write(frame.flags);
        length_out.write(frame.length);
    }
    true
}

#[repr(C)]
struct NativeSocketIdentity {
    device: u64,
    inode: u64,
    ctime_seconds: i64,
    ctime_nanoseconds: u32,
}

fn record_kind(magic: [u8; 8]) -> Option<SocketRecordKind> {
    [SocketRecordKind::Pending, SocketRecordKind::Committed]
        .into_iter()
        .find(|kind| kind.magic() == magic)
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_ipc_socket_record_encode(
    magic: *const [u8; 8],
    identity: *const NativeSocketIdentity,
    anchor: *const u8,
    anchor_length: usize,
    out: *mut [u8; SOCKET_RECORD_SIZE],
) -> bool {
    if magic.is_null()
        || anchor.is_null()
        || !(1..ANCHOR_NAME_CAPACITY).contains(&anchor_length)
        || out.is_null()
    {
        return false;
    }
    // SAFETY: C supplies the fixed magic, an initialized identity, and the
    // initialized bytes before the anchor string's NUL terminator.
    let Some(kind) = record_kind(unsafe { *magic }) else {
        return false;
    };
    let Some(identity) = (unsafe { identity.as_ref() }) else {
        return false;
    };
    let record = SocketRecord {
        device: identity.device,
        inode: identity.inode,
        ctime_seconds: identity.ctime_seconds,
        ctime_nanoseconds: identity.ctime_nanoseconds,
        anchor_name: unsafe { slice::from_raw_parts(anchor, anchor_length) },
    };
    let Some(bytes) = record.encode(kind) else {
        return false;
    };
    // SAFETY: C supplies writable space for one record.
    unsafe { out.write(bytes) };
    true
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_ipc_socket_record_decode(
    magic: *const [u8; 8],
    bytes: *const [u8; SOCKET_RECORD_SIZE],
    identity_out: *mut NativeSocketIdentity,
    anchor_out: *mut [u8; ANCHOR_NAME_CAPACITY],
) -> bool {
    if magic.is_null()
        || bytes.is_null()
        || identity_out.is_null()
        || anchor_out.is_null()
    {
        return false;
    }
    // SAFETY: C supplies the fixed magic and a complete initialized record.
    let Some(kind) = record_kind(unsafe { *magic }) else {
        return false;
    };
    let Some(record) = SocketRecord::decode(unsafe { &*bytes }, kind) else {
        return false;
    };
    let mut anchor = [0; ANCHOR_NAME_CAPACITY];
    anchor[..record.anchor_name.len()].copy_from_slice(record.anchor_name);
    // SAFETY: C supplies separate writable storage for the identity and name.
    unsafe {
        identity_out.write(NativeSocketIdentity {
            device: record.device,
            inode: record.inode,
            ctime_seconds: record.ctime_seconds,
            ctime_nanoseconds: record.ctime_nanoseconds,
        });
        anchor_out.write(anchor);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_adapter_rejects_kind_and_length_without_publishing_outputs() {
        let mut wire = [0xaa; FRAME_HEADER_SIZE];
        let (mut kind, mut flags, mut length) = (0xaa, 0xbbbb, 0xcccc_cccc);
        // SAFETY: All pointers refer to initialized locals with the declared layout.
        unsafe {
            assert!(!mori_ipc_frame_header_encode(&mut wire, 257, 0, 3));
            assert_eq!(wire, [0xaa; FRAME_HEADER_SIZE]);
            assert!(mori_ipc_frame_header_encode(&mut wire, 1, 0x1234, 3));
            assert!(!mori_ipc_frame_header_decode(
                &wire,
                2,
                &mut kind,
                &mut flags,
                &mut length
            ));
            assert_eq!((kind, flags, length), (0xaa, 0xbbbb, 0xcccc_cccc));
            assert!(mori_ipc_frame_header_decode(
                &wire,
                3,
                &mut kind,
                &mut flags,
                &mut length
            ));
        }
        assert_eq!((kind, flags, length), (1, 0x1234, 3));
    }

    #[test]
    fn record_adapter_stages_identity_and_full_nul_padded_name() {
        let mut identity = NativeSocketIdentity {
            device: 3,
            inode: u64::MAX,
            ctime_seconds: 5,
            ctime_nanoseconds: 6,
        };
        let pending = SocketRecordKind::Pending.magic();
        let committed = SocketRecordKind::Committed.magic();
        let mut name = [0xaa; ANCHOR_NAME_CAPACITY];
        let mut wire = [0; SOCKET_RECORD_SIZE];
        // SAFETY: All pointers refer to initialized locals with the declared layout.
        unsafe {
            assert!(mori_ipc_socket_record_encode(
                &pending,
                &identity,
                b"name".as_ptr(),
                4,
                &mut wire
            ));
            identity.inode = 0;
            assert!(!mori_ipc_socket_record_decode(
                &committed,
                &wire,
                &mut identity,
                &mut name
            ));
            assert_eq!(identity.inode, 0);
            assert_eq!(name, [0xaa; ANCHOR_NAME_CAPACITY]);
            assert!(mori_ipc_socket_record_decode(
                &pending,
                &wire,
                &mut identity,
                &mut name
            ));
        }
        assert_eq!(identity.inode, u64::MAX);
        assert_eq!(&name[..4], b"name");
        assert!(name[4..].iter().all(|byte| *byte == 0));
    }
}
