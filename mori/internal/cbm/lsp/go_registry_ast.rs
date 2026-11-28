//! Refine Go registry rows with declarations found in the current source tree.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr;

use crate::native_tree::{BorrowedNode, RawNode};

type Type = *const c_void;
type Node<'tree> = BorrowedNode<'tree>;

const MAX_EMBEDS: usize = 15;
const MAX_MEMBERS: usize = 63;

#[repr(C)]
#[derive(Clone, Copy)]
enum NameRows {
    Embeds,
    Methods,
}

#[repr(C)]
struct RegistryOps {
    name: unsafe extern "C" fn(*mut c_void, *const u8, usize) -> *const c_char,
    find_type: unsafe extern "C" fn(*mut c_void, *const c_char) -> c_int,
    alias: unsafe extern "C" fn(
        *mut c_void,
        c_int,
        *const c_char,
        *const c_char,
        *const c_char,
    ),
    names: unsafe extern "C" fn(
        *mut c_void,
        c_int,
        NameRows,
        *const *const c_char,
        usize,
    ),
    fields: unsafe extern "C" fn(
        *mut c_void,
        c_int,
        *const *const c_char,
        *const Type,
        usize,
    ),
    parse_type: unsafe extern "C" fn(*mut c_void, *const RawNode) -> Type,
}

#[repr(C)]
struct Input {
    root: RawNode,
    source: *const u8,
    source_len: usize,
    package: *const c_char,
    context: *mut c_void,
    ops: *const RegistryOps,
    refine_interfaces: bool,
}

struct Registry<'a> {
    source: &'a [u8],
    package: &'a [u8],
    ops: &'a RegistryOps,
    context: *mut c_void,
}

impl<'a> Registry<'a> {
    fn spelling(&self, node: Node<'_>) -> Option<&'a [u8]> {
        let bytes = self.source.get(node.range())?;
        bytes
            .split(|&byte| byte == 0)
            .next()
            .filter(|text| !text.is_empty())
    }

    fn own(&self, bytes: &[u8]) -> Option<*const c_char> {
        // SAFETY: The host copies the live bytes into the same registry arena.
        let name = unsafe {
            (self.ops.name)(self.context, bytes.as_ptr(), bytes.len())
        };
        (!name.is_null()).then_some(name)
    }

    fn qualify(&self, bare: &[u8]) -> Option<*const c_char> {
        let mut text = Vec::new();
        text.try_reserve_exact(
            self.package.len().checked_add(bare.len())?.checked_add(1)?,
        )
        .ok()?;
        text.extend_from_slice(self.package);
        text.push(b'.');
        text.extend_from_slice(bare);
        self.own(&text)
    }

    fn type_row(&self, qn: *const c_char) -> Option<c_int> {
        // SAFETY: The QN was allocated by the host, whose type order is stable.
        let row = unsafe { (self.ops.find_type)(self.context, qn) };
        (row >= 0).then_some(row)
    }

    fn alias(&self, alias: &[u8], target: &[u8]) {
        let (Some(name), Some(qn), Some(target)) =
            (self.own(alias), self.qualify(alias), self.qualify(target))
        else {
            return;
        };
        let row = self.type_row(qn).unwrap_or(-1);
        // SAFETY: The row and all strings belong to the native registry arena.
        unsafe { (self.ops.alias)(self.context, row, name, qn, target) };
    }

    fn names(&self, row: c_int, kind: NameRows, names: &[*const c_char]) {
        if names.is_empty() {
            return;
        }
        // SAFETY: The host copies the pointer list; the pointed-to strings
        // already belong to its arena.
        unsafe {
            (self.ops.names)(
                self.context,
                row,
                kind,
                names.as_ptr(),
                names.len(),
            )
        };
    }

    fn fields(&self, row: c_int, names: &[*const c_char], types: &[Type]) {
        if names.is_empty() {
            return;
        }
        debug_assert_eq!(names.len(), types.len());
        // SAFETY: The host copies both lists before returning.
        unsafe {
            (self.ops.fields)(
                self.context,
                row,
                names.as_ptr(),
                types.as_ptr(),
                names.len(),
            )
        };
    }

    fn parse_type(&self, node: Node<'_>) -> Type {
        // SAFETY: The live parser node is lent for this callback only.
        unsafe { (self.ops.parse_type)(self.context, node.as_raw()) }
    }
}

fn methods(registry: &Registry<'_>, row: c_int, body: Node<'_>) {
    let mut names = [ptr::null(); MAX_MEMBERS];
    let mut count = 0;
    let methods = body.named_children().filter(|node| {
        matches!(node.kind().to_bytes(), b"method_spec" | b"method_elem")
    });
    for name in methods.filter_map(|node| {
        node.field(c"name").and_then(|name| registry.spelling(name))
    }) {
        if let Some(name) = registry.own(name) {
            names[count] = name;
            count += 1;
            if count == MAX_MEMBERS {
                break;
            }
        }
    }
    registry.names(row, NameRows::Methods, &names[..count]);
}

fn structure(registry: &Registry<'_>, row: c_int, body: Node<'_>) {
    let Some(fields) =
        body.field(c"body").or_else(|| body.named_children().next())
    else {
        return;
    };
    let mut embeds = [ptr::null(); MAX_EMBEDS];
    let mut names = [ptr::null(); MAX_MEMBERS];
    let mut types = [ptr::null(); MAX_MEMBERS];
    let mut embed_count = 0;
    let mut field_count = 0;
    for field in fields
        .named_children()
        .filter(|node| node.kind() == c"field_declaration")
    {
        let Some(ty) = field.field(c"type") else {
            continue;
        };
        match field.field(c"name") {
            None if embed_count < MAX_EMBEDS => {
                if let Some(spelling) = registry.spelling(ty) {
                    let start = spelling
                        .iter()
                        .position(|&byte| byte != b'*')
                        .unwrap_or(spelling.len());
                    if let Some(embed) = registry.qualify(&spelling[start..]) {
                        embeds[embed_count] = embed;
                        embed_count += 1;
                    }
                }
            }
            Some(name) if field_count < MAX_MEMBERS => {
                if let Some(name) =
                    registry.spelling(name).and_then(|name| registry.own(name))
                {
                    names[field_count] = name;
                    types[field_count] = registry.parse_type(ty);
                    field_count += 1;
                }
            }
            _ => {}
        }
    }
    registry.names(row, NameRows::Embeds, &embeds[..embed_count]);
    registry.fields(row, &names[..field_count], &types[..field_count]);
}

fn refine(registry: &Registry<'_>, root: Node<'_>, refine_interfaces: bool) {
    for declaration in root
        .named_children()
        .filter(|node| node.kind() == c"type_declaration")
    {
        for spec in declaration.named_children() {
            let (Some(name), Some(ty)) = (
                spec.field(c"name").and_then(|name| registry.spelling(name)),
                spec.field(c"type"),
            ) else {
                continue;
            };
            match spec.kind().to_bytes() {
                b"type_alias" => {
                    if let Some(target) = registry.spelling(ty) {
                        registry.alias(name, target);
                    }
                }
                b"type_spec"
                    if ty.kind() == c"struct_type"
                        || (refine_interfaces
                            && ty.kind() == c"interface_type") =>
                {
                    let Some(row) = registry
                        .qualify(name)
                        .and_then(|qn| registry.type_row(qn))
                    else {
                        continue;
                    };
                    if ty.kind() == c"struct_type" {
                        structure(registry, row, ty);
                    } else {
                        methods(registry, row, ty);
                    }
                }
                _ => {}
            }
        }
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_go_refine_registry_ast(input: *const Input) {
    // SAFETY: C lends the tree, source, package and callbacks for this call.
    let input = unsafe { &*input };
    let root = unsafe { BorrowedNode::from_root(&input.root) };
    let registry = Registry {
        source: unsafe {
            std::slice::from_raw_parts(input.source, input.source_len)
        },
        package: unsafe { CStr::from_ptr(input.package) }.to_bytes(),
        ops: unsafe { &*input.ops },
        context: input.context,
    };
    refine(&registry, root, input.refine_interfaces);
}
