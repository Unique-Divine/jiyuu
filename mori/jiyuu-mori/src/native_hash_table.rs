//! Shared ABI for the native hash table.

use std::ffi::{c_char, c_void};

pub(super) enum HashTable {}

unsafe extern "C" {
    pub(super) fn cbm_ht_count(table: *const HashTable) -> u32;
    pub(super) fn cbm_ht_create(initial_capacity: u32) -> *mut HashTable;
    pub(super) fn cbm_ht_free(table: *mut HashTable);
    pub(super) fn cbm_ht_get(
        table: *const HashTable,
        key: *const c_char,
    ) -> *mut c_void;
    pub(super) fn cbm_ht_set(
        table: *mut HashTable,
        key: *const c_char,
        value: *mut c_void,
    ) -> *mut c_void;
}
