//! Own and compare semantic manifest rows across the native pipeline ABI.

use crate::abi_rows::{OwnedString, Rows, write_optional};
use std::borrow::Borrow;
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, c_char, c_int, c_void};
use std::hash::{Hash, Hasher};
use std::ptr;

const ERROR: c_int = -1;
const SHA256_HEX_LEN: usize = 64;

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

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_pipeline_semantic_manifests_equal(
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

// While a key is in the set, its string belongs to an initialized row and
// remains at the same address even when the row array grows.
struct PathKey(*const c_char);

impl Borrow<CStr> for PathKey {
    fn borrow(&self) -> &CStr {
        // SAFETY: The owner of the set retains the row and its C string.
        unsafe { CStr::from_ptr(self.0) }
    }
}

impl Hash for PathKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Borrow::<CStr>::borrow(self).hash(state);
    }
}

impl PartialEq for PathKey {
    fn eq(&self, other: &Self) -> bool {
        Borrow::<CStr>::borrow(self) == Borrow::<CStr>::borrow(other)
    }
}

impl Eq for PathKey {}

struct Manifest {
    seen: HashSet<PathKey>,
    rows: Rows<FileHash>,
}

type Collect = unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int;

unsafe extern "C" {
    fn cbm_pipeline_free_semantic_manifest(rows: *mut FileHash, count: c_int);
    fn cbm_pipeline_mark_semantic_manifest_written(
        rows: *const FileHash,
        count: usize,
    );
    fn cbm_pipeline_frozen_checkpoint(pipeline: *mut c_void) -> c_int;
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_manifest_build(
    context: *mut c_void,
    collect: Collect,
    frozen: *mut c_void,
    out: *mut *mut FileHash,
    count: *mut c_int,
) -> c_int {
    let mut manifest = Manifest {
        seen: HashSet::new(),
        rows: Rows::new(cbm_pipeline_free_semantic_manifest),
    };
    // SAFETY: C retains the pipeline and context through the synchronous
    // collection. The callback borrows our collector and cannot retain it.
    let status =
        unsafe { collect(context, ptr::from_mut(&mut manifest).cast()) };
    if status != 0 {
        return status;
    }
    let rows = manifest.rows.as_mut_slice();
    let complete = if frozen.is_null() {
        rows.sort_unstable_by(path_order);
        true
    } else {
        sort_frozen(rows, || {
            // SAFETY: The C caller retains the frozen pipeline until we return.
            unsafe { cbm_pipeline_frozen_checkpoint(frozen) != 0 }
        })
    };
    // SAFETY: Both sorts only move initialized rows, even when cancelled.
    // C reads the fields when consuming or freeing the manifest.
    unsafe {
        cbm_pipeline_mark_semantic_manifest_written(rows.as_ptr(), rows.len())
    };
    if !complete {
        return ERROR;
    }

    // Discard the borrowed keys before transferring their backing strings.
    drop(manifest.seen);
    let (rows, len) = manifest.rows.into_raw();
    // SAFETY: Both outputs are writable, as required by the native wrapper.
    unsafe {
        write_optional(out, rows);
        write_optional(count, len);
    }
    0
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_manifest_contains(
    rows: *const c_void,
    path: *const c_char,
) -> bool {
    // SAFETY: C supplies a live collector and a terminated path.
    let (rows, path) =
        unsafe { (&*rows.cast::<Manifest>(), CStr::from_ptr(path)) };
    rows.seen.contains(path)
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_manifest_add_digest(
    rows: *mut c_void,
    project: *const c_char,
    path: *const c_char,
    sha256: *const c_char,
    mtime_ns: i64,
    size: i64,
) -> c_int {
    if project.is_null() || path.is_null() || sha256.is_null() {
        return ERROR;
    }
    // SAFETY: C retains the collector and these terminated strings for the call.
    let (rows, project, path, sha256) = unsafe {
        (
            &mut *rows.cast::<Manifest>(),
            CStr::from_ptr(project),
            CStr::from_ptr(path),
            CStr::from_ptr(sha256),
        )
    };
    if path.is_empty()
        || sha256.to_bytes().len() != SHA256_HEX_LEN
        || !sha256
            .to_bytes()
            .iter()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        || rows.seen.contains(path)
        || rows.seen.try_reserve(1).is_err()
    {
        return ERROR;
    }
    let mut key = ptr::null();
    if rows
        .rows
        .push(64, || {
            let project = OwnedString::copy(project)?;
            let path = OwnedString::copy(path)?;
            let sha256 = OwnedString::copy(sha256)?;
            key = path.into_raw();
            Ok(FileHash {
                project: project.into_raw(),
                relative_path: key,
                sha256: sha256.into_raw(),
                mtime_ns,
                size,
            })
        })
        .is_err()
    {
        return ERROR;
    }
    rows.seen.insert(PathKey(key));
    0
}

fn path_order(left: &FileHash, right: &FileHash) -> std::cmp::Ordering {
    // SAFETY: Both initialized rows own valid C strings for the comparison.
    unsafe {
        CStr::from_ptr(left.relative_path)
            .cmp(CStr::from_ptr(right.relative_path))
    }
}

// Keep the reference's cooperative checkpoints, including within a heap sift.
fn sort_frozen(
    rows: &mut [FileHash],
    mut stopped: impl FnMut() -> bool,
) -> bool {
    fn sift(
        rows: &mut [FileHash],
        mut root: usize,
        stopped: &mut impl FnMut() -> bool,
    ) -> bool {
        while root < rows.len() / 2 {
            if stopped() {
                return false;
            }
            let mut child = root * 2 + 1;
            if child + 1 < rows.len()
                && path_order(&rows[child], &rows[child + 1]).is_lt()
            {
                child += 1;
            }
            if !path_order(&rows[root], &rows[child]).is_lt() {
                break;
            }
            rows.swap(root, child);
            root = child;
        }
        true
    }

    for root in (0..rows.len() / 2).rev() {
        if !sift(rows, root, &mut stopped) {
            return false;
        }
    }
    for end in (1..rows.len()).rev() {
        if stopped() {
            return false;
        }
        rows.swap(0, end);
        if !sift(&mut rows[..end], 0, &mut stopped) {
            return false;
        }
    }
    !stopped()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;

    fn row(path: &CStr, digest: &CStr, size: i64) -> FileHash {
        FileHash {
            project: c"project".as_ptr(),
            relative_path: path.as_ptr(),
            sha256: digest.as_ptr(),
            mtime_ns: size,
            size,
        }
    }

    fn assert_equal(
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
        }
    }

    #[test]
    fn manifest_publishes_sorted_rows_or_discards_the_partial_array() {
        unsafe extern "C" fn collect(
            context: *mut c_void,
            rows: *mut c_void,
        ) -> c_int {
            let digest = c"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
            // SAFETY: The build lends this collector and the fixture's flag.
            unsafe {
                for (size, path) in
                    [c"z\xff.rs", c"a.rs", c"z\xfe.rs"].into_iter().enumerate()
                {
                    let status = mori_manifest_add_digest(
                        rows,
                        c"project".as_ptr(),
                        path.as_ptr(),
                        digest.as_ptr(),
                        99,
                        size as i64,
                    );
                    if status != 0 {
                        return status;
                    }
                }
                if *context.cast::<bool>() {
                    return mori_manifest_add_digest(
                        rows,
                        c"project".as_ptr(),
                        c"z\xff.rs".as_ptr(),
                        digest.as_ptr(),
                        99,
                        0,
                    );
                }
            }
            0
        }

        for mut duplicate in [false, true] {
            let mut rows = ptr::null_mut();
            let mut len = 0;
            // SAFETY: The flag and output slots live through the build.
            let status = unsafe {
                mori_manifest_build(
                    ptr::from_mut(&mut duplicate).cast(),
                    collect,
                    ptr::null_mut(),
                    &mut rows,
                    &mut len,
                )
            };
            if duplicate {
                assert_eq!((status, rows, len), (ERROR, ptr::null_mut(), 0));
            } else {
                assert_eq!((status, len), (0, 3));
                // SAFETY: Successful build transfers len initialized rows.
                let sizes: Vec<_> =
                    unsafe { std::slice::from_raw_parts(rows, len as usize) }
                        .iter()
                        .map(|row| row.size)
                        .collect();
                // SAFETY: Release the build's owned ABI allocation exactly once.
                unsafe { cbm_pipeline_free_semantic_manifest(rows, len) };
                assert_eq!(sizes, [1, 2, 0]);
            }
        }
    }

    #[test]
    fn frozen_sort_orders_raw_paths_and_honors_checkpoints() {
        let fixture = || {
            [
                row(c"z\xff.rs", c"one", 1),
                row(c"a.rs", c"two", 2),
                row(c"z\xfe.rs", c"one", 3),
            ]
        };
        let mut rows = fixture();
        assert!(sort_frozen(&mut rows, || false));
        assert_eq!(rows.map(|row| row.size), [2, 3, 1]);

        let mut checkpoints = 0;
        assert!(!sort_frozen(&mut fixture(), || {
            checkpoints += 1;
            checkpoints == 2
        }));
        assert_eq!(checkpoints, 2);
        assert!(!sort_frozen(&mut [], || true));
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
            assert_equal(stored.as_ptr(), 2, baseline.as_ptr(), 2, expected);
        }
        // Stored file hashes have a unique path constraint. Preserve the C
        // helper's behavior even for a synthetic duplicate on this side.
        assert_equal(duplicate.as_ptr(), 2, permuted.as_ptr(), 2, true);
        assert_equal(invalid.as_ptr(), 2, permuted.as_ptr(), 2, false);
        assert_equal(stored.as_ptr(), 2, permuted.as_ptr(), 1, false);
        assert_equal(ptr::null(), 0, ptr::null(), 0, true);
        assert_equal(ptr::null(), 2, permuted.as_ptr(), 2, false);
        assert_equal(ptr::null(), -1, ptr::null(), -1, false);
    }
}
