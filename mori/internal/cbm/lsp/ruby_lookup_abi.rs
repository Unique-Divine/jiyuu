//! The C walker owns one ancestry handle per file. Input strings are read
//! during the call. Local constant QNs are borrowed from the handle after
//! collection. The caller must copy them to the file arena before retaining
//! them in emitted rows. Registry callbacks return native typed identities.

use super::{Ancestry, Lookup, MixinKind, Search, Side, constant_at};
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr;

#[repr(C)]
struct RegisteredFunc {
    _opaque: [u8; 0],
}

type TypeLookup =
    unsafe extern "C" fn(*const c_void, *const c_char) -> *const c_char;
type MethodLookup = unsafe extern "C" fn(
    *const c_void,
    *const c_char,
    *const c_char,
) -> *const RegisteredFunc;

#[repr(C)]
struct MethodResult {
    target: *const RegisteredFunc,
    blocked: bool,
    steps: u64,
}

#[unsafe(no_mangle)]
extern "C" fn mori_ruby_ancestry_new() -> *mut Ancestry {
    Box::into_raw(Box::new(Ancestry::default()))
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_ruby_ancestry_free(ancestry: *mut Ancestry) {
    drop(unsafe { Box::from_raw(ancestry) });
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_ruby_ancestry_declare(
    ancestry: &mut Ancestry,
    qn: *const c_char,
    nesting: *const c_char,
    path: *const c_char,
    parent: *const c_char,
) {
    if qn.is_null() {
        return;
    }
    let (qn, nesting, path) = unsafe {
        (
            CStr::from_ptr(qn),
            CStr::from_ptr(nesting),
            CStr::from_ptr(path),
        )
    };
    let parent = (!parent.is_null()).then(|| unsafe { CStr::from_ptr(parent) });
    ancestry.declare(qn, nesting.to_bytes(), path, parent);
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_ruby_ancestry_mixin(
    ancestry: &mut Ancestry,
    owner: *const c_char,
    nesting: *const c_char,
    path: *const c_char,
    kind: c_int,
) {
    let kind = match kind {
        0 => MixinKind::Include,
        1 => MixinKind::Prepend,
        2 => MixinKind::Extend,
        _ => return,
    };
    let (owner, nesting, path) = unsafe {
        (
            CStr::from_ptr(owner),
            CStr::from_ptr(nesting),
            CStr::from_ptr(path),
        )
    };
    ancestry.mixin(owner, kind, nesting.to_bytes(), path);
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_ruby_ancestry_finalize(
    ancestry: &mut Ancestry,
    registry: *const c_void,
    lookup: TypeLookup,
) {
    ancestry.finalize(|path| {
        let qn = unsafe { lookup(registry, path.as_ptr()) };
        (!qn.is_null()).then(|| CString::from(unsafe { CStr::from_ptr(qn) }))
    });
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_ruby_ancestry_constant(
    ancestry: &Ancestry,
    nesting: *const c_char,
    path: *const c_char,
) -> *const c_char {
    let (nesting, path) =
        unsafe { (CStr::from_ptr(nesting), CStr::from_ptr(path)) };
    constant_at(&ancestry.constants, nesting.to_bytes(), path)
        .map_or(ptr::null(), CStr::as_ptr)
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_ruby_ancestry_lookup(
    ancestry: &Ancestry,
    registry: *const c_void,
    lookup: MethodLookup,
    qn: *const c_char,
    name: *const c_char,
    singleton: bool,
    search: c_int,
    max_depth: u32,
) -> MethodResult {
    let mut result = MethodResult {
        target: ptr::null(),
        blocked: false,
        steps: 0,
    };
    let search = match search {
        0 => Search::Receiver,
        1 => Search::SelfDispatch,
        2 => Search::Super,
        _ => return result,
    };
    if qn.is_null() || name.is_null() {
        return result;
    }
    let qn = unsafe { CStr::from_ptr(qn) };
    let side = if singleton {
        Side::Singleton
    } else {
        Side::Instance
    };
    let (found, steps) = ancestry.lookup(qn, side, search, max_depth, |key| {
        let target = unsafe { lookup(registry, key.as_ptr(), name) };
        (!target.is_null()).then_some(target)
    });
    result.steps = steps;
    match found {
        Lookup::Hit(target) => result.target = target,
        Lookup::Miss => {}
        Lookup::Blocked => result.blocked = true,
    }
    result
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_ruby_ancestry_is_ar_model(
    ancestry: &Ancestry,
    qn: *const c_char,
    max_depth: u32,
) -> bool {
    !qn.is_null()
        && ancestry.is_ar_model(unsafe { CStr::from_ptr(qn) }, max_depth)
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_ruby_ancestry_counts(
    ancestry: &Ancestry,
    mixins: &mut usize,
) -> usize {
    *mixins = ancestry
        .classes
        .values()
        .map(|c| c.includes.len() + c.prepends.len() + c.extends.len())
        .sum();
    ancestry.classes.len()
}
