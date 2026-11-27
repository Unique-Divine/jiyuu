//! Collect full discovery results in the order the walker visits entries.

use crate::abi_rows::{OwnedString, Rows, write_optional};
use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr;

const IGNORED_CAP: c_int = 2000;

#[repr(C)]
pub(super) struct FileInfo {
    pub(super) path: *mut c_char,
    pub(super) rel_path: *mut c_char,
    pub(super) language: c_int,
    pub(super) size: i64,
    pub(super) mtime_ns: i64,
}

#[repr(C)]
pub(super) struct IgnoredFile {
    pub(super) path: *mut c_char,
    pub(super) reason: *mut c_char,
}

#[repr(C)]
struct Outputs {
    files: *mut *mut FileInfo,
    count: *mut c_int,
    excluded: *mut *mut *mut c_char,
    excluded_count: *mut c_int,
    ignored: *mut *mut IgnoredFile,
    ignored_count: *mut c_int,
    ignored_total: *mut c_int,
}

struct Results {
    files: Rows<FileInfo>,
    excluded: Option<Rows<*mut c_char>>,
    ignored: Option<Rows<IgnoredFile>>,
    ignored_total: c_int,
}

unsafe extern "C" {
    fn cbm_discover_collect_walk(
        repo: *const c_char,
        opts: *const c_void,
        results: *mut c_void,
    ) -> c_int;
    pub(super) fn cbm_discover_free(files: *mut FileInfo, count: c_int);
    pub(super) fn cbm_discover_free_excluded(
        excluded: *mut *mut c_char,
        count: c_int,
    );
    pub(super) fn cbm_discover_free_ignored(
        ignored: *mut IgnoredFile,
        count: c_int,
    );
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_discover_collect(
    repo: *const c_char,
    opts: *const c_void,
    outputs: *const Outputs,
) -> c_int {
    // SAFETY: The C entry point supplies a live output descriptor. Each non-null
    // slot is writable. The native walk borrows the collector synchronously.
    unsafe {
        let out = &*outputs;
        write_optional(out.excluded, ptr::null_mut());
        write_optional(out.ignored, ptr::null_mut());
        write_optional(out.excluded_count, 0);
        write_optional(out.ignored_count, 0);
        write_optional(out.ignored_total, 0);
        // Native setup clears the resource diagnostic even on invalid arguments.
        if repo.is_null() || out.files.is_null() || out.count.is_null() {
            return cbm_discover_collect_walk(repo, opts, ptr::null_mut());
        }
        write_optional(out.files, ptr::null_mut());
        write_optional(out.count, 0);
        let mut results = Results {
            files: Rows::new(cbm_discover_free),
            excluded: (!out.excluded.is_null())
                .then(|| Rows::new(cbm_discover_free_excluded)),
            ignored: (!out.ignored.is_null())
                .then(|| Rows::new(cbm_discover_free_ignored)),
            ignored_total: 0,
        };
        let status = cbm_discover_collect_walk(
            repo,
            opts,
            ptr::from_mut(&mut results).cast(),
        );
        if status != 0 {
            return status;
        }
        let (files, count) = results.files.into_raw();
        write_optional(out.files, files);
        write_optional(out.count, count);
        if let Some(excluded) = results.excluded {
            let (paths, count) = excluded.into_raw();
            write_optional(out.excluded, paths);
            write_optional(out.excluded_count, count);
        }
        if let Some(ignored) = results.ignored {
            let (files, count) = ignored.into_raw();
            write_optional(out.ignored, files);
            write_optional(out.ignored_count, count);
            write_optional(out.ignored_total, results.ignored_total);
        }
        status
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_discover_push_file(
    results: *mut c_void,
    file: *const FileInfo,
) -> bool {
    // SAFETY: The C walk lends the collector and the entry until we return.
    let (results, file) = unsafe { (&mut *results.cast::<Results>(), &*file) };
    results
        .files
        .push(256, || {
            // SAFETY: Both paths are live, terminated byte strings from the walker.
            let path = OwnedString::copy(unsafe { CStr::from_ptr(file.path) })?;
            let rel_path =
                OwnedString::copy(unsafe { CStr::from_ptr(file.rel_path) })?;
            Ok(FileInfo {
                path: path.into_raw(),
                rel_path: rel_path.into_raw(),
                ..*file
            })
        })
        .is_ok()
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_discover_push_excluded(
    results: *mut c_void,
    path: *const c_char,
) -> bool {
    // SAFETY: The walk lends the collector until we return.
    let results = unsafe { &mut *results.cast::<Results>() };
    let Some(rows) = &mut results.excluded else {
        return true;
    };
    rows.push(64, || {
        // SAFETY: The relative path is live and terminated.
        Ok(OwnedString::copy(unsafe { CStr::from_ptr(path) })?.into_raw())
    })
    .is_ok()
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_discover_push_ignored(
    results: *mut c_void,
    path: *const c_char,
    reason: *const c_char,
) -> bool {
    // SAFETY: The walk lends the collector until we return.
    let results = unsafe { &mut *results.cast::<Results>() };
    let Some(rows) = &mut results.ignored else {
        return true;
    };
    let Some(total) = results.ignored_total.checked_add(1) else {
        return false;
    };
    results.ignored_total = total;
    if rows.len() == IGNORED_CAP {
        return true;
    }
    rows.push(64, || {
        // SAFETY: The relative path and reason are live, terminated byte strings.
        let path = OwnedString::copy(unsafe { CStr::from_ptr(path) })?;
        let reason = OwnedString::copy(unsafe { CStr::from_ptr(reason) })?;
        Ok(IgnoredFile {
            path: path.into_raw(),
            reason: reason.into_raw(),
        })
    })
    .is_ok()
}
