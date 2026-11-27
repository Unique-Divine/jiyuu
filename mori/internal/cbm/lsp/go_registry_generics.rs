//! Refine extracted Go signatures with type parameters found in declarations.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr;

use crate::go_eval::native::{
    Type, TypeView, cbm_go_eval_copy_name, cbm_type_unknown, view_type,
};
use crate::native_tree::{BorrowedNode, RawNode};

type Names = *const *const c_char;
type Types = *const Type;
type Node<'tree> = BorrowedNode<'tree>;

const MAX_TYPE_PARAMS: usize = 15;

#[repr(C)]
struct Function {
    row: c_int,
    signature: Type,
}

#[repr(C)]
struct Replacement {
    names: Names,
    name_count: usize,
    params: Types,
    param_count: usize,
    returns: Types,
    return_count: usize,
}

#[repr(C)]
struct Operations {
    find: unsafe extern "C" fn(*mut c_void, *const c_char) -> Function,
    parse:
        unsafe extern "C" fn(*mut c_void, *const RawNode, Names, usize) -> Type,
    set: unsafe extern "C" fn(*mut c_void, c_int, *const Replacement),
}

#[repr(C)]
struct Input {
    root: RawNode,
    source: *const u8,
    source_len: usize,
    module: *const c_char,
    arena: *mut c_void,
    context: *mut c_void,
    ops: *const Operations,
}

struct Registry<'a> {
    source: &'a [u8],
    module: &'a [u8],
    arena: *mut c_void,
    context: *mut c_void,
    ops: &'a Operations,
}

impl Registry<'_> {
    fn spelling(&self, node: Node<'_>) -> Option<&[u8]> {
        self.source
            .get(node.range())?
            .split(|&byte| byte == 0)
            .next()
            .filter(|text| !text.is_empty())
    }

    fn name(&self, spelling: &[u8]) -> Option<*const c_char> {
        // SAFETY: The host copies the bytes into its arena.
        let name = unsafe {
            cbm_go_eval_copy_name(self.arena, spelling.as_ptr(), spelling.len())
        };
        (!name.is_null()).then_some(name)
    }

    fn function(&self, name: &[u8]) -> Option<Function> {
        let mut qualified = Vec::new();
        qualified
            .try_reserve_exact(
                self.module.len().checked_add(name.len())?.checked_add(1)?,
            )
            .ok()?;
        qualified.extend_from_slice(self.module);
        qualified.push(b'.');
        qualified.extend_from_slice(name);
        let name = self.name(&qualified)?;
        // SAFETY: The host reads a live QN in its native arena.
        let function = unsafe { (self.ops.find)(self.context, name) };
        (function.row >= 0).then_some(function)
    }

    fn parse(&self, node: Option<Node<'_>>, names: &[*const c_char]) -> Type {
        let Some(node) = node else {
            return unsafe { cbm_type_unknown() };
        };
        // SAFETY: Both the tree node and type parameter names live through the call.
        unsafe {
            (self.ops.parse)(
                self.context,
                node.as_raw(),
                names.as_ptr(),
                names.len(),
            )
        }
    }

    fn update(
        &self,
        function: Function,
        decl: Node<'_>,
        names: &[*const c_char],
    ) {
        let (parameters, returns) =
            match unsafe { view_type(function.signature) } {
                TypeView::Function(sig) => {
                    let parameters = sig
                        .has_parameters()
                        .then(|| {
                            decl.field(c"parameters").and_then(|list| {
                                self.parameters(list, names, sig.params())
                            })
                        })
                        .flatten();
                    let returns = decl.field(c"result").and_then(|node| {
                        self.returns(node, names, sig.returns())
                    });
                    (parameters, returns)
                }
                _ => (None, None),
            };
        let replacement = Replacement {
            names: names.as_ptr(),
            name_count: names.len(),
            params: parameters.as_deref().map_or(ptr::null(), <[Type]>::as_ptr),
            param_count: parameters.as_ref().map_or(0, Vec::len),
            returns: returns.as_deref().map_or(ptr::null(), <[Type]>::as_ptr),
            return_count: returns.as_ref().map_or(0, Vec::len),
        };
        // SAFETY: The host copies the replacements before returning.
        unsafe { (self.ops.set)(self.context, function.row, &replacement) };
    }

    fn parameters(
        &self,
        list: Node<'_>,
        names: &[*const c_char],
        old: impl Iterator<Item = Type>,
    ) -> Option<Vec<Type>> {
        let mut old = old.peekable();
        let mut refined = Vec::new();
        for param in list.named_children().filter(|node| {
            matches!(
                node.kind().to_bytes(),
                b"parameter_declaration" | b"variadic_parameter_declaration"
            )
        }) {
            let ty = self.parse(param.field(c"type"), names);
            for _ in 0..multiplicity(param) {
                refined.push(keep_known(ty, old.next()?));
            }
        }
        old.peek().is_none().then_some(refined)
    }

    fn returns(
        &self,
        node: Node<'_>,
        names: &[*const c_char],
        old: impl Iterator<Item = Type>,
    ) -> Option<Vec<Type>> {
        let mut refined: Vec<_> = old.collect();
        if !refined.iter().any(|&ty| refers_to_parameter(ty, names)) {
            return None;
        }
        if node.kind() == c"parameter_list" {
            for (slot, node) in refined.iter_mut().zip(node.named_children()) {
                let ty =
                    self.parse(Some(node.field(c"type").unwrap_or(node)), names);
                *slot = keep_known(ty, *slot);
            }
        } else {
            refined[0] = keep_known(self.parse(Some(node), names), refined[0]);
        }
        Some(refined)
    }
}

fn multiplicity(param: Node<'_>) -> usize {
    let mut fields = 0;
    let mut identifiers = 0;
    for (child, field) in
        param.child_fields().filter(|(node, _)| node.is_named())
    {
        fields += usize::from(field == Some(c"name"));
        identifiers += usize::from(child.kind() == c"identifier");
    }
    if fields > 0 {
        fields
    } else {
        identifiers.max(1)
    }
}

fn keep_known(candidate: Type, existing: Type) -> Type {
    if matches!(unsafe { view_type(candidate) }, TypeView::Unknown)
        && !matches!(unsafe { view_type(existing) }, TypeView::Unknown)
    {
        existing
    } else if candidate.is_null() {
        unsafe { cbm_type_unknown() }
    } else {
        candidate
    }
}

fn refers_to_parameter(ty: Type, names: &[*const c_char]) -> bool {
    let ty = match unsafe { view_type(ty) } {
        TypeView::Pointer(inner) | TypeView::Slice(inner)
            if !inner.is_null() =>
        {
            inner
        }
        _ => ty,
    };
    let TypeView::Named(name) = (unsafe { view_type(ty) }) else {
        return false;
    };
    let name = name
        .to_bytes()
        .rsplit(|&byte| byte == b'.')
        .next()
        .unwrap_or_default();
    names.iter().any(|&candidate| {
        unsafe { CStr::from_ptr(candidate) }.to_bytes() == name
    })
}

fn refine(registry: &Registry<'_>, root: Node<'_>) {
    for decl in root
        .named_children()
        .filter(|node| node.kind() == c"function_declaration")
    {
        let (Some(params), Some(name)) = (
            decl.field(c"type_parameters"),
            decl.field(c"name").and_then(|name| registry.spelling(name)),
        ) else {
            continue;
        };
        let mut names = [ptr::null(); MAX_TYPE_PARAMS];
        let mut count = 0;
        for param in params
            .named_children()
            .filter(|node| node.kind() == c"type_parameter_declaration")
        {
            let ident = param.field(c"name").or_else(|| {
                param
                    .named_children()
                    .find(|node| node.kind() == c"type_identifier")
            });
            if let Some(name) = ident
                .and_then(|name| registry.spelling(name))
                .and_then(|name| registry.name(name))
            {
                names[count] = name;
                count += 1;
                if count == MAX_TYPE_PARAMS {
                    break;
                }
            }
        }
        if count > 0
            && let Some(function) = registry.function(name)
        {
            registry.update(function, decl, &names[..count]);
        }
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_go_refine_generics(input: *const Input) {
    // SAFETY: The host lends this input, source, and AST for the call.
    let input = unsafe { &*input };
    let root = unsafe { BorrowedNode::from_root(&input.root) };
    let registry = Registry {
        source: unsafe {
            std::slice::from_raw_parts(input.source, input.source_len)
        },
        module: unsafe { CStr::from_ptr(input.module) }.to_bytes(),
        arena: input.arena,
        context: input.context,
        ops: unsafe { &*input.ops },
    };
    refine(&registry, root);
}
