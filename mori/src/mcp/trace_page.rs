//! Trace cursor identity, validation, and paging in canonical (hop, node ID) order.

use std::ffi::{CStr, c_char, c_int, c_void};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Leg {
    Outbound,
    Inbound,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Watermark {
    hop: i32,
    node_id: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CursorError {
    Invalid,
    ParamsMismatch,
    Stale,
    Unsupported,
    MissingWatermark,
}

impl CursorError {
    fn message(self) -> &'static CStr {
        match self {
            Self::Invalid => {
                c"invalid_cursor: unrecognized token — re-run the original query without 'cursor'"
            }
            Self::ParamsMismatch => {
                c"cursor_params_mismatch: this cursor was issued for different arguments — \
                 pass the cursor back with ALL other arguments identical"
            }
            Self::Stale => {
                c"stale_cursor: the project was reindexed since this cursor was issued — \
                 re-run the original query without 'cursor' (node identities changed)"
            }
            Self::Unsupported => {
                c"cursor_unsupported: this project's index predates generation tracking, so cursor \
                 staleness cannot be detected. Re-call with a higher 'limit' instead, or re-index \
                 the project to enable pagination."
            }
            Self::MissingWatermark => {
                c"invalid_cursor: watermark is not present in the selected visible trace leg — \
                 re-run the original query without 'cursor'"
            }
        }
    }
}

#[repr(C)]
struct Params {
    project: *const c_char,
    function_name: *const c_char,
    direction: *const c_char,
    mode: *const c_char,
    parameter_name: *const c_char,
    depth: c_int,
    limit: c_int,
    include_tests: bool,
    risk_labels: bool,
    include_evidence: bool,
    include_overrides: bool,
}

fn hash_params(
    fields: [&[u8]; 5],
    params: &Params,
    edges: impl Iterator<Item = impl AsRef<[u8]>>,
) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    let mut add = |bytes: &[u8]| {
        for &byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    };
    for field in fields {
        add(field);
        add(b"|");
    }
    let numbers = format!(
        "{}|{}|{}|{}|{}",
        params.depth,
        u8::from(params.include_tests),
        u8::from(params.risk_labels),
        u8::from(params.include_evidence),
        params.limit
    );
    add(numbers.as_bytes());
    if params.include_overrides {
        add(b"|include_overrides");
    }
    add(b"|edge_types=");
    for kind in edges {
        add(kind.as_ref());
        add(b";");
    }
    hash
}

#[derive(Clone, Copy)]
struct Cursor {
    leg: Leg,
    watermark: Watermark,
}

fn c_decimal(text: &str) -> &str {
    // strtol and strtoll accept leading ASCII whitespace and optional '+'.
    text.trim_start_matches(|ch: char| ch.is_ascii_whitespace() || ch == '\u{b}')
}

impl Cursor {
    fn decode(
        token: &str,
        generation: &str,
        hash: u64,
    ) -> Result<Self, CursorError> {
        let invalid = CursorError::Invalid;
        let mut fields = token.split('.');
        if fields.next() != Some("c1") {
            return Err(invalid);
        }
        let leg = match fields.next() {
            Some("o") => Leg::Outbound,
            Some("i") => Leg::Inbound,
            _ => return Err(invalid),
        };
        let stamp = fields
            .next()
            .filter(|part| !part.is_empty() && part.len() < 96)
            .ok_or(invalid)?;
        let token_hash = fields
            .next()
            .filter(|part| {
                part.len() == 16
                    && part.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
            .ok_or(invalid)?;
        let hop: i32 = c_decimal(fields.next().ok_or(invalid)?)
            .parse()
            .map_err(|_| invalid)?;
        let node_id: i64 = c_decimal(fields.next().ok_or(invalid)?)
            .parse()
            .map_err(|_| invalid)?;
        if fields.next().is_some() || hop < 1 || node_id < 1 {
            return Err(invalid);
        }
        if u64::from_str_radix(token_hash, 16) != Ok(hash) {
            return Err(CursorError::ParamsMismatch);
        }
        if generation != stamp {
            return Err(CursorError::Stale);
        }
        Ok(Self {
            leg,
            watermark: Watermark { hop, node_id },
        })
    }

    fn encode(self, generation: &str, hash: u64) -> String {
        let leg = match self.leg {
            Leg::Outbound => 'o',
            Leg::Inbound => 'i',
        };
        let Watermark { hop, node_id } = self.watermark;
        format!("c1.{leg}.{generation}.{hash:016x}.{hop}.{node_id}")
    }
}

#[repr(C)]
struct Pagination {
    hash: u64,
    node_id: i64,
    hop: c_int,
    leg: c_char,
}

impl Pagination {
    fn cursor(&self) -> Option<Cursor> {
        let leg = match self.leg.to_ne_bytes()[0] {
            b'o' => Leg::Outbound,
            b'i' => Leg::Inbound,
            _ => return None,
        };
        Some(Cursor {
            leg,
            watermark: Watermark {
                hop: self.hop,
                node_id: self.node_id,
            },
        })
    }
}

type NextEdge = unsafe extern "C" fn(*mut c_void) -> *const c_char;

fn generation(stamp: &str) -> Option<&str> {
    (stamp != "legacy").then_some(stamp)
}

#[cfg_attr(feature = "native-engine-abi", unsafe(no_mangle))]
unsafe extern "C" fn mori_trace_cursor(
    params: &Params,
    stamp: *const c_char,
    token: *const c_char,
    next_edge: NextEdge,
    edges: *mut c_void,
    state: &mut Pagination,
) -> *const c_char {
    // SAFETY: C lends normalized request strings, the generation, and an
    // iterator over the original edge_types array for the duration of this call.
    let fields = [
        params.project,
        params.function_name,
        params.direction,
        params.mode,
        params.parameter_name,
    ]
    .map(|ptr| {
        if ptr.is_null() {
            b"".as_slice()
        } else {
            unsafe { CStr::from_ptr(ptr) }.to_bytes()
        }
    });
    let edge_types = std::iter::from_fn(|| {
        let ptr = unsafe { next_edge(edges) };
        (!ptr.is_null()).then(|| unsafe { CStr::from_ptr(ptr) }.to_bytes())
    });
    state.hash = hash_params(fields, params, edge_types);
    if token.is_null() {
        return std::ptr::null();
    }
    let token = unsafe { CStr::from_ptr(token) };
    if token.is_empty() {
        return std::ptr::null();
    }
    let stamp = unsafe { CStr::from_ptr(stamp) }
        .to_str()
        .unwrap_or_default();
    let Some(stamp) = generation(stamp) else {
        return CursorError::Unsupported.message().as_ptr();
    };
    match token
        .to_str()
        .ok()
        .ok_or(CursorError::Invalid)
        .and_then(|token| Cursor::decode(token, stamp, state.hash))
    {
        Ok(cursor) => {
            state.node_id = cursor.watermark.node_id;
            state.hop = cursor.watermark.hop;
            state.leg = match cursor.leg {
                Leg::Outbound => c_char::from_ne_bytes([b'o']),
                Leg::Inbound => c_char::from_ne_bytes([b'i']),
            };
            std::ptr::null()
        }
        Err(error) => error.message().as_ptr(),
    }
}

#[repr(C)]
struct LegRows {
    context: *const c_void,
    count: c_int,
}

#[repr(C)]
struct PageStart {
    outbound: c_int,
    inbound: c_int,
}

#[repr(C)]
struct Page {
    outbound: c_int,
    inbound: c_int,
    has_more: bool,
    next: [c_char; 192],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct WatermarkRow {
    node_id: i64,
    hop: c_int,
}

type ReadRow = unsafe extern "C" fn(*const c_void, c_int) -> WatermarkRow;

fn after_watermark(
    count: c_int,
    watermark: Watermark,
    mut read: impl FnMut(c_int) -> Watermark,
) -> Result<c_int, CursorError> {
    let (mut low, mut high) = (0, count);
    while low < high {
        let mid = low + (high - low) / 2;
        match read(mid).cmp(&watermark) {
            std::cmp::Ordering::Less => low = mid + 1,
            std::cmp::Ordering::Greater => high = mid,
            std::cmp::Ordering::Equal => return Ok(mid + 1),
        }
    }
    Err(CursorError::MissingWatermark)
}

impl LegRows {
    fn watermark(&self, read: ReadRow, index: c_int) -> Watermark {
        // SAFETY: The caller keeps the leg alive through the call and supplies
        // its exact visible count. All indices are checked by the page logic.
        let row = unsafe { read(self.context, index) };
        Watermark {
            hop: row.hop,
            node_id: row.node_id,
        }
    }
}

#[cfg_attr(feature = "native-engine-abi", unsafe(no_mangle))]
extern "C" fn mori_trace_page_start(
    state: &Pagination,
    out: &LegRows,
    inbound: &LegRows,
    read: ReadRow,
    start: &mut PageStart,
) -> *const c_char {
    let Some(cursor) = state.cursor() else {
        return std::ptr::null();
    };
    let rows = match cursor.leg {
        Leg::Outbound => out,
        Leg::Inbound => inbound,
    };
    let index = after_watermark(rows.count, cursor.watermark, |index| {
        rows.watermark(read, index)
    });
    match (cursor.leg, index) {
        (Leg::Outbound, Ok(index)) => start.outbound = index,
        (Leg::Inbound, Ok(index)) => {
            start.outbound = out.count;
            start.inbound = index;
        }
        (_, Err(error)) => return error.message().as_ptr(),
    }
    std::ptr::null()
}

#[cfg_attr(feature = "native-engine-abi", unsafe(no_mangle))]
unsafe extern "C" fn mori_trace_page(
    state: &Pagination,
    stamp: *const c_char,
    out: &LegRows,
    inbound: &LegRows,
    read: ReadRow,
    start: &PageStart,
    budget: c_int,
    page: &mut Page,
) {
    // SAFETY: The store lends a NUL-terminated generation for this call.
    let stamp = unsafe { CStr::from_ptr(stamp) }
        .to_str()
        .unwrap_or_default();
    let out_remaining = out.count - start.outbound;
    let in_remaining = inbound.count - start.inbound;
    page.outbound = out_remaining.min(budget);
    page.inbound = in_remaining.min(budget - page.outbound);
    page.has_more = page.outbound < out_remaining || page.inbound < in_remaining;
    page.next[0] = 0;
    let Some(stamp) = generation(stamp).filter(|_| page.has_more) else {
        return;
    };
    let (leg, rows, end) = if page.inbound > 0 {
        (Leg::Inbound, inbound, start.inbound + page.inbound)
    } else if page.outbound > 0 {
        (Leg::Outbound, out, start.outbound + page.outbound)
    } else {
        return;
    };
    let cursor = Cursor {
        leg,
        watermark: rows.watermark(read, end - 1),
    };
    let encoded = cursor.encode(stamp, state.hash);
    for (slot, byte) in page.next.iter_mut().zip(encoded.bytes().chain([0])) {
        *slot = c_char::from_ne_bytes([byte]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAMP: &CStr = c"u0001020304050607g4";
    const HASH: u64 = 0x2e36648389ed6960;

    fn params() -> Params {
        Params {
            project: c"p".as_ptr(),
            function_name: c"p.app.run".as_ptr(),
            direction: c"both".as_ptr(),
            mode: std::ptr::null(),
            parameter_name: std::ptr::null(),
            depth: 3,
            limit: 2,
            include_tests: false,
            risk_labels: false,
            include_evidence: false,
            include_overrides: false,
        }
    }

    unsafe extern "C" fn next_edge(context: *mut c_void) -> *const c_char {
        // SAFETY: The caller lends the local iterator.
        unsafe { &mut *context.cast::<std::slice::Iter<'_, &CStr>>() }
            .next()
            .map_or_else(std::ptr::null, |edge| edge.as_ptr())
    }

    fn decode(
        params: &Params,
        stamp: &CStr,
        token: &CStr,
        edges: &[&CStr],
    ) -> Result<Pagination, &'static CStr> {
        let mut state = Pagination {
            hash: 0,
            node_id: 0,
            hop: 0,
            leg: 0,
        };
        let mut edges = edges.iter();
        // SAFETY: All strings and the edge iterator remain valid through the call.
        let error = unsafe {
            mori_trace_cursor(
                params,
                stamp.as_ptr(),
                token.as_ptr(),
                next_edge,
                std::ptr::from_mut(&mut edges).cast(),
                &mut state,
            )
        };
        if error.is_null() {
            Ok(state)
        } else {
            // SAFETY: Errors borrow static, terminated strings.
            Err(unsafe { CStr::from_ptr(error) })
        }
    }

    #[test]
    fn cursor_identity_preserves_raw_edge_order_and_overrides() {
        let mut request = params();
        let edges = [c"CALLS", c"IMPORTS"];
        assert_eq!(decode(&request, STAMP, c"", &edges).unwrap().hash, HASH);
        assert_eq!(
            decode(
                &request,
                STAMP,
                c"",
                &[c"CALLS", c"<non-string>", c"IMPORTS"]
            )
            .unwrap()
            .hash,
            0x75e5f6d6908eae5e,
        );
        request.include_overrides = true;
        assert_eq!(
            decode(&request, STAMP, c"", &edges).unwrap().hash,
            0x58a2f0a0814c57de,
        );
        assert_ne!(
            decode(&request, STAMP, c"", &[c"IMPORTS", c"CALLS"])
                .unwrap()
                .hash,
            0x58a2f0a0814c57de,
        );
    }

    unsafe extern "C" fn read_row(
        context: *const c_void,
        index: c_int,
    ) -> WatermarkRow {
        // SAFETY: The caller checks bounds and lends an initialized row array.
        unsafe { *context.cast::<WatermarkRow>().add(index as usize) }
    }

    fn leg(rows: &[WatermarkRow]) -> LegRows {
        LegRows {
            context: rows.as_ptr().cast(),
            count: rows.len() as c_int,
        }
    }

    fn page(
        stamp: &CStr,
        token: &CStr,
        budget: c_int,
    ) -> Result<(PageStart, Page), &'static CStr> {
        let state = decode(&params(), stamp, token, &[c"CALLS", c"IMPORTS"])?;
        let outbound = leg(&[row(1, 7), row(1, 9)]);
        let inbound = leg(&[row(1, 4), row(2, 3), row(2, 5)]);
        let mut start = PageStart {
            outbound: 0,
            inbound: 0,
        };
        let error = mori_trace_page_start(
            &state, &outbound, &inbound, read_row, &mut start,
        );
        if !error.is_null() {
            // SAFETY: Errors borrow static, terminated strings.
            return Err(unsafe { CStr::from_ptr(error) });
        }
        let mut page = Page {
            outbound: 0,
            inbound: 0,
            has_more: false,
            next: [0; 192],
        };
        // SAFETY: The stamp is terminated and the two borrowed legs are alive.
        unsafe {
            mori_trace_page(
                &state,
                stamp.as_ptr(),
                &outbound,
                &inbound,
                read_row,
                &start,
                budget,
                &mut page,
            )
        };
        Ok((start, page))
    }

    const fn row(hop: c_int, node_id: i64) -> WatermarkRow {
        WatermarkRow { node_id, hop }
    }

    impl Page {
        fn token(&self) -> &CStr {
            // SAFETY: The pager always terminates its next token.
            unsafe { CStr::from_ptr(self.next.as_ptr()) }
        }
    }

    #[test]
    fn exact_leg_boundary_and_trimmed_page_use_last_emitted_row() {
        let (_, first) = page(STAMP, c"", 2).unwrap();
        assert_eq!((first.outbound, first.inbound), (2, 0));
        assert_eq!(
            first.token(),
            c"c1.o.u0001020304050607g4.2e36648389ed6960.1.9"
        );
        let (_, trimmed) = page(STAMP, first.token(), 1).unwrap();
        assert_eq!((trimmed.outbound, trimmed.inbound), (0, 1));
        assert_eq!(
            trimmed.token(),
            c"c1.i.u0001020304050607g4.2e36648389ed6960.1.4"
        );
        let (start, last) = page(STAMP, trimmed.token(), 2).unwrap();
        assert_eq!((start.outbound, start.inbound), (2, 1));
        assert_eq!((last.outbound, last.inbound), (0, 2));
        assert!(!last.has_more);
        assert!(last.token().is_empty());
    }

    #[test]
    fn rejects_bad_replays_and_never_mints_a_legacy_cursor() {
        let token = c"c1.o.u0001020304050607g4.2e36648389ed6960.1.7";
        let (_, legacy) = page(c"legacy", c"", 1).unwrap();
        assert!(legacy.has_more);
        assert!(legacy.token().is_empty());
        for (stamp, token, error) in [
            (c"legacy", token, CursorError::Unsupported),
            (c"u0001020304050607g5", token, CursorError::Stale),
            (STAMP, c"c1.o.u0001020304050607g4.3e36648389ed6960.1.7", CursorError::ParamsMismatch),
            (STAMP, c"c1.o.u0001020304050607g4.2e36648389ed6960.1.6", CursorError::MissingWatermark),
            (STAMP, c"c1.o", CursorError::Invalid),
            (STAMP, c"c1.o.u0001020304050607g4.2e36648389ed6960.1.9223372036854775808", CursorError::Invalid),
        ] {
            assert_eq!(page(stamp, token, 1).err(), Some(error.message()));
        }
    }
}
