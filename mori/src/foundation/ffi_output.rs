//! Expose initialized Rust output to C's MemorySanitizer shadow state.

use std::ffi::{CStr, c_char};

#[cfg(not(test))]
unsafe extern "C" {
    fn cbm_ffi_mark_written(address: *const std::ffi::c_void, size: usize);
}

/// Tell instrumented C about an initialized output range written in Rust.
///
/// # Safety
/// The entire range of `count` elements was initialized. The byte count must
/// fit in `usize`.
pub(crate) unsafe fn written<T>(address: *const T, count: usize) {
    #[cfg(not(test))]
    // SAFETY: the caller supplies the initialized range.
    unsafe {
        cbm_ffi_mark_written(address.cast(), std::mem::size_of::<T>() * count);
    }
    #[cfg(test)]
    let _ = (address, count);
}

/// Write an output value before passing its storage back to C.
///
/// # Safety
/// `output` must be valid and properly aligned for a write of `T`.
pub(crate) unsafe fn write<T: Copy>(output: *mut T, value: T) {
    // SAFETY: the caller supplies the writable out parameter.
    unsafe {
        output.write(value);
        written(output, 1);
    }
}

/// Lend a NUL-terminated Rust string to instrumented C.
pub(crate) fn c_str(value: &CStr) -> *const c_char {
    let address = value.as_ptr();
    let length = value.to_bytes_with_nul().len();
    // SAFETY: CStr includes its initialized terminator.
    unsafe { written(address, length) };
    address
}

/// Mark initialized Rust bytes before calling code that may use a C interceptor.
#[cfg(feature = "native-engine-abi")]
pub(crate) fn bytes(value: &[u8]) {
    // SAFETY: a Rust byte slice carries its initialized length.
    unsafe { written(value.as_ptr(), value.len()) }
}
