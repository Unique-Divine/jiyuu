use super::{Registry, Resolver};
use std::ffi::{CStr, c_char, c_int, c_void};
use std::marker::PhantomData;
use std::ptr;

#[repr(C)]
struct PhpContext {
    _opaque: [u8; 0],
}

#[repr(C)]
struct Arena {
    _opaque: [u8; 0],
}

#[repr(C)]
struct TypeRegistry {
    _opaque: [u8; 0],
}

#[repr(C)]
struct RegisteredType {
    _opaque: [u8; 0],
}

#[repr(C)]
struct RegisteredFunc {
    _opaque: [u8; 0],
}

#[repr(C)]
struct ResolverView {
    arena: *mut Arena,
    registry: *const TypeRegistry,
    module: *const c_char,
    external_uses: *const *const c_char,
    external_use_count: c_int,
}

#[repr(C)]
struct TypeView {
    qn: *const c_char,
    parents: *const *const c_char,
    method_names: *const *const c_char,
    method_qns: *const *const c_char,
}

unsafe extern "C" {
    fn cbm_php_resolver_view(ctx: *const PhpContext) -> ResolverView;
    fn cbm_php_type_view(ty: *const RegisteredType) -> TypeView;
    fn cbm_php_visit_short_types(
        registry: *const TypeRegistry,
        name: *const c_char,
        visit: extern "C" fn(*mut c_void, *const RegisteredType),
        data: *mut c_void,
    );
    fn cbm_arena_alloc(arena: *mut Arena, n: usize) -> *mut c_void;
    fn cbm_registry_lookup_type(
        registry: *const TypeRegistry,
        qn: *const c_char,
    ) -> *const RegisteredType;
    fn cbm_registry_lookup_method(
        registry: *const TypeRegistry,
        receiver: *const c_char,
        name: *const c_char,
    ) -> *const RegisteredFunc;
    fn cbm_registry_lookup_func(
        registry: *const TypeRegistry,
        qn: *const c_char,
    ) -> *const RegisteredFunc;
}

#[derive(Clone)]
struct Names<'a> {
    next: *const *const c_char,
    lifetime: PhantomData<&'a CStr>,
}

impl Names<'_> {
    /// # Safety
    ///
    /// `next` is NULL or points to an array of C strings that is
    /// NULL-terminated or bounded by a matching `Iterator::take`. The array
    /// and its strings outlive the iterator.
    unsafe fn new(next: *const *const c_char) -> Self {
        Self {
            next,
            lifetime: PhantomData,
        }
    }
}

impl<'a> Iterator for Names<'a> {
    type Item = &'a CStr;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next.is_null() {
            return None;
        }
        // SAFETY: `new` requires valid C strings and a terminator or bound.
        let name = unsafe { *self.next };
        if name.is_null() {
            self.next = ptr::null();
            return None;
        }
        unsafe {
            self.next = self.next.add(1);
            Some(CStr::from_ptr(name))
        }
    }
}

/// Borrow a C string that may be NULL.
///
/// # Safety
///
/// A non-NULL `name` is a valid C string that outlives `'a`.
unsafe fn optional<'a>(name: *const c_char) -> Option<&'a CStr> {
    (!name.is_null()).then(|| unsafe { CStr::from_ptr(name) })
}

struct NativeRegistry<'a> {
    arena: *mut Arena,
    registry: *const TypeRegistry,
    lifetime: PhantomData<&'a PhpContext>,
}

impl<'a> Registry<'a> for NativeRegistry<'a> {
    type Type = *const RegisteredType;
    type Method = *const RegisteredFunc;

    fn exact_type(&self, qn: &CStr) -> Option<Self::Type> {
        let ty = unsafe { cbm_registry_lookup_type(self.registry, qn.as_ptr()) };
        (!ty.is_null()).then_some(ty)
    }

    // Build the key in the context arena, as the C lookup did, so an
    // exhausted arena still turns this probe into a miss.
    fn joined_type(&self, prefix: &[u8], qn: &CStr) -> Option<Self::Type> {
        let qn = qn.to_bytes_with_nul();
        let len = prefix.len() + 1 + qn.len();
        let key = unsafe { cbm_arena_alloc(self.arena, len) }.cast::<u8>();
        if key.is_null() {
            return None;
        }
        // SAFETY: the arena returned `len` writable bytes.
        let bytes = unsafe { std::slice::from_raw_parts_mut(key, len) };
        bytes[..prefix.len()].copy_from_slice(prefix);
        bytes[prefix.len()] = b'.';
        bytes[prefix.len() + 1..].copy_from_slice(qn);
        let key = CStr::from_bytes_with_nul(bytes).ok()?;
        self.exact_type(key)
    }

    fn method(&self, receiver: &CStr, name: &CStr) -> Option<Self::Method> {
        let row = unsafe {
            cbm_registry_lookup_method(
                self.registry,
                receiver.as_ptr(),
                name.as_ptr(),
            )
        };
        (!row.is_null()).then_some(row)
    }

    fn function(&self, qn: &CStr) -> Option<Self::Method> {
        let row =
            unsafe { cbm_registry_lookup_func(self.registry, qn.as_ptr()) };
        (!row.is_null()).then_some(row)
    }

    fn visit_short_types(&self, name: &CStr, mut visit: impl FnMut(Self::Type)) {
        extern "C" fn forward(data: *mut c_void, row: *const RegisteredType) {
            // SAFETY: the C iterator calls back synchronously with this payload.
            let visit = unsafe {
                &mut *data.cast::<&mut dyn FnMut(*const RegisteredType)>()
            };
            visit(row);
        }
        let mut visit: &mut dyn FnMut(Self::Type) = &mut visit;
        unsafe {
            cbm_php_visit_short_types(
                self.registry,
                name.as_ptr(),
                forward,
                ptr::from_mut(&mut visit).cast(),
            );
        }
    }

    // A row without a key scores no shared module segments, as in C.
    fn type_name(&self, ty: Self::Type) -> &'a CStr {
        unsafe { optional(cbm_php_type_view(ty).qn) }.unwrap_or(c"")
    }

    fn parents(&self, ty: Self::Type) -> impl Iterator<Item = &'a CStr> {
        unsafe { Names::new(cbm_php_type_view(ty).parents) }
    }

    // Method names are NULL-terminated; method keys run parallel to them.
    fn listed_methods(
        &self,
        ty: Self::Type,
    ) -> impl Iterator<Item = (&'a CStr, Option<&'a CStr>)> {
        let view = unsafe { cbm_php_type_view(ty) };
        let names = if view.method_qns.is_null() {
            ptr::null()
        } else {
            view.method_names
        };
        unsafe { Names::new(names) }
            .enumerate()
            .map(move |(i, name)| {
                // SAFETY: the key array has an entry for every listed name.
                (name, unsafe { optional(*view.method_qns.add(i)) })
            })
    }
}

fn resolver(
    ctx: &PhpContext,
) -> Resolver<'_, NativeRegistry<'_>, std::iter::Take<Names<'_>>> {
    // SAFETY: the view borrows data from the live context and registry.
    let view = unsafe { cbm_php_resolver_view(ctx) };
    Resolver {
        registry: NativeRegistry {
            arena: view.arena,
            registry: view.registry,
            lifetime: PhantomData,
        },
        module: unsafe { optional(view.module) },
        external_uses: unsafe { Names::new(view.external_uses) }
            .take(view.external_use_count.max(0) as usize),
    }
}

// The C callers lend the context, registry and C strings for the duration of
// each call. Results are original registry rows with their existing lifetime.

/// Resolve a PHP class QN to its registered type row, or NULL.
#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_php_lookup_type(
    ctx: &PhpContext,
    qn: *const c_char,
) -> *const RegisteredType {
    let Some(qn) = (unsafe { optional(qn) }) else {
        return ptr::null();
    };
    resolver(ctx).lookup_type(qn).unwrap_or(ptr::null())
}

/// True when `qn` names, or lies under, an unresolved vendor `use` target.
#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_php_external_qn(
    ctx: &PhpContext,
    qn: *const c_char,
) -> bool {
    unsafe { optional(qn) }.is_some_and(|qn| resolver(ctx).is_external(qn))
}

/// Look up `name` on `class` or its ancestors, or return NULL.
#[unsafe(no_mangle)]
unsafe extern "C" fn php_lookup_method(
    ctx: &PhpContext,
    class: *const c_char,
    name: *const c_char,
) -> *const RegisteredFunc {
    let (Some(class), Some(name)) =
        (unsafe { optional(class) }, unsafe { optional(name) })
    else {
        return ptr::null();
    };
    resolver(ctx)
        .lookup_method(class, name)
        .unwrap_or(ptr::null())
}
