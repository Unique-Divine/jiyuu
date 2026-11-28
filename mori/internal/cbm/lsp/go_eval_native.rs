//! Read-only views of the native type and registry data used by Go evaluation.

use std::cell::Cell;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr;

use crate::native_tree::{BorrowedNode, RawNode};
use crate::resolver::{ResolveStatus, SourceSite};

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
const TYPE_INTERFACE: u32 = 7;
const TYPE_BUILTIN: u32 = 9;
const TYPE_TUPLE: u32 = 10;
const TYPE_PARAM: u32 = 11;

/// The fields of the native type union read by Go expression evaluation.
#[repr(C)]
pub(crate) struct RawType {
    kind: u32,
    data: Data,
}

#[repr(C)]
union Data {
    name: *const c_char,
    element: Type,
    map: Map,
    signature: Signature,
    tuple: Tuple,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct Map {
    key: Type,
    value: Type,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct Tuple {
    elems: TypeList,
    count: c_int,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub(crate) struct Signature {
    _param_names: *const *const c_char,
    params: TypeList,
    returns: TypeList,
}

pub(crate) type Type = *const RawType;
type TypeList = *const Type;
pub(super) type NameList = *const *const c_char;

/// The first five fields of CBMRegisteredFunc, through the generic parameters.
#[repr(C)]
struct Function {
    qualified_name: *const c_char,
    receiver_type: *const c_char,
    _short_name: *const c_char,
    signature: Type,
    type_params: NameList,
}

/// The native registry lends the type prefix through its interface flag.
#[repr(C)]
struct RawRegisteredType {
    qualified_name: *const c_char,
    _short_name: *const c_char,
    field_names: NameList,
    field_types: TypeList,
    method_names: NameList,
    _method_qns: NameList,
    embedded_types: NameList,
    alias_of: *const c_char,
    _type_param_names: NameList,
    is_interface: bool,
}

#[derive(Clone, Copy)]
pub(super) struct RegisteredType<'file>(&'file RawRegisteredType);

impl<'file> RegisteredType<'file> {
    pub(super) fn name(self) -> Option<&'file CStr> {
        unsafe { optional_name(self.0.qualified_name) }
    }

    pub(super) fn alias(self) -> Option<&'file CStr> {
        unsafe { optional_name(self.0.alias_of) }
    }

    pub(super) fn embeds(self) -> impl Iterator<Item = &'file CStr> {
        terminated(self.0.embedded_types).map(|ty| unsafe { CStr::from_ptr(ty) })
    }

    pub(super) fn field(self, name: &CStr) -> Option<Type> {
        let types = self.0.field_types;
        if types.is_null() {
            return None;
        }
        terminated(self.0.field_names)
            .enumerate()
            .find_map(|(index, field)| {
                if unsafe { CStr::from_ptr(field) } != name {
                    return None;
                }
                // SAFETY: The registry lends type pointers parallel to the field names.
                let ty = unsafe { *types.add(index) };
                (!ty.is_null()).then_some(ty)
            })
    }

    pub(super) fn method_names(self) -> NameList {
        self.0.method_names
    }

    pub(super) fn is_interface(self) -> bool {
        self.0.is_interface
    }
}

unsafe fn optional_name<'file>(ptr: *const c_char) -> Option<&'file CStr> {
    // SAFETY: The caller borrows a C string or passes NULL.
    (!ptr.is_null()).then(|| unsafe { CStr::from_ptr(ptr) })
}

#[repr(C)]
struct Output {
    target: *const c_char,
    strategy: *const c_char,
    reason: *const c_char,
    confidence: f32,
    kind: u32,
    site: SourceSite,
}

pub(crate) enum TypeView<'file> {
    Unknown,
    Named(&'file CStr),
    Builtin(&'file CStr),
    Pointer(Type),
    Slice(Type),
    Map(Type, Type),
    Channel(Type),
    Function(Signature),
    Interface,
    Parameter(&'file CStr),
    Other,
}

/// The caller keeps this native type and its children alive during the view.
pub(crate) unsafe fn view_type<'a>(ty: Type) -> TypeView<'a> {
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
            TYPE_INTERFACE => TypeView::Interface,
            TYPE_BUILTIN if !(*ty).data.name.is_null() => {
                TypeView::Builtin(CStr::from_ptr((*ty).data.name))
            }
            TYPE_PARAM if !(*ty).data.name.is_null() => {
                TypeView::Parameter(CStr::from_ptr((*ty).data.name))
            }
            _ => TypeView::Other,
        }
    }
}

impl Signature {
    pub(crate) fn has_parameters(self) -> bool {
        !self.params.is_null()
    }

    // A signature view is valid while the current arena or registry lives.
    pub(crate) fn params(self) -> impl Iterator<Item = Type> {
        terminated(self.params)
    }

    pub(crate) fn returns(self) -> impl Iterator<Item = Type> {
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

/// The host owns arena storage, scope state, and result publication.
#[repr(C)]
struct Operations {
    scope_lookup:
        unsafe extern "C" fn(*mut GoContext, *const c_char, *mut Type) -> bool,
    import: unsafe extern "C" fn(*mut GoContext, *const c_char) -> *const c_char,
    parse_type: unsafe extern "C" fn(*mut GoContext, *const RawNode) -> Type,
    push_scope: unsafe extern "C" fn(*mut GoContext, *mut *mut Scope) -> bool,
    pop_scope: unsafe extern "C" fn(*mut GoContext, *mut Scope),
    bind: unsafe extern "C" fn(*mut GoContext, *const c_char, Type) -> bool,
    bind_value: unsafe extern "C" fn(
        *mut GoContext,
        *const c_char,
        Type,
        *const c_char,
    ) -> bool,
    set_caller: unsafe extern "C" fn(*mut GoContext, *const c_char),
    scope_callable:
        unsafe extern "C" fn(*mut GoContext, *const c_char) -> *const c_char,
    clear_callable: unsafe extern "C" fn(*mut GoContext, *const c_char),
    emit: unsafe extern "C" fn(*mut GoContext, *const Output) -> bool,
    cancelled: unsafe extern "C" fn(*mut GoContext) -> bool,
}

#[repr(C)]
struct Input {
    root: RawNode,
    source: *const u8,
    source_len: usize,
    package: *const c_char,
    path: *const c_char,
    arena: *mut Arena,
    registry: *const Registry,
    context: *mut GoContext,
    operations: *const Operations,
    depth_limit: usize,
    import_names: NameList,
    import_values: NameList,
    import_count: usize,
}

unsafe extern "C" {
    pub(crate) fn cbm_type_unknown() -> Type;
    pub(crate) fn cbm_type_named(arena: *mut Arena, name: *const c_char)
    -> Type;
    pub(crate) fn cbm_type_builtin(
        arena: *mut Arena,
        name: *const c_char,
    ) -> Type;
    pub(crate) fn cbm_type_pointer(arena: *mut Arena, element: Type) -> Type;
    pub(crate) fn cbm_type_slice(arena: *mut Arena, element: Type) -> Type;
    fn cbm_type_deref(ty: Type) -> Type;
    fn cbm_registry_lookup_type(
        registry: *const Registry,
        name: *const c_char,
    ) -> *const RawRegisteredType;
    fn cbm_registry_lookup_method(
        registry: *const Registry,
        ty: *const c_char,
        name: *const c_char,
    ) -> *const Function;
    fn cbm_go_eval_visit_types(
        registry: *const Registry,
        context: *mut c_void,
        visit: unsafe extern "C" fn(
            *mut c_void,
            *const RawRegisteredType,
        ) -> bool,
    );
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
    pub(crate) fn cbm_go_eval_copy_name(
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
    pub path: Option<&'file CStr>,
    pub depth_limit: usize,
    arena: *mut Arena,
    registry: *const Registry,
    context: *mut GoContext,
    ops: &'file Operations,
    imports: (&'file [*const c_char], &'file [*const c_char]),
    walk_depth: Cell<usize>,
    status: Cell<ResolveStatus>,
}

#[derive(Clone, Copy)]
pub(super) struct Callable<'file> {
    pub qualified_name: Option<&'file CStr>,
    pub receiver_type: Option<&'file CStr>,
    pub signature: Type,
    pub type_params: NameList,
}

impl Callable<'_> {
    pub(super) fn has_signature(&self) -> bool {
        !self.signature.is_null()
    }
}

impl<'file> File<'file> {
    pub(super) fn unknown(&self) -> Type {
        unsafe { cbm_type_unknown() }
    }

    pub(super) fn view(&self, ty: Type) -> TypeView<'file> {
        // SAFETY: The type lives in the current file or shared registry arena.
        unsafe { view_type(ty) }
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
        if name.is_null() {
            self.fail();
            None
        } else {
            Some(unsafe { CStr::from_ptr(name) })
        }
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

    fn callable(&self, func: *const Function) -> Option<Callable<'file>> {
        // SAFETY: The registry lends the initialized function prefix.
        (!func.is_null()).then(|| unsafe {
            Callable {
                qualified_name: optional_name((*func).qualified_name),
                receiver_type: optional_name((*func).receiver_type),
                signature: (*func).signature,
                type_params: (*func).type_params,
            }
        })
    }

    pub(super) fn function(&self, name: &CStr) -> Option<Callable<'file>> {
        self.callable(unsafe {
            cbm_registry_lookup_func(self.registry, name.as_ptr())
        })
    }

    pub(super) fn symbol(
        &self,
        package: &CStr,
        name: &CStr,
    ) -> Option<Callable<'file>> {
        self.callable(unsafe {
            cbm_registry_lookup_symbol(
                self.registry,
                package.as_ptr(),
                name.as_ptr(),
            )
        })
    }

    pub(super) fn registry_type(
        &self,
        name: &CStr,
    ) -> Option<RegisteredType<'file>> {
        let ty =
            unsafe { cbm_registry_lookup_type(self.registry, name.as_ptr()) };
        (!ty.is_null()).then(|| RegisteredType(unsafe { &*ty }))
    }

    pub(super) fn has_named_type(&self, name: &CStr) -> bool {
        self.registry_type(name).is_some()
    }

    pub(super) fn direct_method(
        &self,
        ty: &CStr,
        name: &CStr,
    ) -> Option<Callable<'file>> {
        self.callable(unsafe {
            cbm_registry_lookup_method(self.registry, ty.as_ptr(), name.as_ptr())
        })
    }

    pub(super) fn imports(
        &self,
    ) -> impl Iterator<Item = (&'file CStr, &'file CStr)> {
        self.imports
            .0
            .iter()
            .copied()
            .zip(self.imports.1.iter().copied())
            .filter_map(|(alias, target)| unsafe {
                Some((optional_name(alias)?, optional_name(target)?))
            })
    }

    pub(super) fn for_each_type<F>(&self, mut visitor: F)
    where
        F: FnMut(RegisteredType<'file>) -> bool,
    {
        unsafe extern "C" fn visit<'file, F>(
            ctx: *mut c_void,
            ty: *const RawRegisteredType,
        ) -> bool
        where
            F: FnMut(RegisteredType<'file>) -> bool,
        {
            // SAFETY: The callback borrows a registry row and the live visitor.
            unsafe { (&mut *ctx.cast::<F>())(RegisteredType(&*ty)) }
        }
        unsafe {
            cbm_go_eval_visit_types(
                self.registry,
                (&raw mut visitor).cast(),
                visit::<F>,
            )
        };
    }

    pub(super) fn scope_callable(&self, name: &CStr) -> Option<&'file CStr> {
        unsafe {
            optional_name((self.ops.scope_callable)(self.context, name.as_ptr()))
        }
    }

    pub(super) fn clear_callable(&self, name: &CStr) {
        unsafe { (self.ops.clear_callable)(self.context, name.as_ptr()) };
    }

    pub(super) fn emit(
        &self,
        node: BorrowedNode<'_>,
        target: &CStr,
        strategy: &'static CStr,
        confidence: f32,
        reason: Option<&CStr>,
        kind: u32,
    ) {
        let range = node.range();
        let output = Output {
            target: target.as_ptr(),
            strategy: strategy.as_ptr(),
            reason: reason.map_or(ptr::null(), CStr::as_ptr),
            confidence,
            kind,
            site: SourceSite {
                start: range.start as u32,
                end: range.end as u32,
                origin: 0,
            },
        };
        if self.keep_working()
            && !unsafe { (self.ops.emit)(self.context, &output) }
        {
            self.fail();
        }
    }

    pub(super) fn fail(&self) {
        if self.status.get() == ResolveStatus::Complete {
            self.status.set(ResolveStatus::Failed);
        }
    }

    pub(super) fn keep_working(&self) -> bool {
        if self.status.get() != ResolveStatus::Complete {
            return false;
        }
        // SAFETY: This callback reads the live pipeline token only.
        if unsafe { (self.ops.cancelled)(self.context) } {
            self.status.set(ResolveStatus::Cancelled);
            return false;
        }
        true
    }

    pub(super) fn push_scope(&self) -> Option<ScopeGuard<'_, 'file>> {
        let mut parent = ptr::null_mut();
        if !unsafe { (self.ops.push_scope)(self.context, &mut parent) } {
            self.fail();
            return None;
        }
        Some(ScopeGuard { file: self, parent })
    }

    pub(super) fn bind(&self, name: &CStr, ty: Type) {
        if !unsafe { (self.ops.bind)(self.context, name.as_ptr(), ty) } {
            self.fail();
        }
    }

    /// Reports whether work at `depth` may continue. Past the depth limit,
    /// the caller skips that subtree (walk) or yields `Unknown` (evaluation),
    /// and the rest of the file still resolves, as in the C walker. Only
    /// cancellation or an earlier failure stops the whole file.
    pub(super) fn within_depth(&self, depth: usize) -> bool {
        self.keep_working() && depth < self.depth_limit
    }

    pub(super) fn walk(&self, node: BorrowedNode<'_>) {
        let depth = self.walk_depth.get();
        if !self.within_depth(depth) {
            return;
        }
        self.walk_depth.set(depth + 1);
        super::walk::resolve(self, node);
        self.walk_depth.set(depth);
    }

    pub(super) fn parse_type(&self, node: &RawNode) -> Type {
        unsafe { (self.ops.parse_type)(self.context, node) }
    }

    pub(super) fn bind_value(
        &self,
        name: &CStr,
        ty: Type,
        callable: Option<&CStr>,
    ) {
        if !unsafe {
            (self.ops.bind_value)(
                self.context,
                name.as_ptr(),
                ty,
                callable.map_or(ptr::null(), CStr::as_ptr),
            )
        } {
            self.fail();
        }
    }

    pub(super) fn set_caller(&self, caller: &CStr) {
        unsafe { (self.ops.set_caller)(self.context, caller.as_ptr()) };
    }

    pub(super) fn tuple_element(&self, ty: Type, index: usize) -> Option<Type> {
        // SAFETY: The discriminator and tuple data borrow the current native arena.
        unsafe {
            if ty.is_null() || (*ty).kind != TYPE_TUPLE {
                return None;
            }
            let tuple = (*ty).data.tuple;
            if index >= usize::try_from(tuple.count).ok()?
                || tuple.elems.is_null()
            {
                return None;
            }
            Some(*tuple.elems.add(index))
        }
    }

    pub(super) fn slice(&self, element: Type) -> Type {
        unsafe { cbm_type_slice(self.arena, element) }
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

unsafe fn with_input<T>(
    input: *const Input,
    operation: impl FnOnce(&File<'_>, crate::native_tree::BorrowedNode<'_>) -> T,
) -> T {
    // SAFETY: C lends the source and tree, arena, and current registry context.
    let input = unsafe { &*input };
    let file = File {
        source: unsafe {
            std::slice::from_raw_parts(input.source, input.source_len)
        },
        package: unsafe { CStr::from_ptr(input.package) },
        path: unsafe { optional_name(input.path) },
        depth_limit: input.depth_limit,
        arena: input.arena,
        registry: input.registry,
        context: input.context,
        ops: unsafe { &*input.operations },
        imports: unsafe {
            if input.import_count == 0 {
                (&[], &[])
            } else {
                (
                    std::slice::from_raw_parts(
                        input.import_names,
                        input.import_count,
                    ),
                    std::slice::from_raw_parts(
                        input.import_values,
                        input.import_count,
                    ),
                )
            }
        },
        walk_depth: Cell::new(0),
        status: Cell::new(ResolveStatus::Complete),
    };
    let root =
        unsafe { crate::native_tree::BorrowedNode::from_root(&input.root) };
    operation(&file, root)
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_go_walk_file(input: *const Input) -> ResolveStatus {
    unsafe {
        with_input(input, |file, root| {
            if file.keep_working() {
                super::bindings::file(file, root);
                file.keep_working();
            }
            file.status.get()
        })
    }
}
