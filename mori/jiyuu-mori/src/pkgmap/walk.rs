use super::{Manifest, PATH_LIMIT, PackageEntries, cbm_log, cbm_log_int};
use std::ffi::{CStr, CString, OsStr, c_char, c_int};
use std::fs::{self, Metadata};
use std::path::Path;
use std::ptr::{NonNull, null};
use std::slice;

const MAX_DEPTH: usize = 64;

#[cfg(unix)]
fn os_path(bytes: &[u8]) -> Option<&OsStr> {
    use std::os::unix::ffi::OsStrExt;
    Some(OsStr::from_bytes(bytes))
}

#[cfg(windows)]
fn os_path(bytes: &[u8]) -> Option<&OsStr> {
    Some(OsStr::new(std::str::from_utf8(bytes).ok()?))
}

#[cfg(unix)]
fn name_bytes(name: &OsStr) -> Option<&[u8]> {
    use std::os::unix::ffi::OsStrExt;
    Some(name.as_bytes())
}

#[cfg(windows)]
fn name_bytes(name: &OsStr) -> Option<&[u8]> {
    Some(name.to_str()?.as_bytes())
}

#[cfg(unix)]
fn metadata(path: &Path) -> Option<Metadata> {
    let meta = fs::symlink_metadata(path).ok()?;
    (!meta.file_type().is_symlink()).then_some(meta)
}

#[cfg(windows)]
fn metadata(path: &Path) -> Option<Metadata> {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    // The reference follows metadata for file links on Windows, while
    // directory links and junctions must never be traversed.
    let meta = fs::metadata(path).ok()?;
    if meta.is_dir()
        && fs::symlink_metadata(path).ok()?.file_attributes()
            & FILE_ATTRIBUTE_REPARSE_POINT
            != 0
    {
        return None;
    }
    Some(meta)
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

        let Some(dir) =
            os_path(abs.to_bytes()).and_then(|path| fs::read_dir(path).ok())
        else {
            return 0;
        };
        let mut parsed = 0;
        for entry in dir.flatten() {
            let name = entry.file_name();
            let Some(name) = name_bytes(&name) else {
                continue;
            };
            let child = [abs.to_bytes(), b"/", name].concat();
            let child_rel = if rel.is_empty() {
                name.to_vec()
            } else {
                [rel.to_bytes(), b"/", name].concat()
            };
            if child.len() > PATH_LIMIT || child_rel.len() > PATH_LIMIT {
                continue;
            }
            let (Ok(child), Ok(child_rel), Ok(name)) = (
                CString::new(child),
                CString::new(child_rel),
                CString::new(name),
            ) else {
                continue;
            };
            let Some(meta) = os_path(child.to_bytes())
                .and_then(|path| metadata(Path::new(path)))
            else {
                continue;
            };
            if meta.is_dir() {
                let excluded = unsafe {
                    cbm_pkgmap_skip_dir(
                        name.as_ptr(),
                        child_rel.as_ptr(),
                        self.excluded,
                        self.excluded_count,
                    )
                };
                if !excluded {
                    parsed += self.visit(&child, &child_rel, depth + 1);
                }
            } else if meta.is_file() {
                let Some(kind) = Manifest::detect(name.to_bytes()) else {
                    continue;
                };
                if self.swift_only && !matches!(kind, Manifest::Swift) {
                    continue;
                }
                if let Some(source) = read(&child) {
                    self.entries.parse(kind, Some(&child_rel), source.bytes());
                    if self.swift_only {
                        self.entries.swift_boundary(&child_rel);
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
