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

impl<'a> Value<'a> {
    pub fn is_null(self) -> bool {
        unsafe { cbm_pkgmap_json_is_null(self.0) }
    }

    pub fn is_object(self) -> bool {
        unsafe { cbm_pkgmap_json_is_object(self.0) }
    }

    pub fn equals_string(self, text: &[u8]) -> bool {
        self.string() == Some(text)
            && unsafe { cbm_pkgmap_json_len(self.0) == text.len() }
    }

    pub fn integer(self) -> Option<i32> {
        let mut value = 0;
        unsafe { cbm_pkgmap_json_int(self.0, &mut value) }.then_some(value)
    }

    pub fn boolean(self) -> Option<bool> {
        let mut value = false;
        unsafe { cbm_pkgmap_json_bool(self.0, &mut value) }.then_some(value)
    }

    pub fn array(self) -> Option<impl ExactSizeIterator<Item = Self>> {
        let mut size = 0;
        if !unsafe { cbm_pkgmap_json_array_size(self.0, &mut size) } {
            return None;
        }
        Some((0..size).map(move |index| {
            Self(
                unsafe { cbm_pkgmap_json_array_at(self.0, index) },
                PhantomData,
            )
        }))
    }
}

use super::graph::{Text, with_c_string};
use std::ffi::c_int;

pub(super) struct Writer(NonNull<c_void>);

#[derive(Clone, Copy)]
pub(super) struct Item<'a>(NonNull<c_void>, PhantomData<&'a Writer>);

impl Writer {
    pub fn new() -> Option<Self> {
        NonNull::new(unsafe { yyjson_mut_doc_new(std::ptr::null()) }).map(Self)
    }

    fn item(&self, ptr: *mut c_void) -> Option<Item<'_>> {
        NonNull::new(ptr).map(|ptr| Item(ptr, PhantomData))
    }

    pub fn object(&self) -> Option<Item<'_>> {
        self.item(unsafe { cbm_pkgmap_json_object(self.0.as_ptr()) })
    }

    pub fn array(&self) -> Option<Item<'_>> {
        self.item(unsafe { cbm_pkgmap_json_array(self.0.as_ptr()) })
    }

    pub fn string(&self, text: Option<&[u8]>) -> Option<Item<'_>> {
        self.item(unsafe {
            match text {
                Some(text) => with_c_string(text, |text| {
                    cbm_pkgmap_json_copy_string(self.0.as_ptr(), text.as_ptr())
                }),
                None => cbm_pkgmap_json_copy_string(
                    self.0.as_ptr(),
                    std::ptr::null(),
                ),
            }
        })
    }

    pub fn integer(&self, number: i64) -> Option<Item<'_>> {
        self.item(unsafe { cbm_pkgmap_json_integer(self.0.as_ptr(), number) })
    }

    pub fn boolean(&self, value: bool) -> Option<Item<'_>> {
        self.item(unsafe { cbm_pkgmap_json_boolean(self.0.as_ptr(), value) })
    }

    pub fn copy(&self, value: Value<'_>) -> Option<Item<'_>> {
        self.item(unsafe { yyjson_val_mut_copy(self.0.as_ptr(), value.0) })
    }

    pub fn put(
        &self,
        object: Item<'_>,
        name: &[u8],
        value: Item<'_>,
    ) -> Option<()> {
        with_c_string(name, |name| unsafe {
            cbm_pkgmap_json_put(
                self.0.as_ptr(),
                object.0.as_ptr(),
                name.as_ptr(),
                value.0.as_ptr(),
            )
        })
        .then_some(())
    }

    pub fn append(&self, array: Item<'_>, item: Item<'_>) -> Option<()> {
        unsafe { cbm_pkgmap_json_append(array.0.as_ptr(), item.0.as_ptr()) }
            .then_some(())
    }

    pub fn write(&self, value: Item<'_>) -> Option<Text> {
        Text::new(unsafe { cbm_pkgmap_json_write(value.0.as_ptr()) })
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        unsafe { yyjson_mut_doc_free(self.0.as_ptr()) };
    }
}

unsafe extern "C" {
    fn cbm_pkgmap_json_is_null(value: *const c_void) -> bool;
    fn cbm_pkgmap_json_is_object(value: *const c_void) -> bool;
    fn cbm_pkgmap_json_len(value: *const c_void) -> usize;
    fn cbm_pkgmap_json_int(value: *const c_void, number: *mut c_int) -> bool;
    fn cbm_pkgmap_json_bool(value: *const c_void, boolean: *mut bool) -> bool;
    fn cbm_pkgmap_json_array_size(
        value: *const c_void,
        count: *mut usize,
    ) -> bool;
    fn cbm_pkgmap_json_array_at(
        value: *const c_void,
        index: usize,
    ) -> *const c_void;
    fn yyjson_mut_doc_new(alloc: *const c_void) -> *mut c_void;
    fn yyjson_mut_doc_free(doc: *mut c_void);
    fn yyjson_val_mut_copy(
        doc: *mut c_void,
        value: *const c_void,
    ) -> *mut c_void;
    fn cbm_pkgmap_json_object(doc: *mut c_void) -> *mut c_void;
    fn cbm_pkgmap_json_array(doc: *mut c_void) -> *mut c_void;
    fn cbm_pkgmap_json_copy_string(
        doc: *mut c_void,
        string: *const c_char,
    ) -> *mut c_void;
    fn cbm_pkgmap_json_integer(doc: *mut c_void, number: i64) -> *mut c_void;
    fn cbm_pkgmap_json_boolean(doc: *mut c_void, value: bool) -> *mut c_void;
    fn cbm_pkgmap_json_put(
        doc: *mut c_void,
        object: *mut c_void,
        key: *const c_char,
        value: *mut c_void,
    ) -> bool;
    fn cbm_pkgmap_json_append(array: *mut c_void, value: *mut c_void) -> bool;
    fn cbm_pkgmap_json_write(value: *const c_void) -> *mut c_char;
}
