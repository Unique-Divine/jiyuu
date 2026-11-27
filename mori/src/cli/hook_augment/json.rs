//! Borrowed views of the same yyjson documents the native engine uses.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::slice;

unsafe extern "C" {
    fn mori_hook_json_read(bytes: *const u8, size: usize) -> *mut c_void;
    fn mori_hook_json_free(doc: *mut c_void);
    fn mori_hook_json_root(doc: *const c_void) -> *const c_void;
    fn mori_hook_json_is_object(val: *const c_void) -> bool;
    fn mori_hook_json_get(
        val: *const c_void,
        key: *const c_char,
    ) -> *const c_void;
    fn mori_hook_json_at(val: *const c_void, index: usize) -> *const c_void;
    fn mori_hook_json_next(val: *const c_void) -> *const c_void;
    fn mori_hook_json_array_len(val: *const c_void) -> usize;
    fn mori_hook_json_is_array(val: *const c_void) -> bool;
    fn mori_hook_json_string(val: *const c_void, len: *mut usize) -> *const u8;
    fn mori_hook_json_bool(val: *const c_void) -> c_int;
    fn mori_hook_json_int(val: *const c_void, out: *mut i64) -> bool;
}

pub(super) struct Doc(NonNull<c_void>);

impl Doc {
    pub(super) fn read(bytes: &[u8]) -> Option<Self> {
        // yyjson copies the input when read without the in-situ flag.
        NonNull::new(unsafe { mori_hook_json_read(bytes.as_ptr(), bytes.len()) })
            .map(Self)
    }

    pub(super) fn root(&self) -> Value<'_> {
        Value(unsafe { mori_hook_json_root(self.0.as_ptr()) }, PhantomData)
    }
}

impl Drop for Doc {
    fn drop(&mut self) {
        unsafe { mori_hook_json_free(self.0.as_ptr()) };
    }
}

/// A nullable native value. yyjson's accessors accept null and type mismatches.
#[derive(Clone, Copy)]
pub(super) struct Value<'doc>(*const c_void, PhantomData<&'doc Doc>);

impl<'doc> Value<'doc> {
    pub(super) fn missing(self) -> bool {
        self.0.is_null()
    }

    pub(super) fn is_object(self) -> bool {
        unsafe { mori_hook_json_is_object(self.0) }
    }

    pub(super) fn get(self, key: &CStr) -> Self {
        Self(
            unsafe { mori_hook_json_get(self.0, key.as_ptr()) },
            PhantomData,
        )
    }

    pub(super) fn at(self, index: usize) -> Self {
        Self(unsafe { mori_hook_json_at(self.0, index) }, PhantomData)
    }

    pub(super) fn len(self) -> usize {
        unsafe { mori_hook_json_array_len(self.0) }
    }

    pub(super) fn is_array(self) -> bool {
        unsafe { mori_hook_json_is_array(self.0) }
    }

    pub(super) fn elements(self) -> impl Iterator<Item = Self> {
        let mut current = self.at(0);
        (0..self.len()).map(move |_| {
            let value = current;
            // yyjson stores array entries consecutively, including nested values.
            current.0 = unsafe { mori_hook_json_next(current.0) };
            value
        })
    }

    pub(super) fn raw_string(self) -> Option<&'doc [u8]> {
        let mut len = 0;
        let ptr = unsafe { mori_hook_json_string(self.0, &mut len) };
        // The value and its string storage belong to the borrowed Doc.
        (!ptr.is_null()).then(|| unsafe { slice::from_raw_parts(ptr, len) })
    }

    pub(super) fn string(self) -> Option<&'doc [u8]> {
        self.raw_string().map(c_prefix)
    }

    pub(super) fn field(self, key: &CStr) -> Option<&'doc [u8]> {
        self.get(key).string()
    }

    pub(super) fn boolean(self) -> Option<bool> {
        match unsafe { mori_hook_json_bool(self.0) } {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }

    pub(super) fn integer(self) -> Option<i64> {
        let mut value = 0;
        unsafe { mori_hook_json_int(self.0, &mut value) }.then_some(value)
    }
}

pub(super) fn c_prefix(bytes: &[u8]) -> &[u8] {
    bytes.split(|&byte| byte == 0).next().unwrap_or_default()
}

pub(super) fn quote(bytes: &[u8]) -> Option<String> {
    // Match the default yyjson writer, including uppercase Unicode escapes.
    use std::fmt::Write;
    let text = std::str::from_utf8(bytes).ok()?;
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for ch in text.chars() {
        match ch {
            '"' => quoted.push_str(r#"\""#),
            '\\' => quoted.push_str(r"\\"),
            '\x08' => quoted.push_str(r"\b"),
            '\t' => quoted.push_str(r"\t"),
            '\n' => quoted.push_str(r"\n"),
            '\x0c' => quoted.push_str(r"\f"),
            '\r' => quoted.push_str(r"\r"),
            '\0'..='\x1f' => {
                let _ = write!(quoted, "\\u{:04X}", ch as u32);
            }
            _ => quoted.push(ch),
        }
    }
    quoted.push('"');
    Some(quoted)
}
