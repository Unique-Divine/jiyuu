//! Plans committed Git reads and projects exact capture bytes into the native
//! Git facts ABI. The C owner retains the arena, command budgets, and executor
//! so every published pointer has the original facts lifetime.
use std::cmp::Ordering;
use std::ffi::{CStr, c_char, c_int, c_uint};
use std::mem::{MaybeUninit, size_of};
use std::ptr::{self, null};
use std::slice;

const CHUNK: usize = 65_536;
const INVALID: c_int = 1;
const OOM: c_int = 2;
const COMMAND: c_int = 5;
const LIMIT: c_int = 8;
const UNSUPPORTED: c_int = 10;
const NO_MERGE_BASE: c_int = 11;
const AMBIGUOUS_MERGE_BASE: c_int = 12;
const IDENTITY_MISMATCH: c_int = 13;
const HEAD: c_int = 0;
const BASE: c_int = 1;
const MERGE_BASE: c_int = 2;
const BLOB: c_int = 1;
const COMMIT: c_int = 2;
type Oid = [u8; 65];
type Checked<T> = Result<T, ()>;

#[repr(C)]
struct Facts {
    _opaque: [u8; 0],
}

#[repr(C)]
struct Error {
    status: c_int,
    exit_code: c_int,
    diagnostic: [u8; 512],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Bytes {
    data: *const u8,
    length: usize,
}

impl Bytes {
    fn new(bytes: &[u8]) -> Self {
        Self {
            data: bytes.as_ptr(),
            length: bytes.len(),
        }
    }

    // The caller keeps the backing capture or FFI input live for the view.
    unsafe fn view<'a>(self) -> &'a [u8] {
        if self.length == 0 {
            &[]
        } else {
            unsafe { slice::from_raw_parts(self.data, self.length) }
        }
    }
}

#[repr(C)]
struct Capture {
    out: Bytes,
    _err: Bytes,
    exit_code: c_int,
}

#[repr(C)]
struct Identity {
    root: *const c_char,
    git_dir: *const c_char,
    common_dir: *const c_char,
    oid_hex_length: c_uint,
    head: Oid,
    base: Oid,
    merge_base: Oid,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Entry {
    path: *const u8,
    path_length: usize,
    mode: u32,
    object_type: c_int,
    oid: Oid,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Inventory {
    entries: *const Entry,
    count: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct InventoryCache {
    commit: *const c_char,
    inventory: Inventory,
}

// Shared only with test_impact_git_io.c. Facts stays opaque on the Rust side.
#[repr(C)]
struct Projection {
    identity: Identity,
    command_git_dir: *const c_char,
    diff_flags: [*const c_char; 32],
    diff_flag_count: c_int,
    graft_paths: [*const c_char; 2],
    shallow_paths: [*const c_char; 2],
    inventory_cache: [InventoryCache; 2],
}

#[repr(C)]
struct Blob {
    bytes: Bytes,
    oid: Oid,
    mode: u32,
}

#[repr(C)]
#[derive(Default)]
struct Diff {
    patch: Bytes,
    name_status: Bytes,
}

#[repr(C)]
struct BatchRequest {
    revision: c_int,
    indices: *const usize,
    count: usize,
}

#[repr(C)]
struct BatchLimits {
    max_entries: usize,
    max_input_bytes: usize,
    max_arena_bytes: usize,
}

#[repr(C)]
struct BatchBudget {
    arena_limit: usize,
    arena_used: usize,
    input_limit: usize,
    input_used: usize,
}

#[repr(C)]
struct BatchItem {
    inventory_index: usize,
    entry: *const Entry,
    bytes: Bytes,
}

#[repr(C)]
#[derive(Default)]
struct Batch {
    revision: c_int,
    commit: *const c_char,
    items: *const BatchItem,
    count: usize,
}

unsafe extern "C" {
    fn mori_git_io_projection(facts: *mut Facts) -> *mut Projection;
    fn mori_git_io_gate(facts: *mut Facts, error: *mut Error) -> bool;
    fn mori_git_io_limit(
        facts: *mut Facts,
        error: *mut Error,
        message: *const c_char,
    ) -> bool;
    fn mori_git_io_run(
        facts: *mut Facts,
        args: *const *const c_char,
        count: c_int,
        allow_one: bool,
        input: *const Bytes,
        out: *mut Capture,
        error: *mut Error,
    ) -> bool;
    fn mori_git_io_allocate(
        facts: *mut Facts,
        size: usize,
        error: *mut Error,
    ) -> *mut u8;
    fn mori_git_io_metadata_path(
        facts: *mut Facts,
        path: *const c_char,
        error: *mut Error,
    ) -> *const c_char;
    fn mori_git_io_history_file(path: *const c_char, error: *mut Error) -> bool;
    fn mori_git_io_output_budget(
        facts: *mut Facts,
        remaining: *mut usize,
    ) -> usize;
    fn mori_git_io_begin_batch(
        facts: *mut Facts,
        budget: *mut BatchBudget,
    ) -> bool;
    fn mori_git_io_end_batch(facts: *mut Facts, error: *mut Error);
    fn mori_git_io_msan_written(address: *const std::ffi::c_void, size: usize);
}

fn success(ok: bool) -> Checked<()> {
    if ok { Ok(()) } else { Err(()) }
}

// The native MSan build links an uninstrumented Rust archive. Mark only the
// initialized region before handing it to C.
unsafe fn mark_written<T>(address: *const T, count: usize) {
    if count != 0 {
        unsafe {
            mori_git_io_msan_written(address.cast(), size_of::<T>() * count)
        };
    }
}

unsafe fn write_output<T>(out: *mut T, value: T) {
    unsafe {
        ptr::write(out, value);
        mark_written(out, 1);
    }
}

fn oid_argument(oid: &Oid, width: usize) -> *const c_char {
    unsafe { mark_written(oid.as_ptr(), width + 1) };
    oid.as_ptr().cast()
}

// The native contract serializes calls on one owner and excludes callback
// reentry. Command captures and arena allocations remain live until free. No
// Rust reference into the mutable projection is held across an executor call.
struct Request {
    facts: *mut Facts,
    error: *mut Error,
    in_batch: bool,
}

impl Request {
    fn gate(&self) -> Checked<()> {
        success(
            self.facts.is_null()
                || unsafe { mori_git_io_gate(self.facts, self.error) },
        )
    }

    fn fail<T>(&self, status: c_int, message: &CStr) -> Checked<T> {
        unsafe {
            if status == LIMIT && !self.facts.is_null() {
                mori_git_io_limit(self.facts, self.error, message.as_ptr());
            } else {
                let error = &mut *self.error;
                error.status = status;
                error.diagnostic.fill(0);
                let message = message.to_bytes();
                error.diagnostic[..message.len()].copy_from_slice(message);
                mark_written(error.diagnostic.as_ptr(), error.diagnostic.len());
            }
        }
        Err(())
    }

    fn projection(&self) -> *mut Projection {
        unsafe { mori_git_io_projection(self.facts) }
    }

    fn width(&self) -> usize {
        unsafe { (*self.projection()).identity.oid_hex_length as usize }
    }

    fn revision(&self, revision: c_int) -> Option<*const c_char> {
        let id = unsafe { &raw const (*self.projection()).identity };
        let oid = unsafe {
            match revision {
                HEAD => &raw const (*id).head,
                BASE => &raw const (*id).base,
                MERGE_BASE => &raw const (*id).merge_base,
                _ => return None,
            }
        };
        Some(oid.cast())
    }

    fn run(
        &self,
        args: &[*const c_char],
        allow_one: bool,
        input: Option<Bytes>,
    ) -> Checked<Capture> {
        let mut capture = MaybeUninit::uninit();
        let input = input.as_ref().map_or(null(), ptr::from_ref);
        success(unsafe {
            mark_written(args.as_ptr(), args.len());
            if !input.is_null() {
                mark_written(input, 1);
            }
            mori_git_io_run(
                self.facts,
                args.as_ptr(),
                args.len() as c_int,
                allow_one,
                input,
                capture.as_mut_ptr(),
                self.error,
            )
        })?;
        Ok(unsafe { capture.assume_init() })
    }

    fn allocate<T>(
        &self,
        count: usize,
        oom: &CStr,
        overflow: &CStr,
    ) -> Checked<*mut T> {
        let Some(size) = count.checked_mul(size_of::<T>()) else {
            return self.fail(LIMIT, overflow);
        };
        let buffer: *mut T =
            unsafe { mori_git_io_allocate(self.facts, size, self.error) }.cast();
        if !buffer.is_null() {
            Ok(buffer)
        } else if unsafe { (*self.error).status } == OOM {
            self.fail(OOM, oom)
        } else {
            Err(())
        }
    }

    fn retain(
        &self,
        bytes: &[u8],
        suffix: &[u8],
        oom: &CStr,
    ) -> Checked<*const c_char> {
        let Some(size) = bytes
            .len()
            .checked_add(suffix.len())
            .and_then(|n| n.checked_add(1))
        else {
            return self.fail(LIMIT, c"Git metadata allocation overflow");
        };
        let copy =
            self.allocate::<u8>(size, oom, c"Git metadata allocation overflow")?;
        for (i, chunk) in bytes.chunks(CHUNK).enumerate() {
            if self.in_batch {
                self.gate()?;
            }
            unsafe {
                ptr::copy_nonoverlapping(
                    chunk.as_ptr(),
                    copy.add(i * CHUNK),
                    chunk.len(),
                )
            };
        }
        unsafe {
            ptr::copy_nonoverlapping(
                suffix.as_ptr(),
                copy.add(bytes.len()),
                suffix.len(),
            );
            copy.add(size - 1).write(0);
        }
        if self.in_batch {
            self.gate()?;
        }
        unsafe { mark_written(copy, size) };
        Ok(copy.cast())
    }

    fn output_budget(&self) -> (usize, usize) {
        let mut remaining = 0;
        let stdout =
            unsafe { mori_git_io_output_budget(self.facts, &mut remaining) };
        (stdout, remaining)
    }

    fn metadata(&self, option: &CStr, absolute: bool) -> Checked<*const c_char> {
        let result = self.run(
            &[
                c"rev-parse".as_ptr(),
                c"--path-format=absolute".as_ptr(),
                option.as_ptr(),
            ],
            false,
            None,
        )?;
        let bytes = unsafe { result.out.view() };
        let Some(text) = line_body(bytes) else {
            return self.fail(COMMAND, c"Malformed Git metadata line");
        };
        for chunk in text.chunks(CHUNK) {
            if self.in_batch {
                self.gate()?;
            }
            if chunk.iter().any(|c| matches!(c, 0 | b'\n' | b'\r')) {
                return self.fail(COMMAND, c"Malformed Git metadata line");
            }
        }
        if self.in_batch {
            self.gate()?;
        }
        let path = self.retain(text, b"", c"Cannot retain Git metadata")?;
        if absolute {
            let path = unsafe {
                mori_git_io_metadata_path(self.facts, path, self.error)
            };
            return if path.is_null() { Err(()) } else { Ok(path) };
        }
        Ok(path)
    }

    fn metadata_equal(
        &self,
        a: *const c_char,
        b: *const c_char,
    ) -> Checked<bool> {
        if !self.in_batch {
            return Ok(unsafe { CStr::from_ptr(a) == CStr::from_ptr(b) });
        }
        for at in 0usize.. {
            if at % CHUNK == 0 {
                self.gate()?;
            }
            let (x, y) = unsafe { (*a.add(at), *b.add(at)) };
            if x != y || x == 0 {
                return Ok(x == y);
            }
        }
        unreachable!()
    }

    fn topology_guard(&self) -> Checked<()> {
        self.gate()?;
        let p = self.projection();
        let (dir, expected) =
            unsafe { ((*p).command_git_dir, (*p).identity.common_dir) };
        if dir.is_null() || expected.is_null() {
            return self
                .fail(INVALID, c"Git directory identity is not established");
        }
        let actual = self.metadata(c"--git-common-dir", true)?;
        if !self.metadata_equal(actual, expected)? {
            return self.fail(
                IDENTITY_MISMATCH,
                c"Git common directory identity changed",
            );
        }
        self.gate()
    }

    fn history_guard(&self) -> Checked<()> {
        self.topology_guard()?;
        let (grafts, shallow) = unsafe {
            (
                (*self.projection()).graft_paths,
                (*self.projection()).shallow_paths,
            )
        };
        for i in 0..2 {
            if i == 1 && self.metadata_equal(grafts[0], grafts[1])? {
                continue;
            }
            for path in [grafts[i], shallow[i]] {
                success(unsafe { mori_git_io_history_file(path, self.error) })?;
            }
        }
        self.gate()
    }

    fn resolve(&self, reference: &CStr) -> Checked<Oid> {
        let text = reference.to_bytes();
        if text.is_empty() || text.len() > CHUNK || text[0] == b'-' {
            return self
                .fail(INVALID, c"A bounded commit reference is required");
        }
        let peel =
            self.retain(text, b"^{commit}", c"Cannot retain commit reference")?;
        let result = self.run(
            &[
                c"rev-parse".as_ptr(),
                c"--verify".as_ptr(),
                c"--end-of-options".as_ptr(),
                peel,
            ],
            false,
            None,
        )?;
        self.oid_line(unsafe { result.out.view() })
    }

    fn oid_line(&self, bytes: &[u8]) -> Checked<Oid> {
        let Some(oid) = line(bytes).and_then(|text| oid(text, self.width()))
        else {
            return self
                .fail(COMMAND, c"Git returned an invalid full object ID");
        };
        Ok(oid)
    }

    fn open_identity(
        &self,
        base_ref: &CStr,
        expected_head: Option<&CStr>,
    ) -> Checked<()> {
        let p = self.projection();
        let prefix = self.metadata(c"--show-prefix", false)?;
        if unsafe { *prefix } != 0 {
            return self.fail(INVALID, c"Git root must be a worktree top-level");
        }
        let root = self.metadata(c"--show-toplevel", true)?;
        unsafe { write_output(&raw mut (*p).identity.root, root) };
        let git_dir = self.metadata(c"--absolute-git-dir", true)?;
        unsafe {
            write_output(&raw mut (*p).identity.git_dir, git_dir);
            write_output(&raw mut (*p).command_git_dir, git_dir);
        }
        let common_dir = self.metadata(c"--git-common-dir", true)?;
        unsafe { write_output(&raw mut (*p).identity.common_dir, common_dir) };
        for (i, dir) in [git_dir, common_dir].into_iter().enumerate() {
            let dir = unsafe { CStr::from_ptr(dir) }.to_bytes();
            unsafe {
                (*p).graft_paths[i] = self.retain(
                    dir,
                    b"/info/grafts",
                    c"Cannot retain Git history guard paths",
                )?
            };
        }
        for (i, dir) in [git_dir, common_dir].into_iter().enumerate() {
            let dir = unsafe { CStr::from_ptr(dir) }.to_bytes();
            unsafe {
                (*p).shallow_paths[i] = self.retain(
                    dir,
                    b"/shallow",
                    c"Cannot retain Git history guard paths",
                )?
            };
        }
        self.history_guard()?;
        let format = self.metadata(c"--show-object-format=storage", false)?;
        let width = match unsafe { CStr::from_ptr(format) }.to_bytes() {
            b"sha1" => 40,
            b"sha256" => 64,
            _ => {
                return self.fail(UNSUPPORTED, c"Unsupported Git object format");
            }
        };
        unsafe { write_output(&raw mut (*p).identity.oid_hex_length, width) };
        let shallow = self.metadata(c"--is-shallow-repository", false)?;
        if unsafe { CStr::from_ptr(shallow) } != c"false" {
            return self
                .fail(UNSUPPORTED, c"Complete local Git history is required");
        }
        let head = self.resolve(c"HEAD")?;
        let base = self.resolve(base_ref)?;
        unsafe {
            write_output(&raw mut (*p).identity.head, head);
            write_output(&raw mut (*p).identity.base, base);
        }
        self.history_guard()?;
        if let Some(expected) = expected_head {
            let Some(expected) = oid(expected.to_bytes(), width as usize) else {
                return self
                    .fail(INVALID, c"Expected HEAD must be a full object ID");
            };
            if expected != head {
                return self.fail(
                    IDENTITY_MISMATCH,
                    c"HEAD does not match expected identity",
                );
            }
        }
        unsafe { write_output(&raw mut (*p).command_git_dir, common_dir) };
        self.history_guard()?;
        let result = self.run(
            &[
                c"merge-base".as_ptr(),
                c"--all".as_ptr(),
                oid_argument(&base, width as usize),
                oid_argument(&head, width as usize),
            ],
            true,
            None,
        )?;
        self.history_guard()?;
        let bytes = unsafe { result.out.view() };
        if result.exit_code == 1 && bytes.is_empty() {
            return self.fail(NO_MERGE_BASE, c"No local common ancestor exists");
        }
        if result.exit_code != 0 || bytes.is_empty() {
            return self.fail(COMMAND, c"Malformed common ancestor result");
        }
        // Parse all candidates before rejecting ambiguity. An invalid later
        // record is a command error even if the first two candidates are valid.
        let mut first = None;
        let mut ambiguous = false;
        for base in bytes.split_inclusive(|&b| b == b'\n') {
            if !base.ends_with(b"\n") {
                return self.fail(COMMAND, c"Malformed common ancestor list");
            }
            let base = self.oid_line(base)?;
            if first.is_some() {
                ambiguous = true;
            } else {
                first = Some(base);
            }
        }
        if ambiguous {
            return self.fail(
                AMBIGUOUS_MERGE_BASE,
                c"Multiple common ancestors are unsupported",
            );
        }
        unsafe {
            write_output(&raw mut (*p).identity.merge_base, first.unwrap())
        };
        self.gate()
    }
}

fn oid(text: &[u8], width: usize) -> Option<Oid> {
    if !matches!(width, 40 | 64)
        || text.len() != width
        || !text.iter().all(u8::is_ascii_hexdigit)
    {
        return None;
    }
    let mut out = [0; 65];
    for (dest, &byte) in out.iter_mut().zip(text) {
        *dest = byte.to_ascii_lowercase();
    }
    Some(out)
}

fn line_body(bytes: &[u8]) -> Option<&[u8]> {
    let text = bytes.strip_suffix(b"\n")?;
    #[cfg(windows)]
    let text = text.strip_suffix(b"\r").unwrap_or(text);
    Some(text)
}

fn line(bytes: &[u8]) -> Option<&[u8]> {
    line_body(bytes)
        .filter(|text| !text.iter().any(|c| matches!(c, 0 | b'\n' | b'\r')))
}

fn mode(text: &[u8]) -> Option<u32> {
    text.iter().try_fold(0, |value, &c| match c {
        b'0'..=b'7' => Some(value * 8 + u32::from(c - b'0')),
        _ => None,
    })
}

fn decimal(text: &[u8]) -> Option<usize> {
    if text.is_empty() {
        return None;
    }
    text.iter().try_fold(0usize, |value, &c| match c {
        b'0'..=b'9' => value.checked_mul(10)?.checked_add(usize::from(c - b'0')),
        _ => None,
    })
}

fn with_request<T>(
    facts: *mut Facts,
    error: *mut Error,
    operation: impl FnOnce(&Request) -> T,
) -> T {
    let mut local = MaybeUninit::uninit();
    let error = if error.is_null() {
        local.as_mut_ptr()
    } else {
        error
    };
    let cleared = Error {
        status: 0,
        exit_code: -1,
        diagnostic: [0; 512],
    };
    unsafe { write_output(error, cleared) };
    operation(&Request {
        facts,
        error,
        in_batch: false,
    })
}

#[unsafe(no_mangle)]
extern "C" fn mori_git_valid_oid(text: *const u8, length: usize) -> bool {
    matches!(length, 40 | 64)
        && !text.is_null()
        && oid(unsafe { slice::from_raw_parts(text, length) }, length).is_some()
}

#[unsafe(no_mangle)]
extern "C" fn mori_git_open_identity(
    facts: *mut Facts,
    base_ref: *const c_char,
    expected: *const c_char,
    error: *mut Error,
) -> bool {
    with_request(facts, error, |request| {
        let base_ref = unsafe { CStr::from_ptr(base_ref) };
        let expected =
            (!expected.is_null()).then(|| unsafe { CStr::from_ptr(expected) });
        request.open_identity(base_ref, expected).is_ok()
    })
}

fn inventory_revision(revision: c_int) -> bool {
    matches!(revision, HEAD | MERGE_BASE)
}

// Legacy names and single-blob lookup use argv-compatible paths. Inventory
// and artifact comparisons retain raw tree paths, including names Windows
// cannot pass as arguments.
fn literal_path(path: &[u8]) -> bool {
    if path.len() > CHUNK || path.contains(&0) {
        return false;
    }
    #[cfg(windows)]
    if path.contains(&b'\\')
        || path.get(1) == Some(&b':')
        || std::str::from_utf8(path).is_err()
    {
        return false;
    }
    path.split(|&c| c == b'/').all(valid_component)
}

fn valid_component(component: &[u8]) -> bool {
    !matches!(component, b"" | b"." | b"..")
}

fn valid_status(status: u8) -> bool {
    matches!(status, b'A' | b'M' | b'D' | b'T')
}

impl Request {
    fn find_nul(&self, bytes: &[u8]) -> Checked<Option<usize>> {
        for (i, chunk) in bytes.chunks(CHUNK).enumerate() {
            self.gate()?;
            if let Some(offset) = chunk.iter().position(|&b| b == 0) {
                return Ok(Some(i * CHUNK + offset));
            }
        }
        Ok(None)
    }

    fn raw_path(&self, path: &[u8]) -> Checked<()> {
        if path.is_empty() || path[0] == b'/' {
            return self.fail(COMMAND, c"Malformed tracked-tree path");
        }
        let mut start = 0;
        for i in 0..=path.len() {
            if i % CHUNK == 0 {
                self.gate()?;
            }
            if i == path.len() || path[i] == b'/' {
                if !valid_component(&path[start..i]) {
                    return self.fail(COMMAND, c"Malformed tracked-tree path");
                }
                start = i + 1;
            }
        }
        Ok(())
    }

    fn inventory_record(&self, bytes: &[u8]) -> Checked<(Entry, usize)> {
        let Some(end) = self.find_nul(bytes)? else {
            return self.fail(COMMAND, c"Truncated tracked-tree record");
        };
        let data = &bytes[..end];
        if data.len() < 8 || data[6] != b' ' {
            return self.fail(COMMAND, c"Malformed tracked-tree header");
        }
        let Some(mode) = mode(&data[..6]) else {
            return self.fail(COMMAND, c"Malformed tracked-tree mode");
        };
        let (object_type, expected): (_, &[u8]) = match mode {
            0o100644 | 0o100755 | 0o120000 => (BLOB, b"blob "),
            0o160000 => (COMMIT, b"commit "),
            _ => return self.fail(COMMAND, c"Invalid tracked-tree mode/type"),
        };
        let identity = data[7..].strip_prefix(expected);
        let width = self.width();
        let Some((oid, path)) = identity.and_then(|data| {
            let (text, path) = data.split_at_checked(width)?;
            let path =
                path.strip_prefix(b"\t").filter(|path| !path.is_empty())?;
            Some((oid(text, width)?, path))
        }) else {
            return self.fail(COMMAND, c"Malformed tracked-tree identity/type");
        };
        self.raw_path(path)?;
        let entry = Entry {
            path: path.as_ptr(),
            path_length: path.len(),
            mode,
            object_type,
            oid,
        };
        Ok((entry, end + 1))
    }

    fn inventory(&self, revision: c_int) -> Checked<Inventory> {
        self.topology_guard()?;
        let commit = self.revision(revision).unwrap();
        let result = self.run(
            &[
                c"ls-tree".as_ptr(),
                c"-r".as_ptr(),
                c"-z".as_ptr(),
                c"--full-tree".as_ptr(),
                commit,
            ],
            false,
            None,
        )?;
        let capture = unsafe { result.out.view() };
        let mut bytes = capture;
        let mut count = 0;
        // Validate every record before sizing the arena allocation.
        while !bytes.is_empty() {
            let (_, consumed) = self.inventory_record(bytes)?;
            if count == usize::MAX / size_of::<Entry>() {
                return self.fail(
                    LIMIT,
                    c"Tracked-tree inventory allocation limit exceeded",
                );
            }
            count += 1;
            bytes = &bytes[consumed..];
        }
        self.gate()?;
        let mut entries = ptr::null_mut();
        if count != 0 {
            entries = self.allocate::<Entry>(
                count,
                c"Cannot retain tracked-tree inventory",
                c"Tracked-tree inventory allocation limit exceeded",
            )?;
            bytes = capture;
            for i in 0..count {
                let (entry, consumed) = self.inventory_record(bytes)?;
                unsafe { entries.add(i).write(entry) };
                bytes = &bytes[consumed..];
            }
            let rows = unsafe { slice::from_raw_parts_mut(entries, count) };
            self.sort(rows, || Ok(()), |a, b| self.compare_paths(a, b))?;
            for pair in rows.windows(2) {
                if self.compare_paths(&pair[0], &pair[1])? != Ordering::Less {
                    return self.fail(COMMAND, c"Duplicate tracked-tree path");
                }
            }
            self.gate()?;
            unsafe { mark_written(entries, count) };
        }
        self.topology_guard()?;
        Ok(Inventory { entries, count })
    }

    fn compare_paths(&self, a: &Entry, b: &Entry) -> Checked<Ordering> {
        let a = unsafe { slice::from_raw_parts(a.path, a.path_length) };
        let b = unsafe { slice::from_raw_parts(b.path, b.path_length) };
        let common = a.len().min(b.len());
        for (x, y) in a[..common].chunks(CHUNK).zip(b[..common].chunks(CHUNK)) {
            self.gate()?;
            let order = x.cmp(y);
            if order != Ordering::Equal {
                return Ok(order);
            }
        }
        Ok(a.len().cmp(&b.len()))
    }

    // Heapsort lets each comparison stop on cancellation. It needs no scratch
    // allocation outside the query's arena budget.
    fn sort<T>(
        &self,
        rows: &mut [T],
        check_sift: impl Fn() -> Checked<()>,
        compare: impl Fn(&T, &T) -> Checked<Ordering>,
    ) -> Checked<()> {
        let sift = |rows: &mut [T], mut root| -> Checked<()> {
            while root < rows.len() / 2 {
                check_sift()?;
                let mut child = root * 2 + 1;
                if child + 1 < rows.len()
                    && compare(&rows[child], &rows[child + 1])?.is_lt()
                {
                    child += 1;
                }
                if !compare(&rows[root], &rows[child])?.is_lt() {
                    break;
                }
                rows.swap(root, child);
                root = child;
            }
            Ok(())
        };
        for root in (0..rows.len() / 2).rev() {
            sift(rows, root)?;
        }
        for end in (1..rows.len()).rev() {
            self.gate()?;
            rows.swap(0, end);
            sift(&mut rows[..end], 0)?;
        }
        Ok(())
    }

    fn blob(&self, commit: *const c_char, path: Bytes) -> Checked<Option<Blob>> {
        self.topology_guard()?;
        let owned_path = self.retain(
            unsafe { path.view() },
            b"",
            c"Cannot retain literal Git path",
        )?;
        let result = self.run(
            &[
                c"ls-tree".as_ptr(),
                c"-z".as_ptr(),
                c"--full-tree".as_ptr(),
                commit,
                c"--".as_ptr(),
                owned_path,
            ],
            false,
            None,
        )?;
        let data = unsafe { result.out.view() };
        if data.is_empty() {
            self.topology_guard()?;
            return Ok(None);
        }
        let exact = data
            .strip_suffix(b"\0")
            .filter(|record| !record.contains(&0))
            .and_then(|record| {
                let tab = record.iter().position(|&b| b == b'\t')?;
                (tab >= 8
                    && record[6] == b' '
                    && &record[tab + 1..] == unsafe { path.view() })
                .then_some(&record[..tab])
            });
        let Some(header) = exact else {
            return self.fail(
                COMMAND,
                c"Git tree lookup did not return one exact literal path",
            );
        };
        let Some(mode) = mode(&header[..6]) else {
            return self.fail(COMMAND, c"Malformed Git tree mode");
        };
        let Some(type_end) =
            header[7..].iter().position(|&c| c == b' ').map(|at| at + 7)
        else {
            return self.fail(COMMAND, c"Malformed Git tree object type");
        };
        let width = self.width();
        let Some(oid) = oid(&header[type_end + 1..], width) else {
            return self.fail(COMMAND, c"Invalid Git blob identity");
        };
        if &header[7..type_end] != b"blob" || !regular_mode(mode) {
            return self.fail(
                UNSUPPORTED,
                c"Only regular committed source blobs are supported",
            );
        }
        let result = self.run(
            &[
                c"cat-file".as_ptr(),
                c"-s".as_ptr(),
                oid_argument(&oid, width),
            ],
            false,
            None,
        )?;
        let Some(expected) =
            line(unsafe { result.out.view() }).and_then(decimal)
        else {
            return self.fail(COMMAND, c"Malformed Git blob size");
        };
        let (stdout_limit, remaining) = self.output_budget();
        if expected > stdout_limit || expected > remaining {
            return self.fail(
                LIMIT,
                c"Committed blob exceeds the remaining output budget",
            );
        }
        let result = self.run(
            &[
                c"cat-file".as_ptr(),
                c"blob".as_ptr(),
                oid_argument(&oid, width),
            ],
            false,
            None,
        )?;
        if result.out.length != expected {
            return self.fail(
                COMMAND,
                c"Git blob bytes disagree with the immutable object size",
            );
        }
        self.topology_guard()?;
        Ok(Some(Blob {
            bytes: result.out,
            oid,
            mode,
        }))
    }

    fn names(&self, base: *const c_char) -> Checked<Bytes> {
        let head = self.revision(HEAD).unwrap();
        let args = [
            c"diff".as_ptr(),
            c"--name-status".as_ptr(),
            c"-z".as_ptr(),
            c"--no-renames".as_ptr(),
            c"--no-ext-diff".as_ptr(),
            c"--no-textconv".as_ptr(),
            c"--no-color".as_ptr(),
            c"--no-relative".as_ptr(),
            c"--ignore-submodules=none".as_ptr(),
            c"--no-exit-code".as_ptr(),
            base,
            head,
            c"--".as_ptr(),
        ];
        Ok(self.run(&args, false, None)?.out)
    }

    fn diff(&self) -> Checked<Diff> {
        self.topology_guard()?;
        let base = self.revision(MERGE_BASE).unwrap();
        let names = self.names(base)?;
        let bytes = unsafe { names.view() };
        if !legacy_names_valid(bytes) {
            return self.fail(
                COMMAND,
                c"Malformed or unsupported changed-path metadata",
            );
        }
        let head = self.revision(HEAD).unwrap();
        let tail = [
            c"--patch".as_ptr(),
            c"--no-relative".as_ptr(),
            c"--ignore-submodules=none".as_ptr(),
            c"--submodule=short".as_ptr(),
            c"--no-exit-code".as_ptr(),
            c"--no-indent-heuristic".as_ptr(),
            c"--output-indicator-new=+".as_ptr(),
            c"--output-indicator-old=-".as_ptr(),
            c"--output-indicator-context= ".as_ptr(),
            base,
            head,
            c"--".as_ptr(),
        ];
        let mut args = [null(); 1 + 32 + 12];
        args[0] = c"diff".as_ptr();
        let p = self.projection();
        let flags =
            unsafe { &(&(*p).diff_flags)[..(*p).diff_flag_count as usize] };
        let count = 1 + flags.len() + tail.len();
        args[1..1 + flags.len()].copy_from_slice(flags);
        args[1 + flags.len()..count].copy_from_slice(&tail);
        let patch = self.run(&args[..count], false, None)?.out;
        if (names.length == 0) != (patch.length == 0) {
            return self
                .fail(COMMAND, c"Changed-path metadata and patch disagree");
        }
        self.topology_guard()?;
        Ok(Diff {
            patch,
            name_status: names,
        })
    }

    fn ancestry(
        &self,
        ancestor: &Oid,
        descendant: *const c_char,
    ) -> Checked<Capture> {
        self.run(
            &[
                c"merge-base".as_ptr(),
                c"--is-ancestor".as_ptr(),
                oid_argument(ancestor, self.width()),
                descendant,
            ],
            true,
            None,
        )
    }

    fn ancestor_names(&self, mut bytes: &[u8]) -> Checked<()> {
        while !bytes.is_empty() {
            self.gate()?;
            if bytes.len() < 3 || bytes[1] != 0 {
                return self
                    .fail(COMMAND, c"Malformed ancestor changed-path record");
            }
            if !valid_status(bytes[0]) {
                return self
                    .fail(COMMAND, c"Unsupported ancestor changed-path status");
            }
            bytes = &bytes[2..];
            let Some(end) = self.find_nul(bytes)? else {
                return self
                    .fail(COMMAND, c"Truncated ancestor changed-path record");
            };
            self.gate()?;
            self.raw_path(&bytes[..end])?;
            bytes = &bytes[end + 1..];
        }
        self.gate()
    }

    fn ancestor_changes(&self, ancestor: &Oid) -> Checked<Option<Bytes>> {
        self.history_guard()?;
        let merge_base = self.revision(MERGE_BASE).unwrap();
        let same_commit =
            unsafe { (&(*self.projection()).identity.merge_base) == ancestor };
        if !same_commit {
            self.require_commit(ancestor)?;
            let result = self.ancestry(ancestor, merge_base)?;
            if result.out.length != 0 {
                return self
                    .fail(COMMAND, c"Unexpected artifact ancestry stdout");
            }
            if result.exit_code == 1 {
                self.history_guard()?;
                self.gate()?;
                return Ok(None);
            }
        }
        let names = self.names(oid_argument(ancestor, self.width()))?;
        if names.length != 0 && names.data.is_null() {
            return self.fail(COMMAND, c"Missing ancestor changed-path capture");
        }
        self.ancestor_names(unsafe { names.view() })?;
        self.history_guard()?;
        self.gate()?;
        Ok(Some(names))
    }

    fn require_commit(&self, oid: &Oid) -> Checked<()> {
        let result = self.run(
            &[
                c"cat-file".as_ptr(),
                c"-t".as_ptr(),
                oid_argument(oid, self.width()),
            ],
            false,
            None,
        )?;
        if result.out.length > 8 {
            return self
                .fail(COMMAND, c"Malformed artifact object-type response");
        }
        match line(unsafe { result.out.view() }) {
            Some(b"commit") => Ok(()),
            Some(b"tree" | b"blob" | b"tag") => self.fail(
                INVALID,
                c"Artifact object ID must identify a commit itself",
            ),
            Some(_) => {
                self.fail(COMMAND, c"Unknown artifact object-type response")
            }
            None => {
                self.fail(COMMAND, c"Malformed artifact object-type response")
            }
        }
    }
}

fn regular_mode(mode: u32) -> bool {
    matches!(mode, 0o100644 | 0o100755)
}

fn legacy_names_valid(mut bytes: &[u8]) -> bool {
    while !bytes.is_empty() {
        if bytes.len() < 3 || bytes[1] != 0 || !valid_status(bytes[0]) {
            return false;
        }
        bytes = &bytes[2..];
        let Some(end) = bytes.iter().position(|&b| b == 0) else {
            return false;
        };
        if !literal_path(&bytes[..end]) {
            return false;
        }
        bytes = &bytes[end + 1..];
    }
    true
}

#[unsafe(no_mangle)]
extern "C" fn cbm_git_facts_inventory(
    facts: *mut Facts,
    revision: c_int,
    out: *mut Inventory,
    error: *mut Error,
) -> bool {
    with_request(facts, error, |request| {
        clear_output(out);
        if facts.is_null() || out.is_null() || !inventory_revision(revision) {
            return request
                .fail::<()>(
                    INVALID,
                    c"Inventory requires pinned HEAD or actual merge base",
                )
                .is_ok();
        }
        let Ok(inventory) = request.inventory(revision) else {
            return false;
        };
        unsafe { write_output(out, inventory) };
        true
    })
}

// The ABI records passed to this helper have only scalar/pointer fields, for
// which all-zero is valid. Clear all bytes before validating other arguments.
fn clear_output<T>(out: *mut T) {
    if !out.is_null() {
        unsafe {
            out.write_bytes(0, 1);
            mark_written(out, 1);
        }
    }
}

#[unsafe(no_mangle)]
extern "C" fn cbm_git_facts_read_blob(
    facts: *mut Facts,
    revision: c_int,
    path: *const u8,
    path_length: usize,
    out: *mut Blob,
    error: *mut Error,
) -> c_int {
    with_request(facts, error, |request| {
        clear_output(out);
        let result = (|| {
            if !facts.is_null()
                && !out.is_null()
                && !path.is_null()
                && (1..=CHUNK).contains(&path_length)
            {
                let path = Bytes {
                    data: path,
                    length: path_length,
                };
                if let Some(commit) = request
                    .revision(revision)
                    .filter(|_| literal_path(unsafe { path.view() }))
                {
                    return request.blob(commit, path);
                }
            }
            request.fail(INVALID, c"Blob lookup requires a pinned revision and literal relative path")
        })();
        match result {
            Ok(Some(blob)) => {
                unsafe { write_output(out, blob) };
                1
            }
            Ok(None) => 0,
            Err(()) => -1,
        }
    })
}

#[unsafe(no_mangle)]
extern "C" fn cbm_git_facts_diff(
    facts: *mut Facts,
    out: *mut Diff,
    error: *mut Error,
) -> bool {
    with_request(facts, error, |request| {
        clear_output(out);
        if facts.is_null() || out.is_null() {
            return request
                .fail::<()>(
                    INVALID,
                    c"A Git snapshot and diff output are required",
                )
                .is_ok();
        }
        let Ok(diff) = request.diff() else {
            return false;
        };
        unsafe { write_output(out, diff) };
        true
    })
}

#[unsafe(no_mangle)]
extern "C" fn cbm_git_facts_is_ancestor(
    facts: *mut Facts,
    ancestor: *const c_char,
    descendant: *const c_char,
    error: *mut Error,
) -> c_int {
    with_request(facts, error, |request| {
        let check = || {
            let parse = |id: *const c_char| {
                if facts.is_null() || id.is_null() {
                    return None;
                }
                // Read at most the required OID plus its NUL, even for a huge rejected string.
                let width = request.width();
                let mut text = [0; 64];
                for (i, byte) in text[..width].iter_mut().enumerate() {
                    *byte = unsafe { *id.cast::<u8>().add(i) };
                    if *byte == 0 {
                        return None;
                    }
                }
                if unsafe { *id.add(width) } != 0 {
                    return None;
                }
                oid(&text[..width], width)
            };
            let Some((ancestor, descendant)) = parse(ancestor)
                .and_then(|ancestor| Some((ancestor, parse(descendant)?)))
            else {
                return request
                    .fail(INVALID, c"Ancestry requires two full object IDs");
            };
            request.history_guard()?;
            let result = request.ancestry(
                &ancestor,
                oid_argument(&descendant, request.width()),
            )?;
            request.history_guard()?;
            if result.out.length != 0 {
                return request.fail(COMMAND, c"Unexpected ancestry stdout");
            }
            Ok(c_int::from(result.exit_code == 0))
        };
        check().unwrap_or(-1)
    })
}

#[unsafe(no_mangle)]
extern "C" fn cbm_git_facts_ancestor_changes(
    facts: *mut Facts,
    artifact: *const u8,
    length: usize,
    out: *mut Bytes,
    error: *mut Error,
) -> c_int {
    with_request(facts, error, |request| {
        clear_output(out);
        let check = || {
            if !facts.is_null()
                && !out.is_null()
                && !artifact.is_null()
                && length == request.width()
                && let Some(ancestor) = oid(
                    unsafe { slice::from_raw_parts(artifact, length) },
                    length,
                )
            {
                return request.ancestor_changes(&ancestor);
            }
            request.fail(INVALID, c"Ancestor changes require a snapshot, full artifact OID and output")
        };
        match check() {
            Ok(Some(names)) => {
                unsafe { write_output(out, names) };
                1
            }
            Ok(None) => 0,
            Err(()) => -1,
        }
    })
}

#[repr(C)]
struct Pick {
    entry: *const Entry,
    inventory_index: usize,
    position: usize,
    object_index: usize,
}

#[repr(C)]
struct Object {
    oid: *const u8,
    size: usize,
    frame_length: usize,
    bytes: Bytes,
}

struct BatchPlan<'a> {
    objects: &'a mut [Object],
    lines: &'a [u8],
    line_length: usize,
}

impl Request {
    fn batch_array<T>(&self, count: usize) -> Checked<*mut T> {
        self.allocate(
            count,
            c"Cannot retain Git facts data",
            c"Git batch array size overflow",
        )
    }

    fn cached_inventory(
        &self,
        revision: c_int,
        commit: *const c_char,
    ) -> Checked<Inventory> {
        let state = self.projection();
        let cache = unsafe { (*state).inventory_cache };
        let commit_name = unsafe { CStr::from_ptr(commit) };
        for slot in cache {
            if !slot.commit.is_null()
                && unsafe { CStr::from_ptr(slot.commit) } == commit_name
            {
                self.gate()?;
                return Ok(slot.inventory);
            }
        }
        let inventory = self.inventory(revision)?;
        let Some(index) = cache.iter().position(|slot| slot.commit.is_null())
        else {
            return self
                .fail(INVALID, c"Invalid internal pinned inventory cache");
        };
        unsafe {
            (*state).inventory_cache[index] =
                InventoryCache { commit, inventory }
        };
        Ok(inventory)
    }

    fn batch_header(
        &self,
        capture: &[u8],
        oid: &[u8],
    ) -> Checked<(usize, usize)> {
        self.gate()?;
        let start = oid.len() + 6;
        if capture.len() < start + 2
            || !capture.starts_with(oid)
            || &capture[oid.len()..start] != b" blob "
        {
            return self.fail(COMMAND, c"Malformed Git batch identity/type");
        }
        let mut value = Some(0usize);
        for (i, &byte) in capture[start..].iter().enumerate() {
            if byte == b'\n' {
                if i == 0 {
                    break;
                }
                let length = start + i + 1;
                if let Some(size) =
                    value.filter(|&size| size < usize::MAX - length)
                {
                    self.gate()?;
                    return Ok((size, length));
                }
                return self
                    .fail(LIMIT, c"Git batch object frame size overflow");
            }
            if i % CHUNK == 0 {
                self.gate()?;
            }
            if !byte.is_ascii_digit() || (i > 0 && capture[start] == b'0') {
                return self.fail(COMMAND, c"Malformed Git batch object size");
            }
            // Continue validating digits after overflow.
            value = value.and_then(|n| {
                n.checked_mul(10)?.checked_add(usize::from(byte - b'0'))
            });
        }
        self.fail(COMMAND, c"Truncated Git batch header")
    }

    fn batch_frame(
        &self,
        capture: &mut &[u8],
        oid: &[u8],
        expected: Option<usize>,
        has_payload: bool,
    ) -> Checked<(usize, usize, Bytes)> {
        self.gate()?;
        let (size, length) = self.batch_header(capture, oid)?;
        if expected.is_some_and(|expected| expected != size) {
            return self
                .fail(COMMAND, c"Git batch size changed after preflight");
        }
        *capture = &capture[length..];
        let mut bytes = Bytes::default();
        if has_payload {
            let Some((payload, rest)) = capture
                .split_at_checked(size)
                .filter(|(_, rest)| !rest.is_empty())
            else {
                return self.fail(COMMAND, c"Truncated Git batch payload");
            };
            for _ in payload.chunks(CHUNK) {
                self.gate()?;
            }
            self.gate()?;
            let Some(rest) = rest.strip_prefix(b"\n") else {
                return self
                    .fail(COMMAND, c"Malformed Git batch payload separator");
            };
            bytes = Bytes::new(payload);
            *capture = rest;
        }
        Ok((size, length + size + 1, bytes))
    }

    fn batch_eof(&self, capture: &[u8]) -> Checked<()> {
        if !capture.is_empty() {
            return self
                .fail(COMMAND, c"Unexpected bytes after Git batch frames");
        }
        self.gate()
    }

    fn batch_command(
        &self,
        plan: &mut BatchPlan<'_>,
        first: usize,
        count: usize,
        payload: bool,
    ) -> Checked<()> {
        let flag = if payload {
            c"--batch=%(objectname) %(objecttype) %(objectsize)"
        } else {
            c"--batch-check=%(objectname) %(objecttype) %(objectsize)"
        };
        let lines = &plan.lines
            [first * plan.line_length..(first + count) * plan.line_length];
        let result = self.run(
            &[c"cat-file".as_ptr(), flag.as_ptr()],
            false,
            Some(Bytes::new(lines)),
        )?;
        let mut capture = unsafe { result.out.view() };
        for object in &mut plan.objects[first..first + count] {
            let oid = unsafe {
                slice::from_raw_parts(object.oid, plan.line_length - 1)
            };
            let expected = payload.then_some(object.size);
            let (size, frame_length, bytes) =
                self.batch_frame(&mut capture, oid, expected, payload)?;
            object.size = size;
            object.frame_length = frame_length;
            if payload {
                object.bytes = bytes;
            }
        }
        self.batch_eof(capture)
    }

    fn batch_preflight(&self, plan: &mut BatchPlan<'_>) -> Checked<()> {
        let (stdout_limit, _) = self.output_budget();
        let worst_header =
            (plan.line_length - 1) + 7 + (usize::MAX.ilog10() as usize + 1);
        let batch = (stdout_limit / worst_header).max(1);
        let total_objects = plan.objects.len();
        let mut first = 0;
        while first < total_objects {
            self.gate()?;
            let count = (total_objects - first).min(batch);
            self.batch_command(plan, first, count, false)?;
            first += count;
        }
        let mut total = 0usize;
        for object in &*plan.objects {
            self.gate()?;
            let frame = object.frame_length;
            let Some(next) =
                total.checked_add(frame).filter(|_| frame <= stdout_limit)
            else {
                return self.fail(
                    LIMIT,
                    c"Git batch payload frame exceeds output limit",
                );
            };
            total = next;
        }
        let (_, remaining) = self.output_budget();
        if total > remaining {
            return self.fail(
                LIMIT,
                c"Git batch payloads exceed remaining output limit",
            );
        }
        self.gate()
    }

    fn batch_payloads(&self, plan: &mut BatchPlan<'_>) -> Checked<()> {
        let mut first = 0;
        while first < plan.objects.len() {
            let (stdout_limit, remaining) = self.output_budget();
            let mut count = 0;
            let mut total = 0;
            for object in &plan.objects[first..] {
                self.gate()?;
                if object.frame_length > stdout_limit - total {
                    break;
                }
                total += object.frame_length;
                count += 1;
            }
            if count == 0 || total > remaining {
                return self.fail(
                    LIMIT,
                    c"Git batch payloads exceed remaining output limit",
                );
            }
            self.batch_command(plan, first, count, true)?;
            first += count;
        }
        self.gate()
    }

    fn sorted_picks(
        &self,
        inventory: Inventory,
        indices: &[usize],
        width: usize,
    ) -> Checked<*mut Pick> {
        for &index in indices {
            self.gate()?;
            if index >= inventory.count {
                return self.fail(
                    INVALID,
                    c"Git batch inventory index is out of range",
                );
            }
            let entry = unsafe { &*inventory.entries.add(index) };
            if entry.object_type != BLOB || !regular_mode(entry.mode) {
                return self
                    .fail(UNSUPPORTED, c"Git batch requires regular blobs");
            }
        }
        let picks = self.batch_array::<Pick>(indices.len())?;
        for (position, &index) in indices.iter().enumerate() {
            self.gate()?;
            unsafe {
                picks.add(position).write(Pick {
                    entry: inventory.entries.add(index),
                    inventory_index: index,
                    position,
                    object_index: 0,
                });
            }
        }
        let rows = unsafe { slice::from_raw_parts_mut(picks, indices.len()) };
        self.sort(
            rows,
            || self.gate(),
            |a, b| {
                Ok(pick_oid(a)[..width]
                    .cmp(&pick_oid(b)[..width])
                    .then(a.position.cmp(&b.position)))
            },
        )?;
        self.gate()?;
        Ok(picks)
    }

    fn plan_objects<'a>(
        &'a self,
        picks: &mut [Pick],
        width: usize,
        max_input: usize,
    ) -> Checked<BatchPlan<'a>> {
        let mut unique = 0;
        let mut previous: Option<Oid> = None;
        for pick in picks.iter_mut() {
            self.gate()?;
            let oid = *pick_oid(pick);
            if previous
                .as_ref()
                .is_none_or(|prev| prev[..width] != oid[..width])
            {
                unique += 1;
                previous = Some(oid);
            }
            pick.object_index = unique - 1;
        }
        let objects = self.batch_array::<Object>(unique)?;
        let line_length = width + 1;
        let Some(input_len) = unique
            .checked_mul(line_length)
            .filter(|&size| size <= max_input / 2)
        else {
            return self
                .fail(LIMIT, c"Git batch cumulative stdin limit exceeded");
        };
        let lines = self.batch_array::<u8>(input_len)?;
        let mut next = 0;
        for pick in picks {
            self.gate()?;
            if pick.object_index != next {
                continue;
            }
            let oid = pick_oid(pick);
            if !canonical_oid(oid, width) {
                return self.fail(COMMAND, c"Invalid owned Git batch object ID");
            }
            unsafe {
                objects.add(next).write(Object {
                    oid: oid.as_ptr(),
                    size: 0,
                    frame_length: 0,
                    bytes: Bytes::default(),
                });
                let dest = lines.add(next * line_length);
                ptr::copy_nonoverlapping(oid.as_ptr(), dest, width);
                dest.add(width).write(b'\n');
            }
            next += 1;
        }
        self.gate()?;
        unsafe { mark_written(lines, input_len) };
        Ok(BatchPlan {
            objects: unsafe { slice::from_raw_parts_mut(objects, unique) },
            lines: unsafe { slice::from_raw_parts(lines, input_len) },
            line_length,
        })
    }

    fn blob_batch(
        &self,
        request: &BatchRequest,
        limits: &BatchLimits,
    ) -> Checked<Batch> {
        self.history_guard()?;
        let count = request.count;
        if count > limits.max_entries
            || count > usize::MAX / size_of::<usize>()
            || count > usize::MAX / size_of::<Pick>()
            || count > usize::MAX / size_of::<BatchItem>()
        {
            return self.fail(LIMIT, c"Git batch entry limit exceeded");
        }
        let revision = request.revision;
        let commit = self.revision(revision).unwrap();
        let inventory = self.cached_inventory(revision, commit)?;
        let mut items = ptr::null_mut();
        if count != 0 {
            let indices =
                unsafe { slice::from_raw_parts(request.indices, count) };
            let width = self.width();
            let picks = self.sorted_picks(inventory, indices, width)?;
            let picks = unsafe { slice::from_raw_parts_mut(picks, count) };
            let mut plan =
                self.plan_objects(picks, width, limits.max_input_bytes)?;
            self.batch_preflight(&mut plan)?;
            self.batch_payloads(&mut plan)?;
            items = self.batch_array::<BatchItem>(count)?;
            for pick in picks {
                self.gate()?;
                let bytes = plan.objects[pick.object_index].bytes;
                unsafe {
                    items.add(pick.position).write(BatchItem {
                        inventory_index: pick.inventory_index,
                        entry: pick.entry,
                        bytes,
                    });
                }
            }
        }
        self.history_guard()?;
        self.gate()?;
        unsafe { mark_written(items, count) };
        Ok(Batch {
            revision,
            commit,
            items,
            count,
        })
    }
}

fn pick_oid(pick: &Pick) -> &Oid {
    unsafe { &(*pick.entry).oid }
}

fn canonical_oid(oid: &Oid, width: usize) -> bool {
    oid[width] == 0
        && oid[..width]
            .iter()
            .all(|&c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

#[unsafe(no_mangle)]
extern "C" fn cbm_git_facts_read_blob_batch(
    facts: *mut Facts,
    request: *const BatchRequest,
    limits: *const BatchLimits,
    out: *mut Batch,
    error: *mut Error,
) -> bool {
    with_request(facts, error, |call| {
        clear_output(out);
        let invalid = || {
            call.fail::<()>(INVALID, c"Invalid pinned Git blob batch request")
                .is_ok()
        };
        let (Some(request), Some(limits)) =
            (unsafe { request.as_ref() }, unsafe { limits.as_ref() })
        else {
            return invalid();
        };
        if facts.is_null()
            || out.is_null()
            || limits.max_entries == 0
            || limits.max_input_bytes == 0
            || limits.max_arena_bytes == 0
            || !inventory_revision(request.revision)
            || (request.count != 0 && request.indices.is_null())
        {
            return invalid();
        }
        let mut budget = BatchBudget {
            arena_limit: limits.max_arena_bytes,
            arena_used: 0,
            input_limit: limits.max_input_bytes,
            input_used: 0,
        };
        let budget = &raw mut budget;
        if !unsafe {
            mark_written(budget, 1);
            mori_git_io_begin_batch(facts, budget)
        } {
            return invalid();
        }
        let batch_call = Request {
            in_batch: true,
            ..*call
        };
        let result = batch_call.blob_batch(request, limits);
        unsafe { mori_git_io_end_batch(facts, call.error) };
        let Ok(batch) = result else { return false };
        unsafe { write_output(out, batch) };
        true
    })
}

#[cfg(any(feature = "native-engine-test-seams", test))]
mod fixtures {
    use super::*;

    fn expected_oid(text: *const u8, width: usize) -> Option<Oid> {
        if text.is_null() || !matches!(width, 40 | 64) {
            return None;
        }
        let mut oid = [0; 65];
        for (i, byte) in oid[..=width].iter_mut().enumerate() {
            *byte = unsafe { *text.add(i) };
            if *byte == 0 && i != width {
                return None;
            }
        }
        canonical_oid(&oid, width).then_some(oid)
    }

    #[unsafe(no_mangle)]
    extern "C" fn cbm_git_facts_test_batch_header(
        capture: Bytes,
        oid: *const u8,
        width: usize,
        size: *mut usize,
        length: *mut usize,
        error: *mut Error,
    ) -> bool {
        with_request(ptr::null_mut(), error, |request| {
            clear_output(size);
            clear_output(length);
            let invalid = || {
                request
                    .fail::<()>(INVALID, c"Invalid Git batch header test input")
                    .is_ok()
            };
            if size.is_null()
                || length.is_null()
                || (capture.length != 0 && capture.data.is_null())
            {
                return invalid();
            }
            let Some(oid) = expected_oid(oid, width) else {
                return invalid();
            };
            let Ok((n, header)) =
                request.batch_header(unsafe { capture.view() }, &oid[..width])
            else {
                return false;
            };
            unsafe {
                write_output(size, n);
                write_output(length, header);
            }
            true
        })
    }

    #[unsafe(no_mangle)]
    extern "C" fn cbm_git_facts_test_batch_capture(
        capture: Bytes,
        width: usize,
        oids: *const *const u8,
        sizes: *const usize,
        count: usize,
        has_payload: bool,
        error: *mut Error,
    ) -> bool {
        with_request(ptr::null_mut(), error, |request| {
            let check = || {
                if (capture.length != 0 && capture.data.is_null())
                    || !matches!(width, 40 | 64)
                    || (count != 0
                        && (oids.is_null() || (has_payload && sizes.is_null())))
                    || count > usize::MAX / size_of::<*const u8>()
                    || count > usize::MAX / size_of::<usize>()
                {
                    return request
                        .fail(INVALID, c"Invalid Git batch capture test input");
                }
                for i in 0..count {
                    if expected_oid(unsafe { *oids.add(i) }, width).is_none() {
                        return request.fail(
                            INVALID,
                            c"Invalid expected Git batch object ID",
                        );
                    }
                }
                let mut capture = unsafe { capture.view() };
                for i in 0..count {
                    let oid =
                        unsafe { slice::from_raw_parts(*oids.add(i), width) };
                    let size =
                        (!sizes.is_null()).then(|| unsafe { *sizes.add(i) });
                    request.batch_frame(&mut capture, oid, size, has_payload)?;
                }
                request.batch_eof(capture)
            };
            check().is_ok()
        })
    }
}
