//! Discover package maps and resolve imports for native index runs.

mod graph;
mod json;
mod manifest;
mod namespace;
mod php;
mod python;
mod resolve;
mod rust_use;
mod walk;

use crate::native_hash_table::{
    HashTable, cbm_ht_create, cbm_ht_free, cbm_ht_get, cbm_ht_set,
};
use manifest::{Manifest, basename};
use std::cmp::Ordering;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr::{null, null_mut};
use std::slice;

// C path buffers include the trailing NUL.
const PATH_LIMIT: usize = 1023;

#[repr(C)]
pub(crate) struct PackageEntry {
    name: *mut c_char,
    path: *mut c_char,
    manifest: *mut c_char,
    source: *mut c_char,
    sequence: c_int,
}

#[repr(C)]
pub(crate) struct PackageEntries {
    items: *mut PackageEntry,
    count: c_int,
    capacity: c_int,
}

impl PackageEntries {
    fn empty() -> Self {
        Self {
            items: null_mut(),
            count: 0,
            capacity: 0,
        }
    }

    fn as_slice(&self) -> &[PackageEntry] {
        if self.count == 0 {
            return &[];
        }
        // Only push appends an initialized slot or increments count.
        unsafe { slice::from_raw_parts(self.items, self.count as usize) }
    }

    fn push(&mut self, entry: PackageEntry) {
        if self.count == c_int::MAX {
            entry.release();
            return;
        }
        if self.count == self.capacity {
            let capacity = if self.capacity == 0 {
                16
            } else {
                self.capacity.saturating_mul(2)
            };
            let Some(size) = (capacity as usize)
                .checked_mul(std::mem::size_of::<PackageEntry>())
            else {
                entry.release();
                return;
            };
            let items = unsafe { realloc(self.items.cast(), size) }
                .cast::<PackageEntry>();
            if items.is_null() {
                entry.release();
                return;
            }
            self.items = items;
            self.capacity = capacity;
        }
        let slot = unsafe { self.items.add(self.count as usize) };
        unsafe {
            slot.write(entry);
            // The C worker reader sees the initialized fields, excluding trailing padding.
            cbm_ffi_mark_written(
                slot.cast(),
                std::mem::offset_of!(PackageEntry, sequence)
                    + std::mem::size_of::<c_int>(),
            );
        }
        self.count += 1;
    }

    fn parse(&mut self, kind: Manifest, path: Option<&CStr>, source: &[u8]) {
        let before = self.count;
        let swift = matches!(kind, Manifest::Swift);
        if swift && path.is_none() {
            return;
        }
        kind.parse(path.map_or(b"", CStr::to_bytes), source, |name, entry| {
            let normalize_name = matches!(kind, Manifest::Python);
            let name = duplicate_name(name, normalize_name);
            let entry_path = duplicate(entry);
            let entry = PackageEntry {
                name,
                path: entry_path,
                manifest: null_mut(),
                source: null_mut(),
                sequence: 0,
            };
            if name.is_null() || entry_path.is_null() {
                entry.release();
            } else {
                let count = self.count;
                self.push(entry);
                if swift && self.count > count {
                    // The source and sequence are stamped after parsing, as in
                    // the native worker that owns this array.
                    let added = unsafe {
                        &mut *self.items.add((self.count - 1) as usize)
                    };
                    added.manifest = tracked_duplicate(path);
                }
            }
        });
        if path.is_some() {
            for index in before..self.count {
                let entry = unsafe { &mut *self.items.add(index as usize) };
                entry.source = tracked_duplicate(path);
                entry.sequence = index - before;
            }
        }
    }

    fn swift_boundary(&mut self, path: &CStr) {
        self.push(PackageEntry {
            name: null_mut(),
            path: duplicate(dirname(path.to_bytes())),
            manifest: tracked_duplicate(Some(path)),
            source: null_mut(),
            sequence: 0,
        });
    }

    fn clear(&mut self) {
        for entry in self.as_slice() {
            entry.release();
        }
        unsafe { free(self.items.cast()) };
        *self = Self::empty();
    }
}

impl PackageEntry {
    fn release(&self) {
        // Names and paths use the CRT. Provenance retains the memory-core class
        // expected by the C workers that move whole arrays and release them.
        unsafe {
            free(self.name.cast());
            free(self.path.cast());
            cbm_pkgmap_tracked_free(self.manifest);
            cbm_pkgmap_tracked_free(self.source);
        }
    }

    fn provenance(&self) -> &[u8] {
        unsafe {
            c_bytes(self.source)
                .or_else(|| c_bytes(self.path))
                .unwrap_or_default()
        }
    }
}

fn compare_entries(a: &PackageEntry, b: &PackageEntry) -> Ordering {
    fn depth(path: &[u8]) -> usize {
        if path.is_empty() || path == b"." {
            0
        } else {
            1 + path.iter().filter(|&&byte| byte == b'/').count()
        }
    }
    let ap = a.provenance();
    let bp = b.provenance();
    unsafe {
        c_bytes(a.name)
            .cmp(&c_bytes(b.name))
            .then_with(|| depth(ap).cmp(&depth(bp)))
            .then_with(|| ap.cmp(bp))
            .then_with(|| a.sequence.cmp(&b.sequence))
            .then_with(|| c_bytes(a.path).cmp(&c_bytes(b.path)))
    }
}

fn build(workers: &[PackageEntries], project: *const c_char) -> *mut HashTable {
    let candidates = workers
        .iter()
        .flat_map(PackageEntries::as_slice)
        .filter(|entry| !entry.name.is_null() && !entry.path.is_null());
    let len = candidates.clone().count();
    let Some(size) = len
        .checked_mul(std::mem::size_of::<*const PackageEntry>())
        .filter(|size| *size != 0)
    else {
        return null_mut();
    };
    // The native allocator charges the temporary ordering array to the same
    // class as the previous merger.
    let ordered =
        unsafe { cbm_pkgmap_tracked_calloc(size) }.cast::<*const PackageEntry>();
    if ordered.is_null() {
        return null_mut();
    }
    let sorted = unsafe { slice::from_raw_parts_mut(ordered, len) };
    for (slot, entry) in sorted.iter_mut().zip(candidates) {
        *slot = std::ptr::from_ref(entry);
    }
    sorted.sort_unstable_by(|a, b| unsafe { compare_entries(&**a, &**b) });
    let map = unsafe { cbm_ht_create(64) };
    if map.is_null() {
        unsafe { cbm_pkgmap_tracked_free(ordered.cast()) };
        return map;
    }

    let mut merged = 0;
    for &entry in sorted.iter() {
        // Every pointer above borrowed a member of `workers`.
        let entry = unsafe { &*entry };
        // Do not replace an earlier manifest's entry. PSR-4 keeps a directory
        // path in the map, while other specifiers map to a module QN.
        unsafe {
            if !cbm_ht_get(map, entry.name).is_null() {
                continue;
            }
            let name = CStr::from_ptr(entry.name);
            let path = CStr::from_ptr(entry.path);
            let value = if name.to_bytes().ends_with(b"\\") {
                duplicate(path.to_bytes())
            } else {
                cbm_pipeline_fqn_module(project, path.as_ptr())
            };
            if !value.is_null() && insert(map, duplicate(name.to_bytes()), value)
            {
                merged += 1;
            }
        }
    }
    unsafe { cbm_pkgmap_tracked_free(ordered.cast()) };
    if merged == 0 {
        unsafe { cbm_ht_free(map) };
        return null_mut();
    }
    unsafe {
        cbm_log_int(1, c"pkgmap.build".as_ptr(), c"entries".as_ptr(), merged)
    };
    map
}

#[repr(C)]
pub(crate) struct FileInfo {
    _opaque: [u8; 0],
}

fn build_from_files(
    files: *const FileInfo,
    count: c_int,
    project: *const c_char,
    repo: Option<&CStr>,
    excluded: *mut *mut c_char,
    excluded_count: c_int,
    log_scan: bool,
) -> *mut HashTable {
    let mut entries = PackageEntries::empty();
    let mut discovered = 0;
    for index in 0..count {
        let mut path = null();
        let rel = unsafe { cbm_pkgmap_file_at(files, index, &mut path) };
        let Some(rel) = (unsafe { c_string(rel) }) else {
            continue;
        };
        if let Some(kind) = Manifest::detect(basename(rel)) {
            discovered += 1;
            if let Some(source) = unsafe { c_string(path) }.and_then(walk::read)
            {
                entries.parse(kind, Some(rel), source.bytes());
            }
        }
    }
    let walked = walk::scan(repo, &mut entries, excluded, excluded_count, false);
    if log_scan {
        let files = CString::new(discovered.to_string()).unwrap();
        let walk = CString::new(walked.to_string()).unwrap();
        let total = CString::new(entries.count.to_string()).unwrap();
        for value in [&files, &walk, &total] {
            mark_cstr(value);
        }
        unsafe {
            cbm_log(
                1,
                c"pkgmap.scan".as_ptr(),
                c"manifests_from_files".as_ptr(),
                files.as_ptr(),
                c"manifests_from_walk".as_ptr(),
                walk.as_ptr(),
                c"entries".as_ptr(),
                total.as_ptr(),
                null::<c_char>(),
            );
        }
    }
    let map = build(slice::from_ref(&entries), project);
    entries.clear();
    map
}

// These entry points are required by both the packaged engine and its C test
// runner. All pointers are borrowed for the call unless the API returns an
// owned table or appends owned rows to the caller's array.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn cbm_pkg_entries_init(
    entries: *mut PackageEntries,
) {
    unsafe {
        entries.write(PackageEntries::empty());
        cbm_ffi_mark_written(
            entries.cast(),
            std::mem::size_of::<PackageEntries>(),
        );
    };
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn cbm_pkg_entries_free(
    entries: *mut PackageEntries,
) {
    unsafe { (*entries).clear() };
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn cbm_pkgmap_try_parse(
    name: *const c_char,
    rel: *const c_char,
    source: *const c_char,
    len: c_int,
    entries: *mut PackageEntries,
) -> bool {
    let Some(kind) = unsafe { c_bytes(name) }.and_then(Manifest::detect) else {
        return false;
    };
    if source.is_null() || len <= 0 || entries.is_null() {
        return false;
    }
    unsafe {
        (*entries).parse(
            kind,
            c_string(rel),
            slice::from_raw_parts(source.cast(), len as usize),
        );
    }
    true
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn cbm_pkgmap_build(
    workers: *const PackageEntries,
    count: c_int,
    project: *const c_char,
) -> *mut HashTable {
    if workers.is_null() || count <= 0 {
        return null_mut();
    }
    build(
        unsafe { slice::from_raw_parts(workers, count as usize) },
        project,
    )
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn cbm_pkgmap_scan_repo(
    repo: *const c_char,
    entries: *mut PackageEntries,
    excluded: *mut *mut c_char,
    count: c_int,
) -> c_int {
    if let (Some(repo), Some(entries)) =
        unsafe { (c_string(repo), entries.as_mut()) }
    {
        return walk::scan(Some(repo), entries, excluded, count, false);
    }
    0
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn cbm_pkgmap_scan_swift_targets(
    repo: *const c_char,
    entries: *mut PackageEntries,
    excluded: *mut *mut c_char,
    count: c_int,
) -> c_int {
    if let (Some(repo), Some(entries)) =
        unsafe { (c_string(repo), entries.as_mut()) }
    {
        return walk::scan(Some(repo), entries, excluded, count, true);
    }
    0
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn cbm_pkgmap_build_from_repo(
    repo: *const c_char,
    files: *const FileInfo,
    count: c_int,
    project: *const c_char,
    excluded: *mut *mut c_char,
    excluded_count: c_int,
) -> *mut HashTable {
    build_from_files(
        files,
        count,
        project,
        unsafe { c_string(repo) },
        excluded,
        excluded_count,
        true,
    )
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn cbm_pkgmap_build_from_files(
    files: *const FileInfo,
    count: c_int,
    project: *const c_char,
) -> *mut HashTable {
    build_from_files(files, count, project, None, null_mut(), 0, false)
}

unsafe extern "C" fn release_entry(
    key: *const c_char,
    value: *mut c_void,
    _: *mut c_void,
) {
    unsafe {
        free(key.cast_mut().cast());
        free(value);
    }
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn cbm_pkgmap_free(map: *mut HashTable) {
    if !map.is_null() {
        unsafe {
            cbm_ht_foreach(map, release_entry, null_mut());
            cbm_ht_free(map);
        }
    }
}

unsafe fn c_string<'a>(pointer: *const c_char) -> Option<&'a CStr> {
    (!pointer.is_null()).then(|| unsafe { CStr::from_ptr(pointer) })
}

unsafe fn c_bytes<'a>(pointer: *const c_char) -> Option<&'a [u8]> {
    unsafe { c_string(pointer) }.map(CStr::to_bytes)
}

fn trimmed_c_string(bytes: &[u8]) -> &[u8] {
    &bytes[..bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len())]
}

fn dirname(path: &[u8]) -> &[u8] {
    path.iter()
        .rposition(|&byte| byte == b'/')
        .map_or(b"", |index| &path[..index])
}

fn limit(mut bytes: Vec<u8>) -> Vec<u8> {
    bytes.truncate(PATH_LIMIT);
    bytes
}

fn join(directory: &[u8], suffix: &[u8]) -> Vec<u8> {
    if directory.is_empty() {
        limit(suffix.to_vec())
    } else {
        limit([directory, b"/", suffix].concat())
    }
}

fn mark_cstr(value: &CStr) {
    // The bytes may have been constructed by Rust without native instrumentation.
    unsafe {
        cbm_ffi_mark_written(value.as_ptr().cast(), value.count_bytes() + 1)
    };
}

fn duplicate(bytes: &[u8]) -> *mut c_char {
    duplicate_name(bytes, false)
}

fn duplicate_name(bytes: &[u8], python_name: bool) -> *mut c_char {
    let bytes = trimmed_c_string(bytes);
    let result =
        unsafe { realloc(null_mut(), bytes.len() + 1) }.cast::<c_char>();
    if !result.is_null() {
        unsafe {
            result
                .cast::<u8>()
                .copy_from_nonoverlapping(bytes.as_ptr(), bytes.len());
            if python_name {
                let name =
                    slice::from_raw_parts_mut(result.cast::<u8>(), bytes.len());
                for byte in name {
                    if *byte == b'-' {
                        *byte = b'_';
                    }
                }
            }
            result.add(bytes.len()).write(0);
            cbm_ffi_mark_written(result.cast(), bytes.len() + 1);
        }
    }
    result
}

fn tracked_duplicate(value: Option<&CStr>) -> *mut c_char {
    value.map_or(null_mut(), |value| unsafe {
        cbm_pkgmap_tracked_strdup(value.as_ptr())
    })
}

// The table borrows pointers. Verify ownership transfer before returning.
unsafe fn insert(
    map: *mut HashTable,
    key: *mut c_char,
    value: *mut c_char,
) -> bool {
    unsafe {
        if !key.is_null() && !value.is_null() {
            cbm_ht_set(map, key, value.cast());
            if cbm_ht_get(map, key) == value.cast() {
                return true;
            }
        }
        free(key.cast());
        free(value.cast());
    }
    false
}

unsafe extern "C" {
    fn cbm_ffi_mark_written(address: *const c_void, size: usize);
    fn realloc(ptr: *mut c_void, size: usize) -> *mut c_void;
    fn free(ptr: *mut c_void);
    fn cbm_ht_foreach(
        map: *const HashTable,
        callback: unsafe extern "C" fn(*const c_char, *mut c_void, *mut c_void),
        data: *mut c_void,
    );
    fn cbm_pipeline_fqn_module(
        project: *const c_char,
        path: *const c_char,
    ) -> *mut c_char;
    fn cbm_pkgmap_file_at(
        files: *const FileInfo,
        index: c_int,
        abs: *mut *const c_char,
    ) -> *const c_char;
    fn cbm_pkgmap_tracked_calloc(size: usize) -> *mut c_void;
    fn cbm_pkgmap_tracked_strdup(value: *const c_char) -> *mut c_char;
    fn cbm_pkgmap_tracked_free(value: *mut c_char);
    fn cbm_log(level: c_int, event: *const c_char, ...);
    fn cbm_log_int(
        level: c_int,
        event: *const c_char,
        key: *const c_char,
        value: i64,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(
        name: &CStr,
        path: &CStr,
        source: &CStr,
        sequence: c_int,
    ) -> PackageEntry {
        // The comparator only borrows these strings.
        PackageEntry {
            name: name.as_ptr().cast_mut(),
            path: path.as_ptr().cast_mut(),
            manifest: null_mut(),
            source: source.as_ptr().cast_mut(),
            sequence,
        }
    }

    #[test]
    fn go_module_order_uses_depth_then_manifest_path() {
        let root = entry(c"github.com/acme/tool/v2", c"", c"go.mod", 0);
        let fixture = entry(
            c"github.com/acme/tool/v2",
            c".github/codeql/tests/fixture",
            c".github/codeql/tests/fixture/go.mod",
            0,
        );
        let alpha = entry(
            c"github.com/acme/tool/v2",
            c"examples/alpha",
            c"examples/alpha/go.mod",
            0,
        );
        let beta = entry(
            c"github.com/acme/tool/v2",
            c"examples/beta",
            c"examples/beta/go.mod",
            0,
        );

        for (earlier, later) in [(&root, &fixture), (&alpha, &beta)] {
            assert_eq!(compare_entries(earlier, later), Ordering::Less);
            assert_eq!(compare_entries(later, earlier), Ordering::Greater);
        }
    }

    #[test]
    fn pyproject_orders_candidates_by_declaration() {
        let src = entry(
            c"flaskr",
            c"examples/tutorial/src/flaskr/__init__",
            c"examples/tutorial/pyproject.toml",
            0,
        );
        let root = entry(
            c"flaskr",
            c"examples/tutorial/flaskr/__init__",
            c"examples/tutorial/pyproject.toml",
            1,
        );
        assert_eq!(compare_entries(&src, &root), Ordering::Less);
        assert_eq!(compare_entries(&root, &src), Ordering::Greater);
    }
}
