use super::*;

const KEY: &[u8] = b"0123456789abcdef";
const REQUEST: &[u8; 12] = b"CBMD\x01\x01\x12\x34\0\0\0\x03";
const RESPONSE: &[u8; 12] = b"CBMD\x01\x02\x00\xa5\0\0\0\x02";
const ANCHOR: &[u8] = b"cbm-0123456789abcdef.anc";

const SOCKET: SocketRecord<'static> = SocketRecord {
    device: 0x0123_4567,
    inode: 0x8877_6655_4433_2211,
    ctime_seconds: 0x0102_0304_0506_0708,
    ctime_nanoseconds: 123_456_789,
    anchor_name: ANCHOR,
};

// The fixed part of a committed record written by the C codec at 8bdd8fc.
const COMMITTED_FIELDS: &[u8; 40] = b"CBMSOC\x02\0\x67\x45\x23\x01\0\0\0\0\
    \x11\x22\x33\x44\x55\x66\x77\x88\x08\x07\x06\x05\x04\x03\x02\x01\
    \x15\xcd\x5b\x07\0\0\0\0";

fn committed_fixture() -> [u8; SOCKET_RECORD_SIZE] {
    let mut bytes = [0; SOCKET_RECORD_SIZE];
    bytes[..40].copy_from_slice(COMMITTED_FIELDS);
    bytes[40..40 + ANCHOR.len()].copy_from_slice(ANCHOR);
    bytes
}

#[test]
fn endpoint_key_requires_sixteen_lowercase_hex_bytes() {
    assert!(valid_instance_key(KEY));
    for key in [
        &KEY[..15],
        b"0123456789abcdef0",
        b"0123456789abcdeF",
        b"0123456789abcde/",
        b"0123456789abcde\0",
        b"0123456789abcde\xff",
    ] {
        assert!(!valid_instance_key(key));
    }
}

#[test]
fn request_and_response_headers_preserve_order_and_flags() {
    let mut wire = Vec::new();
    wire.extend_from_slice(REQUEST);
    wire.extend_from_slice(b"\xff\0\x80");
    wire.extend_from_slice(RESPONSE);
    wire.extend_from_slice(b"ok");

    let mut stream = wire.as_slice();
    for (expected_kind, expected_flags, expected_payload) in [
        (FrameKind::Request, 0x1234, b"\xff\0\x80".as_slice()),
        (FrameKind::Response, 0x00a5, b"ok".as_slice()),
    ] {
        let (header, remaining) = stream.split_at(FRAME_HEADER_SIZE);
        let decoded = FrameHeader::decode(header, 3).unwrap();
        assert_eq!(decoded.kind, expected_kind);
        assert_eq!(decoded.flags, expected_flags);
        assert_eq!(decoded.encode().unwrap(), header);
        let (payload, remaining) = remaining.split_at(decoded.length as usize);
        assert_eq!(payload, expected_payload);
        stream = remaining;
    }
    assert!(stream.is_empty());
}

#[test]
fn frame_checks_exact_header_and_both_length_limits() {
    for length in 0..FRAME_HEADER_SIZE {
        assert_eq!(FrameHeader::decode(&REQUEST[..length], u32::MAX), None);
    }
    assert_eq!(
        FrameHeader::decode(b"CBMD\x01\x01\x12\x34\0\0\0\x03\0", 3),
        None
    );
    assert!(FrameHeader::decode(REQUEST, 3).is_some());
    assert_eq!(FrameHeader::decode(REQUEST, 2), None);

    for index in [0, 4, 5] {
        let mut bytes = *REQUEST;
        bytes[index] = 0xff;
        assert_eq!(FrameHeader::decode(&bytes, u32::MAX), None);
    }
    let mut frame = FrameHeader {
        kind: FrameKind::Request,
        flags: u16::MAX,
        length: MAX_FRAME_PAYLOAD,
    };
    let max = frame.encode().unwrap();
    assert_eq!(&max[8..], &[0, 0xa0, 0, 0]);
    assert_eq!(FrameHeader::decode(&max, u32::MAX), Some(frame));
    frame.length += 1;
    assert_eq!(frame.encode(), None);
    let mut oversized = max;
    oversized[11] = 1;
    assert_eq!(FrameHeader::decode(&oversized, u32::MAX), None);
    frame.length = 0;
    assert_eq!(
        FrameHeader::decode(&frame.encode().unwrap(), 0),
        Some(frame)
    );
}

#[test]
fn socket_record_preserves_ids_and_distinguishes_publication_stages() {
    use SocketRecordKind::{Committed, Pending};

    let committed = committed_fixture();
    let mut pending = committed;
    pending[..8].copy_from_slice(b"CBMPEN\x01\0");
    assert_eq!(SOCKET.encode(Committed), Some(committed));
    assert_eq!(SOCKET.encode(Pending), Some(pending));
    assert_eq!(SocketRecord::decode(&pending, Pending), Some(SOCKET));
    assert_eq!(SocketRecord::decode(&committed, Committed), Some(SOCKET));
    assert_eq!(SocketRecord::decode(&pending, Committed), None);
    assert_eq!(SocketRecord::decode(&committed, Pending), None);
}

#[test]
fn socket_record_rejects_truncation_and_noncanonical_fields() {
    use SocketRecordKind::Committed;

    let record = committed_fixture();
    for length in 0..SOCKET_RECORD_SIZE {
        assert_eq!(SocketRecord::decode(&record[..length], Committed), None);
    }
    let mut too_long = record.to_vec();
    too_long.push(0);
    assert_eq!(SocketRecord::decode(&too_long, Committed), None);

    for (offset, byte) in [
        (6, 1),                      // Old committed record version.
        (31, 0x80), // Seconds outside the nonnegative i64 range.
        (35, 0xff), // Nanoseconds outside a single second.
        (36, 1),    // Reserved bytes.
        (40, b'/'), // Path separator in the name.
        (40, 0),    // Empty name.
        (SOCKET_RECORD_SIZE - 1, 1), // Nonzero padding.
    ] {
        let mut corrupt = record;
        corrupt[offset] = byte;
        assert_eq!(SocketRecord::decode(&corrupt, Committed), None, "{offset}");
    }
    let mut unterminated = record;
    unterminated[40..].fill(b'a');
    assert_eq!(SocketRecord::decode(&unterminated, Committed), None);
}

#[test]
fn anchor_names_are_bounded_bytes() {
    use SocketRecordKind::Pending;

    let max = [0xff; ANCHOR_NAME_CAPACITY - 1];
    let mut record = SocketRecord {
        anchor_name: &max,
        ctime_seconds: i64::MAX,
        ctime_nanoseconds: 999_999_999,
        ..SOCKET
    };
    let bytes = record.encode(Pending).unwrap();
    assert_eq!(SocketRecord::decode(&bytes, Pending), Some(record));
    for name in [
        b"".as_slice(),
        b"a/b",
        b"a\0b",
        &[b'a'; ANCHOR_NAME_CAPACITY],
    ] {
        record.anchor_name = name;
        assert_eq!(record.encode(Pending), None);
    }
    record.anchor_name = ANCHOR;
    record.ctime_nanoseconds += 1;
    assert_eq!(record.encode(Pending), None);
    record.ctime_nanoseconds = 0;
    record.ctime_seconds = -1;
    assert_eq!(record.encode(Pending), None);
}
