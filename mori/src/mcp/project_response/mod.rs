//! Assemble project read responses from the request data gathered by C.

use std::borrow::Cow;
use std::ffi::{CStr, c_char, c_void};
use std::{ptr, slice};

unsafe extern "C" {
    fn cbm_output_encode_text(
        data: *const c_char,
        len: usize,
        encoded: *mut *mut c_char,
        encoded_len: *mut usize,
    ) -> bool;
    fn free(allocation: *mut c_void);
}

/// Apply the native output encoding for non-UTF-8 paths and reserved prefixes.
pub(crate) fn encode_text(text: &CStr) -> Option<Cow<'_, str>> {
    let mut encoded = ptr::null_mut();
    let mut encoded_len = 0;
    // SAFETY: C reads text's live bytes and writes both output variables.
    if !unsafe {
        cbm_output_encode_text(
            crate::ffi_output::c_str(text),
            text.count_bytes(),
            &mut encoded,
            &mut encoded_len,
        )
    } {
        return None;
    }
    if encoded.is_null() {
        return text.to_str().ok().map(Cow::Borrowed);
    }
    // SAFETY: the encoder returned a C allocation of encoded_len bytes.
    let result = unsafe { slice::from_raw_parts(encoded.cast(), encoded_len) };
    let result = str::from_utf8(result)
        .ok()
        .map(|text| Cow::Owned(text.to_owned()));
    // SAFETY: C allocated the encoded string with malloc.
    unsafe { free(encoded.cast()) };
    result
}

#[cfg(feature = "c-backend")]
mod index_status;

#[cfg(feature = "c-backend")]
pub use index_status::register_index_status_renderer;
