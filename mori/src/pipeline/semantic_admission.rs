//! Apply the per-function edge budget to scored semantic pairs.

use std::{cmp::Ordering, ffi::c_int, mem::MaybeUninit, slice};

// Keep this layout in sync with semantic/semantic_admission.h.
#[repr(C)]
struct Pair {
    source_id: i64,
    target_id: i64,
    score: f32,
    same_file: bool,
    below: bool,
    i: c_int,
    j: c_int,
    c: c_int,
}

/// The caller supplies `n` pairs in canonical order and two writable arrays
/// of length `n`. Each endpoint indexes `counts`, which has `func_count`
/// initialized slots. The arrays do not overlap.
#[unsafe(no_mangle)]
unsafe extern "C" fn mori_semantic_admit_best_first(
    pairs: *const Pair,
    n: c_int,
    max_edges: c_int,
    counts: *mut c_int,
    func_count: c_int,
    order: *mut MaybeUninit<c_int>,
    admitted: *mut MaybeUninit<bool>,
) {
    if n <= 0 {
        return;
    }
    // SAFETY: the caller lends these arrays and keeps them alive for this call.
    let (pairs, counts, order, admitted) = unsafe {
        (
            slice::from_raw_parts(pairs, n as usize),
            slice::from_raw_parts_mut(counts, func_count as usize),
            slice::from_raw_parts_mut(order, n as usize),
            slice::from_raw_parts_mut(admitted, n as usize),
        )
    };

    admitted.fill(MaybeUninit::new(false));
    let mut eligible_count = 0;
    for (index, pair) in pairs.iter().enumerate() {
        if !pair.below {
            order[eligible_count].write(index as c_int);
            eligible_count += 1;
        }
    }
    // SAFETY: exactly the eligible prefix was initialized above.
    let order = unsafe {
        slice::from_raw_parts_mut(
            order.as_mut_ptr().cast::<c_int>(),
            eligible_count,
        )
    };
    order.sort_unstable_by(|&a, &b| {
        let a_score = pairs[a as usize].score;
        let b_score = pairs[b as usize].score;
        // Equal scores, including signed zero, keep canonical pair order.
        let by_score = if a_score == b_score {
            Ordering::Equal
        } else {
            b_score.total_cmp(&a_score)
        };
        by_score.then_with(|| a.cmp(&b))
    });

    for &index in order.iter() {
        let pair = &pairs[index as usize];
        let (i, j) = (pair.i as usize, pair.j as usize);
        if counts[i] < max_edges && counts[j] < max_edges {
            counts[i] += 1;
            counts[j] += 1;
            admitted[index as usize].write(true);
        }
    }
    // SAFETY: every admission slot was initialized before ranking.
    unsafe { crate::ffi_output::written(admitted.as_ptr(), admitted.len()) };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(i: c_int, j: c_int, c: c_int, score: f32, below: bool) -> Pair {
        Pair {
            source_id: i.into(),
            target_id: j.into(),
            score,
            same_file: false,
            below,
            i,
            j,
            c,
        }
    }

    fn admit(
        pairs: &[Pair],
        max_edges: c_int,
        counts: &mut [c_int],
    ) -> Vec<bool> {
        let mut order = vec![MaybeUninit::uninit(); pairs.len()];
        let mut admitted = vec![MaybeUninit::uninit(); pairs.len()];
        // SAFETY: the test owns every array for the duration of the call.
        unsafe {
            mori_semantic_admit_best_first(
                pairs.as_ptr(),
                pairs.len() as c_int,
                max_edges,
                counts.as_mut_ptr(),
                counts.len() as c_int,
                order.as_mut_ptr(),
                admitted.as_mut_ptr(),
            );
        }
        admitted
            .iter()
            // SAFETY: the admission writes every slot when n is positive.
            .map(|slot| unsafe { slot.assume_init() })
            .collect()
    }

    #[test]
    fn keeps_the_best_partners_within_budget() {
        let pairs = [
            pair(0, 1, 0, 0.80, false),
            pair(0, 2, 1, 0.90, false),
            pair(0, 3, 2, 0.95, false),
            pair(0, 4, 3, 0.99, true),
            pair(5, 6, 0, 0.90, false),
        ];
        let mut counts = [0; 7];
        assert_eq!(
            admit(&pairs, 2, &mut counts),
            [false, true, true, false, true]
        );
        assert_eq!(counts, [2, 0, 1, 1, 0, 1, 1]);
    }

    #[test]
    fn equal_scores_keep_canonical_order() {
        let pairs = [
            pair(0, 2, 0, 0.85, false),
            pair(1, 2, 0, 0.85, false),
            pair(3, 4, 0, -0.0, false),
            pair(3, 5, 1, 0.0, false),
        ];
        let mut counts = [0; 6];
        assert_eq!(admit(&pairs, 1, &mut counts), [true, false, true, false]);
        assert_eq!(counts, [1, 0, 1, 1, 1, 0]);
    }

    #[test]
    fn existing_counts_and_zero_budget_admit_nothing_over_budget() {
        let pairs = [pair(0, 1, 0, 0.9, false), pair(1, 2, 0, 0.8, false)];
        let mut counts = [0, 1, 0];
        assert_eq!(admit(&pairs, 1, &mut counts), [false, false]);
        assert_eq!(counts, [0, 1, 0]);

        let mut counts = [0; 3];
        assert_eq!(admit(&pairs, 0, &mut counts), [false, false]);
        assert_eq!(counts, [0; 3]);
    }

    #[test]
    fn empty_and_all_below_threshold_inputs_leave_counts_alone() {
        let mut counts = [0; 2];
        assert!(admit(&[], 1, &mut counts).is_empty());
        let pairs = [pair(0, 1, 0, 0.9, true)];
        assert_eq!(admit(&pairs, 1, &mut counts), [false]);
        assert_eq!(counts, [0, 0]);
    }
}
