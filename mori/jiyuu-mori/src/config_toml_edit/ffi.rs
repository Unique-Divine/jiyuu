//! Entry points declared by cli/config_toml_edit.h.

use std::borrow::Cow;
use std::ffi::{CStr, c_char, c_int};
use std::path::Path;
use std::ptr;
use std::slice;

use super::codex::{self, Failure};
use super::config_file::ConfigFile;
use super::managed::{self, Markers};
use super::named::{self, Operation};
use super::syntax::{bare, parse_key, text_is_safe};
use super::{Edit, Error, MAX_BYTES, Result, escape_basic_with, legacy};

const SHORT_INPUT: usize = 4096;
const MAX_PATH: usize = 32768;

#[derive(Clone, Copy)]
struct Arg(*const c_char);

impl Arg {
    // Each FFI caller must supply readable NUL-terminated inputs for the
    // duration of the call. Stop scanning as soon as the limit is exceeded.
    unsafe fn bytes<'a>(self, max: usize) -> Result<&'a [u8]> {
        if self.0.is_null() {
            return Err(Error);
        }
        for len in 0..=max {
            // SAFETY: shorter prefixes were readable and contained no NUL.
            if unsafe { *self.0.add(len) } == 0 {
                // SAFETY: the scan established the readable length.
                return Ok(unsafe { slice::from_raw_parts(self.0.cast(), len) });
            }
        }
        Err(Error)
    }

    unsafe fn text<'a>(self, max: usize, multiline: bool) -> Result<&'a [u8]> {
        // SAFETY: the ABI caller supplies a C string or NULL.
        let bytes = unsafe { self.bytes(max)? };
        if text_is_safe(bytes, multiline) {
            Ok(bytes)
        } else {
            Err(Error)
        }
    }

    unsafe fn path<'a>(self) -> Result<&'a Path> {
        // SAFETY: the ABI caller supplies a C string or NULL.
        let bytes = unsafe { self.text(MAX_PATH, false)? };
        if bytes.is_empty() {
            return Err(Error);
        }
        Ok(Path::new(std::str::from_utf8(bytes).map_err(|_| Error)?))
    }

    unsafe fn identifier<'a>(self) -> Result<&'a [u8]> {
        // SAFETY: the ABI caller supplies a C string or NULL.
        let bytes = unsafe { self.bytes(SHORT_INPUT)? };
        if !bytes.is_empty() && bytes.iter().copied().all(bare) {
            Ok(bytes)
        } else {
            Err(Error)
        }
    }
}

unsafe fn markers<'a>(begin: Arg, end: Arg) -> Result<Markers<'a>> {
    // SAFETY: the ABI caller supplies C strings or NULL.
    let (begin, end) = unsafe {
        (
            begin.text(SHORT_INPUT, false)?,
            end.text(SHORT_INPUT, false)?,
        )
    };
    if begin.is_empty() || end.is_empty() || begin == end {
        return Err(Error);
    }
    Ok(Markers { begin, end })
}

fn read(path: &Path) -> Result<ConfigFile<'_>> {
    let file = ConfigFile::read(path).map_err(|_| Error)?;
    if !text_is_safe(file.bytes(), true) {
        return Err(Error);
    }
    Ok(file)
}

fn apply(file: &ConfigFile<'_>, edit: Edit) -> Result<c_int> {
    match edit {
        Edit::Unchanged => Ok(0),
        Edit::Foreign => Ok(1),
        Edit::Write(out) => {
            file.write(&out).map_err(|_| Error)?;
            Ok(0)
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_toml_escape_basic_string(
    input: *const c_char,
    out: *mut c_char,
    out_size: usize,
) -> c_int {
    if input.is_null() || out.is_null() || out_size == 0 {
        return -1;
    }
    // SAFETY: the caller provides a writable output buffer.
    unsafe { out.write(0) };
    // SAFETY: the caller provides a readable C string.
    let Ok(bytes) = (unsafe { Arg(input).bytes(MAX_BYTES) }) else {
        return -1;
    };
    let input_address = input.addr();
    let out_address = out.addr();
    let input = if input_address < out_address.saturating_add(out_size)
        && out_address < input_address.saturating_add(bytes.len())
    {
        Cow::Owned(bytes.to_vec())
    } else {
        Cow::Borrowed(bytes)
    };
    let result = escape_basic_with(&input, out_size, |offset, bytes| {
        // SAFETY: escaping checks the output bounds before copying. The input
        // is disjoint from the writable range or was copied above.
        unsafe {
            ptr::copy_nonoverlapping(
                bytes.as_ptr().cast(),
                out.add(offset),
                bytes.len(),
            )
        };
    });
    if result.is_ok() { 0 } else { -1 }
}

unsafe fn managed_edit(
    path: Arg,
    begin: Arg,
    end: Arg,
    body: Option<Arg>,
    install: bool,
) -> Result<c_int> {
    // SAFETY: the ABI caller supplies C strings or NULL.
    let (path, markers, body) = unsafe {
        (
            path.path()?,
            markers(begin, end)?,
            body.map(|body| body.text(MAX_BYTES, true)).transpose()?,
        )
    };
    if let Some(body) = body {
        markers.validate_body(body)?;
    }
    let file = read(path)?;
    let out = if install {
        Some(managed::upsert(file.bytes(), markers, body.ok_or(Error)?)?)
    } else {
        managed::remove(file.bytes(), markers, body)?
    };
    apply(&file, out.map_or(Edit::Unchanged, Edit::Write))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_toml_upsert_managed_block(
    path: *const c_char,
    begin: *const c_char,
    end: *const c_char,
    block: *const c_char,
) -> c_int {
    // SAFETY: forwarded under the C ABI input contract.
    unsafe {
        managed_edit(Arg(path), Arg(begin), Arg(end), Some(Arg(block)), true)
    }
    .unwrap_or(-1)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_toml_remove_managed_block(
    path: *const c_char,
    begin: *const c_char,
    end: *const c_char,
) -> c_int {
    // SAFETY: forwarded under the C ABI input contract.
    unsafe { managed_edit(Arg(path), Arg(begin), Arg(end), None, false) }
        .unwrap_or(-1)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_toml_remove_managed_block_owned(
    path: *const c_char,
    begin: *const c_char,
    end: *const c_char,
    owned: *const c_char,
) -> c_int {
    // SAFETY: forwarded under the C ABI input contract.
    unsafe {
        managed_edit(Arg(path), Arg(begin), Arg(end), Some(Arg(owned)), false)
    }
    .unwrap_or(-1)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_toml_remove_legacy_table(
    path: *const c_char,
    table: *const c_char,
    begin: *const c_char,
    end: *const c_char,
) -> c_int {
    let result = || {
        if table.is_null() {
            return Err(Error);
        }
        // SAFETY: the ABI caller supplies readable C strings.
        let (path, markers, table) = unsafe {
            (
                Arg(path).path()?,
                markers(Arg(begin), Arg(end))?,
                CStr::from_ptr(table).to_bytes(),
            )
        };
        let key = parse_key(table)?;
        let file = read(path)?;
        apply(&file, legacy::remove(file.bytes(), &key, markers)?)
    };
    result().unwrap_or(-1)
}

unsafe fn named_edit(
    path: Arg,
    table: Arg,
    identity_key: Arg,
    identity: Arg,
    operation: Operation<'_>,
) -> Result<c_int> {
    // SAFETY: the ABI caller supplies C strings or NULL.
    let (path, table, identity_key, identity) = unsafe {
        (
            path.path()?,
            table.identifier()?,
            identity_key.identifier()?,
            identity.text(SHORT_INPUT, false)?,
        )
    };
    let entries = named::validate_body(&operation, identity_key, identity)?;
    let file = read(path)?;
    apply(
        &file,
        named::edit(
            file.bytes(),
            table,
            identity_key,
            identity,
            operation,
            entries,
        )?,
    )
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_toml_upsert_named_array_table(
    path: *const c_char,
    table: *const c_char,
    key: *const c_char,
    value: *const c_char,
    body: *const c_char,
) -> c_int {
    // SAFETY: forwarded under the C ABI input contract.
    unsafe {
        Arg(body).text(MAX_BYTES, true).and_then(|body| {
            named_edit(
                Arg(path),
                Arg(table),
                Arg(key),
                Arg(value),
                Operation::Merge(body),
            )
        })
    }
    .unwrap_or(-1)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_toml_remove_named_array_table(
    path: *const c_char,
    table: *const c_char,
    key: *const c_char,
    value: *const c_char,
) -> c_int {
    // SAFETY: forwarded under the C ABI input contract.
    unsafe {
        named_edit(
            Arg(path),
            Arg(table),
            Arg(key),
            Arg(value),
            Operation::Remove,
        )
    }
    .unwrap_or(-1)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_toml_upsert_owned_named_array_table(
    path: *const c_char,
    table: *const c_char,
    key: *const c_char,
    value: *const c_char,
    body: *const c_char,
) -> c_int {
    // SAFETY: forwarded under the C ABI input contract.
    unsafe {
        Arg(body).text(MAX_BYTES, true).and_then(|body| {
            named_edit(
                Arg(path),
                Arg(table),
                Arg(key),
                Arg(value),
                Operation::InstallExact(body),
            )
        })
    }
    .unwrap_or(-1)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_toml_remove_owned_named_array_table(
    path: *const c_char,
    table: *const c_char,
    key: *const c_char,
    value: *const c_char,
    body: *const c_char,
) -> c_int {
    // SAFETY: forwarded under the C ABI input contract.
    unsafe {
        Arg(body).text(MAX_BYTES, true).and_then(|body| {
            named_edit(
                Arg(path),
                Arg(table),
                Arg(key),
                Arg(value),
                Operation::RemoveExact(body),
            )
        })
    }
    .unwrap_or(-1)
}

#[unsafe(no_mangle)]
pub extern "C" fn cbm_toml_codex_hook_failure_name(
    failure: c_int,
) -> *const c_char {
    codex::failure_name(failure).as_ptr()
}

unsafe fn reconcile_codex(
    path: Arg,
    begin: Arg,
    end: Arg,
    command: Arg,
    windows: Arg,
    action: c_int,
    check_only: bool,
) -> std::result::Result<bool, Failure> {
    if !matches!(action, 0 | 1) || command.0.is_null() || windows.0.is_null() {
        return Err(Failure::InvalidArgument);
    }
    let argument = |_| Failure::InvalidArgument;
    let render = |_| Failure::CommandRender;
    // SAFETY: the ABI caller supplies C strings or NULL.
    let (path, markers, command, windows) = unsafe {
        (
            path.path().map_err(argument)?,
            markers(begin, end).map_err(argument)?,
            command.bytes(MAX_BYTES).map_err(render)?,
            windows.bytes(MAX_BYTES).map_err(render)?,
        )
    };
    let block = codex::build_block(command, windows, true).map_err(render)?;
    let file = ConfigFile::read(path).map_err(|_| Failure::ConfigRead)?;
    if !text_is_safe(file.bytes(), true) {
        return Err(Failure::UnsafeContent);
    }
    let (out, pretool) = codex::reconcile(
        file.bytes(),
        markers,
        command,
        windows,
        block,
        action == 0,
    )?;
    if !check_only {
        file.write(&out).map_err(|_| Failure::ConfigWrite)?;
    }
    Ok(pretool)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_toml_reconcile_codex_hooks_detailed(
    path: *const c_char,
    begin: *const c_char,
    end: *const c_char,
    command: *const c_char,
    windows: *const c_char,
    action: c_int,
    check_only: c_int,
    failure: *mut c_int,
    pretool_written: *mut c_int,
) -> c_int {
    // SAFETY: non-NULL output pointers refer to writable C integers.
    unsafe {
        if !failure.is_null() {
            failure.write(0);
        }
        if !pretool_written.is_null() {
            pretool_written.write(0);
        }
    }
    // SAFETY: forwarded under the C ABI input contract.
    let result = unsafe {
        reconcile_codex(
            Arg(path),
            Arg(begin),
            Arg(end),
            Arg(command),
            Arg(windows),
            action,
            check_only != 0,
        )
    };
    match result {
        Ok(written) => {
            if !pretool_written.is_null() {
                // SAFETY: the caller supplies a writable C integer.
                unsafe { pretool_written.write(c_int::from(written)) };
            }
            0
        }
        Err(reason) => {
            if !failure.is_null() {
                // SAFETY: the caller supplies a writable C integer.
                unsafe { failure.write(reason as c_int) };
            }
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_toml_reconcile_codex_hooks(
    path: *const c_char,
    begin: *const c_char,
    end: *const c_char,
    command: *const c_char,
    windows: *const c_char,
    action: c_int,
    check_only: c_int,
) -> c_int {
    // SAFETY: forwarded under the C ABI input contract.
    unsafe {
        cbm_toml_reconcile_codex_hooks_detailed(
            path,
            begin,
            end,
            command,
            windows,
            action,
            check_only,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    }
}
