use super::Packages;
use std::ffi::{CStr, c_char};
use std::ptr::{self, NonNull};

#[repr(C)]
struct TypeRegistry {
    _opaque: [u8; 0],
}

#[repr(C)]
struct RegisteredFunc {
    _opaque: [u8; 0],
}

#[repr(C)]
struct RegistryLookup {
    exact: unsafe extern "C" fn(
        *const TypeRegistry,
        *const c_char,
    ) -> *const RegisteredFunc,
    direct_method: unsafe extern "C" fn(
        *const TypeRegistry,
        *const c_char,
        *const c_char,
    ) -> *const RegisteredFunc,
}

#[unsafe(no_mangle)]
extern "C" fn mori_perl_packages_new() -> *mut Packages {
    Box::into_raw(Box::new(Packages::default()))
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_perl_packages_free(packages: *mut Packages) {
    // SAFETY: C transfers the handle returned by new exactly once.
    unsafe { drop(Box::from_raw(packages)) };
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_perl_packages_add_parent(
    packages: &mut Packages,
    package: *const c_char,
    parent: *const c_char,
) {
    // SAFETY: The parser supplies live, terminated strings for this call.
    unsafe {
        packages.add_parent(CStr::from_ptr(package), CStr::from_ptr(parent))
    };
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_perl_packages_add_sub(
    packages: &mut Packages,
    package: *const c_char,
    name: *const c_char,
    module: *const c_char,
) {
    // SAFETY: Package and sub names are live C strings. The module can be null.
    unsafe {
        packages.add_sub(
            CStr::from_ptr(package),
            CStr::from_ptr(name),
            (!module.is_null()).then(|| CStr::from_ptr(module)),
        );
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_perl_packages_lookup(
    packages: &Packages,
    registry: *const TypeRegistry,
    lookup: &RegistryLookup,
    package: *const c_char,
    name: *const c_char,
    super_call: bool,
) -> *const RegisteredFunc {
    if package.is_null() || name.is_null() {
        return ptr::null();
    }
    // SAFETY: C supplies live strings and registry for the lookup. Both
    // callbacks borrow their inputs and return pointers into that registry.
    unsafe {
        packages
            .lookup(
                CStr::from_ptr(package),
                CStr::from_ptr(name),
                super_call,
                |qn| {
                    NonNull::new(
                        (lookup.exact)(registry, qn.as_ptr()).cast_mut(),
                    )
                },
                |pkg, method| {
                    NonNull::new(
                        (lookup.direct_method)(
                            registry,
                            pkg.as_ptr(),
                            method.as_ptr(),
                        )
                        .cast_mut(),
                    )
                },
            )
            .map_or(ptr::null(), |target| target.as_ptr().cast_const())
    }
}
