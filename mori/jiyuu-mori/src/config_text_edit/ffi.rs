//! The exported functions match cli/config_text_edit.h.

use std::ffi::{CStr, c_char, c_int, c_uint};
use std::io;
use std::path::Path;

use super::{MAX_BYTES, Markers, Ownership, Write, invalid, single_line};

const MAX_PATH_BYTES: usize = 32768;

// SAFETY: the caller supplies a readable NUL-terminated string.
unsafe fn bytes<'a>(value: *const c_char) -> io::Result<&'a [u8]> {
    if value.is_null() {
        return Err(invalid("missing string argument"));
    }
    Ok(unsafe { CStr::from_ptr(value) }.to_bytes())
}

unsafe fn path<'a>(value: *const c_char) -> io::Result<&'a Path> {
    let value = unsafe { bytes(value) }?;
    Ok(Path::new(single_line(value, MAX_PATH_BYTES)?))
}

unsafe fn markers<'a>(
    begin: *const c_char,
    end: *const c_char,
) -> io::Result<Markers<'a>> {
    unsafe { Markers::new(bytes(begin)?, bytes(end)?) }
}

unsafe fn releases<'a>(
    values: *const *const c_char,
    count: usize,
) -> io::Result<Vec<&'a [u8]>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    if values.is_null()
        || count > isize::MAX as usize / std::mem::size_of::<*const c_char>()
    {
        return Err(invalid("invalid release list"));
    }
    // SAFETY: the caller provides count readable pointers to strings.
    unsafe {
        std::slice::from_raw_parts(values, count)
            .iter()
            .map(|&value| bytes(value))
            .collect()
    }
}

fn status(operation: impl FnOnce() -> io::Result<Ownership>) -> c_int {
    match operation() {
        Ok(Ownership::Owned) => 0,
        Ok(Ownership::Foreign) => 1,
        Err(_) => -1,
    }
}

fn edit(operation: impl FnOnce() -> io::Result<()>) -> c_int {
    status(|| operation().map(|()| Ownership::Owned))
}

// The C ABI requires all non-null pointers to be valid for their declared
// lengths, or NUL-terminated where the header omits a length.
#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_text_upsert_managed_block(
    file: *const c_char,
    begin: *const c_char,
    end: *const c_char,
    owned: *const c_char,
) -> c_int {
    unsafe {
        cbm_text_upsert_managed_block_limited(file, begin, end, owned, MAX_BYTES)
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_text_upsert_managed_block_limited(
    file: *const c_char,
    begin: *const c_char,
    end: *const c_char,
    owned: *const c_char,
    limit: usize,
) -> c_int {
    edit(|| unsafe {
        super::upsert(path(file)?, markers(begin, end)?, bytes(owned)?, limit)
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_text_managed_block_status(
    file: *const c_char,
    begin: *const c_char,
    end: *const c_char,
) -> c_int {
    status(|| unsafe { super::block_status(path(file)?, markers(begin, end)?) })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_text_remove_managed_block(
    file: *const c_char,
    begin: *const c_char,
    end: *const c_char,
) -> c_int {
    edit(|| unsafe { super::remove_block(path(file)?, markers(begin, end)?) })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_text_write_owned_document(
    file: *const c_char,
    owned: *const c_char,
) -> c_int {
    edit(|| unsafe {
        super::write_document(path(file)?, bytes(owned)?, Write::Overwrite)
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_text_write_owned_document_if_unchanged(
    file: *const c_char,
    owned: *const c_char,
    expected: *const c_char,
    length: usize,
) -> c_int {
    edit(|| {
        if length > MAX_BYTES || (expected.is_null() && length != 0) {
            return Err(invalid("invalid expected document length"));
        }
        let expected = if expected.is_null() {
            None
        } else {
            // SAFETY: the caller supplies length readable bytes.
            Some(unsafe { std::slice::from_raw_parts(expected.cast(), length) })
        };
        unsafe {
            super::write_document(
                path(file)?,
                bytes(owned)?,
                Write::IfUnchanged(expected),
            )
        }
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_text_create_owned_document(
    file: *const c_char,
    owned: *const c_char,
) -> c_int {
    edit(|| unsafe {
        super::write_document(path(file)?, bytes(owned)?, Write::Create)
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_text_ensure_owned_document(
    file: *const c_char,
    owned: *const c_char,
) -> c_int {
    edit(|| unsafe {
        super::write_document(path(file)?, bytes(owned)?, Write::Ensure)
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_text_owned_document_status(
    file: *const c_char,
    current: *const c_char,
    released: *const *const c_char,
    count: usize,
) -> c_int {
    status(|| unsafe {
        super::document_status(
            path(file)?,
            bytes(current)?,
            &releases(released, count)?,
        )
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_text_migrate_owned_document(
    file: *const c_char,
    current: *const c_char,
    released: *const *const c_char,
    count: usize,
) -> c_int {
    status(|| unsafe {
        super::migrate(
            path(file)?,
            bytes(current)?,
            &releases(released, count)?,
            None,
        )
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_text_migrate_owned_document_mode(
    file: *const c_char,
    current: *const c_char,
    released: *const *const c_char,
    count: usize,
    mode: c_uint,
) -> c_int {
    status(|| unsafe {
        super::migrate(
            path(file)?,
            bytes(current)?,
            &releases(released, count)?,
            Some(mode),
        )
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_text_remove_owned_document(
    file: *const c_char,
    owned: *const c_char,
) -> c_int {
    status(|| unsafe { super::remove_document(path(file)?, bytes(owned)?, &[]) })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_text_remove_owned_document_any(
    file: *const c_char,
    current: *const c_char,
    released: *const *const c_char,
    count: usize,
) -> c_int {
    status(|| unsafe {
        super::remove_document(
            path(file)?,
            bytes(current)?,
            &releases(released, count)?,
        )
    })
}
