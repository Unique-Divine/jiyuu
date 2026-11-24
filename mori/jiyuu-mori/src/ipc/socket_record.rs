pub const ANCHOR_NAME_CAPACITY: usize = 64;
pub const SOCKET_RECORD_SIZE: usize = 40 + ANCHOR_NAME_CAPACITY;
const NANOS_PER_SECOND: u32 = 1_000_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SocketRecordKind {
    Pending,
    Committed,
}

impl SocketRecordKind {
    pub(super) const fn magic(self) -> [u8; 8] {
        match self {
            Self::Pending => *b"CBMPEN\x01\0",
            Self::Committed => *b"CBMSOC\x02\0",
        }
    }
}

/// POSIX socket identity and the generation's anchor filename. The caller
/// must cross-check the anchor against its endpoint and the live filesystem.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SocketRecord<'a> {
    pub device: u64,
    pub inode: u64,
    pub ctime_seconds: i64,
    pub ctime_nanoseconds: u32,
    pub anchor_name: &'a [u8],
}

impl<'a> SocketRecord<'a> {
    pub fn encode(
        self,
        kind: SocketRecordKind,
    ) -> Option<[u8; SOCKET_RECORD_SIZE]> {
        if !self.valid() {
            return None;
        }

        let mut record = [0; SOCKET_RECORD_SIZE];
        record[..8].copy_from_slice(&kind.magic());
        record[8..16].copy_from_slice(&self.device.to_le_bytes());
        record[16..24].copy_from_slice(&self.inode.to_le_bytes());
        record[24..32].copy_from_slice(&self.ctime_seconds.to_le_bytes());
        record[32..36].copy_from_slice(&self.ctime_nanoseconds.to_le_bytes());
        record[40..40 + self.anchor_name.len()]
            .copy_from_slice(self.anchor_name);
        Some(record)
    }

    pub fn decode(bytes: &'a [u8], kind: SocketRecordKind) -> Option<Self> {
        let bytes: &[u8; SOCKET_RECORD_SIZE] = bytes.try_into().ok()?;
        if bytes[..8] != kind.magic() || bytes[36..40] != [0; 4] {
            return None;
        }

        let anchor = &bytes[40..];
        let anchor_end = anchor.iter().position(|byte| *byte == 0)?;
        if anchor[anchor_end..].iter().any(|byte| *byte != 0) {
            return None;
        }

        let record = Self {
            device: u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
            inode: u64::from_le_bytes(bytes[16..24].try_into().unwrap()),
            ctime_seconds: i64::from_le_bytes(bytes[24..32].try_into().unwrap()),
            ctime_nanoseconds: u32::from_le_bytes(
                bytes[32..36].try_into().unwrap(),
            ),
            anchor_name: &anchor[..anchor_end],
        };
        record.valid().then_some(record)
    }

    fn valid(self) -> bool {
        self.ctime_seconds >= 0
            && self.ctime_nanoseconds < NANOS_PER_SECOND
            && (1..ANCHOR_NAME_CAPACITY).contains(&self.anchor_name.len())
            && !self.anchor_name.iter().any(|byte| matches!(byte, 0 | b'/'))
    }
}
