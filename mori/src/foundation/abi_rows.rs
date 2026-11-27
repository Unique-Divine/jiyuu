//! Grow result arrays in the allocator used by their public C destructors.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr;

unsafe extern "C" {
    fn cbm_result_realloc(block: *mut c_void, bytes: usize) -> *mut c_void;
    fn cbm_result_strdup(text: *const c_char) -> *mut c_char;
    fn cbm_result_free(block: *mut c_void);
    fn cbm_ffi_mark_written(address: *const c_void, bytes: usize);
}

pub(crate) struct OwnedString(*mut c_char);

impl OwnedString {
    pub(crate) fn copy(text: &CStr) -> Result<Self, ()> {
        // SAFETY: The input is terminated and the result uses the C allocator.
        let copy = unsafe { cbm_result_strdup(text.as_ptr()) };
        if copy.is_null() {
            Err(())
        } else {
            Ok(Self(copy))
        }
    }

    pub(crate) fn into_raw(self) -> *mut c_char {
        let allocation = self.0;
        std::mem::forget(self);
        allocation
    }
}

impl Drop for OwnedString {
    fn drop(&mut self) {
        // SAFETY: This wrapper retains the allocation until it hands it off.
        unsafe { cbm_result_free(self.0.cast()) };
    }
}

pub(crate) struct Rows<T> {
    data: *mut T,
    len: c_int,
    capacity: c_int,
    destroy: unsafe extern "C" fn(*mut T, c_int),
}

impl<T> Rows<T> {
    pub(crate) fn new(destroy: unsafe extern "C" fn(*mut T, c_int)) -> Self {
        Self {
            data: ptr::null_mut(),
            len: 0,
            capacity: 0,
            destroy,
        }
    }

    pub(crate) fn push(
        &mut self,
        initial: c_int,
        row: impl FnOnce() -> Result<T, ()>,
    ) -> Result<(), ()> {
        if self.len == self.capacity {
            let capacity = self.capacity.checked_mul(2).ok_or(())?.max(initial);
            let bytes = (capacity as usize)
                .checked_mul(std::mem::size_of::<T>())
                .filter(|&bytes| bytes <= isize::MAX as usize)
                .ok_or(())?;
            // SAFETY: data is null or owns a C allocation. On failure it stays valid.
            let data = unsafe { cbm_result_realloc(self.data.cast(), bytes) }
                .cast::<T>();
            if data.is_null() {
                return Err(());
            }
            self.data = data;
            self.capacity = capacity;
        }
        let row = row()?;
        // SAFETY: Capacity includes this slot, and it is not yet initialized.
        unsafe { write_optional(self.data.add(self.len as usize), row) };
        self.len += 1;
        Ok(())
    }

    pub(crate) fn len(&self) -> c_int {
        self.len
    }

    pub(crate) fn as_mut_slice(&mut self) -> &mut [T] {
        if self.len == 0 {
            return &mut [];
        }
        // SAFETY: The prefix through len is initialized and exclusively borrowed.
        unsafe { std::slice::from_raw_parts_mut(self.data, self.len as usize) }
    }

    pub(crate) fn into_raw(self) -> (*mut T, c_int) {
        let result = (self.data, self.len);
        std::mem::forget(self);
        result
    }
}

impl<T> Drop for Rows<T> {
    fn drop(&mut self) {
        // SAFETY: The destructor accepts null or a C allocation with len live rows.
        unsafe { (self.destroy)(self.data, self.len) };
    }
}

pub(crate) unsafe fn write_optional<T>(output: *mut T, value: T) {
    if !output.is_null() {
        // SAFETY: The caller supplied a writable slot. C's memory sanitizer
        // cannot see the Rust write, so mark the slot for the C reader.
        unsafe {
            output.write(value);
            cbm_ffi_mark_written(output.cast(), std::mem::size_of::<T>());
        }
    }
}
