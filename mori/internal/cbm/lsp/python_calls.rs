//! Choose Python invocation targets and their graph-join provenance.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr;

use crate::native_tree::{BorrowedNode as Node, RawNode};

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Name {
    data: *const c_char,
    size: usize,
}

impl From<&CStr> for Name {
    fn from(text: &CStr) -> Self {
        Self {
            data: text.as_ptr(),
            size: text.to_bytes_with_nul().len(),
        }
    }
}

impl Name {
    const NONE: Self = Self {
        data: ptr::null(),
        size: 0,
    };
}

type Context = *mut c_void;
type Spelling = *const c_char;
type Type = *const c_void;

#[repr(C)]
struct Binding {
    present: bool,
    callable: Spelling,
    named: Spelling,
}

#[repr(C)]
struct Receiver {
    kind: u32,
    name: Spelling,
    type_: Type,
    members: usize,
}

const NAMED: u32 = 1;
const BUILTIN: u32 = 2;
const TEMPLATE: u32 = 3;
const MODULE: u32 = 4;
const UNION: u32 = 5;

#[repr(C)]
struct Function {
    name: Spelling,
    exact: bool,
}

#[repr(C)]
struct ModuleMember {
    function: Function,
    class: Spelling,
}

#[repr(C)]
struct Import {
    index: c_int,
    target: Spelling,
}

#[repr(C)]
struct Proof {
    target: Spelling,
    lexical_alias: bool,
}

struct Callable<'a> {
    target: &'a CStr,
    lexical_alias: bool,
}

#[repr(C)]
struct SuperTarget {
    method: Spelling,
    constructor_base: Spelling,
}

#[repr(C)]
struct Native {
    text: unsafe extern "C" fn(Context, *const RawNode) -> Spelling,
    lambda: unsafe extern "C" fn(Context, Name, *const RawNode) -> bool,
    scope: unsafe extern "C" fn(Context, Name) -> Binding,
    aliased_scope_named: unsafe extern "C" fn(Context, Name) -> Spelling,
    imported: unsafe extern "C" fn(Context, Name, c_int) -> Import,
    symbol: unsafe extern "C" fn(Context, Name, Name) -> Function,
    class: unsafe extern "C" fn(Context, Name) -> Spelling,
    method: unsafe extern "C" fn(Context, Name, Name, bool) -> Function,
    dispatch: unsafe extern "C" fn(Context, Name, *const RawNode) -> Spelling,
    super_target: unsafe extern "C" fn(Context, Name) -> SuperTarget,
    receiver: unsafe extern "C" fn(Context, *const RawNode, bool) -> Receiver,
    union_member: unsafe extern "C" fn(Type, usize) -> Spelling,
    module_member: unsafe extern "C" fn(Context, Name, Name) -> ModuleMember,
    proof_enabled: unsafe extern "C" fn(Context) -> bool,
    field: unsafe extern "C" fn(Context, Name, Name) -> bool,
    emit: unsafe extern "C" fn(Context, *const RawNode, Name, Name, f32, Name),
    reference: unsafe extern "C" fn(Context, *const RawNode, Name, Name, Name),
    unresolved_reference: unsafe extern "C" fn(Context, *const RawNode, Name),
}

struct File<'a> {
    context: Context,
    native: &'a Native,
    module: Option<&'a CStr>,
    class_body: bool,
}

fn leaf(qn: &CStr) -> &[u8] {
    qn.to_bytes()
        .rsplit(|&byte| byte == b'.')
        .next()
        .unwrap_or_default()
}

fn qualified(left: &CStr, right: &CStr) -> CString {
    let bytes = [left.to_bytes(), b".", right.to_bytes_with_nul()].concat();
    // SAFETY: The only NUL is the terminator of right.
    unsafe { CString::from_vec_with_nul_unchecked(bytes) }
}

fn unique_method<'a>(
    methods: impl Iterator<Item = &'a CStr>,
) -> Option<&'a CStr> {
    let mut target = None;
    let mut ambiguous = false;
    for method in methods {
        ambiguous |= target.replace(method).is_some();
    }
    if ambiguous { None } else { target }
}

fn project_initializer<'a>(
    exact_spelling: bool,
    lookup: impl FnOnce() -> Option<&'a CStr>,
) -> Option<&'a CStr> {
    exact_spelling
        .then(lookup)
        .flatten()
        .filter(|name| !name.to_bytes().starts_with(b"builtins."))
}

impl File<'_> {
    fn name(&self, spelling: Spelling) -> Option<&CStr> {
        // SAFETY: The ABI returns nullable, arena-backed or static spellings.
        (!spelling.is_null()).then(|| unsafe { CStr::from_ptr(spelling) })
    }

    fn text(&self, node: Node<'_>) -> Option<&CStr> {
        // SAFETY: The node borrows the active tree.
        self.name(unsafe { (self.native.text)(self.context, node.as_raw()) })
    }

    fn emit(
        &self,
        call: Node<'_>,
        target: &CStr,
        strategy: &'static CStr,
        confidence: f32,
        reason: Option<&CStr>,
    ) {
        // SAFETY: The host copies temporary target and reason strings. Strategy
        // names are static literals at every call site.
        unsafe {
            (self.native.emit)(
                self.context,
                call.as_raw(),
                target.into(),
                strategy.into(),
                confidence,
                reason.map_or(Name::NONE, Name::from),
            )
        }
    }

    fn method_row(
        &self,
        class: &CStr,
        member: &CStr,
        inherited: bool,
    ) -> Function {
        // SAFETY: Both strings are borrowed during the lookup.
        unsafe {
            (self.native.method)(
                self.context,
                class.into(),
                member.into(),
                inherited,
            )
        }
    }

    fn method(
        &self,
        class: &CStr,
        member: &CStr,
        inherited: bool,
    ) -> Option<&CStr> {
        self.name(self.method_row(class, member, inherited).name)
    }

    fn symbol_row(&self, module: &CStr, name: &CStr) -> Function {
        // SAFETY: Both strings are borrowed during the lookup.
        unsafe { (self.native.symbol)(self.context, module.into(), name.into()) }
    }

    fn exact_function(&self, function: Function) -> Option<&CStr> {
        function.exact.then(|| self.name(function.name)).flatten()
    }

    fn imports<'a>(&'a self, name: &'a CStr) -> impl Iterator<Item = &'a CStr> {
        let name = Name::from(name);
        let mut index = -1;
        std::iter::from_fn(move || {
            // SAFETY: The name is valid until the caller consumes the iterator.
            let entry =
                unsafe { (self.native.imported)(self.context, name, index) };
            index = entry.index;
            self.name(entry.target)
        })
        .fuse()
    }

    fn scope(&self, name: &CStr) -> Binding {
        // SAFETY: The scope borrows this name during lookup.
        unsafe { (self.native.scope)(self.context, name.into()) }
    }

    fn receiver(&self, object: Node<'_>, resolve_alias: bool) -> Receiver {
        // SAFETY: The operand borrows the active tree. The evaluator retains
        // all returned type facts in the current file arena.
        unsafe {
            (self.native.receiver)(self.context, object.as_raw(), resolve_alias)
        }
    }

    fn module_entry(&self, module: &CStr, member: &CStr) -> ModuleMember {
        // SAFETY: Both names are borrowed throughout lookup.
        unsafe {
            (self.native.module_member)(
                self.context,
                module.into(),
                member.into(),
            )
        }
    }

    fn class(&self, name: &CStr) -> Option<&CStr> {
        // SAFETY: The name is borrowed during the lookup.
        self.name(unsafe { (self.native.class)(self.context, name.into()) })
    }

    fn identifier(&self, scope_name: &CStr, call: Node<'_>) {
        // Class execution and its locals cannot replay a legacy lambda body.
        // SAFETY: The host scopes the lambda execution and restores its frame
        // before returning. It borrows this call node and name for that replay.
        if !self.class_body
            && unsafe {
                (self.native.lambda)(
                    self.context,
                    scope_name.into(),
                    call.as_raw(),
                )
            }
        {
            return;
        }

        let binding = self.scope(scope_name);
        if binding.present {
            if let Some(target) = self.name(binding.callable) {
                self.emit(
                    call,
                    target,
                    c"lsp_callable_alias",
                    0.97,
                    Some(scope_name),
                );
            } else if let Some(qn) = self.name(binding.named) {
                self.named_binding(call, scope_name, qn);
            }
            // An unproven local binding shadows every module/global symbol.
            return;
        }

        if let Some(target) = self.module.and_then(|module| {
            self.name(self.symbol_row(module, scope_name).name)
        }) {
            self.emit(call, target, c"lsp_direct", 0.95, None);
        } else if let Some(target) =
            self.name(self.symbol_row(c"builtins", scope_name).name)
        {
            self.emit(call, target, c"lsp_builtin", 0.92, None);
        } else if let Some(target) =
            self.class(&qualified(c"builtins", scope_name))
        {
            self.emit(call, target, c"lsp_builtin_constructor", 0.88, None);
        }
    }

    fn named_binding(&self, call: Node<'_>, spelling: &CStr, qn: &CStr) {
        let exact_spelling = leaf(qn) == spelling.to_bytes();
        let imported =
            !exact_spelling && self.imports(spelling).any(|target| target == qn);
        if imported {
            self.emit(call, qn, c"lsp_import_alias", 0.85, Some(spelling));
        } else if self.class(qn).is_some() {
            // Inherited initializers and calls through instances keep the
            // original class target. Only the class's own project initializer
            // can carry the constructor-join spelling.
            let initializer = project_initializer(exact_spelling, || {
                self.method(qn, c"__init__", false)
            });
            if let Some(target) = initializer {
                self.emit(
                    call,
                    target,
                    c"lsp_constructor_init",
                    0.85,
                    Some(spelling),
                );
            } else {
                self.emit(call, qn, c"lsp_constructor", 0.85, None);
            }
        }
    }

    fn dictionary(&self, fn_node: Node<'_>, call: Node<'_>) -> Option<()> {
        let container = fn_node.field(c"value")?;
        let key = fn_node.field(c"subscript")?;
        if container.kind() != c"identifier" || key.kind() != c"string" {
            return None;
        }
        let name = self.text(container)?;
        // SAFETY: The key borrows the active tree. Native decodes the literal
        // the same way when recording the dictionary's keys.
        let target = self.name(unsafe {
            (self.native.dispatch)(self.context, name.into(), key.as_raw())
        })?;
        self.emit(call, target, c"lsp_dict_dispatch", 0.86, Some(name));
        Some(())
    }

    fn super_call(
        &self,
        call: Node<'_>,
        object: Node<'_>,
        member: &CStr,
    ) -> Option<()> {
        if object.kind() != c"call" {
            return None;
        }
        let fn_node = object.field(c"function")?;
        if fn_node.kind() != c"identifier" || self.text(fn_node)? != c"super" {
            return None;
        }
        // SAFETY: The member and active class context remain live.
        let result =
            unsafe { (self.native.super_target)(self.context, member.into()) };
        let synthesized;
        let target = if let Some(method) = self.name(result.method) {
            method
        } else {
            synthesized =
                qualified(self.name(result.constructor_base)?, c"__init__");
            &synthesized
        };
        let (strategy, confidence) = if member == c"__init__" {
            (c"lsp_super_init", 0.90)
        } else {
            (c"lsp_super", 0.88)
        };
        self.emit(call, target, strategy, confidence, None);
        Some(())
    }

    fn module_member(&self, call: Node<'_>, module: &CStr, member: &CStr) {
        let entry = self.module_entry(module, member);
        let entry = self
            .name(entry.function.name)
            .map(|f| (f, c"lsp_module_attr", 0.92))
            .or_else(|| {
                self.name(entry.class)
                    .map(|t| (t, c"lsp_constructor", 0.88))
            });
        if let Some((target, strategy, confidence)) = entry {
            if leaf(target) == member.to_bytes() {
                self.emit(call, target, strategy, confidence, None);
            } else {
                self.emit(
                    call,
                    target,
                    c"lsp_import_alias",
                    confidence,
                    Some(member),
                );
            }
        } else {
            self.emit(
                call,
                &qualified(module, member),
                c"lsp_module_attr_unresolved",
                0.55,
                None,
            );
        }
    }

    fn union_member(&self, receiver: Receiver, member: &CStr) -> Option<&CStr> {
        let methods = (0..receiver.members).filter_map(|index| {
            // SAFETY: The arena-backed union has this number of members.
            let class =
                unsafe { (self.native.union_member)(receiver.type_, index) };
            self.name(class)
                .and_then(|qn| self.method(qn, member, true))
        });
        unique_method(methods)
    }

    fn attribute(&self, fn_node: Node<'_>, call: Node<'_>) -> Option<()> {
        let object = fn_node.field(c"object")?;
        let member = self.text(fn_node.field(c"attribute")?)?;
        if self.super_call(call, object, member).is_some() {
            return Some(());
        }
        let receiver = self.receiver(object, false);
        if receiver.kind == MODULE {
            self.module_member(call, self.name(receiver.name)?, member);
            return Some(());
        }
        let (method, strategy, confidence) = match receiver.kind {
            NAMED => (
                self.method(self.name(receiver.name)?, member, true),
                c"lsp_method",
                0.90,
            ),
            BUILTIN => (
                self.method(
                    &qualified(c"builtins", self.name(receiver.name)?),
                    member,
                    true,
                ),
                c"lsp_builtin_method",
                0.90,
            ),
            TEMPLATE => {
                let name = self.name(receiver.name)?;
                let method = self
                    .method(&qualified(c"builtins", name), member, true)
                    .or_else(|| self.method(name, member, true));
                (method, c"lsp_generic_method", 0.88)
            }
            UNION => (
                self.union_member(receiver, member),
                c"lsp_method_union",
                0.85,
            ),
            _ => return None,
        };
        self.emit(call, method?, strategy, confidence, None);
        Some(())
    }

    fn callable(&self, mut node: Node<'_>) -> Option<Callable<'_>> {
        // SAFETY: The flag lives in the active resolver context.
        if !unsafe { (self.native.proof_enabled)(self.context) } {
            return None;
        }
        while node.kind() == c"parenthesized_expression" {
            let mut children = node.named_children();
            let inner = children.next()?;
            if children.next().is_some() {
                return None;
            }
            node = inner;
        }
        if node.kind() == c"identifier" {
            let name = self.text(node)?;
            let bound = self.scope(name);
            let symbol =
                || self.module.map(|module| self.symbol_row(module, name));
            if bound.present {
                let target = self.name(bound.callable)?;
                let same_symbol =
                    symbol().and_then(|row| self.name(row.name)) == Some(target);
                return Some(Callable {
                    target,
                    lexical_alias: !same_symbol,
                });
            }
            let target = self.exact_function(symbol()?)?;
            return Some(Callable {
                target,
                lexical_alias: false,
            });
        }
        if node.kind() != c"attribute" {
            return None;
        }
        let object = node.field(c"object")?;
        let attribute = node.field(c"attribute")?;
        let member = self.text(attribute);
        let receiver = self.receiver(object, true);
        let member = member?;
        let name = self.name(receiver.name)?;
        let function = match receiver.kind {
            MODULE => self.module_entry(name, member).function,
            NAMED => {
                // SAFETY: Both strings remain live through the field lookup.
                if unsafe {
                    (self.native.field)(self.context, name.into(), member.into())
                } {
                    return None;
                }
                self.method_row(name, member, true)
            }
            _ => return None,
        };
        let target = self.exact_function(function)?;
        Some(Callable {
            target,
            lexical_alias: false,
        })
    }

    fn imported_reference<'a>(&'a self, name: &'a CStr) -> Option<&'a CStr> {
        let target = single_import(self.imports(name))?;
        // SAFETY: The name is live and only borrowed by the scope query.
        let binding = unsafe {
            (self.native.aliased_scope_named)(self.context, name.into())
        };
        (self.name(binding) == Some(target)).then_some(target)
    }

    fn reference(&self, arg: Node<'_>) {
        let arg = if arg.kind() == c"keyword_argument" {
            let Some(value) = arg.field(c"value") else {
                return;
            };
            value
        } else {
            arg
        };
        let kind = arg.kind().to_bytes();
        if !matches!(
            kind,
            b"identifier" | b"attribute" | b"parenthesized_expression"
        ) {
            return;
        }
        let exact = self.callable(arg);
        let source = self.text(arg);
        if let Some(exact) = exact {
            let strategy = if exact.lexical_alias {
                c"lsp_callable_alias"
            } else {
                c"lsp_callable_value_reference"
            };
            // SAFETY: The outer argument span, including parentheses, is the
            // span held by the usage occurrence. Native retains copied strings.
            unsafe {
                (self.native.reference)(
                    self.context,
                    arg.as_raw(),
                    exact.target.into(),
                    source.map_or(Name::NONE, Name::from),
                    strategy.into(),
                )
            };
        } else if kind == b"identifier"
            && let Some(candidate) =
                source.and_then(|name| self.imported_reference(name))
        {
            // SAFETY: The project pass receives the same argument occurrence.
            unsafe {
                (self.native.unresolved_reference)(
                    self.context,
                    arg.as_raw(),
                    candidate.into(),
                )
            };
        }
    }

    fn invoke(&self, call: Node<'_>) {
        let Some(fn_node) = call.field(c"function") else {
            return;
        };
        match fn_node.kind().to_bytes() {
            b"identifier" => {
                if let Some(name) = self.text(fn_node) {
                    self.identifier(name, call);
                }
            }
            b"subscript" if !self.class_body => {
                self.dictionary(fn_node, call);
            }
            b"attribute" => {
                self.attribute(fn_node, call);
            }
            _ => {}
        }
    }
}

fn single_import<'a>(
    mut imports: impl Iterator<Item = &'a CStr>,
) -> Option<&'a CStr> {
    let target = imports.next()?;
    imports
        .all(|candidate| candidate == target)
        .then_some(target)
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_py_invoke(
    context: Context,
    native: *const Native,
    call: *const RawNode,
    module: Spelling,
    class_body: bool,
) {
    // SAFETY: The host lends this call's live tree, context, native queries, and module.
    let file = unsafe { File::borrow(context, native, module, class_body) };
    file.invoke(unsafe { Node::from_root(&*call) });
}

impl File<'_> {
    unsafe fn borrow(
        context: Context,
        native: *const Native,
        module: Spelling,
        class_body: bool,
    ) -> Self {
        // SAFETY: The host binds these borrowed inputs to the active resolver call.
        let (native, module) = unsafe {
            (
                &*native,
                (!module.is_null()).then(|| CStr::from_ptr(module)),
            )
        };
        Self {
            context,
            native,
            module,
            class_body,
        }
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_py_callable_value(
    context: Context,
    native: *const Native,
    node: *const RawNode,
    module: Spelling,
) -> Proof {
    // SAFETY: The host lends the expression's tree, context, native queries, and module.
    let file = unsafe { File::borrow(context, native, module, false) };
    file.callable(unsafe { Node::from_root(&*node) }).map_or(
        Proof {
            target: ptr::null(),
            lexical_alias: false,
        },
        |callable| Proof {
            target: callable.target.as_ptr(),
            lexical_alias: callable.lexical_alias,
        },
    )
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_py_argument_references(
    context: Context,
    native: *const Native,
    call: *const RawNode,
    module: Spelling,
) {
    // SAFETY: The host lends the call's tree, context, native queries, and module.
    let file = unsafe { File::borrow(context, native, module, false) };
    let node = unsafe { Node::from_root(&*call) };
    if let Some(arguments) = node.field(c"arguments") {
        for arg in arguments.named_children() {
            file.reference(arg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_retargeting_requires_exact_class_spelling_and_project_target() {
        let target = c"app.Foo.__init__";
        assert_eq!(project_initializer(true, || Some(target)), Some(target));
        assert_eq!(
            project_initializer(true, || Some(c"builtins.dict.__init__")),
            None
        );
        assert_eq!(project_initializer(true, || None), None);
        assert_eq!(
            project_initializer(false, || panic!(
                "instance cannot query initializer"
            )),
            None
        );
    }

    #[test]
    fn only_duplicate_identical_imports_prove_one_target() {
        let target = c"app.impl.F";
        assert_eq!(single_import([].into_iter()), None);
        assert_eq!(single_import([target, target].into_iter()), Some(target));
        assert_eq!(single_import([target, c"app.other.F"].into_iter()), None);
    }

    #[test]
    fn union_requires_one_matching_member_even_when_targets_coincide() {
        let method = c"app.Base.work";
        assert_eq!(unique_method([method].into_iter()), Some(method));
        assert_eq!(unique_method([].into_iter()), None);
        assert_eq!(unique_method([method, method].into_iter()), None);
        assert_eq!(unique_method([method, c"app.Other.work"].into_iter()), None);
    }
}
