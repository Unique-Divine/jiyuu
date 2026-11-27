//! Score a symbol after the index pass has gathered its facts.

use std::ffi::{c_double, c_int};

const GENERIC_MIN_FILES: c_int = 5;

extern "C" fn score(
    num_refs: c_int,
    name_distinct_files: c_int,
    private_name: bool,
    distinctive_name: bool,
    test_scaffolding: bool,
) -> c_double {
    let mut score = f64::from(num_refs.max(0)).sqrt();
    if private_name {
        score *= 0.1;
    }
    if name_distinct_files >= GENERIC_MIN_FILES {
        score *= 0.1;
    }
    if distinctive_name {
        score *= 10.0;
    }
    if test_scaffolding {
        score *= 0.1;
    }
    score
}

#[cfg(not(test))]
unsafe extern "C" {
    fn cbm_pipeline_set_importance_scorer(
        scorer: extern "C" fn(c_int, c_int, bool, bool, bool) -> c_double,
    );
}

#[cfg(not(test))]
pub(crate) unsafe fn register() {
    // SAFETY: Registration runs before indexing, and score has process lifetime.
    unsafe { cbm_pipeline_set_importance_scorer(score) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores_with_the_native_limits_and_multipliers() {
        assert_eq!(score(c_int::MIN, 5, true, true, true).to_bits(), 0);
        assert_eq!(score(9, 4, false, false, false), 3.0);
        assert_eq!(score(9, 5, false, false, false), 3.0 * 0.1);
        assert_eq!(score(9, 4, true, false, false), 3.0 * 0.1);
        assert_eq!(score(9, 4, false, true, false), 30.0);
        assert_eq!(score(9, 4, false, false, true), 3.0 * 0.1);
        assert_eq!(
            score(9, 5, true, true, true),
            (((3.0 * 0.1) * 0.1) * 10.0) * 0.1
        );
    }

    #[cfg(feature = "c-backend")]
    #[test]
    fn matches_the_native_rule_bit_for_bit() {
        use std::ffi::c_char;
        use std::ptr::null;

        #[repr(C)]
        struct Inputs {
            name: *const c_char,
            file_path: *const c_char,
            num_refs: c_int,
            name_distinct_files: c_int,
            tests_target: bool,
        }

        unsafe extern "C" {
            fn cbm_pipeline_importance_score(inputs: *const Inputs) -> c_double;
        }

        for (name, private, distinctive) in [
            (null(), false, false),
            (c"_plain".as_ptr(), true, false),
            (c"_long_key".as_ptr(), true, true),
            (c"getUser".as_ptr(), false, false),
            (c"getURLId".as_ptr(), false, true),
            (c"longword".as_ptr(), false, false),
            (c"get_name".as_ptr(), false, true),
        ] {
            for (
                num_refs,
                name_distinct_files,
                path,
                tests_target,
                test_scaffolding,
            ) in [
                (c_int::MIN, 5, null(), false, false),
                (-1, 4, null(), false, false),
                (0, 6, null(), false, false),
                (1, 4, null(), false, false),
                (2, 5, c"pkg/testutil_helpers.go".as_ptr(), false, false),
                (37, 6, c"pkg/thing_test.go".as_ptr(), false, true),
                (c_int::MAX, c_int::MAX, null(), true, true),
            ] {
                let inputs = Inputs {
                    name,
                    file_path: path,
                    num_refs,
                    name_distinct_files,
                    tests_target,
                };
                // SAFETY: Inputs and any non-null strings stay live
                // for the call. Test binaries leave C's rule installed.
                let native = unsafe { cbm_pipeline_importance_score(&inputs) };
                let rust = score(
                    num_refs,
                    name_distinct_files,
                    private,
                    distinctive,
                    test_scaffolding,
                );
                assert_eq!(native.to_bits(), rust.to_bits());
            }
        }
    }
}
