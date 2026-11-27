pub const FRAME_HEADER_SIZE: usize = 12;
pub const MAX_FRAME_PAYLOAD: u32 = 10 * 1024 * 1024;

// Version 1 stays stable across daemon generations so that incompatible
// executables can still exchange the HELLO conflict envelope.
const FRAME_PREFIX: [u8; 5] = *b"CBMD\x01";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum FrameKind {
    Request = 1,
    Response = 2,
}

impl TryFrom<u8> for FrameKind {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Request),
            2 => Ok(Self::Response),
            _ => Err(()),
        }
    }
}

/// An IPC header. Flags are opaque to the frame codec.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameHeader {
    pub kind: FrameKind,
    pub flags: u16,
    pub length: u32,
}

impl FrameHeader {
    pub fn encode(self) -> Option<[u8; FRAME_HEADER_SIZE]> {
        if self.length > MAX_FRAME_PAYLOAD {
            return None;
        }
        let mut header = [0; FRAME_HEADER_SIZE];
        header[..5].copy_from_slice(&FRAME_PREFIX);
        header[5] = self.kind as u8;
        header[6..8].copy_from_slice(&self.flags.to_be_bytes());
        header[8..].copy_from_slice(&self.length.to_be_bytes());
        Some(header)
    }

    /// Checks both the wire limit and the caller's limit before payload I/O.
    pub fn decode(bytes: &[u8], max_payload_length: u32) -> Option<Self> {
        let [
            prefix @ ..,
            kind,
            flags_hi,
            flags_lo,
            len_3,
            len_2,
            len_1,
            len_0,
        ] = *<&[u8; FRAME_HEADER_SIZE]>::try_from(bytes).ok()?;
        let length = u32::from_be_bytes([len_3, len_2, len_1, len_0]);
        if prefix != FRAME_PREFIX
            || length > MAX_FRAME_PAYLOAD.min(max_payload_length)
        {
            return None;
        }
        Some(Self {
            kind: kind.try_into().ok()?,
            flags: u16::from_be_bytes([flags_hi, flags_lo]),
            length,
        })
    }
}
