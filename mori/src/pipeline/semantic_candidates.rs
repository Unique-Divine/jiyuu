//! Enumerate LSH candidates in the order used for semantic edge admission.

use std::{ffi::c_int, mem::MaybeUninit, slice};

// Keep these constants and the C layouts below in sync with pass_semantic_edges.c.
const BAND_COUNT: usize = 16;
const BAND_BITS: usize = 4;
const BAND_VALUES: usize = 1 << BAND_BITS;
const MAX_CANDIDATES: usize = 200;
const SEEN_CAPACITY: usize = 8192;

#[derive(Clone, Copy)]
#[repr(C)]
struct Bucket {
    items: *const c_int,
    count: c_int,
    _capacity: c_int,
}

type Bands = [[Bucket; BAND_VALUES]; BAND_COUNT];

#[repr(C)]
struct Seen {
    ids: [c_int; SEEN_CAPACITY],
    stamps: [c_int; SEEN_CAPACITY],
}

impl Seen {
    fn insert(&mut self, target: c_int, stamp: c_int) -> bool {
        let mask = SEEN_CAPACITY - 1;
        let first = target as usize & mask;
        // The worker reuses these arrays across sources. A new stamp makes
        // the previous source's slots available without clearing the table.
        for probe in 0..SEEN_CAPACITY {
            let slot = (first + probe) & mask;
            if self.stamps[slot] != stamp {
                self.stamps[slot] = stamp;
                self.ids[slot] = target;
                return true;
            }
            if self.ids[slot] == target {
                return false;
            }
        }
        false
    }
}

/// C lends an immutable band table and its bucket storage for this call. The
/// worker exclusively owns zero-initialized scratch and an output array sized
/// for `MAX_CANDIDATES`. Each call uses a distinct, nonnegative source index
/// below `c_int::MAX` on that worker. Only the returned prefix is initialized.
#[unsafe(no_mangle)]
unsafe extern "C" fn mori_semantic_collect_candidates(
    bands: *const Bands,
    mut signature: u64,
    source: c_int,
    seen: *mut Seen,
    candidates: *mut [MaybeUninit<c_int>; MAX_CANDIDATES],
) -> c_int {
    // SAFETY: the synchronous caller supplies these arrays for the call and
    // shares only the read-only band table across workers.
    let (bands, seen, candidates) =
        unsafe { (&*bands, &mut *seen, &mut *candidates) };
    let stamp = source + 1;
    let mut count = 0;
    for band in bands {
        let bucket = &band[signature as usize & (BAND_VALUES - 1)];
        signature >>= BAND_BITS;
        if bucket.count <= 0 || bucket.count as usize > MAX_CANDIDATES {
            continue;
        }
        // SAFETY: a nonempty bucket has count valid C int entries. Empty
        // buckets can have null item pointers and were skipped above.
        let items = unsafe {
            slice::from_raw_parts(bucket.items, bucket.count as usize)
        };
        for &target in items {
            if target > source && seen.insert(target, stamp) {
                candidates[count].write(target);
                count += 1;
                if count == MAX_CANDIDATES {
                    return count as c_int;
                }
            }
        }
    }
    count as c_int
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bucket(items: &[c_int]) -> Bucket {
        Bucket {
            items: items.as_ptr(),
            count: items.len() as c_int,
            _capacity: items.len() as c_int,
        }
    }

    fn bands() -> Bands {
        [[Bucket {
            items: std::ptr::null(),
            count: 0,
            _capacity: 0,
        }; BAND_VALUES]; BAND_COUNT]
    }

    fn collect(
        bands: &Bands,
        signature: u64,
        source: c_int,
        seen: &mut Seen,
    ) -> Vec<c_int> {
        let mut output = [MaybeUninit::uninit(); MAX_CANDIDATES];
        // SAFETY: the test keeps the arrays and bucket items alive throughout
        // the call. Each reused Seen is passed a new source index.
        let count = unsafe {
            mori_semantic_collect_candidates(
                bands,
                signature,
                source,
                seen,
                &mut output,
            )
        };
        output[..count as usize]
            .iter()
            // SAFETY: the collector initializes exactly this prefix.
            .map(|item| unsafe { item.assume_init() })
            .collect()
    }

    #[test]
    fn keeps_band_and_bucket_order_when_candidates_repeat() {
        let mut bands = bands();
        let colliding = SEEN_CAPACITY as c_int + 8;
        let first = [0, 2, 8, colliding];
        let second = [3, 8, 9, colliding];
        bands[0][6] = bucket(&first);
        bands[15][11] = bucket(&second);
        let signature = 0xb000_0000_0000_0006;
        let mut seen = Seen {
            ids: [0; SEEN_CAPACITY],
            stamps: [0; SEEN_CAPACITY],
        };

        assert_eq!(
            collect(&bands, signature, 2, &mut seen),
            [8, colliding, 3, 9]
        );
        assert_eq!(collect(&bands, signature, 8, &mut seen), [colliding, 9]);
        assert!(collect(&bands, 0, 0, &mut seen).is_empty());
    }

    #[test]
    fn skips_overfull_buckets_and_caps_unique_candidates_across_bands() {
        let mut bands = bands();
        let overfull: Vec<_> = (1..=201).collect();
        let exactly_full: Vec<_> = (200..400).collect();
        let last = [199, 200, 400, 401, 402];
        bands[0][0] = bucket(&overfull);
        bands[1][0] = bucket(&exactly_full);
        bands[2][0] = bucket(&last);
        let mut seen = Seen {
            ids: [0; SEEN_CAPACITY],
            stamps: [0; SEEN_CAPACITY],
        };

        assert_eq!(collect(&bands, 0, 0, &mut seen), exactly_full);
        let expected: Vec<_> = (202..400).chain([400, 401]).collect();
        assert_eq!(collect(&bands, 0, 201, &mut seen), expected);
    }
}
