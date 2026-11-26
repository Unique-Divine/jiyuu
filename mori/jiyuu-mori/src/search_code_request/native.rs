use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::mem::MaybeUninit;
use std::ops::Deref;
use std::{ptr, slice};

pub(super) use crate::ffi_output::c_str;
use crate::ffi_output::written;

#[repr(C)]
pub(super) struct RawMatch {
    file: *const c_char,
    pub line: c_int,
    content: [c_char; 1024],
    pub content_start_byte: usize,
    pub content_returned_bytes: usize,
    pub content_total_bytes: usize,
    pub match_start_byte: usize,
    pub match_end_byte: usize,
    pub match_known: bool,
    pub content_truncated: bool,
}

impl RawMatch {
    pub fn file(&self) -> &CStr {
        // SAFETY: the native collector owns a NUL-terminated file per row.
        unsafe { CStr::from_ptr(self.file) }
    }

    pub fn content(&self) -> &CStr {
        array_str(&self.content)
    }
}

#[repr(C)]
pub(super) struct SearchResult {
    node_id: i64,
    qualified_name: *const c_char,
    label: [c_char; 64],
    file: *const c_char,
    pub start_line: c_int,
    pub end_line: c_int,
    pub in_degree: c_int,
    pub out_degree: c_int,
    match_lines: *const c_int,
    pub match_count: c_int,
    match_stored_count: c_int,
    match_capacity: c_int,
    pub last_match_line: c_int,
}

impl SearchResult {
    pub fn qualified_name(&self) -> &CStr {
        // SAFETY: a gathered row owns this terminated string.
        unsafe { CStr::from_ptr(self.qualified_name) }
    }

    pub fn label(&self) -> &CStr {
        array_str(&self.label)
    }

    pub fn file(&self) -> &CStr {
        // SAFETY: a gathered row owns this terminated string.
        unsafe { CStr::from_ptr(self.file) }
    }

    pub fn matches(&self) -> &[c_int] {
        // SAFETY: C initialized match_stored_count entries in this row.
        unsafe { view(self.match_lines, self.match_stored_count) }
    }
}

pub(super) fn array_str(array: &[c_char]) -> &CStr {
    // SAFETY: c_char and u8 have the same size, alignment, and valid bit patterns.
    let bytes =
        unsafe { slice::from_raw_parts(array.as_ptr().cast(), array.len()) };
    CStr::from_bytes_until_nul(bytes).expect("native array is NUL-terminated")
}

unsafe fn view<'a, T>(ptr: *const T, count: c_int) -> &'a [T] {
    if count == 0 {
        return &[];
    }
    // SAFETY: the caller provides count initialized entries live for this borrow.
    unsafe { slice::from_raw_parts(ptr, count as usize) }
}

#[repr(C)]
pub(super) struct GatherArgs {
    pub pattern: *const c_char,
    pub project: *const c_char,
    pub root: *const c_char,
    pub file_pattern: *const c_char,
    pub path_regex: *mut c_void,
    pub raw_offset: c_int,
    pub raw_limit: c_int,
    pub raw_content_offset: usize,
    pub raw_content_offset_set: bool,
    pub use_regex: bool,
    pub debug: bool,
}

#[repr(C)]
pub(super) struct GatheredData {
    results: *mut SearchResult,
    raw: *mut RawMatch,
    pub result_count: c_int,
    pub raw_count: c_int,
    pub raw_stored_count: c_int,
    pub raw_content_truncated: c_int,
    pub grep_count: c_int,
    pub scope_ms: u64,
    pub scan_ms: u64,
    pub enrich_ms: u64,
    pub timed_out: bool,
    pub error: [c_char; 256],
}

impl GatheredData {
    pub fn results(&self) -> &[SearchResult] {
        // SAFETY: the gather operation owns these initialized rows.
        unsafe { view(self.results, self.result_count) }
    }

    pub fn raw(&self) -> &[RawMatch] {
        // SAFETY: only raw_stored_count entries are backed by the returned array.
        unsafe { view(self.raw, self.raw_stored_count) }
    }
}

#[repr(C)]
pub(super) struct NativeOps {
    project: unsafe extern "C" fn(*const c_char) -> *mut c_char,
    root: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_char,
    project_error:
        unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_char,
    regex_open: unsafe extern "C" fn(*const c_char, *mut c_int) -> *mut c_void,
    regex_close: unsafe extern "C" fn(*mut c_void),
    gather:
        unsafe extern "C" fn(*mut c_void, *const GatherArgs, *mut GatheredData),
    release: unsafe extern "C" fn(*mut GatheredData),
    source: unsafe extern "C" fn(
        *mut c_void,
        *const c_char,
        *const c_char,
        c_int,
        c_int,
    ) -> *mut c_char,
    text_result: unsafe extern "C" fn(*const c_char, bool) -> *mut c_char,
    copy: unsafe extern "C" fn(*const c_char) -> *mut c_char,
    free_string: unsafe extern "C" fn(*mut c_char),
    json_tree: unsafe extern "C" fn(*const c_char) -> *mut c_char,
    tree_table: unsafe extern "C" fn(
        *const c_char,
        c_int,
        *const *const c_char,
        c_int,
        *const *const c_char,
        *const bool,
        *const bool,
    ) -> *mut c_char,
    now_ms: unsafe extern "C" fn() -> u64,
    log_slow: unsafe extern "C" fn(u64),
}

#[derive(Clone, Copy)]
pub(super) struct Native<'a> {
    pub context: *mut c_void,
    pub ops: &'a NativeOps,
}

pub(super) struct StringGuard<'a> {
    ptr: *mut c_char,
    ops: &'a NativeOps,
}

impl Deref for StringGuard<'_> {
    type Target = CStr;

    fn deref(&self) -> &CStr {
        // SAFETY: this guard owns a non-null native C string.
        unsafe { CStr::from_ptr(self.ptr) }
    }
}

impl Drop for StringGuard<'_> {
    fn drop(&mut self) {
        // SAFETY: use the same allocator that supplied the string.
        unsafe { (self.ops.free_string)(self.ptr) };
    }
}

pub(super) struct Regex<'a> {
    pub handle: *mut c_void,
    ops: &'a NativeOps,
}

impl Drop for Regex<'_> {
    fn drop(&mut self) {
        // SAFETY: the native callback owns the regex representation.
        unsafe { (self.ops.regex_close)(self.handle) };
    }
}

pub(super) struct Gathered<'a> {
    pub data: GatheredData,
    ops: &'a NativeOps,
}

impl Drop for Gathered<'_> {
    fn drop(&mut self) {
        // SAFETY: this guard owns the native arrays, even on a scan error.
        unsafe {
            written(&self.data, 1);
            (self.ops.release)(&mut self.data);
        }
    }
}

impl<'a> Native<'a> {
    fn string(self, ptr: *mut c_char) -> Option<StringGuard<'a>> {
        (!ptr.is_null()).then(|| StringGuard { ptr, ops: self.ops })
    }

    pub fn project(self, key: &CStr) -> Option<StringGuard<'a>> {
        self.string(unsafe { (self.ops.project)(c_str(key)) })
    }

    pub fn root(self, project: &CStr) -> Option<StringGuard<'a>> {
        self.string(unsafe { (self.ops.root)(self.context, c_str(project)) })
    }

    pub fn project_error(self, message: &CStr) -> *mut c_char {
        unsafe { (self.ops.project_error)(self.context, c_str(message)) }
    }

    pub fn regex(self, pattern: &CStr) -> Result<Regex<'a>, c_int> {
        let mut status = MaybeUninit::uninit();
        // SAFETY: the callback initializes status on all paths.
        let handle = unsafe {
            (self.ops.regex_open)(c_str(pattern), status.as_mut_ptr())
        };
        if handle.is_null() {
            Err(unsafe { status.assume_init() })
        } else {
            Ok(Regex {
                handle,
                ops: self.ops,
            })
        }
    }

    pub fn gather(self, args: GatherArgs) -> Gathered<'a> {
        let mut data = MaybeUninit::uninit();
        // SAFETY: args only borrows live strings/regexes for this synchronous
        // call. The callback initializes the full output, including on failure.
        let data = unsafe {
            written(&args, 1);
            (self.ops.gather)(self.context, &args, data.as_mut_ptr());
            data.assume_init()
        };
        Gathered {
            data,
            ops: self.ops,
        }
    }

    pub fn source(
        self,
        root: &CStr,
        file: &CStr,
        start: i32,
        end: i32,
    ) -> Option<StringGuard<'a>> {
        self.string(unsafe {
            (self.ops.source)(self.context, c_str(root), c_str(file), start, end)
        })
    }

    pub fn text_result(self, text: &CStr, error: bool) -> *mut c_char {
        unsafe { (self.ops.text_result)(c_str(text), error) }
    }

    pub fn copy(self, str: &CStr) -> *mut c_char {
        unsafe { (self.ops.copy)(c_str(str)) }
    }

    pub fn now_ms(self) -> u64 {
        unsafe { (self.ops.now_ms)() }
    }

    pub fn log_slow(self, elapsed: u64) {
        unsafe { (self.ops.log_slow)(elapsed) };
    }

    pub fn json_tree(self, json: &CStr) -> Option<StringGuard<'a>> {
        self.string(unsafe { (self.ops.json_tree)(c_str(json)) })
    }

    pub fn tree_table<const N: usize>(
        self,
        name: &CStr,
        columns: [&CStr; N],
        rows: Vec<[CString; N]>,
        strings: [bool; N],
        prefixes: [bool; N],
    ) -> Option<StringGuard<'a>> {
        let cols = columns.map(c_str);
        let cells: Vec<_> =
            rows.iter().flatten().map(|cell| c_str(cell)).collect();
        // SAFETY: the table callback borrows these initialized arrays and the
        // terminated strings only until it returns.
        self.string(unsafe {
            written(cols.as_ptr(), N);
            written(cells.as_ptr(), cells.len());
            written(strings.as_ptr(), N);
            written(prefixes.as_ptr(), N);
            (self.ops.tree_table)(
                c_str(name),
                rows.len() as c_int,
                cols.as_ptr(),
                N as c_int,
                if cells.is_empty() {
                    ptr::null()
                } else {
                    cells.as_ptr()
                },
                strings.as_ptr(),
                prefixes.as_ptr(),
            )
        })
    }
}
