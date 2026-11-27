//! Normalize the decoded hops and count of one `ingest_traces` entry.

const MIN_HOPS: usize = 2;
const MAX_HOPS: usize = 256;

fn normalize_count(
    hop_count: usize,
    hops_are_present: impl FnOnce() -> bool,
    count: i64,
) -> Option<i64> {
    if !(MIN_HOPS..=MAX_HOPS).contains(&hop_count) || !hops_are_present() {
        return None;
    }
    Some(count.max(1))
}

#[cfg(all(feature = "c-backend", not(test)))]
mod native {
    use std::ffi::c_char;

    type TraceNormalizer =
        unsafe extern "C" fn(*const *const c_char, usize, *mut i64) -> bool;

    unsafe extern "C" {
        fn cbm_mcp_set_ingest_trace_normalizer(normalize: TraceNormalizer);
    }

    unsafe extern "C" fn normalize(
        hops: *const *const c_char,
        hop_count: usize,
        count: *mut i64,
    ) -> bool {
        // SAFETY: C supplies a live count and initializes exactly hop_count
        // pointers whenever the hop count is valid. Each non-null pointer
        // addresses a NUL-terminated string owned by the request document.
        let observed_count = unsafe { *count };
        let Some(normalized) = super::normalize_count(
            hop_count,
            || unsafe {
                std::slice::from_raw_parts(hops, hop_count)
                    .iter()
                    .all(|&hop| !hop.is_null() && *hop != 0)
            },
            observed_count,
        ) else {
            return false;
        };
        // SAFETY: count is the caller's live output slot.
        unsafe { *count = normalized };
        true
    }

    pub(crate) unsafe fn register() {
        // SAFETY: main registers before request threads start.
        unsafe { cbm_mcp_set_ingest_trace_normalizer(normalize) };
    }
}

#[cfg(all(feature = "c-backend", not(test)))]
pub(crate) use native::register;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_hop_counts_before_reading_hops() {
        for hop_count in [0, 1, MAX_HOPS + 1] {
            assert_eq!(
                normalize_count(hop_count, || panic!("invalid hop count"), 1),
                None
            );
        }
    }

    #[test]
    fn preserves_positive_counts_and_defaults_other_counts_to_one() {
        assert_eq!(normalize_count(MIN_HOPS, || true, 0), Some(1));
        assert_eq!(normalize_count(MIN_HOPS, || true, -2), Some(1));
        assert_eq!(normalize_count(MAX_HOPS, || true, i64::MAX), Some(i64::MAX));
        assert_eq!(normalize_count(MAX_HOPS, || false, 7), None);
    }
}
