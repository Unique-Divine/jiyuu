use super::{
    Manifest, PATH_LIMIT, PackageEntries, cbm_log, cbm_log_int, mark_cstr,
};
use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr::{NonNull, null};
use std::slice;

const MAX_DEPTH: usize = 64;
// Matches the C path-kind adapter.
const DIRECTORY: c_int = 1;
const REGULAR_FILE: c_int = 2;

struct Directory(NonNull<c_void>);

impl Directory {
    fn open(path: &CStr) -> Option<Self> {
        NonNull::new(unsafe { cbm_opendir(path.as_ptr()) }).map(Self)
    }

    fn next(&mut self) -> Option<&CStr> {
        // Compatibility directory entries are borrowed until the next read.
        let name = unsafe { cbm_pkgmap_dir_next(self.0.as_ptr()) };
        (!name.is_null()).then(|| unsafe { CStr::from_ptr(name) })
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        unsafe { cbm_closedir(self.0.as_ptr()) };
    }
}

pub(super) struct ManifestSource {
    data: NonNull<c_char>,
    len: usize,
}

impl ManifestSource {
    pub fn bytes(&self) -> &[u8] {
        // The native reader initialized this many bytes and owns the allocation
        // until this wrapper releases it.
        unsafe { slice::from_raw_parts(self.data.as_ptr().cast(), self.len) }
    }
}

impl Drop for ManifestSource {
    fn drop(&mut self) {
        unsafe { cbm_pkgmap_free_manifest(self.data.as_ptr()) };
    }
}

pub(super) fn read(path: &CStr) -> Option<ManifestSource> {
    let mut len = 0;
    let data = NonNull::new(unsafe {
        cbm_pkgmap_read_manifest(path.as_ptr(), &mut len)
    })?;
    let source = ManifestSource {
        data,
        len: len as usize,
    };
    (len > 0).then_some(source)
}

fn child_path<'a>(
    parent: &CStr,
    name: &CStr,
    out: &'a mut [u8; PATH_LIMIT + 1],
) -> Option<&'a CStr> {
    let parent = parent.to_bytes();
    let name = name.to_bytes();
    let offset = parent.len() + usize::from(!parent.is_empty());
    let len = offset + name.len();
    if len > PATH_LIMIT {
        return None;
    }
    out[..parent.len()].copy_from_slice(parent);
    if !parent.is_empty() {
        out[parent.len()] = b'/';
    }
    out[offset..len].copy_from_slice(name);
    out[len] = 0;
    let path = CStr::from_bytes_with_nul(&out[..=len]).ok()?;
    mark_cstr(path);
    Some(path)
}

struct Walker<'a> {
    entries: &'a mut PackageEntries,
    excluded: *mut *mut c_char,
    excluded_count: c_int,
    swift_only: bool,
}

impl Walker<'_> {
    fn visit(&mut self, abs: &CStr, rel: &CStr, depth: usize) -> c_int {
        if depth >= MAX_DEPTH {
            let rel = if rel.is_empty() { c"." } else { rel };
            unsafe {
                cbm_log(
                    1,
                    c"pkgmap.walk".as_ptr(),
                    c"depth_cap".as_ptr(),
                    rel.as_ptr(),
                    null::<c_char>(),
                )
            };
            return 0;
        }

        let Some(mut dir) = Directory::open(abs) else {
            return 0;
        };
        let mut parsed = 0;
        while let Some(name) = dir.next() {
            let mut absolute = [0; PATH_LIMIT + 1];
            let mut relative = [0; PATH_LIMIT + 1];
            let (Some(child), Some(child_rel)) = (
                child_path(abs, name, &mut absolute),
                child_path(rel, name, &mut relative),
            ) else {
                continue;
            };
            let kind = unsafe { cbm_pkgmap_path_kind(child.as_ptr()) };
            if kind == DIRECTORY {
                let excluded = unsafe {
                    cbm_pkgmap_skip_dir(
                        name.as_ptr(),
                        child_rel.as_ptr(),
                        self.excluded,
                        self.excluded_count,
                    )
                };
                if !excluded {
                    parsed += self.visit(child, child_rel, depth + 1);
                }
            } else if kind == REGULAR_FILE {
                let Some(kind) = Manifest::detect(name.to_bytes()) else {
                    continue;
                };
                if self.swift_only && !matches!(kind, Manifest::Swift) {
                    continue;
                }
                if let Some(source) = read(child) {
                    self.entries.parse(kind, Some(child_rel), source.bytes());
                    if self.swift_only {
                        self.entries.swift_boundary(child_rel);
                    }
                    parsed += 1;
                }
            }
        }
        parsed
    }
}

pub(super) fn scan(
    repo: Option<&CStr>,
    entries: &mut PackageEntries,
    excluded: *mut *mut c_char,
    excluded_count: c_int,
    swift_only: bool,
) -> c_int {
    let Some(repo) = repo else {
        return 0;
    };
    let parsed = Walker {
        entries,
        excluded,
        excluded_count,
        swift_only,
    }
    .visit(repo, c"", 0);
    if !swift_only {
        unsafe {
            cbm_log_int(
                1,
                c"pkgmap.scan_repo".as_ptr(),
                c"manifests".as_ptr(),
                i64::from(parsed),
            )
        };
    }
    parsed
}

unsafe extern "C" {
    fn cbm_opendir(path: *const c_char) -> *mut c_void;
    fn cbm_closedir(dir: *mut c_void);
    fn cbm_pkgmap_dir_next(dir: *mut c_void) -> *const c_char;
    fn cbm_pkgmap_path_kind(path: *const c_char) -> c_int;
    fn cbm_pkgmap_read_manifest(
        path: *const c_char,
        len: *mut c_int,
    ) -> *mut c_char;
    fn cbm_pkgmap_free_manifest(source: *mut c_char);
    fn cbm_pkgmap_skip_dir(
        name: *const c_char,
        rel: *const c_char,
        excluded: *mut *mut c_char,
        count: c_int,
    ) -> bool;
}
