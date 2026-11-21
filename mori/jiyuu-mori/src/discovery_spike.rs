//! Compare a Rust walk to C discovery without entering the indexer.

use std::ffi::{CStr, CString, OsStr, c_char, c_int, c_void};
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr::{NonNull, null, null_mut};
use std::time::Instant;

const IGNORED_CAP: usize = 2_000;

#[derive(Clone, Copy, Debug)]
#[repr(C)]
enum Mode {
    Full = 0,
    Moderate = 1,
    Fast = 2,
}

impl Mode {
    const ALL: [Self; 3] = [Self::Full, Self::Moderate, Self::Fast];
}

#[repr(C)]
struct Options {
    mode: Mode,
    ignore_file: *const c_char,
    max_file_size: i64,
    resource_policy: *const c_void,
    resource_violation: *mut c_void,
}

#[repr(C)]
struct FileInfo {
    path: *mut c_char,
    rel_path: *mut c_char,
    language: c_int,
    size: i64,
    mtime_ns: i64,
}

#[repr(C)]
struct IgnoredFile {
    rel_path: *mut c_char,
    reason: *mut c_char,
}

unsafe extern "C" {
    fn cbm_discover_ex2(
        repo_path: *const c_char,
        opts: *const Options,
        files: *mut *mut FileInfo,
        count: *mut c_int,
        excluded: *mut *mut *mut c_char,
        excluded_count: *mut c_int,
        ignored: *mut *mut IgnoredFile,
        ignored_count: *mut c_int,
        ignored_total: *mut c_int,
    ) -> c_int;
    fn cbm_discover_free(files: *mut FileInfo, count: c_int);
    fn cbm_discover_free_excluded(excluded: *mut *mut c_char, count: c_int);
    fn cbm_discover_free_ignored(ignored: *mut IgnoredFile, count: c_int);

    fn cbm_gitignore_load(path: *const c_char) -> *mut c_void;
    fn cbm_gitignore_match_result(
        matcher: *const c_void,
        rel_path: *const c_char,
        is_dir: bool,
    ) -> c_int;
    fn cbm_gitignore_free(matcher: *mut c_void);
    fn cbm_should_skip_dir(name: *const c_char, mode: Mode) -> bool;
    fn cbm_discover_is_safety_core_directory(name: *const c_char) -> bool;
    fn cbm_discover_path_in_jvm_package(rel_path: *const c_char) -> bool;
    fn cbm_discover_path_has_skip_suffix(rel_path: *const c_char) -> bool;
    fn cbm_has_ignored_suffix(name: *const c_char, mode: Mode) -> bool;
    fn cbm_should_skip_filename(name: *const c_char, mode: Mode) -> bool;
    fn cbm_matches_fast_pattern(name: *const c_char, mode: Mode) -> bool;
    fn cbm_language_for_file(name: *const c_char, path: *const c_char) -> c_int;
    fn cbm_language_name(language: c_int) -> *const c_char;
}

fn c_path(path: &Path) -> CString {
    CString::new(path.as_os_str().as_bytes())
        .expect("filesystem path contains no NUL")
}

fn c_text(ptr: *const c_char) -> Vec<u8> {
    // SAFETY: native discovery result entries and language names point to
    // live, NUL-terminated strings. Copy before their owner releases them.
    unsafe { CStr::from_ptr(ptr).to_bytes().to_vec() }
}

#[derive(Default)]
struct NativeWalk {
    files: *mut FileInfo,
    count: c_int,
    excluded: *mut *mut c_char,
    excluded_count: c_int,
    ignored: *mut IgnoredFile,
    ignored_count: c_int,
    ignored_total: c_int,
}

impl NativeWalk {
    fn run(root: &Path, mode: Mode, size_cap: u64) -> Self {
        let path = c_path(root);
        let options = Options {
            mode,
            ignore_file: null(),
            max_file_size: size_cap.try_into().expect("size cap fits i64"),
            resource_policy: null(),
            resource_violation: null_mut(),
        };
        let mut out = Self::default();
        // SAFETY: arguments follow discover.h. On success C transfers all
        // three arrays to out. On failure C frees any partial results.
        let status = unsafe {
            cbm_discover_ex2(
                path.as_ptr(),
                &options,
                &mut out.files,
                &mut out.count,
                &mut out.excluded,
                &mut out.excluded_count,
                &mut out.ignored,
                &mut out.ignored_count,
                &mut out.ignored_total,
            )
        };
        assert_eq!(status, 0, "native discovery of {}", root.display());
        out
    }

    fn snapshot(&self) -> Snapshot {
        fn items<T>(
            ptr: *const T,
            count: c_int,
        ) -> impl Iterator<Item = *const T> {
            (0..count as usize).map(move |index| ptr.wrapping_add(index))
        }

        // SAFETY: successful discovery supplies that many initialized
        // entries in each owned array. Count zero also permits a null array.
        unsafe {
            Snapshot {
                files: items(self.files, self.count)
                    .map(|ptr| {
                        let file = &*ptr;
                        File {
                            path: c_text(file.rel_path).into(),
                            language: file.language,
                            size: file.size as u64,
                        }
                    })
                    .collect(),
                excluded: items(self.excluded, self.excluded_count)
                    .map(|ptr| c_text(*ptr).into())
                    .collect(),
                ignored: items(self.ignored, self.ignored_count)
                    .map(|ptr| {
                        let file = &*ptr;
                        (c_text(file.rel_path).into(), c_text(file.reason))
                    })
                    .collect(),
                ignored_total: self.ignored_total as usize,
            }
        }
    }
}

impl Drop for NativeWalk {
    fn drop(&mut self) {
        // SAFETY: release the owned arrays exactly once. These functions
        // accept null pointers as well.
        unsafe {
            cbm_discover_free(self.files, self.count);
            cbm_discover_free_excluded(self.excluded, self.excluded_count);
            cbm_discover_free_ignored(self.ignored, self.ignored_count);
        }
    }
}

// Keep raw Unix path bytes so the comparison cannot alias different names
// that happen to have the same lossy UTF-8 rendering.
#[derive(Debug, Eq, Ord, PartialEq, PartialOrd)]
struct RelativePath(Vec<u8>);

impl From<Vec<u8>> for RelativePath {
    fn from(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
}

impl From<&Path> for RelativePath {
    fn from(path: &Path) -> Self {
        Self(path.as_os_str().as_bytes().to_vec())
    }
}

#[derive(Debug, Eq, Ord, PartialEq, PartialOrd)]
struct File {
    path: RelativePath,
    language: c_int,
    size: u64,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Snapshot {
    files: Vec<File>,
    excluded: Vec<RelativePath>,
    ignored: Vec<(RelativePath, Vec<u8>)>,
    ignored_total: usize,
}

impl Snapshot {
    fn sort(&mut self) {
        self.files.sort_unstable();
        self.excluded.sort_unstable();
        self.ignored.sort_unstable();
    }

    fn ignore(&mut self, path: &Path, reason: &str) {
        self.ignored_total += 1;
        if self.ignored.len() < IGNORED_CAP {
            self.ignored.push((path.into(), reason.as_bytes().to_vec()));
        }
    }
}

struct Ignore(NonNull<c_void>);

impl Ignore {
    fn load(path: &Path) -> Option<Self> {
        if !path.is_file() {
            return None;
        }
        // SAFETY: the loader consumes the string only during this call and
        // returns null or transfers one matcher.
        let path = c_path(path);
        NonNull::new(unsafe { cbm_gitignore_load(path.as_ptr()) }).map(Self)
    }

    fn verdict(&self, path: &Path, is_dir: bool) -> c_int {
        let path = c_path(path);
        // SAFETY: this matcher and the path string outlive the call.
        unsafe {
            cbm_gitignore_match_result(self.0.as_ptr(), path.as_ptr(), is_dir)
        }
    }
}

impl Drop for Ignore {
    fn drop(&mut self) {
        // SAFETY: this is the allocator's matching release routine.
        unsafe { cbm_gitignore_free(self.0.as_ptr()) }
    }
}

struct IgnoreAt {
    base: PathBuf,
    matcher: Ignore,
}

struct Walk {
    mode: Mode,
    size_cap: u64,
    cbmignore: Option<Ignore>,
    snapshot: Snapshot,
}

impl Walk {
    fn run(root: &Path, mode: Mode, size_cap: u64) -> io::Result<Snapshot> {
        let mut walk = Self {
            mode,
            size_cap,
            cbmignore: Ignore::load(&root.join(".cbmignore")),
            snapshot: Snapshot::default(),
        };
        walk.directory(root, Path::new(""), &mut Vec::new())?;
        Ok(walk.snapshot)
    }

    fn directory(
        &mut self,
        path: &Path,
        rel: &Path,
        stack: &mut Vec<IgnoreAt>,
    ) -> io::Result<()> {
        let entries = fs::read_dir(path)?;
        let old_len = stack.len();
        if let Some(matcher) = Ignore::load(&path.join(".gitignore")) {
            stack.push(IgnoreAt {
                base: rel.to_owned(),
                matcher,
            });
        }

        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let rel = rel.join(&name);
            let entry_path = entry.path();
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                self.snapshot.ignore(&rel, "symlink");
            } else if kind.is_dir() {
                if self.excludes_dir(&name, &rel, stack) {
                    self.snapshot.excluded.push(rel.as_path().into());
                } else {
                    self.directory(&entry_path, &rel, stack)?;
                }
            } else if kind.is_file() {
                self.file(
                    &name,
                    &entry_path,
                    &rel,
                    stack,
                    entry.metadata()?.len(),
                );
            }
        }
        stack.truncate(old_len);
        Ok(())
    }

    fn cbm_verdict(&self, path: &Path, is_dir: bool) -> c_int {
        self.cbmignore
            .as_ref()
            .map_or(0, |matcher| matcher.verdict(path, is_dir))
    }

    fn excludes_dir(
        &self,
        name: &OsStr,
        rel: &Path,
        stack: &[IgnoreAt],
    ) -> bool {
        let cbm = self.cbm_verdict(rel, true);
        let name = c_path(Path::new(name));
        let native_rel = c_path(rel);
        // SAFETY: the policy helpers read only these live C strings.
        let hard_skip = unsafe {
            let safety_core =
                cbm_discover_is_safety_core_directory(name.as_ptr());
            let default_skip = cbm_should_skip_dir(name.as_ptr(), self.mode)
                && (safety_core
                    || !cbm_discover_path_in_jvm_package(native_rel.as_ptr()));
            (default_skip
                || cbm_discover_path_has_skip_suffix(native_rel.as_ptr()))
                && (cbm >= 0 || safety_core)
        };
        hard_skip || cbm > 0 || git_ignored(rel, true, stack)
    }

    fn file(
        &mut self,
        name: &OsStr,
        path: &Path,
        rel: &Path,
        stack: &[IgnoreAt],
        size: u64,
    ) {
        let name = c_path(Path::new(name));
        // SAFETY: the three filters read the live name and return a flag.
        let native_reason = unsafe {
            if cbm_has_ignored_suffix(name.as_ptr(), self.mode) {
                Some("ignored-suffix")
            } else if cbm_should_skip_filename(name.as_ptr(), self.mode) {
                Some("skip-list")
            } else if cbm_matches_fast_pattern(name.as_ptr(), self.mode) {
                Some("fast-pattern")
            } else {
                None
            }
        };
        let ignore_reason = native_reason.or_else(|| {
            if git_ignored(rel, false, stack) {
                Some("gitignore")
            } else if self.cbm_verdict(rel, false) > 0 {
                Some("cbmignore")
            } else if self.size_cap > 0 && size > self.size_cap {
                Some("size-cap")
            } else {
                None
            }
        });
        if let Some(reason) = ignore_reason {
            self.snapshot.ignore(rel, reason);
            return;
        }

        let path = c_path(path);
        // SAFETY: the classifier reads both live strings and returns a
        // CBMLanguage value. cbm_language_name serves a static string.
        let (language, known) = unsafe {
            let language = cbm_language_for_file(name.as_ptr(), path.as_ptr());
            let known = CStr::from_ptr(cbm_language_name(language)).to_bytes()
                != b"Unknown";
            (language, known)
        };
        if known {
            self.snapshot.files.push(File {
                path: rel.into(),
                language,
                size,
            });
        }
    }
}

fn git_ignored(rel: &Path, is_dir: bool, stack: &[IgnoreAt]) -> bool {
    stack
        .iter()
        .rev()
        .map(|rule| {
            rule.matcher
                .verdict(rel.strip_prefix(&rule.base).unwrap(), is_dir)
        })
        .find(|&verdict| verdict != 0)
        .is_some_and(|verdict| verdict > 0)
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let out = std::process::Command::new("mktemp")
            .arg("-d")
            .arg(std::env::temp_dir().join("mori-discovery-XXXXXXXX"))
            .output()
            .expect("run mktemp");
        assert!(out.status.success(), "{out:?}");
        let mut bytes = out.stdout;
        assert_eq!(bytes.pop(), Some(b'\n'));
        Self(PathBuf::from(OsStr::from_bytes(&bytes)))
    }

    fn write(&self, rel: &str, content: impl AsRef<[u8]>) {
        let path = self.0.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn matches_c_filters_in_all_modes() {
    let scratch = Scratch::new();
    scratch.write(".gitignore", "/nested/root.cpp\n*.gen.ts\n");
    scratch.write("nested/.gitignore", "/skip/\n!keep.gen.ts\n");
    scratch.write(
        ".cbmignore",
        "drop.py\n/private/\n!build/\n!node_modules/\n",
    );
    for path in [
        "src/lib.rs",
        "nested/keep.gen.ts",
        "nested/root.cpp",
        "nested/skip/hidden.rs",
        "nested/keep/main.rs",
        "drop.py",
        "private/no.py",
        "build/kept.rs",
        "node_modules/drop.rs",
        "src/types.d.ts",
        "Cargo.lock",
        "src/unsupported.zzz",
    ] {
        scratch.write(path, "sample\n");
    }
    scratch.write("large.py", [b'x'; 128]);
    scratch.write("script", "#!/usr/bin/env python3\npass\n");
    std::os::unix::fs::symlink("src/lib.rs", scratch.0.join("link.rs")).unwrap();
    std::os::unix::fs::symlink("nested", scratch.0.join("linked-dir")).unwrap();

    for mode in Mode::ALL {
        let mut native = NativeWalk::run(&scratch.0, mode, 64).snapshot();
        let mut rust = Walk::run(&scratch.0, mode, 64).unwrap();
        native.sort();
        rust.sort();
        assert!(
            rust.files
                .iter()
                .any(|file| file.path.0 == b"nested/keep.gen.ts"),
            "{mode:?}"
        );
        assert!(
            rust.ignored
                .iter()
                .any(|(path, reason)| path.0 == b"large.py"
                    && reason == b"size-cap"),
            "{mode:?}"
        );
        assert_eq!(native, rust, "{mode:?}");
    }
}

#[test]
#[ignore = "requires MORI_DISCOVERY_ROOT"]
fn corpus() {
    let root = PathBuf::from(
        std::env::var_os("MORI_DISCOVERY_ROOT").expect("MORI_DISCOVERY_ROOT"),
    )
    .canonicalize()
    .expect("canonicalize corpus root");
    let mode = match std::env::var("MORI_DISCOVERY_MODE")
        .as_deref()
        .unwrap_or("full")
    {
        "full" => Mode::Full,
        "moderate" => Mode::Moderate,
        "fast" => Mode::Fast,
        other => panic!("unknown mode {other:?}"),
    };
    let backend = std::env::var("MORI_DISCOVERY_BACKEND")
        .unwrap_or_else(|_| "compare".into());
    let start = Instant::now();
    match backend.as_str() {
        "c" => {
            let result = NativeWalk::run(&root, mode, 0);
            eprintln!(
                "c {mode:?} {:?} files={} excluded={} ignored={}",
                start.elapsed(),
                result.count,
                result.excluded_count,
                result.ignored_total
            );
        }
        "rust" => {
            let result = Walk::run(&root, mode, 0).unwrap();
            eprintln!(
                "rust {mode:?} {:?} files={} excluded={} ignored={}",
                start.elapsed(),
                result.files.len(),
                result.excluded.len(),
                result.ignored_total
            );
        }
        "compare" => {
            let mut native = NativeWalk::run(&root, mode, 0).snapshot();
            let mut rust = Walk::run(&root, mode, 0).unwrap();
            native.sort();
            rust.sort();

            report_diff("files", &native.files, &rust.files);
            report_diff("excluded subtrees", &native.excluded, &rust.excluded);
            eprintln!(
                "ignored files: c={} rust={} (stored c={} rust={})",
                native.ignored_total,
                rust.ignored_total,
                native.ignored.len(),
                rust.ignored.len()
            );
            if native.ignored_total <= IGNORED_CAP
                && rust.ignored_total <= IGNORED_CAP
            {
                report_diff("ignored entries", &native.ignored, &rust.ignored);
            } else {
                // C stores the first 2,000 entries in its traversal order.
                // Different walks can retain different samples of the same set.
                native.ignored.clear();
                rust.ignored.clear();
            }
            assert!(native == rust, "discovery output differs");
        }
        other => panic!("unknown backend {other:?}"),
    }
}

fn report_diff<T: Ord>(label: &str, native: &[T], rust: &[T]) {
    let only_native = native
        .iter()
        .filter(|item| rust.binary_search(item).is_err())
        .count();
    let only_rust = rust
        .iter()
        .filter(|item| native.binary_search(item).is_err())
        .count();
    eprintln!(
        "{label}: c={} rust={} only_c={only_native} only_rust={only_rust}",
        native.len(),
        rust.len()
    );
}
