//! Resolve an expanded MSBuild import against the evaluator's indexed files.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::{ptr, slice};

const PATH_MARK: u8 = 1;
const NO_TARGET: c_int = -1;
const OUTSIDE: c_int = -2;
const UNEVALUABLE: c_int = -3;

type ScratchAlloc = unsafe extern "C" fn(*mut c_void, usize) -> *mut c_char;
type FindFile = unsafe extern "C" fn(*mut c_void, *const c_char) -> c_int;

fn space(byte: u8) -> bool {
    byte.is_ascii_whitespace() || byte == b'\x0b'
}

fn import_path(value: &[u8]) -> Result<(&[u8], bool), c_int> {
    // Match the native reader's C whitespace, including vertical tabs.
    let start = value
        .iter()
        .position(|&byte| !space(byte))
        .unwrap_or(value.len());
    let end = value
        .iter()
        .rposition(|&byte| !space(byte))
        .map_or(start, |i| i + 1);
    let value = &value[start..end];
    let marked = value.first() == Some(&PATH_MARK);
    let path = &value[usize::from(marked)..];
    if path.is_empty()
        || path
            .iter()
            .any(|&byte| matches!(byte, PATH_MARK | b'*' | b'?'))
    {
        return Err(UNEVALUABLE);
    }
    if !marked && (matches!(path[0], b'/' | b'\\') || path.get(1) == Some(&b':'))
    {
        return Err(OUTSIDE);
    }
    Ok((path, marked))
}

// Output segments carry a trailing slash until the final NUL replaces it.
// The write position never overtakes the input, including when segments overlap.
fn normalize(path: &mut [u8], work: &mut usize) -> bool {
    for byte in path.iter_mut() {
        if *byte == b'\\' {
            *byte = b'/';
        }
    }

    let len = path.len() - 1; // exclude NUL
    let mut read = 0;
    let mut written = 0;
    while read < len {
        let start = read;
        while read < len && path[read] != b'/' {
            read += 1;
        }
        let end = read;
        read += usize::from(read < len);
        match &path[start..end] {
            b"" | b"." => {}
            b".." => {
                if written == 0 {
                    return false;
                }
                written -= 1;
                while written > 0 && path[written - 1] != b'/' {
                    written -= 1;
                }
            }
            segment => {
                let bytes = segment.len();
                path.copy_within(start..end, written);
                written += bytes;
                path[written] = b'/';
                written += 1;
                *work += bytes + 1;
            }
        }
    }
    path[written.saturating_sub(1)] = 0;
    true
}

/// The expanded value is null for an unknown value. Otherwise it and the
/// directory are NUL-terminated strings that remain readable for this call.
/// The host allocates disjoint scratch storage, and the index callback borrows
/// the resulting path only for its call. `work` receives the bytes copied.
#[unsafe(no_mangle)]
unsafe extern "C" fn mori_msbuild_import_target(
    value: *const c_char,
    directory: *const c_char,
    context: *mut c_void,
    alloc: ScratchAlloc,
    find: FindFile,
    work: &mut usize,
) -> c_int {
    *work = 0;
    if value.is_null() {
        return UNEVALUABLE;
    }
    // SAFETY: The host keeps the expansion alive through allocation and lookup.
    let value = unsafe { CStr::from_ptr(value) }.to_bytes();
    let (value, marked) = match import_path(value) {
        Ok(path) => path,
        Err(status) => return status,
    };
    let directory = if marked {
        &[][..]
    } else {
        // SAFETY: The directory belongs to the importing file.
        unsafe { CStr::from_ptr(directory) }.to_bytes()
    };

    let len = directory.len() + value.len() + 2;
    // Preserve the original scratch reservation, which included the marker.
    // SAFETY: The native context is live for both callbacks.
    let buffer =
        unsafe { alloc(context, len + usize::from(marked)) }.cast::<u8>();
    if buffer.is_null() {
        return NO_TARGET;
    }
    // SAFETY: The callback returned len writable bytes. Initialize all of them
    // before borrowing the buffer as a slice.
    let path = unsafe {
        ptr::copy_nonoverlapping(directory.as_ptr(), buffer, directory.len());
        buffer.add(directory.len()).write(b'/');
        ptr::copy_nonoverlapping(
            value.as_ptr(),
            buffer.add(directory.len() + 1),
            value.len(),
        );
        buffer.add(len - 1).write(0);
        slice::from_raw_parts_mut(buffer, len)
    };
    *work = len;
    if !normalize(path, work) {
        return OUTSIDE;
    }
    // SAFETY: The normalized path ends in NUL and remains readable for lookup.
    let target = unsafe { find(context, buffer.cast()) };
    if target < 0 { OUTSIDE } else { target }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_path_classifies_markers_and_globs_before_absolute_paths() {
        assert_eq!(
            import_path(b" \x0b\x01/a\\Mixed.props\r\n"),
            Ok((&b"/a\\Mixed.props"[..], true))
        );
        for path in [
            b" ".as_slice(),
            b"\x01",
            b"a\x01.props",
            b"C:/*.props",
            b"/?.props",
        ] {
            assert_eq!(import_path(path), Err(UNEVALUABLE));
        }
        for path in [b"/a".as_slice(), b"\\a", b"x:a"] {
            assert_eq!(import_path(path), Err(OUTSIDE));
        }
    }

    #[test]
    fn normalization_preserves_path_bytes_and_counts_popped_segments() {
        let mut path = *b"src\\nested//.././\xff/Mixed.props/\0";
        let mut work = 0;
        assert!(normalize(&mut path, &mut work));
        assert_eq!(
            CStr::from_bytes_until_nul(&path).unwrap().to_bytes(),
            b"src/\xff/Mixed.props"
        );
        assert_eq!(work, b"src/nested/\xff/Mixed.props/".len());

        let mut path = *b"/a/../..\0";
        let mut work = 0;
        assert!(!normalize(&mut path, &mut work));
        assert_eq!(work, b"a/".len());
    }
}
