//! Compare a Rust walk to C discovery without entering the indexer.

use std::collections::HashSet;
use std::ffi::{CStr, CString, OsStr, c_char, c_int, c_void};
use std::fmt;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::ptr::{NonNull, null, null_mut};
use std::rc::Rc;
use std::time::Instant;

mod admission;
mod git_excludes;
mod sphinx;

use crate::discover_walk::{self, Walker};
use sphinx::Suffixes;

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
    fn cbm_discover_count_bounded(
        repo_path: *const c_char,
        opts: *const Options,
        max_files: c_int,
        deadline_ms: u64,
        count: *mut c_int,
    ) -> c_int;
    fn cbm_discover_ex2_with_walk(
        repo_path: *const c_char,
        opts: *const Options,
        files: *mut *mut FileInfo,
        count: *mut c_int,
        excluded: *mut *mut *mut c_char,
        excluded_count: *mut c_int,
        ignored: *mut *mut IgnoredFile,
        ignored_count: *mut c_int,
        ignored_total: *mut c_int,
        walker: Option<Walker>,
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
    fn cbm_language_classify_with(
        config: *const c_void,
        name: *const c_char,
        head: *const u8,
        head_len: usize,
        more: bool,
        readable: bool,
    ) -> c_int;
    fn cbm_language_name(language: c_int) -> *const c_char;
    fn cbm_workspace_cache_dir() -> *const c_char;
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
        let (status, out) = Self::run_with(root, mode, size_cap, None);
        assert_eq!(status, 0, "native discovery of {}", root.display());
        out
    }

    fn run_with(
        root: &Path,
        mode: Mode,
        size_cap: u64,
        walker: Option<Walker>,
    ) -> (c_int, Self) {
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
            cbm_discover_ex2_with_walk(
                path.as_ptr(),
                &options,
                &mut out.files,
                &mut out.count,
                &mut out.excluded,
                &mut out.excluded_count,
                &mut out.ignored,
                &mut out.ignored_count,
                &mut out.ignored_total,
                walker,
            )
        };
        (status, out)
    }

    fn native_fields(&self) -> Vec<(RelativePath, i64)> {
        // SAFETY: a successful result contains count initialized file entries.
        (0..self.count as usize)
            .map(|index| unsafe {
                let file = &*self.files.add(index);
                (c_text(file.path).into(), file.mtime_ns)
            })
            .collect()
    }

    fn compare_pipeline(root: &Path, mode: Mode, size_cap: u64) -> Self {
        let native = Self::run(root, mode, size_cap);
        let (status, rust) =
            Self::run_with(root, mode, size_cap, Some(discover_walk::walk));
        assert_eq!(status, 0, "Rust discovery of {}", root.display());
        assert_eq!(native.snapshot(), rust.snapshot(), "{mode:?}");
        assert_eq!(native.native_fields(), rust.native_fields(), "{mode:?}");
        native
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
#[derive(Eq, Ord, PartialEq, PartialOrd)]
struct RelativePath(Vec<u8>);

impl fmt::Debug for RelativePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        OsStr::from_bytes(&self.0).fmt(formatter)
    }
}

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

type IgnoreStack = Vec<Rc<IgnoreAt>>;

struct IgnoreAt {
    directory: PathBuf,
    matcher: Ignore,
}

impl IgnoreAt {
    fn load(directory: &Path, source: &Path) -> Option<Rc<Self>> {
        Ignore::load(source).map(|matcher| {
            Rc::new(Self {
                directory: directory.to_owned(),
                matcher,
            })
        })
    }
}

#[derive(Default)]
struct Tracked {
    files: HashSet<PathBuf>,
    dirs: HashSet<PathBuf>,
}

impl Tracked {
    fn load(root: &Path) -> Self {
        let Some(bytes) =
            git(root, &["-c", "core.fsmonitor=false", "ls-files", "-z"])
        else {
            return Self::default();
        };
        let mut tracked = Self::default();
        for raw in bytes
            .split(|&byte| byte == 0)
            .filter(|path| !path.is_empty())
        {
            let path = Path::new(OsStr::from_bytes(raw));
            tracked.files.insert(path.to_owned());
            for parent in path
                .ancestors()
                .skip(1)
                .take_while(|path| !path.as_os_str().is_empty())
            {
                tracked.dirs.insert(parent.to_owned());
            }
        }
        tracked
    }
}

fn git(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let mut command = Command::new("git");
    // Keep an inherited Git command's repository and index out of this walk.
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_PREFIX",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        command.env_remove(var);
    }
    let out = command.current_dir(root).args(args).output().ok()?;
    out.status.success().then_some(out.stdout)
}

fn git_common_dir(root: &Path) -> Option<PathBuf> {
    if !root.join(".git").exists() {
        return None;
    }
    let mut raw = git(
        root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    if raw.pop() != Some(b'\n') {
        return None;
    }
    Some(PathBuf::from(OsStr::from_bytes(&raw)))
}

enum Directory {
    Keep,
    Skip,
    GitIgnored,
}

enum FileVerdict {
    Include(c_int),
    Ignore(&'static str),
    Unknown,
}

struct Walk {
    mode: Mode,
    size_cap: u64,
    cbmignore: Option<Ignore>,
    user_ignore: Option<Ignore>,
    tracked: Tracked,
    cache: Option<PathBuf>,
    snapshot: Snapshot,
}

impl Walk {
    fn run(root: &Path, mode: Mode, size_cap: u64) -> io::Result<Snapshot> {
        let (mut walk, root, mut stack) = Self::prepare(root, mode, size_cap)?;
        walk.directory(
            &root,
            Path::new(""),
            false,
            &mut stack,
            &Suffixes::default(),
        )?;
        Ok(walk.snapshot)
    }

    fn prepare(
        root: &Path,
        mode: Mode,
        size_cap: u64,
    ) -> io::Result<(Self, PathBuf, IgnoreStack)> {
        let root = root.canonicalize()?;
        let repository = root.ancestors().find_map(|directory| {
            git_common_dir(directory).map(|common| (directory, common))
        });
        let rule_root = repository
            .as_ref()
            .map_or(root.as_path(), |(path, _)| *path);
        let ancestors: Vec<_> = root
            .ancestors()
            .take_while(|directory| directory.starts_with(rule_root))
            .collect();
        let mut stack = Vec::new();
        // The last matching rule wins. Paths stay relative to the directory
        // that owns each rule. C reads info/exclude after the root .gitignore.
        for directory in ancestors.into_iter().rev() {
            stack.extend(IgnoreAt::load(
                directory,
                &directory.join(".gitignore"),
            ));
            if let Some((git_root, common)) = &repository
                && directory == *git_root
            {
                stack.extend(IgnoreAt::load(
                    directory,
                    &common.join("info/exclude"),
                ));
            }
        }
        // SAFETY: the getter returns null or a live NUL-terminated path.
        // Copy before another native call can modify its backing buffer.
        let cache =
            unsafe { cbm_workspace_cache_dir().as_ref().map(|ptr| c_text(ptr)) }
                .filter(|path| !path.is_empty())
                .map(|path| PathBuf::from(OsStr::from_bytes(&path)));
        let walk = Self {
            mode,
            size_cap,
            cbmignore: Ignore::load(&root.join(".cbmignore")),
            user_ignore: repository
                .as_ref()
                .filter(|(_, common)| common.join("config").is_file())
                .and_then(|_| git_excludes::user_ignore_path())
                .and_then(|path| Ignore::load(&path)),
            tracked: if repository.is_some() {
                Tracked::load(&root)
            } else {
                Tracked::default()
            },
            cache,
            snapshot: Snapshot::default(),
        };
        Ok((walk, root, stack))
    }

    fn directory(
        &mut self,
        path: &Path,
        rel: &Path,
        under_gitignore: bool,
        stack: &mut IgnoreStack,
        inherited_suffixes: &Suffixes,
    ) -> io::Result<()> {
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            // The ordinary C walk abandons a child it cannot open.
            Err(_) if !rel.as_os_str().is_empty() => return Ok(()),
            Err(error) => return Err(error),
        };
        let old_len = stack.len();
        if !rel.as_os_str().is_empty() {
            stack.extend(IgnoreAt::load(path, &path.join(".gitignore")));
        }
        let local_suffixes = Suffixes::read(path);
        let suffixes = local_suffixes.as_ref().unwrap_or(inherited_suffixes);

        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let rel = rel.join(&name);
            let entry_path = entry.path();
            let Ok(metadata) = fs::symlink_metadata(&entry_path) else {
                // The ordinary C walk also skips entries that fail lstat.
                continue;
            };
            let kind = metadata.file_type();
            if kind.is_symlink() {
                self.snapshot.ignore(&rel, "symlink");
            } else if kind.is_dir() {
                match self.classify_dir(
                    &name,
                    &entry_path,
                    &rel,
                    under_gitignore,
                    stack,
                ) {
                    Directory::Keep => self.directory(
                        &entry_path,
                        &rel,
                        false,
                        stack,
                        suffixes,
                    )?,
                    Directory::GitIgnored
                        if self.tracked.dirs.contains(&rel) =>
                    {
                        self.directory(
                            &entry_path,
                            &rel,
                            true,
                            stack,
                            suffixes,
                        )?;
                    }
                    Directory::GitIgnored | Directory::Skip => {
                        self.snapshot.excluded.push(rel.as_path().into());
                    }
                }
            } else if kind.is_file() {
                let size = metadata.len();
                match self.classify_file(
                    &entry_path,
                    &rel,
                    size,
                    under_gitignore,
                    stack,
                    suffixes,
                ) {
                    FileVerdict::Include(language) => {
                        self.snapshot.files.push(File {
                            path: rel.as_path().into(),
                            language,
                            size,
                        })
                    }
                    FileVerdict::Ignore(reason) => {
                        self.snapshot.ignore(&rel, reason)
                    }
                    FileVerdict::Unknown => {}
                }
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

    fn user_ignored(&self, rel: &Path, is_dir: bool) -> bool {
        self.user_ignore
            .as_ref()
            .is_some_and(|matcher| matcher.verdict(rel, is_dir) > 0)
    }

    fn classify_dir(
        &self,
        name: &OsStr,
        absolute: &Path,
        rel: &Path,
        under_gitignore: bool,
        stack: &[Rc<IgnoreAt>],
    ) -> Directory {
        if self
            .cache
            .as_ref()
            .is_some_and(|cache| absolute.starts_with(cache))
        {
            return Directory::Skip;
        }
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
        if hard_skip || cbm > 0 {
            Directory::Skip
        } else if under_gitignore
            || git_ignored(absolute, true, stack)
            || (cbm >= 0 && self.user_ignored(rel, true))
        {
            Directory::GitIgnored
        } else {
            Directory::Keep
        }
    }

    fn classify_file(
        &self,
        absolute: &Path,
        rel: &Path,
        size: u64,
        under_gitignore: bool,
        stack: &[Rc<IgnoreAt>],
        suffixes: &Suffixes,
    ) -> FileVerdict {
        let name = c_path(Path::new(rel.file_name().expect("file has a name")));
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
            let tracked = self.tracked.files.contains(rel);
            if !tracked
                && (under_gitignore || git_ignored(absolute, false, stack))
            {
                return Some("gitignore");
            }
            let cbm = self.cbm_verdict(rel, false);
            if cbm > 0 {
                Some("cbmignore")
            } else if self.size_cap > 0 && size > self.size_cap {
                Some("size-cap")
            } else if !tracked && cbm >= 0 && self.user_ignored(rel, false) {
                Some("gitignore")
            } else {
                None
            }
        });
        if let Some(reason) = ignore_reason {
            return FileVerdict::Ignore(reason);
        }

        let path = c_path(absolute);
        // SAFETY: the classifier reads both live strings and returns a
        // CBMLanguage value. cbm_language_name serves a static string.
        let language = unsafe {
            let language = cbm_language_for_file(name.as_ptr(), path.as_ptr());
            let known = CStr::from_ptr(cbm_language_name(language)).to_bytes()
                != b"Unknown";
            if known {
                Some(language)
            } else if suffixes.matches(name.as_bytes()) {
                // The C fallback uses the built-in reStructuredText ID even
                // when a user has overridden the normal .rst classification.
                Some(cbm_language_classify_with(
                    null(),
                    c"page.rst".as_ptr(),
                    null(),
                    0,
                    false,
                    false,
                ))
            } else {
                None
            }
        };
        language.map_or(FileVerdict::Unknown, FileVerdict::Include)
    }
}

fn git_ignored(absolute: &Path, is_dir: bool, stack: &[Rc<IgnoreAt>]) -> bool {
    stack
        .iter()
        .rev()
        .map(|rule| {
            let relative = absolute
                .strip_prefix(&rule.directory)
                .expect("ignore anchor is an ancestor of the entry");
            rule.matcher.verdict(relative, is_dir)
        })
        .find(|&verdict| verdict != 0)
        .is_some_and(|verdict| verdict > 0)
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let out = Command::new("mktemp")
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

fn native_admission(
    root: &Path,
    mode: Mode,
    max_files: c_int,
    deadline_ms: u64,
) -> (c_int, c_int) {
    let path = c_path(root);
    let options = Options {
        mode,
        ignore_file: null(),
        max_file_size: 0,
        resource_policy: null(),
        resource_violation: null_mut(),
    };
    let mut count = -2;
    // SAFETY: the path, options, and count remain live for the native call.
    let status = unsafe {
        cbm_discover_count_bounded(
            path.as_ptr(),
            &options,
            max_files,
            deadline_ms,
            &mut count,
        )
    };
    (status, count)
}

#[test]
fn admission_counts_indexable_files_up_to_the_ceiling() {
    let scratch = Scratch::new();
    scratch.write(".gitignore", "ignored.rs\n");
    scratch.write("ignored.rs", "fn ignored() {}\n");
    scratch.write("src/first.rs", "fn first() {}\n");
    scratch.write("src/second.rs", "fn second() {}\n");
    std::os::unix::fs::symlink("src/first.rs", scratch.0.join("link.rs"))
        .unwrap();

    for mode in Mode::ALL {
        for (max_files, expected) in [(0, (1, 0)), (1, (1, 1)), (2, (0, 2))] {
            assert_eq!(
                native_admission(&scratch.0, mode, max_files, 0),
                expected
            );
            let count = max_files as usize;
            let expected = if expected.0 == 0 {
                admission::Count::Exact(count)
            } else {
                admission::Count::LimitExceeded(count)
            };
            assert_eq!(
                admission::count_bounded(&scratch.0, mode, count, 0).unwrap(),
                expected
            );
        }
        assert_eq!(native_admission(&scratch.0, mode, 2, 1), (-1, -1));
        let expired = admission::count_bounded(&scratch.0, mode, 2, 1);
        assert_eq!(expired.unwrap_err().kind(), io::ErrorKind::TimedOut);
    }
}

#[test]
fn admission_refuses_an_unobservable_child() {
    use std::os::unix::fs::PermissionsExt;

    let scratch = Scratch::new();
    scratch.write("visible.rs", "fn visible() {}\n");
    scratch.write("child/hidden.rs", "fn hidden() {}\n");
    let child = scratch.0.join("child");

    for (mode_bits, expected_error) in
        [(0o100, "open child directory"), (0o400, "stat child entry")]
    {
        fs::set_permissions(&child, fs::Permissions::from_mode(mode_bits))
            .unwrap();
        let error = if mode_bits == 0o100 {
            fs::read_dir(&child).err()
        } else {
            fs::symlink_metadata(child.join("hidden.rs")).err()
        };
        let outcomes =
            Mode::ALL.map(|mode| native_admission(&scratch.0, mode, 2, 0));
        let rust_outcomes = Mode::ALL
            .map(|mode| admission::count_bounded(&scratch.0, mode, 2, 0));
        let native_limit = native_admission(&scratch.0, Mode::Full, 0, 0);
        let rust_limit = admission::count_bounded(&scratch.0, Mode::Full, 0, 0);
        fs::set_permissions(&child, fs::Permissions::from_mode(0o700)).unwrap();

        // The root file exceeds the ceiling before the walk opens the child.
        assert_eq!(native_limit, (1, 0));
        assert_eq!(rust_limit.unwrap(), admission::Count::LimitExceeded(0));
        // A privileged process may be able to observe both paths.
        if let Some(error) = error {
            assert_eq!(
                error.kind(),
                io::ErrorKind::PermissionDenied,
                "{expected_error}"
            );
            assert_eq!(outcomes, [(-1, -1); 3], "{expected_error}");
            for outcome in rust_outcomes {
                assert_eq!(
                    outcome.unwrap_err().kind(),
                    io::ErrorKind::PermissionDenied,
                    "{expected_error}"
                );
            }
        }
    }
}

#[test]
fn matches_c_when_a_child_cannot_be_read_or_statted() {
    use std::os::unix::fs::PermissionsExt;

    let scratch = Scratch::new();
    scratch.write("src/main.rs", "fn main() {}\n");
    scratch.write("blocked-read/hidden.rs", "fn hidden() {}\n");
    scratch.write("blocked-stat/hidden.rs", "fn hidden() {}\n");
    let blocked_read = scratch.0.join("blocked-read");
    let blocked_stat = scratch.0.join("blocked-stat");

    fs::set_permissions(&blocked_read, fs::Permissions::from_mode(0o100))
        .unwrap();
    fs::set_permissions(&blocked_stat, fs::Permissions::from_mode(0o400))
        .unwrap();
    let read_error = fs::read_dir(&blocked_read).err();
    let stat_error = fs::symlink_metadata(blocked_stat.join("hidden.rs")).err();
    let blocked_root = Walk::run(&blocked_read, Mode::Full, 0);
    NativeWalk::compare_pipeline(&blocked_read, Mode::Full, 0);
    let snapshots = Mode::ALL.map(|mode| {
        let native =
            NativeWalk::compare_pipeline(&scratch.0, mode, 0).snapshot();
        (mode, native, Walk::run(&scratch.0, mode, 0))
    });
    fs::set_permissions(&blocked_read, fs::Permissions::from_mode(0o700))
        .unwrap();
    fs::set_permissions(&blocked_stat, fs::Permissions::from_mode(0o700))
        .unwrap();

    // A test process with DAC override can read both paths.
    if read_error.is_none() && stat_error.is_none() {
        return;
    }
    assert_eq!(read_error.unwrap().kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(stat_error.unwrap().kind(), io::ErrorKind::PermissionDenied);
    assert!(matches!(
        blocked_root,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied
    ));
    for (mode, mut native, rust) in snapshots {
        assert_eq!(native.files.len(), 1, "{mode:?}");
        assert_eq!(native.files[0].path.0, b"src/main.rs");
        let mut rust = rust.unwrap();
        native.sort();
        rust.sort();
        assert_eq!(native, rust, "{mode:?}");
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
        let mut native =
            NativeWalk::compare_pipeline(&scratch.0, mode, 64).snapshot();
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
fn matches_c_tracked_paths_and_info_exclude() {
    let scratch = Scratch::new();
    assert!(git(&scratch.0, &["init", "-q"]).is_some());
    scratch.write(".git/info/exclude", "/cache/\n!keep.rs\n");
    scratch.write(".gitignore", "/generated/\nkeep.rs\n");
    scratch.write(".cbmignore", "generated/hide.rs\n");
    for path in [
        "generated/keep.rs",
        "generated/hide.rs",
        "generated/untracked.rs",
        "generated/nested/untracked.rs",
        "cache/ignored.rs",
        "keep.rs",
    ] {
        scratch.write(path, "fn sample() {}\n");
    }
    assert!(
        git(
            &scratch.0,
            &["add", "-f", "generated/keep.rs", "generated/hide.rs"]
        )
        .is_some()
    );

    let mut native =
        NativeWalk::compare_pipeline(&scratch.0, Mode::Full, 0).snapshot();
    let mut rust = Walk::run(&scratch.0, Mode::Full, 0).unwrap();
    native.sort();
    rust.sort();
    assert_eq!(
        rust.files
            .iter()
            .map(|file| file.path.0.as_slice())
            .collect::<Vec<_>>(),
        [b"generated/keep.rs".as_slice(), b"keep.rs".as_slice()]
    );
    assert_eq!(native, rust);
}

#[test]
fn matches_c_ignores_from_enclosing_repository() {
    let scratch = Scratch::new();
    assert!(git(&scratch.0, &["init", "-q"]).is_some());
    scratch.write(
        ".gitignore",
        "/secret.rs\n*.gen.rs\n/pkg/run/conflict.rs\n/pkg/run/tracked/\n",
    );
    scratch.write(
        ".git/info/exclude",
        "!/pkg/run/conflict.rs\n/pkg/run/excluded.rs\n",
    );
    scratch.write("pkg/.gitignore", "!/run/revived.gen.rs\n/run/scratch/\n");
    scratch.write("pkg/run/.gitignore", "!local.gen.rs\n");
    for path in [
        "conflict.rs",
        "excluded.rs",
        "filtered.gen.rs",
        "local.gen.rs",
        "revived.gen.rs",
        "scratch/hidden.rs",
        "secret.rs",
        "tracked/keep.rs",
        "tracked/untracked.rs",
    ] {
        scratch.write(&format!("pkg/run/{path}"), "fn sample() {}\n");
    }
    assert!(
        git(&scratch.0, &["add", "-f", "pkg/run/tracked/keep.rs"]).is_some()
    );

    let root = scratch.0.join("pkg/run");
    for mode in Mode::ALL {
        let mut native = NativeWalk::compare_pipeline(&root, mode, 0).snapshot();
        let mut rust = Walk::run(&root, mode, 0).unwrap();
        native.sort();
        rust.sort();
        assert_eq!(native, rust, "{mode:?}");
        assert_eq!(
            rust.files
                .iter()
                .filter(|file| file.path.0.ends_with(b".rs"))
                .map(|file| file.path.0.as_slice())
                .collect::<Vec<_>>(),
            [
                b"conflict.rs".as_slice(),
                b"local.gen.rs",
                b"revived.gen.rs",
                b"secret.rs",
                b"tracked/keep.rs",
            ],
            "{mode:?}"
        );
        assert_eq!(rust.excluded, [RelativePath::from(Path::new("scratch"))]);
        assert_eq!(
            rust.ignored
                .iter()
                .map(|(path, reason)| (path.0.as_slice(), reason.as_slice()))
                .collect::<Vec<_>>(),
            [
                (b"excluded.rs".as_slice(), b"gitignore".as_slice()),
                (b"filtered.gen.rs", b"gitignore"),
                (b"tracked/untracked.rs", b"gitignore"),
            ],
            "{mode:?}"
        );
    }
}

#[test]
fn matches_c_user_git_excludes() {
    const FIXTURE: &str = "MORI_DISCOVERY_USER_EXCLUDES_FIXTURE";
    let Some(root) = std::env::var_os(FIXTURE) else {
        let scratch = Scratch::new();
        // A separate process gives C and Rust the same isolated user config
        // without changing the other tests' environment.
        let result = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "discovery_spike::matches_c_user_git_excludes"])
            .env(FIXTURE, &scratch.0)
            .env("HOME", scratch.0.join("home"))
            .env("XDG_CONFIG_HOME", scratch.0.join("xdg"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    };
    // The parent owns this scratch directory and removes it after the child exits.
    let scratch = std::mem::ManuallyDrop::new(Scratch(root.into()));
    let repo = scratch.0.join("repo");
    fs::create_dir(&repo).unwrap();
    assert!(git(&repo, &["init", "-q"]).is_some());
    scratch.write("home/.gitconfig", "[CoRe]\n excludesFile = ~/unused\n ExcludesFILE = \"~/custom #ignore\" ; comment\n");
    scratch.write("home/custom #ignore", "home.rs\ntracked.rs\nrescued.rs\ncbm-owned.rs\ntoo-large.rs\nglobal-dir/\nrescued-dir/\n");
    scratch.write("xdg/git/config", "[core]\n excludesFile = ~/xdg-ignore\n");
    scratch.write("home/xdg-ignore", "xdg-config.rs\n");
    scratch.write("xdg/git/ignore", "xdg-default.rs\n");
    scratch.write(
        "repo/.cbmignore",
        "!rescued.rs\ncbm-owned.rs\n!rescued-dir/\n",
    );
    scratch.write("repo/.gitignore", "!home.rs\n");
    for path in [
        "home.rs",
        "xdg-config.rs",
        "xdg-default.rs",
        "tracked.rs",
        "rescued.rs",
        "cbm-owned.rs",
        "global-dir/filtered.rs",
        "rescued-dir/kept.rs",
    ] {
        scratch.write(&format!("repo/{path}"), "fn sample() {}\n");
    }
    scratch.write("repo/too-large.rs", [b'x'; 128]);
    assert!(git(&repo, &["add", "-f", "tracked.rs"]).is_some());

    for (precedence, excluded_file) in [
        ("home/.gitconfig", "home.rs"),
        ("xdg/git/config", "xdg-config.rs"),
        ("xdg/git/ignore", "xdg-default.rs"),
    ] {
        for mode in Mode::ALL {
            let mut native =
                NativeWalk::compare_pipeline(&repo, mode, 64).snapshot();
            let mut rust = Walk::run(&repo, mode, 64).unwrap();
            native.sort();
            rust.sort();
            assert_eq!(native, rust, "{precedence} {mode:?}");
            for (path, reason) in [
                (excluded_file, "gitignore"),
                ("cbm-owned.rs", "cbmignore"),
                ("too-large.rs", "size-cap"),
            ] {
                assert!(
                    rust.ignored
                        .iter()
                        .any(|(file, why)| file.0 == path.as_bytes()
                            && why == reason.as_bytes()),
                    "{precedence} {mode:?}: {rust:?}"
                );
            }
            for retained in ["tracked.rs", "rescued.rs", "rescued-dir/kept.rs"] {
                assert!(
                    rust.files
                        .iter()
                        .any(|file| file.path.0 == retained.as_bytes()),
                    "{precedence} {mode:?}: {rust:?}"
                );
            }
            let global_directory = RelativePath::from(Path::new("global-dir"));
            assert_eq!(
                rust.excluded.contains(&global_directory),
                precedence == "home/.gitconfig"
            );
        }
        fs::remove_file(scratch.0.join(precedence)).unwrap();
    }
}

#[test]
fn matches_c_sphinx_suffixes() {
    let scratch = Scratch::new();
    scratch.write(
        "guide/conf.py",
        "raise RuntimeError('do not execute this file')\n\
         source_suffix = {\n\
             '.note': 'restructuredtext',\n\
             '.mdonly': 'markdown',\n\
             '.rst': 'restructuredtext',\n\
             '.py': 'restructuredtext',\n\
             '.bad.name': 'restructuredtext',\n\
         }\n\
         source_suffix = '.later'\n",
    );
    scratch.write(
        "guide/nested/conf.py",
        "source_suffix = ['.nested', '.rst']\n",
    );
    scratch.write("guide/scalar/conf.py", "source_suffix = '.scalar'\n");
    let mut padded = vec![b'#'; 65_536];
    padded.extend_from_slice(b"\nsource_suffix = '.late'\n");
    scratch.write("guide/padded/conf.py", padded);
    for path in [
        "guide/overview.note",
        "guide/sub/chapter.note",
        "guide/code.py",
        "guide/markdown.mdonly",
        "guide/second.later",
        "guide/invalid.bad.name",
        "guide/nested/inherited.note",
        "guide/nested/override.nested",
        "guide/scalar/content.scalar",
        "guide/padded/beyond.late",
        "guide/padded/inherited.note",
        "outside.note",
    ] {
        scratch.write(path, "Example\n=======\n");
    }

    for mode in Mode::ALL {
        let mut native =
            NativeWalk::compare_pipeline(&scratch.0, mode, 0).snapshot();
        let mut rust = Walk::run(&scratch.0, mode, 0).unwrap();
        native.sort();
        rust.sort();
        assert_eq!(native, rust, "{mode:?}");
        let rst_paths: Vec<_> = rust
            .files
            .iter()
            .filter_map(|file| {
                // SAFETY: discovery returns native language IDs, and the name
                // function returns a static NUL-terminated string for each ID.
                let name =
                    unsafe { CStr::from_ptr(cbm_language_name(file.language)) };
                (name == c"reStructuredText").then_some(file.path.0.as_slice())
            })
            .collect();
        assert_eq!(
            rst_paths,
            [
                b"guide/nested/override.nested".as_slice(),
                b"guide/overview.note",
                b"guide/scalar/content.scalar",
                b"guide/sub/chapter.note",
            ],
            "{mode:?}"
        );
    }
}

#[test]
fn production_walk_preserves_order_and_the_capped_ignore_sample() {
    use std::os::unix::fs::symlink;

    let scratch = Scratch::new();
    scratch.write("root.rs", "fn root() {}");
    let name = if cfg!(target_os = "linux") {
        OsStr::from_bytes(b"raw-\xff.rs")
    } else {
        OsStr::new("other.rs")
    };
    fs::write(scratch.0.join(name), "fn other() {}").unwrap();
    for dir in ["alpha", "beta", "gamma"] {
        scratch.write(&format!("{dir}/lib.rs"), "pub fn lib() {}");
        for index in 0..(IGNORED_CAP / 3 + 1) {
            symlink("lib.rs", scratch.0.join(format!("{dir}/link-{index}")))
                .unwrap();
        }
    }
    let native = NativeWalk::compare_pipeline(&scratch.0, Mode::Full, 0);
    assert_eq!(native.count, 5);
    assert_eq!(native.ignored_count as usize, IGNORED_CAP);
    assert_eq!(native.ignored_total as usize, IGNORED_CAP + 1);
    assert!(
        native.snapshot().files[..2]
            .iter()
            .all(|file| !file.path.0.contains(&b'/'))
    );
}

#[test]
fn production_walk_preserves_failed_discovery_outputs() {
    let scratch = Scratch::new();
    scratch.write("file.rs", "fn main() {}");
    let roots = [scratch.0.join("missing"), scratch.0.join("file.rs")];
    // The long lexical spelling reaches the same root. The first entry's
    // absolute path then exceeds the native walk's 4096-byte buffer.
    #[cfg(target_os = "linux")]
    let roots = {
        let root = scratch.0.to_str().unwrap();
        let root = PathBuf::from(format!(
            "{root}{}",
            "/.".repeat((4095 - root.len()) / 2)
        ));
        assert!(root.is_dir());
        [roots[0].clone(), roots[1].clone(), root]
    };
    for root in roots {
        for walker in [None, Some(discover_walk::walk as Walker)] {
            let (status, output) =
                NativeWalk::run_with(&root, Mode::Full, 0, walker);
            assert_eq!(status, -1);
            assert_eq!(output.snapshot(), Snapshot::default());
        }
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
        "pipeline-compare" => {
            let result = NativeWalk::compare_pipeline(&root, mode, 0);
            eprintln!(
                "c and Rust pipeline {mode:?} {:?} files={} excluded={} ignored={}",
                start.elapsed(),
                result.count,
                result.excluded_count,
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

fn report_diff<T: fmt::Debug + Ord>(label: &str, native: &[T], rust: &[T]) {
    let only_native = || {
        native
            .iter()
            .filter(|item| rust.binary_search(item).is_err())
    };
    let only_rust = || {
        rust.iter()
            .filter(|item| native.binary_search(item).is_err())
    };
    eprintln!(
        "{label}: c={} rust={} only_c={} only_rust={}",
        native.len(),
        rust.len(),
        only_native().count(),
        only_rust().count()
    );
    if std::env::var_os("MORI_DISCOVERY_SHOW_DIFF").is_some() {
        for item in only_native().take(10) {
            eprintln!("  only C: {item:?}");
        }
        for item in only_rust().take(10) {
            eprintln!("  only Rust: {item:?}");
        }
    }
}
