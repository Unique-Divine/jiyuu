//! Namespace buckets retain input order, including entries recovered from
//! spilled extraction results.

use super::graph::{
    FileResult, Text, cbm_pipeline_fqn_compute, cbm_pkgmap_namespace,
};
use super::{
    HashTable, c_bytes, cbm_ht_create, cbm_ht_foreach, cbm_ht_free, cbm_ht_get,
    cbm_ht_set, cbm_pkgmap_tracked_free, cbm_pkgmap_tracked_strdup, free,
    realloc,
};
use std::ffi::{c_char, c_int, c_void};
use std::ptr::null_mut;

fn add(
    map: &mut HashTable,
    project: *const c_char,
    rel: *const c_char,
    name: *const c_char,
) -> bool {
    let Some(source) = unsafe { c_bytes(name) }.filter(|name| !name.is_empty())
    else {
        return true;
    };
    if rel.is_null() {
        return false;
    }
    let Some(qn) = Text::new(unsafe {
        cbm_pipeline_fqn_compute(project, rel, c"__file__".as_ptr())
    }) else {
        return false;
    };
    let key = unsafe { cbm_pkgmap_tracked_strdup(name) };
    if key.is_null() {
        return false;
    }
    // The table owns the normalized key on insertion, including its NUL.
    let bytes = unsafe {
        std::slice::from_raw_parts_mut(key.cast::<u8>(), source.len())
    };
    for byte in bytes {
        if matches!(*byte, b'\\' | b':' | b'/') {
            *byte = b'.';
        }
    }
    let stored = unsafe { cbm_ht_get_key(map, key) };
    if !stored.is_null() {
        let replaced = append_file(map, stored, qn.bytes()).is_some();
        unsafe { cbm_pkgmap_tracked_free(key) };
        return replaced;
    }
    let qn = super::graph::take_text(Some(qn));
    unsafe {
        cbm_ht_set(map, key, qn.cast());
        let owns_key = cbm_ht_get_key(map, key) == key;
        let owns_qn = cbm_ht_get(map, key) == qn.cast();
        if !owns_key {
            cbm_pkgmap_tracked_free(key);
        }
        if !owns_qn {
            free(qn.cast());
        }
        owns_key && owns_qn
    }
}

fn append_file(
    map: &mut HashTable,
    key: *const c_char,
    qn: &[u8],
) -> Option<()> {
    let previous = unsafe { cbm_ht_get(map, key) };
    let previous_bytes = unsafe { c_bytes(previous.cast()) }?;
    let prefix_len = previous_bytes.len().checked_add(1)?;
    let contents_len = prefix_len.checked_add(qn.len())?;
    let value = unsafe { realloc(null_mut(), contents_len.checked_add(1)?) }
        .cast::<u8>();
    if value.is_null() {
        return None;
    }
    unsafe {
        value.copy_from_nonoverlapping(
            previous_bytes.as_ptr(),
            previous_bytes.len(),
        );
        value.add(prefix_len - 1).write(b'\n');
        value
            .add(prefix_len)
            .copy_from_nonoverlapping(qn.as_ptr(), qn.len());
        value.add(contents_len).write(0);
        // Preserve the stored key's address on replacement.
        cbm_ht_set(map, key, value.cast());
        if cbm_ht_get(map, key) != value.cast() {
            free(value.cast());
            return None;
        }
        free(previous);
    }
    Some(())
}

fn build(
    project: *const c_char,
    rels: *const *const c_char,
    count: c_int,
    names: impl Fn(usize) -> *const c_char,
) -> *mut HashTable {
    let map = unsafe { cbm_ht_create(64) };
    let Some(table) = (unsafe { map.as_mut() }) else {
        return null_mut();
    };
    for index in 0..count as usize {
        if !add(table, project, unsafe { *rels.add(index) }, names(index)) {
            unsafe { cbm_pipeline_namespace_map_free(map) };
            return null_mut();
        }
    }
    map
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn cbm_pipeline_namespace_map_build_names(
    project: *const c_char,
    names: *const *const c_char,
    rels: *const *const c_char,
    count: c_int,
) -> *mut HashTable {
    if count < 0 || (count > 0 && (names.is_null() || rels.is_null())) {
        return null_mut();
    }
    build(project, rels, count, |index| unsafe { *names.add(index) })
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn cbm_pipeline_namespace_map_build(
    project: *const c_char,
    results: *const *const FileResult,
    rels: *const *const c_char,
    count: c_int,
) -> *mut HashTable {
    if count < 0 || (count > 0 && (results.is_null() || rels.is_null())) {
        return null_mut();
    }
    build(project, rels, count, |index| unsafe {
        cbm_pkgmap_namespace(*results.add(index))
    })
}

unsafe extern "C" fn release(
    key: *const c_char,
    value: *mut c_void,
    _: *mut c_void,
) {
    unsafe {
        cbm_pkgmap_tracked_free(key.cast_mut());
        free(value);
    }
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn cbm_pipeline_namespace_map_free(
    map: *mut HashTable,
) {
    if !map.is_null() {
        unsafe {
            cbm_ht_foreach(map, release, null_mut());
            cbm_ht_free(map);
        }
    }
}

unsafe extern "C" {
    fn cbm_ht_get_key(
        map: *const HashTable,
        key: *const c_char,
    ) -> *const c_char;
}
