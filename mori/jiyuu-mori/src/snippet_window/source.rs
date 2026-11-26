//! Prepare a snippet from the byte buffer that supplied its freshness verdict.

use super::Window;
use std::alloc::{Layout, alloc, dealloc};
use std::ffi::{CStr, c_char, c_int};
use std::{ptr, slice};

fn physical_lines(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    bytes.split_inclusive(|&byte| byte == b'\n')
}

fn captured_window<'a>(bytes: &'a [u8], window: &Window) -> Option<&'a [u8]> {
    let mut begin = 0;
    let mut end = 0;
    for (index, line) in
        physical_lines(bytes).take(window.end as usize).enumerate()
    {
        end += line.len();
        if index < (window.start - 1) as usize {
            begin = end;
        }
    }
    if begin == end {
        return None;
    }
    let source = &bytes[begin..end];
    // The C response has always treated the first NUL as the end of source.
    Some(source.split(|&byte| byte == 0).next().unwrap_or_default())
}

fn for_each_utf8_fragment(
    bytes: &[u8],
    normalize_crlf: bool,
    mut emit: impl FnMut(&[u8]),
) {
    for chunk in bytes.utf8_chunks() {
        let mut valid = chunk.valid().as_bytes();
        if normalize_crlf {
            while let Some(cr) =
                valid.windows(2).position(|pair| pair == b"\r\n")
            {
                emit(&valid[..cr]);
                valid = &valid[cr + 1..];
            }
        }
        emit(valid);
        // Preserve one replacement for each invalid byte, including every
        // byte of an incomplete multibyte sequence.
        for _ in chunk.invalid() {
            emit("\u{fffd}".as_bytes());
        }
    }
}

fn copy_source_text(bytes: &[u8], normalize_crlf: bool) -> *mut c_char {
    let mut length = Some(1_usize); // Include the terminating NUL.
    for_each_utf8_fragment(bytes, normalize_crlf, |part| {
        length = length.and_then(|length| length.checked_add(part.len()));
    });
    let Some(layout) =
        length.and_then(|length| Layout::array::<u8>(length).ok())
    else {
        return ptr::null_mut();
    };
    // SAFETY: The layout contains at least the NUL byte. Unlike String or
    // CString allocation, alloc returns null on failure.
    let output = unsafe { alloc(layout) };
    if output.is_null() {
        return ptr::null_mut();
    }
    let mut cursor = output;
    for_each_utf8_fragment(bytes, normalize_crlf, |part| {
        // SAFETY: The same traversal measured this allocation. Input slices
        // cannot overlap the newly allocated output.
        unsafe {
            ptr::copy_nonoverlapping(part.as_ptr(), cursor, part.len());
            cursor = cursor.add(part.len());
        }
    });
    // SAFETY: The last byte was reserved for the NUL.
    unsafe { cursor.write(0) };
    #[cfg(feature = "native-engine-abi")]
    // SAFETY: The entire result, including its terminator, is initialized.
    unsafe {
        crate::ffi_output::written(output, layout.size());
    }
    output.cast()
}

/// Extract source from a captured file, without opening a path. A null result
/// means the window has no source, including when content drift is confirmed.
///
/// # Safety
/// The caller must provide an initialized window from the planner. If length
/// is nonzero, bytes must point to that many readable bytes in one allocation
/// (at most isize::MAX bytes) for the duration of the call.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mori_snippet_captured_source(
    window: *const Window,
    source_drift: bool,
    bytes: *const u8,
    len: usize,
) -> *mut c_char {
    // SAFETY: The caller borrows an initialized window for this call.
    let window = unsafe { &*window };
    if source_drift || window.outline || len == 0 {
        return ptr::null_mut();
    }
    // SAFETY: A nonempty input remains readable for the duration of the call.
    let bytes = unsafe { slice::from_raw_parts(bytes, len) };
    let Some(source) = captured_window(bytes, window) else {
        return ptr::null_mut();
    };
    copy_source_text(source, cfg!(windows))
}

/// Repair UTF-8 in already-selected source, such as PDF text or a window
/// read without a captured file.
///
/// # Safety
/// A non-null source must be a readable C string for this call.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mori_snippet_source_text(
    source: *const c_char,
) -> *mut c_char {
    if source.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: The caller borrows a terminated string for this call.
    let source = unsafe { CStr::from_ptr(source) };
    copy_source_text(source.to_bytes(), false)
}

/// # Safety
/// A non-null source must come from one of the two snippet source functions.
/// Release it once, without modifying its terminating NUL or adding a NUL.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mori_snippet_free_source(source: *mut c_char) {
    if !source.is_null() {
        // SAFETY: The original allocation is still live and the location of
        // its terminator is unchanged, so its byte slice recovers the layout.
        unsafe {
            let layout =
                Layout::for_value(CStr::from_ptr(source).to_bytes_with_nul());
            dealloc(source.cast(), layout);
        }
    }
}

/// Count physical lines, including a final line without a newline.
///
/// # Safety
/// A non-null source must be a readable C string for this call.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mori_snippet_source_line_count(
    source: *const c_char,
) -> c_int {
    if source.is_null() {
        return 0;
    }
    // SAFETY: The caller borrows a terminated string for this call.
    let bytes = unsafe { CStr::from_ptr(source) }.to_bytes();
    c_int::try_from(physical_lines(bytes).count()).unwrap_or(c_int::MAX)
}

/// Return the byte length of a prefix of complete physical lines.
///
/// # Safety
/// If len is nonzero, source must point to that many readable bytes in one
/// allocation (at most isize::MAX bytes), excluding the terminating NUL.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mori_snippet_source_prefix_len(
    source: *const u8,
    len: usize,
    lines: c_int,
) -> usize {
    if len == 0 || lines <= 0 {
        return 0;
    }
    // SAFETY: A nonempty input remains readable for the duration of the call.
    let bytes = unsafe { slice::from_raw_parts(source, len) };
    physical_lines(bytes)
        .take(lines as usize)
        .map(<[u8]>::len)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::super::{Request, plan};
    use super::*;

    fn window(start: c_int, lines: c_int) -> Window {
        plan(&Request {
            node_start: 1,
            node_end: 4,
            requested_start: start,
            max_lines: lines,
            max_output_tokens: 128,
            explicit_full: true,
            explicit_outline: false,
            container: false,
        })
    }

    fn take_source(source: *mut c_char) -> Option<String> {
        if source.is_null() {
            return None;
        }
        // SAFETY: The source functions return a live UTF-8 C string.
        let text = unsafe { CStr::from_ptr(source) }
            .to_str()
            .unwrap()
            .to_owned();
        // SAFETY: The original pointer has not been released or modified.
        unsafe { mori_snippet_free_source(source) };
        Some(text)
    }

    fn capture(window: &Window, drift: bool, bytes: &[u8]) -> Option<String> {
        // SAFETY: The window comes from the planner and the slice is readable.
        take_source(unsafe {
            mori_snippet_captured_source(
                window,
                drift,
                bytes.as_ptr(),
                bytes.len(),
            )
        })
    }

    #[test]
    fn captured_pages_keep_physical_lines_and_repair_each_invalid_byte() {
        let bytes = b"first\r\n\xe2\x82\r\n\xf0\x9f\x98\x80\nlast";
        let page = window(1, 2);
        assert!(page.clipped);
        let source = capture(&page, false, bytes).unwrap();
        let expected = if cfg!(windows) {
            "first\n\u{fffd}\u{fffd}\n"
        } else {
            "first\r\n\u{fffd}\u{fffd}\r\n"
        };
        assert_eq!(source, expected);
        assert_eq!(
            capture(&window(page.end + 1, 2), false, bytes).as_deref(),
            Some("😀\nlast"),
        );
    }

    #[test]
    fn verified_capture_and_drift_are_distinct() {
        // Model the old capture surviving a write between freshness and slicing.
        let verified = b"before\nfn indexed() {}\nafter\n".to_vec();
        let rewritten = b"before\nfn intruder() {}\nafter\n";
        let window = window(2, 1);
        assert_eq!(
            capture(&window, false, &verified).as_deref(),
            Some("fn indexed() {}\n"),
        );
        assert!(capture(&window, true, rewritten).is_none());
    }

    #[test]
    fn empty_window_and_window_beginning_with_nul_are_distinct() {
        assert!(capture(&window(2, 1), false, b"one\n").is_none());
        assert!(capture(&window(1, 1), false, b"").is_none());
        assert_eq!(
            capture(&window(2, 1), false, b"one\n\0tail\n").as_deref(),
            Some(""),
        );
    }

    #[test]
    fn output_budget_prefixes_keep_complete_lines() {
        let source = c"\xe2\x82\nlast";
        // SAFETY: All inputs are live NUL-terminated strings.
        unsafe {
            let safe = mori_snippet_source_text(source.as_ptr());
            assert_eq!(mori_snippet_source_line_count(safe), 2);
            assert_eq!(mori_snippet_source_line_count(c"".as_ptr()), 0);
            assert_eq!(mori_snippet_source_line_count(c"\n".as_ptr()), 1);
            let len = CStr::from_ptr(safe).to_bytes().len();
            let prefix_len = mori_snippet_source_prefix_len(safe.cast(), len, 1);
            let text = take_source(safe).unwrap();
            assert_eq!(&text[..prefix_len], "\u{fffd}\u{fffd}\n");
            assert_eq!(&text[prefix_len..], "last");
        }
    }

    #[test]
    fn source_copy_normalizes_only_crlf_pairs_when_requested() {
        let bytes = b"one\r\n\xfftwo\r\nthree\r\xff\n";
        assert_eq!(
            take_source(copy_source_text(bytes, true)).as_deref(),
            Some("one\n\u{fffd}two\nthree\r\u{fffd}\n")
        );
        assert_eq!(
            take_source(copy_source_text(bytes, false)).as_deref(),
            Some("one\r\n\u{fffd}two\r\nthree\r\u{fffd}\n")
        );
    }
}
