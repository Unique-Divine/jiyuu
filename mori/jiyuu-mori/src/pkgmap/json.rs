//! Borrow the shared JSON parser so object lookup and iteration preserve the
//! parser's duplicate-key behavior and source order.

use std::ffi::{CStr, c_char, c_void};
use std::marker::PhantomData;
use std::ptr::NonNull;

use super::c_bytes;

pub(super) struct Document(NonNull<c_void>);

#[derive(Clone, Copy)]
pub(super) struct Value<'a>(*const c_void, PhantomData<&'a Document>);

impl Document {
    pub fn read(bytes: &[u8]) -> Option<Self> {
        NonNull::new(unsafe {
            cbm_pkgmap_json_read(bytes.as_ptr().cast(), bytes.len())
        })
        .map(Self)
    }

    pub fn root(&self) -> Value<'_> {
        Value(
            unsafe { cbm_pkgmap_json_root(self.0.as_ptr()) },
            PhantomData,
        )
    }
}

impl Drop for Document {
    fn drop(&mut self) {
        unsafe { cbm_pkgmap_json_free(self.0.as_ptr()) };
    }
}

impl<'a> Value<'a> {
    pub fn get(self, key: &CStr) -> Self {
        Self(
            unsafe { cbm_pkgmap_json_get(self.0, key.as_ptr()) },
            PhantomData,
        )
    }

    pub fn string(self) -> Option<&'a [u8]> {
        unsafe { c_bytes(cbm_pkgmap_json_string(self.0)) }
    }

    pub fn pairs(self) -> impl Iterator<Item = (Self, Self)> + 'a {
        let mut count = 0;
        let mut next = unsafe { cbm_pkgmap_json_first_key(self.0, &mut count) };
        (0..count).map(move |_| {
            let key = next;
            let value = unsafe { cbm_pkgmap_json_next_pair(&mut next) };
            (Self(key, PhantomData), Self(value, PhantomData))
        })
    }
}

unsafe extern "C" {
    fn cbm_pkgmap_json_read(bytes: *const c_char, len: usize) -> *mut c_void;
    fn cbm_pkgmap_json_free(doc: *mut c_void);
    fn cbm_pkgmap_json_root(doc: *const c_void) -> *const c_void;
    fn cbm_pkgmap_json_get(
        object: *const c_void,
        key: *const c_char,
    ) -> *const c_void;
    fn cbm_pkgmap_json_string(value: *const c_void) -> *const c_char;
    fn cbm_pkgmap_json_first_key(
        value: *const c_void,
        count: *mut usize,
    ) -> *const c_void;
    fn cbm_pkgmap_json_next_pair(next: *mut *const c_void) -> *const c_void;
}
