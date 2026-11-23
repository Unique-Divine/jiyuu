//! Compare semantic manifest rows without copying paths or digests.

use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_int};

/// Layout of `cbm_file_hash_t` in `store.h`.
#[repr(C)]
struct FileHash {
    project: *const c_char,
    relative_path: *const c_char,
    sha256: *const c_char,
    mtime_ns: i64,
    size: i64,
}

impl FileHash {
    /// The row's C strings remain live for the duration of this borrow.
    unsafe fn semantic_fields(&self) -> Option<(&[u8], &[u8])> {
        if self.relative_path.is_null() || self.sha256.is_null() {
            return None;
        }
        // SAFETY: the manifest owner retains both NUL-terminated strings.
        Some(unsafe {
            (
                CStr::from_ptr(self.relative_path).to_bytes(),
                CStr::from_ptr(self.sha256).to_bytes(),
            )
        })
    }
}

unsafe extern "C" fn equal(
    stored: *const FileHash,
    stored_count: c_int,
    baseline: *const FileHash,
    baseline_count: c_int,
) -> bool {
    if stored_count != baseline_count || stored_count < 0 {
        return false;
    }
    if stored_count == 0 {
        return true;
    }
    if stored.is_null() || baseline.is_null() {
        return false;
    }
    // SAFETY: C owns both fully initialized arrays and their input strings
    // for the duration of this synchronous callback.
    let (stored, baseline) = unsafe {
        (
            std::slice::from_raw_parts(stored, stored_count as usize),
            std::slice::from_raw_parts(baseline, baseline_count as usize),
        )
    };
    let mut by_path = HashMap::new();
    if by_path.try_reserve(baseline.len()).is_err() {
        return false;
    }
    for row in baseline {
        // SAFETY: the manifest owner retains these strings.
        let Some((path, digest)) = (unsafe { row.semantic_fields() }) else {
            return false;
        };
        if by_path.insert(path, digest).is_some() {
            return false;
        }
    }
    stored.iter().all(|row| {
        // SAFETY: the manifest owner retains these strings.
        unsafe { row.semantic_fields() }
            .is_some_and(|(path, digest)| by_path.get(path) == Some(&digest))
    })
}

#[cfg(not(test))]
unsafe extern "C" {
    fn cbm_pipeline_set_noop_manifest_equal(
        implementation: unsafe extern "C" fn(
            *const FileHash,
            c_int,
            *const FileHash,
            c_int,
        ) -> bool,
    );
}

/// Register before C starts any pipeline workers. The setting affects only
/// the exact no-op check for existing generations.
#[cfg(not(test))]
pub(crate) unsafe fn register() {
    if std::env::var_os("MORI_IMPL_INDEX_MANIFEST").as_deref()
        == Some(std::ffi::OsStr::new("rust"))
    {
        // SAFETY: no runtime consumers exist yet. The callback is static.
        unsafe { cbm_pipeline_set_noop_manifest_equal(equal) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;

    unsafe extern "C" {
        fn cbm_pipeline_semantic_manifests_equal(
            left: *const FileHash,
            left_count: c_int,
            right: *const FileHash,
            right_count: c_int,
        ) -> bool;
    }

    fn row(path: &CStr, digest: &CStr, size: i64) -> FileHash {
        FileHash {
            project: c"project".as_ptr(),
            relative_path: path.as_ptr(),
            sha256: digest.as_ptr(),
            mtime_ns: size,
            size,
        }
    }

    fn assert_parity(
        stored: *const FileHash,
        stored_count: c_int,
        baseline: *const FileHash,
        baseline_count: c_int,
        expected: bool,
    ) {
        // SAFETY: the call sites use live fixture arrays or null inputs.
        unsafe {
            assert_eq!(
                cbm_pipeline_semantic_manifests_equal(
                    stored,
                    stored_count,
                    baseline,
                    baseline_count
                ),
                expected
            );
            assert_eq!(
                equal(stored, stored_count, baseline, baseline_count),
                expected
            );
        }
    }

    #[test]
    fn compares_paths_and_digests_with_native_manifest_semantics() {
        let stored = [row(c"a.rs", c"one", 1), row(c"z\xff.rs", c"two", 2)];
        let permuted = [row(c"z\xff.rs", c"two", 3), row(c"a.rs", c"one", 4)];
        let changed = [row(c"a.rs", c"one", 1), row(c"z\xff.rs", c"three", 2)];
        let duplicate = [row(c"a.rs", c"one", 1), row(c"a.rs", c"one", 2)];
        let replaced = [row(c"a.rs", c"one", 1), row(c"x.rs", c"two", 2)];
        let mut invalid = [row(c"a.rs", c"one", 1), row(c"z\xff.rs", c"two", 2)];
        invalid[1].sha256 = ptr::null();
        for (baseline, expected) in [
            (&permuted, true),
            (&changed, false),
            (&duplicate, false),
            (&replaced, false),
            (&invalid, false),
        ] {
            assert_parity(stored.as_ptr(), 2, baseline.as_ptr(), 2, expected);
        }
        // Stored file hashes have a unique path constraint. Preserve the C
        // helper's behavior even for a synthetic duplicate on this side.
        assert_parity(duplicate.as_ptr(), 2, permuted.as_ptr(), 2, true);
        assert_parity(invalid.as_ptr(), 2, permuted.as_ptr(), 2, false);
        assert_parity(stored.as_ptr(), 2, permuted.as_ptr(), 1, false);
        assert_parity(ptr::null(), 0, ptr::null(), 0, true);
        assert_parity(ptr::null(), 2, permuted.as_ptr(), 2, false);
        assert_parity(ptr::null(), -1, ptr::null(), -1, false);
    }
}
