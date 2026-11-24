//! C entry points. Callers must supply readable inputs and writable out
//! parameters as declared in cli/config_json_like.h. Every returned buffer
//! uses the native allocator so the caller can release it with free().

use std::ffi::{CStr, c_char, c_int, c_uint, c_void};
use std::ptr;
use std::slice;

use super::edit::{Edit, lookup, update};
use super::file::{Snapshot, write};
use super::parser::{decoded_string, scan, validate_document, validate_entry};
use super::{Invalid, MAX_BYTES, MAX_DEPTH, MAX_KEY_BYTES, Result};

unsafe extern "C" {
    fn malloc(size: usize) -> *mut c_void;
}

fn status(result: Result<c_int>) -> c_int {
    result.unwrap_or(-1)
}

unsafe fn text<'a>(value: *const c_char) -> Result<&'a CStr> {
    if value.is_null() {
        return Err(Invalid);
    }
    // SAFETY: non-null inputs are terminated C strings by the ABI contract.
    Ok(unsafe { CStr::from_ptr(value) })
}

unsafe fn input_slice<'a, T>(value: *const T, len: usize) -> Result<&'a [T]> {
    if len == 0 {
        return Ok(&[]);
    }
    if value.is_null() || len > isize::MAX as usize / size_of::<T>() {
        return Err(Invalid);
    }
    // SAFETY: the caller supplies len readable contiguous elements.
    Ok(unsafe { slice::from_raw_parts(value, len) })
}

fn valid_key(bytes: &[u8]) -> Result<&[u8]> {
    if bytes.len() > MAX_KEY_BYTES
        || std::str::from_utf8(bytes)
            .map_err(|_| Invalid)?
            .chars()
            .any(char::is_control)
    {
        return Err(Invalid);
    }
    Ok(bytes)
}

fn valid_string(bytes: &[u8]) -> Result<&[u8]> {
    if bytes.len() > MAX_BYTES {
        return Err(Invalid);
    }
    std::str::from_utf8(bytes).map_err(|_| Invalid)?;
    Ok(bytes)
}

struct Location<'a> {
    file: &'a CStr,
    path: Vec<&'a [u8]>,
    key: &'a [u8],
}

impl<'a> Location<'a> {
    unsafe fn new(
        file: *const c_char,
        path: *const *const c_char,
        path_len: usize,
        key: *const c_char,
    ) -> Result<Self> {
        // SAFETY: these are the caller's ABI arguments.
        let file = unsafe { text(file)? };
        if file.is_empty() || path.is_null() || path_len >= MAX_DEPTH {
            return Err(Invalid);
        }
        let key = valid_key(unsafe { text(key)? }.to_bytes())?;
        let path = unsafe { input_slice(path, path_len)? }
            .iter()
            .map(|&component| valid_key(unsafe { text(component)? }.to_bytes()))
            .collect::<Result<_>>()?;
        Ok(Self { file, path, key })
    }

    fn load(&self) -> Result<Snapshot> {
        Snapshot::read(self.file).map_err(|_| Invalid)
    }

    fn edit(&self, operation: Edit<'_>, expected: Expected) -> Result<c_int> {
        let source = self.load()?;
        let bytes = source.content();
        if !expected.matches(bytes) {
            return Err(Invalid);
        }
        if let Some(updated) =
            update(bytes.unwrap_or_default(), &self.path, self.key, operation)?
        {
            write(self.file, &source, &updated)?;
        }
        Ok(0)
    }

    fn get(&self, source: &Snapshot) -> Result<Option<std::ops::Range<usize>>> {
        lookup(source.content().unwrap_or_default(), &self.path, self.key)
    }
}

// Keep the pointer until after the length check. The C implementation only
// reads expected_content when the snapshot has exactly expected_length bytes.
enum Expected {
    Any,
    Exact(*const c_char, usize),
}

impl Expected {
    fn matches(self, bytes: Option<&[u8]>) -> bool {
        match (self, bytes) {
            (Self::Any, _) => true,
            (Self::Exact(expected, _), None) => expected.is_null(),
            (Self::Exact(expected, len), Some(bytes))
                if !expected.is_null() && len == bytes.len() =>
            {
                // SAFETY: the conditional ABI promises len readable bytes.
                unsafe { input_slice(expected.cast::<u8>(), len) }
                    .is_ok_and(|value| value == bytes)
            }
            _ => false,
        }
    }
}

fn owned(bytes: &[u8]) -> Result<*mut c_char> {
    // SAFETY: allocate one extra byte for the terminator. The returned
    // allocation is independent of the borrowed source.
    unsafe {
        let buffer = malloc(bytes.len() + 1).cast::<u8>();
        if buffer.is_null() {
            return Err(Invalid);
        }
        ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len());
        buffer.add(bytes.len()).write(0);
        Ok(buffer.cast())
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_json_like_read_document(
    file: *const c_char,
    content: *mut *mut c_char,
    length: *mut usize,
) -> c_int {
    status((|| {
        // SAFETY: out pointers are writable and file is a C string when supplied.
        let file = unsafe { text(file)? };
        if content.is_null() || length.is_null() {
            return Err(Invalid);
        }
        unsafe {
            *content = ptr::null_mut();
            *length = 0;
        }
        let snapshot = Snapshot::read(file).map_err(|_| Invalid)?;
        let Some(bytes) = snapshot.content() else {
            return Ok(1);
        };
        unsafe {
            *content = owned(bytes)?;
            *length = bytes.len();
        }
        Ok(0)
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_json_like_get_raw_entry(
    file: *const c_char,
    path: *const *const c_char,
    path_len: usize,
    key: *const c_char,
    content: *mut *mut c_char,
    length: *mut usize,
) -> c_int {
    status((|| {
        if content.is_null() || length.is_null() {
            return Err(Invalid);
        }
        // SAFETY: non-null out pointers are writable and inputs follow the ABI.
        unsafe {
            *content = ptr::null_mut();
            *length = 0;
        }
        let location = unsafe { Location::new(file, path, path_len, key)? };
        let snapshot = location.load()?;
        let Some(range) = location.get(&snapshot)? else {
            return Ok(1);
        };
        let bytes = &snapshot.content().ok_or(Invalid)?[range];
        unsafe {
            *content = owned(bytes)?;
            *length = bytes.len();
        }
        Ok(0)
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_json_like_get_string_at_path(
    file: *const c_char,
    path: *const *const c_char,
    path_len: usize,
    key: *const c_char,
    content: *mut *mut c_char,
) -> c_int {
    status((|| {
        if content.is_null() {
            return Err(Invalid);
        }
        // SAFETY: content is writable and inputs follow the ABI.
        unsafe { *content = ptr::null_mut() };
        let location = unsafe { Location::new(file, path, path_len, key)? };
        let snapshot = location.load()?;
        let Some(range) = location.get(&snapshot)? else {
            return Ok(1);
        };
        let value = decoded_string(&snapshot.content().ok_or(Invalid)?[range])?;
        unsafe { *content = owned(&value)? };
        Ok(0)
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_json_like_upsert_entry(
    file: *const c_char,
    path: *const *const c_char,
    path_len: usize,
    key: *const c_char,
    raw: *const c_char,
) -> c_int {
    status((|| {
        // SAFETY: the caller supplies C strings and the path array.
        let location = unsafe { Location::new(file, path, path_len, key)? };
        let raw = validate_entry(unsafe { text(raw)? }.to_bytes())?;
        location.edit(Edit::Upsert(raw), Expected::Any)
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_json_like_upsert_entry_if_unchanged(
    file: *const c_char,
    path: *const *const c_char,
    path_len: usize,
    key: *const c_char,
    raw: *const c_char,
    expected: *const c_char,
    expected_len: usize,
) -> c_int {
    status((|| {
        // SAFETY: the caller supplies C strings and the path array.
        let location = unsafe { Location::new(file, path, path_len, key)? };
        let raw = validate_entry(unsafe { text(raw)? }.to_bytes())?;
        location.edit(Edit::Upsert(raw), Expected::Exact(expected, expected_len))
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_json_like_remove_entry(
    file: *const c_char,
    path: *const *const c_char,
    path_len: usize,
    key: *const c_char,
) -> c_int {
    status((|| {
        // SAFETY: the caller supplies C strings and the path array.
        unsafe { Location::new(file, path, path_len, key)? }
            .edit(Edit::Remove, Expected::Any)
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_json_like_remove_entry_if_unchanged(
    file: *const c_char,
    path: *const *const c_char,
    path_len: usize,
    key: *const c_char,
    expected: *const c_char,
    expected_len: usize,
) -> c_int {
    status((|| {
        // SAFETY: the caller supplies C strings and the path array.
        unsafe { Location::new(file, path, path_len, key)? }
            .edit(Edit::Remove, Expected::Exact(expected, expected_len))
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_json_like_replace_field_raw_if_unchanged(
    file: *const c_char,
    path: *const *const c_char,
    path_len: usize,
    key: *const c_char,
    field: *const c_char,
    raw: *const c_char,
    expected: *const c_char,
    expected_len: usize,
) -> c_int {
    status((|| {
        // SAFETY: the caller supplies C strings and the path array.
        let location = unsafe { Location::new(file, path, path_len, key)? };
        let key = unsafe { text(field)? }.to_bytes();
        if key.is_empty() {
            return Err(Invalid);
        }
        let raw = validate_entry(unsafe { text(raw)? }.to_bytes())?;
        location.edit(
            Edit::ReplaceField { key, raw },
            Expected::Exact(expected, expected_len),
        )
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_json_like_add_unique_string_at_path(
    file: *const c_char,
    path: *const *const c_char,
    path_len: usize,
    key: *const c_char,
    value: *const c_char,
) -> c_int {
    status((|| {
        // SAFETY: the caller supplies C strings and the path array.
        let location = unsafe { Location::new(file, path, path_len, key)? };
        let value = valid_string(unsafe { text(value)? }.to_bytes())?;
        location.edit(Edit::AddString(value), Expected::Any)
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_json_like_remove_string_at_path(
    file: *const c_char,
    path: *const *const c_char,
    path_len: usize,
    key: *const c_char,
    value: *const c_char,
) -> c_int {
    status((|| {
        // SAFETY: the caller supplies C strings and the path array.
        let location = unsafe { Location::new(file, path, path_len, key)? };
        let value = valid_string(unsafe { text(value)? }.to_bytes())?;
        location.edit(Edit::RemoveString(value), Expected::Any)
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_json_like_add_unique_string(
    file: *const c_char,
    key: *const c_char,
    value: *const c_char,
) -> c_int {
    // SAFETY: forwards the caller's C strings with a non-null empty path.
    unsafe {
        cbm_json_like_add_unique_string_at_path(
            file,
            [ptr::null()].as_ptr(),
            0,
            key,
            value,
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_json_like_remove_string(
    file: *const c_char,
    key: *const c_char,
    value: *const c_char,
) -> c_int {
    // SAFETY: forwards the caller's C strings with a non-null empty path.
    unsafe {
        cbm_json_like_remove_string_at_path(
            file,
            [ptr::null()].as_ptr(),
            0,
            key,
            value,
        )
    }
}

#[repr(C)]
pub struct ObjectField {
    key: *const c_char,
    shape: c_uint,
    expected: *const c_char,
    flags: c_uint,
}

const STRING: c_uint = 0;
const EMPTY_ARRAY: c_uint = 1;
const SINGLE_STRING_ARRAY: c_uint = 2;
const LITERAL: c_uint = 3;
const REQUIRED: c_uint = 1;
const CAPTURE: c_uint = 2;

const MATCH: c_int = 0;
const MISSING: c_int = 1;
const MISMATCH: c_int = 2;
const MATCH_WITH_EXTRAS: c_int = 3;

struct Field<'a> {
    key: &'a [u8],
    shape: c_uint,
    expected: Option<&'a [u8]>,
    flags: c_uint,
}

fn nonempty(bytes: &[u8]) -> Result<&[u8]> {
    (!bytes.is_empty()).then_some(bytes).ok_or(Invalid)
}

unsafe fn fields<'a>(raw: &[ObjectField]) -> Result<Vec<Field<'a>>> {
    let mut fields: Vec<Field<'_>> = Vec::new();
    let mut captures = 0;
    for field in raw {
        // SAFETY: non-null field strings follow the ABI contract.
        let key = nonempty(unsafe { text(field.key)? }.to_bytes())?;
        let expected = if field.expected.is_null() {
            None
        } else {
            Some(unsafe { text(field.expected)? }.to_bytes())
        };
        let capture = field.flags & CAPTURE != 0;
        if field.shape > LITERAL
            || field.flags & !(REQUIRED | CAPTURE) != 0
            || (capture && matches!(field.shape, EMPTY_ARRAY | LITERAL))
            || (field.shape == LITERAL && expected.is_none())
            || fields.iter().any(|prior| prior.key == key)
        {
            return Err(Invalid);
        }
        captures += usize::from(capture);
        fields.push(Field {
            key,
            shape: field.shape,
            expected,
            flags: field.flags,
        });
    }
    (captures == 1).then_some(fields).ok_or(Invalid)
}

fn field_value(raw: &[u8], shape: c_uint) -> Result<Option<Vec<u8>>> {
    match shape {
        STRING => decoded_string(raw).map(Some),
        EMPTY_ARRAY if scan(raw, 0, b'[', None)?.count == 0 => Ok(None),
        SINGLE_STRING_ARRAY => {
            let array = scan(raw, 0, b'[', None)?;
            if array.count != 1 {
                return Err(Invalid);
            }
            decoded_string(&raw[array.first.ok_or(Invalid)?..array.last_end])
                .map(Some)
        }
        // C compares the copied token through strcmp.
        LITERAL => Ok(Some(
            raw.split(|&byte| byte == 0)
                .next()
                .unwrap_or_default()
                .to_vec(),
        )),
        _ => Err(Invalid),
    }
}

fn match_object(
    document: &[u8],
    path: &[&[u8]],
    entry: &[u8],
    fields: &[Field<'_>],
) -> Result<(c_int, Option<Vec<u8>>)> {
    if document.is_empty() {
        return Ok((MISSING, None));
    }

    let mut start = validate_document(document)?;
    for key in path.iter().copied().chain([entry]) {
        let object = scan(document, start, b'{', Some(key))?;
        let Some(hit) = object.hit else {
            return Ok((MISSING, None));
        };
        start = hit.value.start;
        if object.matches != 1 || document[start] != b'{' {
            return Ok((MISMATCH, None));
        }
    }

    let mut capture = None;
    let mut found = 0;
    let mut members = 0;
    for field in fields {
        let object = scan(document, start, b'{', Some(field.key))?;
        members = object.count;
        if object.matches > 1
            || (object.matches == 0 && field.flags & REQUIRED != 0)
        {
            return Ok((MISMATCH, None));
        }
        let Some(hit) = object.hit else { continue };
        found += 1;
        let Ok(value) = field_value(&document[hit.value], field.shape) else {
            return Ok((MISMATCH, None));
        };
        if field.expected.is_some() && field.expected != value.as_deref() {
            return Ok((MISMATCH, None));
        }
        if field.flags & CAPTURE != 0 {
            capture = value;
        }
    }
    Ok(match capture {
        None => (MISMATCH, None),
        Some(value) if members == found => (MATCH, Some(value)),
        Some(value) => (MATCH_WITH_EXTRAS, Some(value)),
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_json_like_match_object_entry(
    document: *const c_char,
    length: usize,
    path: *const *const c_char,
    path_len: usize,
    key: *const c_char,
    raw_fields: *const ObjectField,
    field_count: usize,
    captured: *mut *mut c_char,
) -> c_int {
    status((|| {
        if captured.is_null() {
            return Err(Invalid);
        }
        // SAFETY: captured is writable and the inputs follow the ABI.
        unsafe { *captured = ptr::null_mut() };
        if document.is_null() || length > MAX_BYTES || field_count == 0 {
            return Err(Invalid);
        }
        let key = nonempty(unsafe { text(key)? }.to_bytes())?;
        let path = unsafe { input_slice(path, path_len)? }
            .iter()
            .map(|&component| nonempty(unsafe { text(component)? }.to_bytes()))
            .collect::<Result<Vec<_>>>()?;
        let fields = unsafe { fields(input_slice(raw_fields, field_count)?)? };
        let document = unsafe { input_slice(document.cast(), length)? };
        let (result, value) = match_object(document, &path, key, &fields)?;
        if let Some(value) = value {
            unsafe { *captured = owned(&value)? };
        }
        Ok(result)
    })())
}

#[cfg(feature = "native-json-editor-fixtures")]
mod hooks {
    use super::*;
    use std::cell::Cell;
    use std::thread::LocalKey;

    type Callback = unsafe extern "C" fn(*const c_char, *mut c_void);
    type Hook = Cell<(Option<Callback>, *mut c_void)>;

    thread_local! {
        static PRECOMMIT: Hook = const { Cell::new((None, ptr::null_mut())) };
        static PREPUBLISH: Hook = const { Cell::new((None, ptr::null_mut())) };
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn cbm_json_like_set_precommit_hook_for_testing(
        hook: Option<Callback>,
        context: *mut c_void,
    ) {
        PRECOMMIT.set((hook, context));
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn cbm_json_like_set_prepublish_hook_for_testing(
        hook: Option<Callback>,
        context: *mut c_void,
    ) {
        PREPUBLISH.set((hook, context));
    }

    fn run(slot: &'static LocalKey<Hook>, path: &CStr) {
        let (hook, context) = slot.get();
        if let Some(hook) = hook {
            // SAFETY: the fixture registered the callback and owns its context.
            unsafe { hook(path.as_ptr(), context) };
        }
    }

    pub(in crate::config_json_like) fn run_precommit(path: &CStr) {
        run(&PRECOMMIT, path);
    }

    pub(in crate::config_json_like) fn run_prepublish(path: &CStr) {
        run(&PREPUBLISH, path);
    }
}

#[cfg(feature = "native-json-editor-fixtures")]
pub(super) use hooks::{run_precommit, run_prepublish};
