//! Apply the per-function edge budget to scored semantic pairs.
//!
//! Admission is best-first, not first-come: when a function has more
//! candidate partners than its budget, it keeps its best ones. Judged on
//! held-out repositories (2026-10-07), the pairs first-come admitted over
//! budget were 0 of 17 related; the ones best-first admits instead, 7 of 20.

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

    /// The removed C admission, kept as the oracle: stable descending score,
    /// equal scores (signed zeros included) in pair order.
    fn reference(
        pairs: &[Pair],
        max_edges: c_int,
        counts: &mut [c_int],
    ) -> Vec<bool> {
        let mut order: Vec<usize> =
            (0..pairs.len()).filter(|&k| !pairs[k].below).collect();
        order.sort_by(|&a, &b| {
            let (x, y) = (pairs[a].score, pairs[b].score);
            if x == y {
                Ordering::Equal
            } else if x > y {
                Ordering::Less
            } else {
                Ordering::Greater
            }
        });
        let mut admitted = vec![false; pairs.len()];
        for k in order {
            let (i, j) = (pairs[k].i as usize, pairs[k].j as usize);
            if counts[i] < max_edges && counts[j] < max_edges {
                counts[i] += 1;
                counts[j] += 1;
                admitted[k] = true;
            }
        }
        admitted
    }

    /// The cases from the C parity comparison: a fixed-seed generator with
    /// frequent ties, signed zeros, infinities and functions that start at
    /// or over the budget.
    #[test]
    fn matches_the_c_reference_on_generated_cases() {
        const SCORES: [f32; 12] = [
            0.95,
            0.90,
            0.90,
            0.85,
            0.80,
            0.0,
            -0.0,
            f32::INFINITY,
            f32::NEG_INFINITY,
            0.75,
            1.0,
            0.85,
        ];
        let mut state: u32 = 0x5eed_1234;
        let mut next = || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            state >> 8
        };
        for case in 0..4000 {
            let func_count = 2 + (next() % 9) as usize;
            let n = (next() % 49) as usize;
            let max_edges = (next() % 5) as c_int;
            let start: Vec<c_int> = (0..func_count)
                .map(|_| match next() % 8 {
                    r @ 6..=7 => max_edges + r as c_int - 6,
                    _ => 0,
                })
                .collect();
            let pairs: Vec<Pair> = (0..n)
                .map(|k| {
                    let i = (next() % (func_count as u32 - 1)) as c_int;
                    let j = i
                        + 1
                        + (next() % (func_count as u32 - 1 - i as u32)) as c_int;
                    let score = SCORES[(next() % SCORES.len() as u32) as usize];
                    pair(i, j, k as c_int, score, next() % 4 == 0)
                })
                .collect();
            let mut want_counts = start.clone();
            let mut got_counts = start;
            let want = reference(&pairs, max_edges, &mut want_counts);
            let got = admit(&pairs, max_edges, &mut got_counts);
            assert_eq!(got, want, "case {case}");
            assert_eq!(got_counts, want_counts, "case {case}");
        }
    }
}
