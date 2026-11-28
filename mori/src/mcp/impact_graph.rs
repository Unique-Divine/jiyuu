//! Native store rows and walk operations shared by the MCP impact requests.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::ops::Deref;
use std::ptr::{null, null_mut};
use std::slice;

type Text = *const c_char;
type Id = i64;
type Index = c_int;

#[repr(C)]
pub struct Store {
    _private: [u8; 0],
}

#[repr(C)]
pub struct ImpactWalk {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Default)]
pub struct Node {
    pub id: Id,
    _project: Text,
    pub label: Text,
    _name: Text,
    pub qualified_name: Text,
    pub file_path: Text,
    pub start_line: Index,
    pub end_line: Index,
    _properties_json: Text,
}

#[repr(C)]
pub struct WalkPolicy {
    pub project: Text,
    pub edge_types: *const Text,
    pub edge_type_count: Index,
    pub follow_routes: bool,
    pub follow_variant_keys: bool,
    pub max_hops: Index,
}

#[repr(C)]
pub struct Hit {
    pub id: Id,
    pub hop: Index,
    pub via_id: Id,
    pub via_edge: Text,
    pub qualified_name: Text,
    pub label: Text,
}

unsafe extern "C" {
    fn cbm_ffi_mark_written(address: *const c_void, size: usize);
    pub fn cbm_store_free_nodes(rows: *mut Node, count: Index);
    pub fn cbm_impact_walk_run(
        walk: *mut ImpactWalk,
        ids: *const Id,
        count: Index,
    ) -> Index;
    pub fn cbm_impact_walk_count(walk: *const ImpactWalk) -> Index;
    pub fn cbm_impact_walk_hits(walk: *const ImpactWalk) -> *const Hit;
    pub fn cbm_impact_walk_close(walk: *mut ImpactWalk);
}

pub unsafe fn view<'a, T>(ptr: *const T, count: usize) -> &'a [T] {
    if count == 0 {
        &[]
    } else {
        unsafe { slice::from_raw_parts(ptr, count) }
    }
}

pub unsafe fn bytes<'a>(text: Text) -> &'a [u8] {
    if text.is_null() {
        &[]
    } else {
        unsafe { CStr::from_ptr(text) }.to_bytes()
    }
}

// C may be instrumented by MemorySanitizer while Rust is not.
pub fn mark_written<T>(items: &[T]) {
    if !items.is_empty() {
        unsafe {
            cbm_ffi_mark_written(
                items.as_ptr().cast(),
                std::mem::size_of_val(items),
            )
        };
    }
}

// Store result allocations are independent of later reads in the same scope.
pub struct StoreRows<T> {
    rows: *mut T,
    count: Index,
    free: unsafe extern "C" fn(*mut T, Index),
}

impl<T> StoreRows<T> {
    pub fn read(
        free: unsafe extern "C" fn(*mut T, Index),
        query: impl FnOnce(*mut *mut T, *mut Index) -> Index,
    ) -> Result<Self, ()> {
        let mut rows = null_mut();
        let mut count = 0;
        if query(&mut rows, &mut count) == 0 {
            Ok(Self { rows, count, free })
        } else {
            Err(())
        }
    }
}

impl<T> Deref for StoreRows<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        unsafe { view(self.rows, self.count as usize) }
    }
}

impl<T> Drop for StoreRows<T> {
    fn drop(&mut self) {
        unsafe { (self.free)(self.rows, self.count) };
    }
}

pub fn ptr_or_null<T>(items: &[T]) -> *const T {
    if items.is_empty() {
        null()
    } else {
        items.as_ptr()
    }
}
