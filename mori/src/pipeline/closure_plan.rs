//! Classify a closure repair's semantic manifest delta before native probes.

use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, c_char, c_int, c_void};
use std::slice;

/// Layout of `cbm_file_hash_t` in `store.h`.
#[repr(C)]
struct FileHash {
    project: *const c_char,
    path: *const c_char,
    sha256: *const c_char,
    mtime_ns: i64,
    size: i64,
}

/// Layout of `closure_manifest_delta_t` in `pipeline_incremental.c`.
#[repr(C)]
struct ManifestDelta {
    stored: *const FileHash,
    fresh: *const FileHash,
    semantic_input_prefix: *const c_char,
    paths: *mut *mut c_char,
    stored_count: c_int,
    fresh_count: c_int,
    changed_count: c_int,
    deleted_count: c_int,
}

type PathFn = unsafe extern "C" fn(*mut c_void, *const c_char) -> bool;

/// C validates both manifest arrays and their path and digest strings before
/// calling the planner. The output buffer holds one slot per input row.
#[unsafe(no_mangle)]
unsafe extern "C" fn mori_closure_classify_manifest(
    delta: *mut ManifestDelta,
    context: *mut c_void,
    discovered: PathFn,
    try_seed_alias: PathFn,
) -> *const c_char {
    // SAFETY: the caller lends an exclusive request for this call.
    let delta = unsafe { &mut *delta };
    let Some(capacity) = delta.stored_count.checked_add(delta.fresh_count)
    else {
        return c"alloc".as_ptr();
    };
    if capacity == 0 {
        return c"no_file_delta".as_ptr();
    }
    // SAFETY: each nonempty manifest array stays live and initialized, and
    // the output buffer is disjoint from them and sized by the caller.
    let (stored, fresh, paths, semantic_input_prefix) = unsafe {
        (
            manifest_rows(delta.stored, delta.stored_count),
            manifest_rows(delta.fresh, delta.fresh_count),
            slice::from_raw_parts_mut(delta.paths, capacity as usize),
            CStr::from_ptr(delta.semantic_input_prefix).to_bytes(),
        )
    };

    let mut old_digests = HashMap::new();
    let mut fresh_paths = HashSet::new();
    if old_digests.try_reserve(stored.len()).is_err()
        || fresh_paths.try_reserve(fresh.len()).is_err()
    {
        return c"alloc".as_ptr();
    }
    for row in stored {
        // SAFETY: both strings belong to a validated, live manifest row.
        let (path, digest) = unsafe {
            (
                CStr::from_ptr(row.path).to_bytes(),
                CStr::from_ptr(row.sha256).to_bytes(),
            )
        };
        old_digests.insert(path, digest);
    }

    let mut changed = 0;
    let mut governed_seeded = false;
    for row in fresh {
        // SAFETY: both strings belong to a validated, live manifest row.
        let path = unsafe { CStr::from_ptr(row.path).to_bytes() };
        let digest = unsafe { CStr::from_ptr(row.sha256).to_bytes() };
        fresh_paths.insert(path);
        let old_digest = old_digests.get(path).copied();
        if old_digest == Some(digest) {
            continue;
        }
        if path.starts_with(semantic_input_prefix) {
            return c"semantic_input_changed".as_ptr();
        }
        // SAFETY: the matching callbacks borrow this context and path only
        // during their calls. Alias seeding updates C-owned scratch state.
        if unsafe { discovered(context, row.path) } {
            if old_digest.is_none() {
                return c"new_file".as_ptr();
            }
            paths[changed] = row.path.cast_mut();
            changed += 1;
        } else if unsafe { try_seed_alias(context, row.path) } {
            governed_seeded = true;
        } else {
            return c"control_file_changed".as_ptr();
        }
    }

    let mut end = changed;
    for row in stored {
        // SAFETY: this string belongs to a validated, live manifest row.
        let path = unsafe { CStr::from_ptr(row.path).to_bytes() };
        if fresh_paths.contains(path) {
            continue;
        }
        // Preserve the C treatment of absent rows, including virtual rows.
        // SAFETY: the context and path stay live through both callbacks.
        if unsafe {
            !discovered(context, row.path) && try_seed_alias(context, row.path)
        } {
            governed_seeded = true;
        } else {
            paths[end] = row.path.cast_mut();
            end += 1;
        }
    }

    if end == 0 && !governed_seeded {
        return c"no_file_delta".as_ptr();
    }
    delta.changed_count = changed as c_int;
    delta.deleted_count = (end - changed) as c_int;
    std::ptr::null()
}

/// The caller owns `count` initialized rows, or may pass NULL for zero rows.
unsafe fn manifest_rows<'a>(
    rows: *const FileHash,
    count: c_int,
) -> &'a [FileHash] {
    if count == 0 {
        &[]
    } else {
        // SAFETY: the caller owns the nonempty array for this borrow.
        unsafe { slice::from_raw_parts(rows, count as usize) }
    }
}
