//! Materialize Rust's parsed types in the C arena used by the current TS registry.

use std::ffi::{CStr, CString, c_char, c_int, c_long, c_void};
use std::ptr::null;

use crate::go_eval::native::{
    Type, cbm_type_builtin, cbm_type_named, cbm_type_unknown,
};

use super::{MAX_MEMBERS, Name, Param, Shape, charge_budget, parse};

type Arena = *mut c_void;

unsafe extern "C" {
    fn cbm_ffi_mark_written(address: *const c_void, size: usize);
    fn cbm_arena_strndup(
        arena: Arena,
        text: *const c_char,
        len: usize,
    ) -> *mut c_char;

    fn cbm_type_type_param(arena: Arena, name: *const c_char) -> Type;
    fn cbm_type_infer(arena: Arena, name: *const c_char) -> Type;
    fn cbm_type_typeof_query(arena: Arena, expr: *const c_char) -> Type;
    fn cbm_type_keyof(arena: Arena, operand: Type) -> Type;
    fn cbm_type_conditional(
        arena: Arena,
        check: Type,
        extends: Type,
        yes: Type,
        no: Type,
    ) -> Type;
    fn cbm_type_func(
        arena: Arena,
        names: *const *const c_char,
        types: *const Type,
        returns: *const Type,
    ) -> Type;
    fn cbm_type_tuple(arena: Arena, elems: *const Type, count: c_int) -> Type;
    fn cbm_type_union(arena: Arena, members: *const Type, count: c_int) -> Type;
    fn cbm_type_intersection(
        arena: Arena,
        members: *const Type,
        count: c_int,
    ) -> Type;
    fn cbm_type_template(
        arena: Arena,
        name: *const c_char,
        args: *const Type,
        count: c_int,
    ) -> Type;
}

impl Name<'_> {
    fn c_string(self) -> CString {
        let bytes = match self {
            Self::Bare(name) => name.to_vec(),
            Self::Qualified(module, name) => [module, b".", name].concat(),
        };
        let name = CString::new(bytes).expect("name comes from a C string");
        mark_written(name.as_bytes_with_nul());
        name
    }
}

fn mark_written<T>(values: &[T]) {
    if !values.is_empty() {
        // SAFETY: Only initialized bytes in this live slice are marked for MSan.
        unsafe {
            cbm_ffi_mark_written(values.as_ptr().cast(), size_of_val(values));
        }
    }
}

unsafe fn emit(arena: Arena, shape: Shape<'_>, children: &[Type]) -> Type {
    let c_string = |bytes: &[u8]| Name::Bare(bytes).c_string();
    mark_written(children);
    // SAFETY: The caller lends the arena for the whole parse. Every child is
    // an arena type or the C unknown singleton. The constructors copy strings
    // and arrays before returning. The function constructor copies the name
    // pointers, so each parameter name must be allocated in the arena first.
    unsafe {
        match shape {
            Shape::Unknown => cbm_type_unknown(),
            Shape::Named(name) => {
                cbm_type_named(arena, name.c_string().as_ptr())
            }
            Shape::Builtin(name) => {
                cbm_type_builtin(arena, c_string(name).as_ptr())
            }
            Shape::This => cbm_type_type_param(arena, c"this".as_ptr()),
            Shape::Infer(name) => cbm_type_infer(arena, c_string(name).as_ptr()),
            Shape::Typeof(expr) => {
                cbm_type_typeof_query(arena, c_string(expr).as_ptr())
            }
            Shape::Keyof => cbm_type_keyof(arena, children[0]),
            Shape::Conditional => cbm_type_conditional(
                arena,
                children[0],
                children[1],
                children[2],
                children[3],
            ),
            Shape::Function(params) => {
                let mut names = [null(); MAX_MEMBERS + 1];
                let mut types = [null(); MAX_MEMBERS + 1];
                for (i, param) in params.into_iter().enumerate() {
                    let positional;
                    let name = match param {
                        Param::Explicit(name) => name,
                        Param::Positional(index) => {
                            positional = format!("_{index}");
                            positional.as_bytes()
                        }
                    };
                    mark_written(name);
                    names[i] = cbm_arena_strndup(
                        arena,
                        name.as_ptr().cast(),
                        name.len(),
                    );
                    types[i] = children[i + 1];
                }
                let returns = [children[0], null()];
                mark_written(&names);
                mark_written(&types);
                mark_written(&returns);
                cbm_type_func(
                    arena,
                    names.as_ptr(),
                    types.as_ptr(),
                    returns.as_ptr(),
                )
            }
            Shape::Tuple => {
                cbm_type_tuple(arena, children.as_ptr(), children.len() as c_int)
            }
            Shape::Union => {
                cbm_type_union(arena, children.as_ptr(), children.len() as c_int)
            }
            Shape::Intersection => cbm_type_intersection(
                arena,
                children.as_ptr(),
                children.len() as c_int,
            ),
            Shape::Template(name) => cbm_type_template(
                arena,
                name.c_string().as_ptr(),
                children.as_ptr(),
                children.len() as c_int,
            ),
        }
    }
}

/// The caller supplies a live CBM arena and a writable, thread-local budget.
/// Each string is either null or a readable NUL-terminated C string.
#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_ts_parse_type_text(
    arena: Arena,
    text: *const c_char,
    module: *const c_char,
    budget: *mut c_long,
) -> Type {
    // SAFETY: The C caller holds all three borrows until parsing finishes.
    unsafe {
        let text = if text.is_null() {
            &[]
        } else {
            CStr::from_ptr(text).to_bytes()
        };
        let module =
            (!module.is_null()).then(|| CStr::from_ptr(module).to_bytes());
        let budget = &mut *budget;
        parse(
            text,
            module,
            |len| charge_budget(budget, len),
            |shape, children| emit(arena, shape, children),
        )
    }
}
