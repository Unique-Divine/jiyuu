//! Shared definition and file properties for serial and parallel extraction.

use std::ffi::{CStr, c_char, c_int};
use std::fmt::{self, Write};
use std::mem::MaybeUninit;
use std::ptr::null_mut;
use std::slice;

// The stack fallback uses the same optional-field reserve as the native pass.
const PREFIX_RESERVE: usize = 1024;
const FIELD_OVERHEAD: usize = 6;
const END_RESERVE: usize = 2;
const ARRAY_MARGIN: usize = 10;
const SWIFT_MARGIN: usize = 80;
const FINGERPRINT_COUNT: usize = 64;
const FINGERPRINT_HEX: usize = FINGERPRINT_COUNT * 8;
const FINGERPRINT_MARGIN: usize = FINGERPRINT_HEX + 10;
const PROFILE_MARGIN: usize = 200;
const TOKENS_MARGIN: usize = 512;

type Text = *const c_char;
type TextArray = *const Text;
type Allocate = unsafe extern "C" fn(usize) -> *mut c_char;

/// Matches the borrowed projection in definition_props.h.
#[repr(C)]
struct Definition {
    label: Text,
    call_selector: Text,
    docstring: Text,
    variants: Text,
    variant_key: Text,
    signature: Text,
    return_type: Text,
    parent_class: Text,
    route_path: Text,
    route_method: Text,
    client_route_path: Text,
    http_client: Text,
    http_base_url: Text,
    structural_profile: Text,
    body_tokens: Text,
    decorators: TextArray,
    base_classes: TextArray,
    param_names: TextArray,
    param_types: TextArray,
    extra_props: TextArray,
    fingerprint: *const [u32; FINGERPRINT_COUNT],
    param_default_mask: u64,
    complexity: c_int,
    cognitive: c_int,
    loop_count: c_int,
    loop_depth: c_int,
    param_count: c_int,
    max_access_depth: c_int,
    linear_scan_in_loop: c_int,
    alloc_in_loop: c_int,
    lines: c_int,
    test_role: c_int,
    selector_param_count: u8,
    has_signature: bool,
    is_recursive: bool,
    recursion_in_loop: bool,
    unguarded_recursion: bool,
    is_exported: bool,
    is_test: bool,
    is_entry_point: bool,
}

enum Field<'a> {
    Text(&'a [u8], &'a [u8], usize),
    Raw(&'a [u8], &'a [u8]),
    Strings(&'a [u8], TextArray),
    Fingerprint(&'a [u32; FINGERPRINT_COUNT]),
}

fn escape(byte: u8) -> Option<u8> {
    match byte {
        b'"' | b'\\' => Some(byte),
        b'\n' => Some(b'n'),
        b'\r' => Some(b'r'),
        b'\t' => Some(b't'),
        _ => None,
    }
}

fn escaped_len(bytes: &[u8]) -> usize {
    bytes.len() + bytes.iter().filter(|&&byte| escape(byte).is_some()).count()
}

/// Builders borrow C strings and null-terminated pointer arrays from one live
/// file result. No field or iterator escapes that result's serialization call.
unsafe fn text<'a>(value: Text) -> Option<&'a [u8]> {
    (!value.is_null()).then(|| unsafe { CStr::from_ptr(value).to_bytes() })
}

unsafe fn texts<'a>(mut cursor: TextArray) -> impl Iterator<Item = &'a [u8]> {
    std::iter::from_fn(move || {
        if cursor.is_null() {
            return None;
        }
        let value = unsafe { text(*cursor) }?;
        cursor = unsafe { cursor.add(1) };
        Some(value)
    })
}

impl Definition {
    /// Every pointer in this projection must remain valid through iteration.
    unsafe fn fields(&self) -> impl Iterator<Item = Field<'_>> {
        let string = |key: &'static [u8], value| {
            Field::Text(key, unsafe { text(value) }.unwrap_or_default(), 0)
        };
        let before_extras = [
            string(b"call_selector", self.call_selector),
            string(b"docstring", self.docstring),
            Field::Raw(
                b"variants",
                unsafe { text(self.variants) }.unwrap_or_default(),
            ),
            string(b"variant_key", self.variant_key),
            string(b"signature", self.signature),
            string(b"return_type", self.return_type),
            string(b"parent_class", self.parent_class),
            Field::Strings(b"decorators", self.decorators),
            Field::Strings(b"base_classes", self.base_classes),
            Field::Strings(b"param_names", self.param_names),
            Field::Strings(b"param_types", self.param_types),
            string(b"route_path", self.route_path),
            string(b"route_method", self.route_method),
            string(b"client_route_path", self.client_route_path),
            string(b"http_client", self.http_client),
            string(b"http_base_url", self.http_base_url),
        ];
        let mut extras = unsafe { texts(self.extra_props) };
        let extras = std::iter::from_fn(move || {
            let (key, value) = (extras.next()?, extras.next()?);
            Some(Field::Text(key, value, 0))
        });
        before_extras
            .into_iter()
            .chain(extras)
            .chain(unsafe { self.fingerprint.as_ref() }.map(Field::Fingerprint))
            .chain(
                unsafe { text(self.structural_profile) }
                    .map(|value| Field::Text(b"sp", value, PROFILE_MARGIN)),
            )
            .chain(
                unsafe { text(self.body_tokens) }
                    .map(|value| Field::Text(b"bt", value, TOKENS_MARGIN)),
            )
    }

    unsafe fn prefix(&self, out: &mut Buffer<'_>) -> fmt::Result {
        write!(out, "{{\"complexity\":{}", self.complexity)?;
        if matches!(unsafe { text(self.label) }, Some(b"Function" | b"Method")) {
            write!(
                out,
                ",\"cognitive\":{},\"loop_count\":{},\"loop_depth\":{},\"self_recursive\":{},\"param_count\":{},\"max_access_depth\":{},\"linear_scan_in_loop\":{},\"alloc_in_loop\":{},\"recursion_in_loop\":{},\"unguarded_recursion\":{}",
                self.cognitive,
                self.loop_count,
                self.loop_depth,
                self.is_recursive,
                self.param_count,
                self.max_access_depth,
                self.linear_scan_in_loop,
                self.alloc_in_loop,
                self.recursion_in_loop,
                self.unguarded_recursion,
            )?;
        }
        write!(
            out,
            ",\"lines\":{},\"is_exported\":{},\"is_test\":{},\"is_entry_point\":{}",
            self.lines, self.is_exported, self.is_test, self.is_entry_point
        )?;
        match self.test_role {
            1 => out.write_str(",\"test_role\":\"case\"")?,
            2 => out.write_str(",\"test_role\":\"suite\"")?,
            _ => {}
        }
        if self.has_signature && out.remaining() > SWIFT_MARGIN {
            write!(
                out,
                ",\"param_default_mask\":\"{:016x}\",\"selector_param_count\":{}",
                self.param_default_mask, self.selector_param_count
            )?;
        }
        Ok(())
    }
}

fn field_len(key: &[u8], value_len: usize) -> usize {
    key.len()
        .saturating_add(value_len)
        .saturating_add(FIELD_OVERHEAD)
}

impl Field<'_> {
    // Same reserve policy as the two extraction loops. Oversized optional
    // fields stay atomic when the allocator leaves us on the stack buffer.
    fn reserve(&self) -> usize {
        match *self {
            Self::Text(key, value, margin) => {
                let length = if value.is_empty() {
                    0
                } else {
                    field_len(key, escaped_len(value))
                };
                if margin == 0 {
                    length
                } else {
                    length.saturating_add(margin + 1)
                }
            }
            Self::Raw(key, value) => {
                if value.is_empty() {
                    0
                } else {
                    field_len(key, value.len())
                }
            }
            Self::Strings(key, values) => {
                // SAFETY: These arrays come from the borrowed Definition.
                let mut values = unsafe { texts(values) }.peekable();
                if values.peek().is_none() {
                    return 0;
                }
                values.enumerate().fold(
                    field_len(key, 0),
                    |size, (index, value)| {
                        size.saturating_add(
                            escaped_len(value) + 2 + usize::from(index != 0),
                        )
                    },
                )
            }
            Self::Fingerprint(_) => FINGERPRINT_MARGIN + 1,
        }
    }

    fn append(&self, out: &mut Buffer<'_>) -> fmt::Result {
        match *self {
            Self::Text(key, value, margin) => {
                if out.remaining() > margin {
                    out.string(key, value, true)?;
                }
            }
            Self::Raw(key, value) => {
                if !value.is_empty()
                    && field_len(key, value.len()).saturating_add(END_RESERVE)
                        <= out.remaining()
                {
                    out.key(key, true)?;
                    out.put(value)?;
                }
            }
            Self::Strings(key, values) => {
                if out.remaining() <= ARRAY_MARGIN
                    || self.reserve().saturating_add(END_RESERVE)
                        > out.remaining()
                {
                    return Ok(());
                }
                // SAFETY: These arrays come from the borrowed Definition.
                let mut values = unsafe { texts(values) }.peekable();
                if values.peek().is_none() {
                    return Ok(());
                }
                out.key(key, true)?;
                out.put(b"[")?;
                for (index, value) in values.enumerate() {
                    if index != 0 {
                        out.put(b",")?;
                    }
                    out.put(b"\"")?;
                    out.escaped(value, END_RESERVE)?;
                    out.put(b"\"")?;
                }
                out.put(b"]")?;
            }
            Self::Fingerprint(values) => {
                if out.remaining() > FINGERPRINT_MARGIN {
                    out.key(b"fp", true)?;
                    out.put(b"\"")?;
                    for value in values {
                        write!(out, "{value:08x}")?;
                    }
                    out.put(b"\"")?;
                }
            }
        }
        Ok(())
    }
}

struct Buffer<'a> {
    bytes: &'a mut [MaybeUninit<u8>],
    len: usize,
}

impl Buffer<'_> {
    fn remaining(&self) -> usize {
        self.bytes.len() - self.len
    }

    fn put(&mut self, bytes: &[u8]) -> fmt::Result {
        if self.remaining() < bytes.len() {
            return Err(fmt::Error);
        }
        let output = &mut self.bytes[self.len..][..bytes.len()];
        // SAFETY: The output region is disjoint and has the requested capacity.
        unsafe {
            output
                .as_mut_ptr()
                .cast::<u8>()
                .copy_from_nonoverlapping(bytes.as_ptr(), bytes.len())
        };
        self.len += bytes.len();
        Ok(())
    }

    fn key(&mut self, key: &[u8], comma: bool) -> fmt::Result {
        self.put(if comma { b",\"" } else { b"\"" })?;
        self.put(key)?;
        self.put(b"\":")
    }

    fn escaped(&mut self, bytes: &[u8], margin: usize) -> fmt::Result {
        for &byte in bytes {
            if self.remaining() <= margin {
                break;
            }
            if let Some(code) = escape(byte) {
                self.put(&[b'\\', code])?;
            } else {
                self.put(&[if byte < 0x20 { b' ' } else { byte }])?;
            }
        }
        Ok(())
    }

    fn string(&mut self, key: &[u8], value: &[u8], comma: bool) -> fmt::Result {
        if value.is_empty()
            || field_len(key, escaped_len(value)).saturating_add(END_RESERVE)
                > self.remaining()
        {
            return Ok(());
        }
        self.key(key, comma)?;
        self.put(b"\"")?;
        self.escaped(value, END_RESERVE + 1)?;
        self.put(b"\"")
    }

    fn finish(&mut self, result: fmt::Result) {
        if result.is_err() || self.put(b"}\0").is_err() {
            self.len = 0;
            let _ = self.put(b"\0");
        }
        mark_written(&self.bytes[..self.len]);
    }
}

impl Write for Buffer<'_> {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        self.put(value.as_bytes())
    }
}

fn mark_written(bytes: &[MaybeUninit<u8>]) {
    #[cfg(feature = "native-engine-abi")]
    unsafe {
        cbm_ffi_mark_written(bytes.as_ptr().cast(), bytes.len());
    }
    #[cfg(not(feature = "native-engine-abi"))]
    let _ = bytes;
}

#[cfg(feature = "native-engine-abi")]
unsafe extern "C" {
    fn cbm_ffi_mark_written(address: *const std::ffi::c_void, size: usize);
}

#[cfg_attr(feature = "native-engine-abi", unsafe(no_mangle))]
unsafe extern "C" fn cbm_rust_definition_props(
    definition: *const Definition,
    stack: *mut c_char,
    stack_size: usize,
    allocate: Allocate,
) -> *mut c_char {
    // SAFETY: The C caller lends a complete projection and an exclusive
    // initialized-or-uninitialized byte buffer. The allocator returns owned
    // native storage or null, leaving the stack buffer available on failure.
    let definition = unsafe { &*definition };
    let (mut storage, mut capacity) = (stack, stack_size);
    let reserve = unsafe { definition.fields() }
        .fold(PREFIX_RESERVE + END_RESERVE, |size, field| {
            size.saturating_add(field.reserve())
        });
    if reserve > capacity && reserve <= isize::MAX as usize {
        let allocated = unsafe { allocate(reserve) };
        if !allocated.is_null() {
            (storage, capacity) = (allocated, reserve);
        }
    }
    let bytes = unsafe { slice::from_raw_parts_mut(storage.cast(), capacity) };
    let mut out = Buffer { bytes, len: 0 };
    let result = unsafe { definition.prefix(&mut out) }.and_then(|()| {
        for field in unsafe { definition.fields() } {
            field.append(&mut out)?;
        }
        Ok(())
    });
    out.finish(result);
    storage
}

#[cfg_attr(feature = "native-engine-abi", unsafe(no_mangle))]
unsafe extern "C" fn cbm_rust_file_doc_props(
    old: Text,
    doc: Text,
    allocate: Allocate,
) -> *mut c_char {
    let old = unsafe { text(old) }.unwrap_or(b"{}");
    let Some(doc) = unsafe { text(doc) }.filter(|doc| !doc.is_empty()) else {
        return null_mut();
    };
    if old.len() < 2 || old.last() != Some(&b'}') {
        return null_mut();
    }
    let size = old
        .len()
        .saturating_add(field_len(b"docstring", escaped_len(doc)))
        .saturating_add(END_RESERVE + 1);
    if size > isize::MAX as usize {
        return null_mut();
    }
    let storage = unsafe { allocate(size) };
    if storage.is_null() {
        return storage;
    }
    let bytes = unsafe { slice::from_raw_parts_mut(storage.cast(), size) };
    let mut out = Buffer { bytes, len: 0 };
    let result = out
        .put(&old[..old.len() - 1])
        .and_then(|()| out.string(b"docstring", doc, old.len() != 2));
    out.finish(result);
    storage
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" fn no_allocation(_: usize) -> *mut c_char {
        null_mut()
    }

    #[test]
    fn oversized_fields_drop_whole_with_the_stack_fallback() {
        let wide = std::ffi::CString::new("a".repeat(4096)).unwrap();
        let args =
            [c"const\nchar *".as_ptr(), c"int".as_ptr(), std::ptr::null()];
        // All fields are pointers, integers, or booleans with valid zero values.
        let mut def: Definition = unsafe { std::mem::zeroed() };
        def.label = c"Method".as_ptr();
        def.docstring = wide.as_ptr();
        def.param_types = args.as_ptr();
        def.variant_key = c"contains\tnewline\nquote\"".as_ptr();
        def.variants = c"[{\"start_line\":2}]".as_ptr();
        def.test_role = 2;
        def.has_signature = true;
        def.param_default_mask = 5;
        def.selector_param_count = 2;
        let mut stack = [MaybeUninit::<c_char>::uninit(); 2048];
        let output = unsafe {
            cbm_rust_definition_props(
                &def,
                stack.as_mut_ptr().cast(),
                stack.len(),
                no_allocation,
            )
        };
        assert_eq!(output, stack.as_mut_ptr().cast());
        let json = unsafe { CStr::from_ptr(output) }.to_str().unwrap();
        assert!(json.contains(r#""test_role":"suite","param_default_mask":"0000000000000005","selector_param_count":2"#));
        assert!(json.contains(r#""variants":[{"start_line":2}],"variant_key":"contains\tnewline\nquote\"""#));
        assert!(json.contains(r#""param_types":["const\nchar *","int"]"#));
        assert!(!json.contains("docstring"));
    }

    #[test]
    fn plain_definition_keeps_the_smaller_property_set() {
        let mut def: Definition = unsafe { std::mem::zeroed() };
        def.label = c"Macro".as_ptr();
        def.complexity = 7;
        def.lines = 2;
        def.is_exported = true;
        let mut stack = [MaybeUninit::<c_char>::uninit(); 2048];
        let output = unsafe {
            cbm_rust_definition_props(
                &def,
                stack.as_mut_ptr().cast(),
                stack.len(),
                no_allocation,
            )
        };
        assert_eq!(
            unsafe { CStr::from_ptr(output) }.to_bytes(),
            br#"{"complexity":7,"lines":2,"is_exported":true,"is_test":false,"is_entry_point":false}"#
        );
        assert!(
            unsafe {
                cbm_rust_file_doc_props(
                    c"[]".as_ptr(),
                    c"docs".as_ptr(),
                    no_allocation,
                )
            }
            .is_null()
        );
    }
}
