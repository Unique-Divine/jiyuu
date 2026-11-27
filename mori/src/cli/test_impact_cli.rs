//! Command line for the native test-impact engine.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr::{self, NonNull};
use std::slice;

use libc::FILE;

const USAGE: &str = concat!(
    "usage: mori test-impact publish --repo DIR --out DIR\n",
    "           [--previous BUNDLE] [--coverage DIR --observed-at EPOCH]\n",
    "           [--platform LABEL] [--config FILE] [--work DIR] [--deadline-s N]\n",
    "       mori test-impact select --repo DIR --base REF\n",
    "           [--config FILE] [--artifact BUNDLE --artifact-commit SHA\n",
    "           --artifact-verified] [--platform LABEL] [--work DIR]\n",
    "           [--deadline-s N] [--vetoed-out FILE]\n",
);

const DEFAULT_DEADLINE_S: i64 = 1800;
const OK: c_int = 0;

// Keep these borrowed requests in sync with test_impact_engine.h. Native jobs
// join before returning, so argv and the out-slot pointers live for the job.
#[repr(C)]
#[derive(Default)]
struct PublishRequest {
    repo_root: *const c_char,
    work_parent: *const c_char,
    out_dir: *const c_char,
    previous_dir: *const c_char,
    coverage_dir: *const c_char,
    observed_at: i64,
    platform: *const c_char,
    config_path: *const c_char,
    deadline_ms: u64,
}

#[repr(C)]
#[derive(Default)]
struct SelectRequest {
    repo_root: *const c_char,
    base_ref: *const c_char,
    work_parent: *const c_char,
    config_path: *const c_char,
    deadline_ms: u64,
    artifact_dir: *const c_char,
    artifact_commit: *const c_char,
    artifact_verified: bool,
    platform: *const c_char,
    vetoed_out: *mut *mut c_void,
}

unsafe extern "C" {
    fn cbm_test_impact_cli_write(
        stream: *mut FILE,
        bytes: *const u8,
        length: usize,
        newline: bool,
    ) -> bool;
    fn cbm_test_impact_cli_close(stream: *mut FILE) -> bool;
    fn cbm_now_ms() -> u64;
    fn cbm_resolve_cache_dir() -> *const c_char;
    fn cbm_mkdir_p(path: *const c_char, mode: c_int) -> bool;
    fn cbm_fopen(path: *const c_char, mode: *const c_char) -> *mut FILE;
    fn cbm_test_impact_publish(
        request: *const PublishRequest,
        diagnostic: *mut c_char,
        diagnostic_len: usize,
    ) -> c_int;
    fn cbm_test_impact_run(
        request: *const SelectRequest,
        out: *mut *mut c_void,
        diagnostic: *mut c_char,
        diagnostic_len: usize,
    ) -> c_int;
    fn cbm_test_result_json(
        result: *const c_void,
        length: *mut usize,
    ) -> *const c_char;
    fn cbm_test_result_free(result: *mut c_void);
}

// Use the dispatcher's CRT streams, including its buffering and Windows text
// mode. Only the veto sidecar opens a new stream, in binary mode.
#[derive(Clone, Copy)]
struct Stream(NonNull<FILE>);

impl Stream {
    fn write(self, bytes: &[u8]) -> bool {
        self.write_bytes(bytes, false)
    }

    fn line(self, bytes: &[u8]) -> bool {
        self.write_bytes(bytes, true)
    }

    fn write_bytes(self, bytes: &[u8], newline: bool) -> bool {
        // SAFETY: The caller keeps the stream open and C borrows the slice.
        unsafe {
            cbm_test_impact_cli_write(
                self.0.as_ptr(),
                bytes.as_ptr(),
                bytes.len(),
                newline,
            )
        }
    }
}

struct NativeResult(NonNull<c_void>);

impl NativeResult {
    fn write_json(&self, stream: Stream) -> bool {
        let mut length = 0;
        // SAFETY: The JSON remains borrowed from this owned result until drop.
        unsafe {
            let json = cbm_test_result_json(self.0.as_ptr(), &mut length);
            !json.is_null()
                && stream.line(slice::from_raw_parts(json.cast(), length))
        }
    }
}

impl Drop for NativeResult {
    fn drop(&mut self) {
        // SAFETY: Both engine out slots transfer ownership to the caller.
        unsafe { cbm_test_result_free(self.0.as_ptr()) };
    }
}

fn parse<'a>(
    mut args: impl Iterator<Item = &'a CStr>,
    values: &mut [(&[u8], &mut *const c_char)],
    mut verified: Option<&mut bool>,
    stderr: Stream,
) -> bool {
    while let Some(arg) = args.next() {
        let arg = arg.to_bytes();
        if arg == b"--artifact-verified"
            && let Some(verified) = verified.as_deref_mut()
        {
            *verified = true;
            continue;
        }
        let Some((_, value)) = values.iter_mut().find(|(name, _)| *name == arg)
        else {
            stderr.write(b"test-impact: unknown argument ");
            stderr.line(arg);
            return false;
        };
        let Some(next) = args.next() else {
            stderr.write(b"test-impact: ");
            stderr.write(arg);
            stderr.line(b" needs a value");
            return false;
        };
        **value = next.as_ptr();
    }
    true
}

fn positive_number(text: *const c_char, default: i64) -> Option<i64> {
    // SAFETY: A non-null input is a borrowed argv string. strtoll preserves
    // the legacy whitespace, sign and overflow behavior of these flags.
    unsafe {
        if text.is_null() || *text == 0 {
            return Some(default);
        }
        let mut end = ptr::null_mut();
        let number = libc::strtoll(text, &mut end, 10);
        (number > 0 && !end.is_null() && *end == 0).then_some(number)
    }
}

fn work_parent(
    requested: *const c_char,
    buffer: &mut [u8; 4096],
) -> Option<*const c_char> {
    if !requested.is_null() {
        return Some(requested);
    }
    // SAFETY: Native cache resolution borrows a thread-local path. Use the
    // same private mkdir policy and bounded path as the previous CLI.
    unsafe {
        let cache = cbm_resolve_cache_dir();
        if cache.is_null() {
            return None;
        }
        let bytes = CStr::from_ptr(cache)
            .to_bytes()
            .iter()
            .chain(b"/test-impact");
        // Leave the last byte for NUL, matching the previous 4096-byte buffer.
        for (slot, byte) in buffer[..4095].iter_mut().zip(bytes) {
            *slot = *byte;
        }
        let path = buffer.as_ptr().cast();
        cbm_mkdir_p(path, 0o700).then_some(path)
    }
}

fn deadline(seconds: i64) -> u64 {
    // SAFETY: Use the same clock as the native engine's cancellation checks.
    unsafe { cbm_now_ms() }.wrapping_add((seconds as u64).wrapping_mul(1000))
}

fn usage(stderr: Stream) -> c_int {
    stderr.write(USAGE.as_bytes());
    2
}

fn publish<'a>(args: impl Iterator<Item = &'a CStr>, stderr: Stream) -> c_int {
    let mut request = PublishRequest::default();
    let (mut observed, mut seconds) = (ptr::null(), ptr::null());
    if !parse(
        args,
        &mut [
            (b"--config", &mut request.config_path),
            (b"--repo", &mut request.repo_root),
            (b"--out", &mut request.out_dir),
            (b"--previous", &mut request.previous_dir),
            (b"--coverage", &mut request.coverage_dir),
            (b"--observed-at", &mut observed),
            (b"--platform", &mut request.platform),
            (b"--work", &mut request.work_parent),
            (b"--deadline-s", &mut seconds),
        ],
        None,
        stderr,
    ) || request.repo_root.is_null()
        || request.out_dir.is_null()
    {
        return usage(stderr);
    }
    let mut work = [0; 4096];
    let (Some(observed), Some(seconds), Some(parent)) = (
        positive_number(observed, 0),
        positive_number(seconds, DEFAULT_DEADLINE_S),
        work_parent(request.work_parent, &mut work),
    ) else {
        return usage(stderr);
    };
    request.observed_at = observed;
    request.work_parent = parent;
    request.deadline_ms = deadline(seconds);

    let mut diagnostic = [0; 512];
    // SAFETY: The native call borrows the request and buffer until it returns.
    let status = unsafe {
        cbm_test_impact_publish(
            &request,
            diagnostic.as_mut_ptr(),
            diagnostic.len(),
        )
    };
    // SAFETY: The initialized buffer contains a native NUL-terminated diagnostic.
    let note = unsafe { CStr::from_ptr(diagnostic.as_ptr()) }.to_bytes();
    stderr.write(b"test-impact publish: status ");
    if note.is_empty() {
        stderr.line(status.to_string().as_bytes());
    } else {
        stderr.write(status.to_string().as_bytes());
        stderr.write(b": ");
        stderr.line(note);
    }
    c_int::from(status != OK)
}

fn select<'a>(
    args: impl Iterator<Item = &'a CStr>,
    stdout: Stream,
    stderr: Stream,
) -> c_int {
    let mut request = SelectRequest::default();
    let (mut seconds, mut vetoed_path) = (ptr::null(), ptr::null());
    if !parse(
        args,
        &mut [
            (b"--repo", &mut request.repo_root),
            (b"--base", &mut request.base_ref),
            (b"--config", &mut request.config_path),
            (b"--artifact", &mut request.artifact_dir),
            (b"--artifact-commit", &mut request.artifact_commit),
            (b"--platform", &mut request.platform),
            (b"--work", &mut request.work_parent),
            (b"--deadline-s", &mut seconds),
            (b"--vetoed-out", &mut vetoed_path),
        ],
        Some(&mut request.artifact_verified),
        stderr,
    ) || request.repo_root.is_null()
        || request.base_ref.is_null()
    {
        return usage(stderr);
    }
    let mut work = [0; 4096];
    let (Some(seconds), Some(parent)) = (
        positive_number(seconds, DEFAULT_DEADLINE_S),
        work_parent(request.work_parent, &mut work),
    ) else {
        return usage(stderr);
    };
    request.work_parent = parent;
    request.deadline_ms = deadline(seconds);

    let (mut result, mut vetoed) = (ptr::null_mut(), ptr::null_mut());
    if !vetoed_path.is_null() {
        request.vetoed_out = &mut vetoed;
    }
    let mut diagnostic = [0; 512];
    // SAFETY: The request, diagnostic and both result slots live until the
    // native job joins. Any returned result is released by NativeResult.
    let status = unsafe {
        cbm_test_impact_run(
            &request,
            &mut result,
            diagnostic.as_mut_ptr(),
            diagnostic.len(),
        )
    };
    let result = NonNull::new(result).map(NativeResult);
    let vetoed = NonNull::new(vetoed).map(NativeResult);
    // SAFETY: The initialized buffer contains a native NUL-terminated diagnostic.
    let note = unsafe { CStr::from_ptr(diagnostic.as_ptr()) }.to_bytes();
    if !note.is_empty() {
        stderr.write(b"test-impact select: ");
        stderr.line(note);
    }
    if status != OK {
        stderr.write(b"test-impact select: status ");
        stderr.line(status.to_string().as_bytes());
        return 1;
    }
    if !result.is_some_and(|result| result.write_json(stdout)) {
        return 1;
    }
    if let Some(vetoed) = vetoed {
        // SAFETY: This result slot was requested only with a live argv path.
        let Some(file) =
            NonNull::new(unsafe { cbm_fopen(vetoed_path, c"wb".as_ptr()) })
        else {
            return 1;
        };
        let written = vetoed.write_json(Stream(file));
        // SAFETY: This is the only close of the owned file, even on write error.
        let closed = unsafe { cbm_test_impact_cli_close(file.as_ptr()) };
        return c_int::from(!written || !closed);
    }
    0
}

/// Entered by the native dispatcher after initializing allocators and memory.
#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_test_impact_cli(
    argc: c_int,
    argv: *const *const c_char,
    stdout: *mut FILE,
    stderr: *mut FILE,
) -> c_int {
    let (Some(stdout), Some(stderr)) =
        (NonNull::new(stdout), NonNull::new(stderr))
    else {
        return 1;
    };
    // SAFETY: C supplies its live argv suffix, excluding the program and
    // test-impact command. Each slot points to a NUL-terminated C string.
    let argv = if argc > 0 {
        unsafe { slice::from_raw_parts(argv, argc as usize) }
    } else {
        &[]
    };
    let mut args = argv.iter().map(|&arg| unsafe { CStr::from_ptr(arg) });
    let stderr = Stream(stderr);
    match args.next().map(CStr::to_bytes) {
        Some(b"publish") => publish(args, stderr),
        Some(b"select") => select(args, Stream(stdout), stderr),
        _ => usage(stderr),
    }
}
