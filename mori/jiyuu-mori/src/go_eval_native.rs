//! Read-only views of the native type and registry data used by Go evaluation.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr;

use crate::native_tree::RawNode;

type Arena = c_void;
type Registry = c_void;
type GoContext = c_void;
type Scope = c_void;

const TYPE_UNKNOWN: u32 = 0;
const TYPE_NAMED: u32 = 1;
const TYPE_POINTER: u32 = 2;
const TYPE_SLICE: u32 = 3;
const TYPE_MAP: u32 = 4;
const TYPE_CHANNEL: u32 = 5;
const TYPE_FUNC: u32 = 6;
const TYPE_PARAM: u32 = 11;

/// The fields of the native type union read by Go expression evaluation.
#[repr(C)]
pub(super) struct RawType {
    kind: u32,
    data: Data,
}

#[repr(C)]
union Data {
    name: *const c_char,
    element: Type,
    map: Map,
    signature: Signature,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct Map {
    key: Type,
    value: Type,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub(super) struct Signature {
    _param_names: *const *const c_char,
    params: TypeList,
    returns: TypeList,
}

pub(super) type Type = *const RawType;
type TypeList = *const Type;
pub(super) type NameList = *const *const c_char;

/// The first five fields of CBMRegisteredFunc, through the generic parameters.
#[repr(C)]
struct Function {
    _qualified_name: *const c_char,
    _receiver_type: *const c_char,
    _short_name: *const c_char,
    signature: Type,
    type_params: NameList,
}

pub(super) enum TypeView<'file> {
    Unknown,
    Named(&'file CStr),
    Pointer(Type),
    Slice(Type),
    Map(Type, Type),
    Channel(Type),
    Function(Signature),
    Parameter(&'file CStr),
    Other,
}

impl Signature {
    // A signature view is valid while the current arena or registry lives.
    pub(super) fn params(self) -> impl Iterator<Item = Type> {
        terminated(self.params)
    }

    pub(super) fn returns(self) -> impl Iterator<Item = Type> {
        terminated(self.returns)
    }
}

fn terminated<T>(items: *const *const T) -> impl Iterator<Item = *const T> {
    let mut cursor = items;
    std::iter::from_fn(move || {
        if cursor.is_null() {
            return None;
        }
        // SAFETY: The native context lends a terminated array in its arena.
        let item = unsafe { *cursor };
        if item.is_null() {
            cursor = ptr::null();
            return None;
        }
        cursor = unsafe { cursor.add(1) };
        Some(item)
    })
}

/// The host callbacks expose native arena storage, lexical scope state, and
/// existing statement traversal while Rust owns expression evaluation.
#[repr(C)]
struct Operations {
    scope_lookup:
        unsafe extern "C" fn(*mut GoContext, *const c_char, *mut Type) -> bool,
    import: unsafe extern "C" fn(*mut GoContext, *const c_char) -> *const c_char,
    field: unsafe extern "C" fn(
        *mut GoContext,
        *const c_char,
        *const c_char,
    ) -> Type,
    method: unsafe extern "C" fn(
        *mut GoContext,
        *const c_char,
        *const c_char,
    ) -> *const Function,
    parse_type: unsafe extern "C" fn(*mut GoContext, *const RawNode) -> Type,
    push_scope: unsafe extern "C" fn(*mut GoContext) -> *mut Scope,
    pop_scope: unsafe extern "C" fn(*mut GoContext, *mut Scope),
    bind: unsafe extern "C" fn(*mut GoContext, *const c_char, Type),
    walk: unsafe extern "C" fn(*mut GoContext, *const RawNode),
}

#[repr(C)]
struct Input {
    root: RawNode,
    source: *const u8,
    source_len: usize,
    package: *const c_char,
    arena: *mut Arena,
    registry: *const Registry,
    context: *mut GoContext,
    operations: *const Operations,
    depth_limit: usize,
}

unsafe extern "C" {
    fn cbm_type_unknown() -> Type;
    fn cbm_type_named(arena: *mut Arena, name: *const c_char) -> Type;
    fn cbm_type_builtin(arena: *mut Arena, name: *const c_char) -> Type;
    fn cbm_type_pointer(arena: *mut Arena, element: Type) -> Type;
    fn cbm_type_deref(ty: Type) -> Type;
    fn cbm_registry_lookup_type(
        registry: *const Registry,
        name: *const c_char,
    ) -> *const c_void;
    fn cbm_registry_lookup_func(
        registry: *const Registry,
        name: *const c_char,
    ) -> *const Function;
    fn cbm_registry_lookup_symbol(
        registry: *const Registry,
        package: *const c_char,
        name: *const c_char,
    ) -> *const Function;

    // Bridge Rust-owned lists and strings through C with explicit sizes for MSan.
    fn cbm_go_eval_copy_name(
        arena: *mut Arena,
        data: *const u8,
        len: usize,
    ) -> *const c_char;
    fn cbm_go_eval_func(
        arena: *mut Arena,
        params: TypeList,
        param_count: usize,
        returns: TypeList,
        return_count: usize,
    ) -> Type;
    fn cbm_go_eval_tuple(
        arena: *mut Arena,
        values: TypeList,
        count: c_int,
    ) -> Type;
    fn cbm_go_eval_substitute(
        arena: *mut Arena,
        ty: Type,
        names: NameList,
        values: TypeList,
        count: usize,
    ) -> Type;
}

pub(super) struct File<'file> {
    pub source: &'file [u8],
    pub package: &'file CStr,
    pub depth_limit: usize,
    arena: *mut Arena,
    registry: *const Registry,
    context: *mut GoContext,
    ops: &'file Operations,
}

#[derive(Clone, Copy)]
pub(super) struct Callable {
    pub signature: Type,
    pub type_params: NameList,
}

impl Callable {
    pub(super) fn has_signature(&self) -> bool {
        !self.signature.is_null()
    }
}

impl<'file> File<'file> {
    pub(super) fn unknown(&self) -> Type {
        unsafe { cbm_type_unknown() }
    }

    pub(super) fn view(&self, ty: Type) -> TypeView<'file> {
        if ty.is_null() {
            return TypeView::Unknown;
        }
        // SAFETY: A type in this file or project outlives this traversal.
        // A discriminator determines which initialized union field to read.
        unsafe {
            match (*ty).kind {
                TYPE_UNKNOWN => TypeView::Unknown,
                TYPE_NAMED if !(*ty).data.name.is_null() => {
                    TypeView::Named(CStr::from_ptr((*ty).data.name))
                }
                TYPE_POINTER => TypeView::Pointer((*ty).data.element),
                TYPE_SLICE => TypeView::Slice((*ty).data.element),
                TYPE_MAP => {
                    let Map { key, value } = (*ty).data.map;
                    TypeView::Map(key, value)
                }
                TYPE_CHANNEL => TypeView::Channel((*ty).data.element),
                TYPE_FUNC => TypeView::Function((*ty).data.signature),
                TYPE_PARAM if !(*ty).data.name.is_null() => {
                    TypeView::Parameter(CStr::from_ptr((*ty).data.name))
                }
                _ => TypeView::Other,
            }
        }
    }

    pub(super) fn names(
        &self,
        names: NameList,
    ) -> impl Iterator<Item = &'file CStr> {
        // SAFETY: The native registry owns the NUL-terminated names.
        terminated(names).map(|name| unsafe { CStr::from_ptr(name) })
    }

    pub(super) fn name(&self, bytes: &[u8]) -> Option<&'file CStr> {
        let bytes = bytes.split(|&byte| byte == 0).next().unwrap_or_default();
        // SAFETY: The arena copies the bytes and retains the NUL-terminated result.
        let name = unsafe {
            cbm_go_eval_copy_name(self.arena, bytes.as_ptr(), bytes.len())
        };
        (!name.is_null()).then(|| unsafe { CStr::from_ptr(name) })
    }

    pub(super) fn scope_lookup(&self, name: &CStr) -> Option<Type> {
        let mut ty = ptr::null();
        // SAFETY: The live lexical scope is borrowed through the original context.
        unsafe { (self.ops.scope_lookup)(self.context, name.as_ptr(), &mut ty) }
            .then(|| if ty.is_null() { self.unknown() } else { ty })
    }

    pub(super) fn import(&self, name: &CStr) -> Option<&'file CStr> {
        let qn = unsafe { (self.ops.import)(self.context, name.as_ptr()) };
        (!qn.is_null()).then(|| unsafe { CStr::from_ptr(qn) })
    }

    fn callable(&self, func: *const Function) -> Option<Callable> {
        // SAFETY: The registry lends the initialized function prefix.
        (!func.is_null()).then(|| unsafe {
            Callable {
                signature: (*func).signature,
                type_params: (*func).type_params,
            }
        })
    }

    pub(super) fn function(&self, name: &CStr) -> Option<Callable> {
        self.callable(unsafe {
            cbm_registry_lookup_func(self.registry, name.as_ptr())
        })
    }

    pub(super) fn symbol(
        &self,
        package: &CStr,
        name: &CStr,
    ) -> Option<Callable> {
        self.callable(unsafe {
            cbm_registry_lookup_symbol(
                self.registry,
                package.as_ptr(),
                name.as_ptr(),
            )
        })
    }

    pub(super) fn has_named_type(&self, name: &CStr) -> bool {
        !unsafe { cbm_registry_lookup_type(self.registry, name.as_ptr()) }
            .is_null()
    }

    pub(super) fn method(&self, ty: &CStr, name: &CStr) -> Option<Callable> {
        self.callable(unsafe {
            (self.ops.method)(self.context, ty.as_ptr(), name.as_ptr())
        })
    }

    pub(super) fn field(&self, ty: &CStr, name: &CStr) -> Type {
        unsafe { (self.ops.field)(self.context, ty.as_ptr(), name.as_ptr()) }
    }

    pub(super) fn push_scope(&self) -> ScopeGuard<'_, 'file> {
        let parent = unsafe { (self.ops.push_scope)(self.context) };
        ScopeGuard { file: self, parent }
    }

    pub(super) fn bind(&self, name: &CStr, ty: Type) {
        unsafe { (self.ops.bind)(self.context, name.as_ptr(), ty) }
    }

    pub(super) fn walk(&self, node: &RawNode) {
        unsafe { (self.ops.walk)(self.context, node) };
    }

    pub(super) fn parse_type(&self, node: &RawNode) -> Type {
        unsafe { (self.ops.parse_type)(self.context, node) }
    }

    pub(super) fn pointer(&self, element: Type) -> Type {
        unsafe { cbm_type_pointer(self.arena, element) }
    }

    pub(super) fn deref(&self, ty: Type) -> Type {
        unsafe { cbm_type_deref(ty) }
    }

    pub(super) fn named(&self, name: &CStr) -> Type {
        unsafe { cbm_type_named(self.arena, name.as_ptr()) }
    }

    pub(super) fn builtin(&self, name: &'static CStr) -> Type {
        unsafe { cbm_type_builtin(self.arena, name.as_ptr()) }
    }

    pub(super) fn func(&self, params: &[Type], returns: &[Type]) -> Type {
        unsafe {
            cbm_go_eval_func(
                self.arena,
                params.as_ptr(),
                params.len(),
                returns.as_ptr(),
                returns.len(),
            )
        }
    }

    pub(super) fn tuple(&self, values: &[Type]) -> Type {
        let Ok(count) = c_int::try_from(values.len()) else {
            return self.unknown();
        };
        unsafe { cbm_go_eval_tuple(self.arena, values.as_ptr(), count) }
    }

    pub(super) fn substitute(
        &self,
        ty: Type,
        names: NameList,
        values: &[Type],
    ) -> Type {
        unsafe {
            cbm_go_eval_substitute(
                self.arena,
                ty,
                names,
                values.as_ptr(),
                values.len(),
            )
        }
    }
}

pub(super) struct ScopeGuard<'borrow, 'file> {
    file: &'borrow File<'file>,
    parent: *mut Scope,
}

impl Drop for ScopeGuard<'_, '_> {
    fn drop(&mut self) {
        unsafe { (self.file.ops.pop_scope)(self.file.context, self.parent) };
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_go_eval_expr(input: *const Input) -> Type {
    // SAFETY: C lends the source and tree, arena, and current registry context.
    let input = unsafe { &*input };
    let file = File {
        source: unsafe {
            std::slice::from_raw_parts(input.source, input.source_len)
        },
        package: unsafe { CStr::from_ptr(input.package) },
        depth_limit: input.depth_limit,
        arena: input.arena,
        registry: input.registry,
        context: input.context,
        ops: unsafe { &*input.operations },
    };
    let root =
        unsafe { crate::native_tree::BorrowedNode::from_root(&input.root) };
    super::evaluate(&file, Some(root), 0)
}
