//! Namespace buckets retain input order, including entries recovered from
//! spilled extraction results.

use super::graph::{
    FileResult, Text, cbm_pipeline_fqn_compute, cbm_pkgmap_namespace,
    with_c_string,
};
use super::{
    HashTable, c_bytes, cbm_ht_create, cbm_ht_foreach, cbm_ht_free, cbm_ht_get,
    cbm_ht_set, cbm_pkgmap_tracked_free, cbm_pkgmap_tracked_strdup, duplicate,
    free,
};
use std::ffi::{c_char, c_int, c_void};
use std::ptr::null_mut;

fn add(
    map: &mut HashTable,
    project: *const c_char,
    rel: *const c_char,
    name: *const c_char,
) -> bool {
    let Some(name) = unsafe { c_bytes(name) }.filter(|name| !name.is_empty())
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
    let normalized: Vec<_> = name
        .iter()
        .map(|&byte| match byte {
            b'\\' | b':' | b'/' => b'.',
            _ => byte,
        })
        .collect();
    with_c_string(&normalized, |key| unsafe {
        let stored = cbm_ht_get_key(map, key.as_ptr());
        if stored.is_null() {
            let key = cbm_pkgmap_tracked_strdup(key.as_ptr());
            let qn = super::graph::take_text(Some(qn));
            if !key.is_null() {
                cbm_ht_set(map, key, qn.cast());
                if cbm_ht_get(map, key) == qn.cast() {
                    return true;
                }
            }
            cbm_pkgmap_tracked_free(key);
            free(qn.cast());
            return false;
        }
        let previous = cbm_ht_get(map, stored);
        let Some(previous_bytes) = c_bytes(previous.cast()) else {
            return false;
        };
        let value = duplicate(&[previous_bytes, b"\n", qn.bytes()].concat());
        if value.is_null() {
            return false;
        }
        // Preserve the stored key's address on replacement.
        cbm_ht_set(map, stored, value.cast());
        if cbm_ht_get(map, stored) != value.cast() {
            free(value.cast());
            return false;
        }
        free(previous);
        true
    })
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
