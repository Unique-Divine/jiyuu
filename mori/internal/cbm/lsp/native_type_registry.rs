//! Rust's one declaration of the native type registry lookups.
//!
//! Several Rust modules call these C functions. Declaring each symbol once,
//! over one set of opaque handles, keeps their signatures from drifting apart
//! and avoids `clashing_extern_declarations`. Callers that read fields of a
//! registry row cast the returned pointer to their own `#[repr(C)]` view.

use std::ffi::c_char;

/// A native `CBMTypeRegistry`.
#[repr(C)]
pub(crate) struct TypeRegistry {
    _opaque: [u8; 0],
}

/// A `CBMRegisteredType` row owned by a registry.
#[repr(C)]
pub(crate) struct RegisteredType {
    _opaque: [u8; 0],
}

/// A `CBMRegisteredFunc` row owned by a registry.
#[repr(C)]
pub(crate) struct RegisteredFunc {
    _opaque: [u8; 0],
}

unsafe extern "C" {
    // The registry owns the returned rows; callers keep it alive while they
    // borrow them.
    pub(crate) fn cbm_registry_lookup_type(
        registry: *const TypeRegistry,
        qn: *const c_char,
    ) -> *const RegisteredType;
    pub(crate) fn cbm_registry_lookup_method(
        registry: *const TypeRegistry,
        receiver: *const c_char,
        name: *const c_char,
    ) -> *const RegisteredFunc;
    pub(crate) fn cbm_registry_lookup_func(
        registry: *const TypeRegistry,
        qn: *const c_char,
    ) -> *const RegisteredFunc;
    pub(crate) fn cbm_registry_lookup_symbol(
        registry: *const TypeRegistry,
        package: *const c_char,
        name: *const c_char,
    ) -> *const RegisteredFunc;
}
