//! Adapter for the C Go walker and its arena-owned type representation.

use std::ffi::{CStr, c_char, c_int, c_void};

use crate::go_types::{
    Channel, GoTypeFacts, Type, TypeContext, TypeMode, TypeTree,
};
use crate::native_tree::RawNode;

type TypeId = *const c_void;

#[repr(C)]
struct Bytes {
    data: *const u8,
    len: usize,
}

impl From<&[u8]> for Bytes {
    fn from(bytes: &[u8]) -> Self {
        Self {
            data: bytes.as_ptr(),
            len: bytes.len(),
        }
    }
}

#[repr(C)]
enum Kind {
    Unknown,
    Builtin,
    Named,
    Pointer,
    Slice,
    Map,
    Channel,
    Function,
    Interface,
    Struct,
    Tuple,
    TypeParameter,
}

type Import = unsafe extern "C" fn(*mut c_void, Bytes) -> *const c_char;
type MakeType = unsafe extern "C" fn(
    *mut c_void,
    Kind,
    Bytes,
    Bytes,
    *const TypeId,
    usize,
    c_int,
) -> TypeId;

/// Private wrapper for a non-null node in the tree lent to `mori_go_parse_type`.
#[derive(Clone, Copy)]
struct Node(RawNode);

impl TypeTree for Node {
    fn kind(self) -> &'static [u8] {
        // SAFETY: The parser lends the tree for the walk. The pinned Go
        // grammar's node-kind strings are static.
        unsafe { self.0.kind() }.to_bytes()
    }

    fn span(self) -> std::ops::Range<usize> {
        // SAFETY: The parser lends the tree for the walk.
        unsafe { self.0.range() }
    }

    fn field(self, name: &'static CStr) -> Option<Self> {
        // SAFETY: The parser lends the tree for the walk.
        unsafe { self.0.field(name) }.map(Self)
    }

    fn children(self) -> impl Iterator<Item = Self> {
        // SAFETY: The cursor is used and dropped within the parse callback.
        unsafe { self.0.named_children() }.map(Self)
    }
}

struct NativeContext<'a> {
    package: &'a [u8],
    native: *mut c_void,
    import: Option<Import>,
    type_params: *const *const c_char,
    make_type: MakeType,
}

impl TypeContext for NativeContext<'_> {
    type Id = TypeId;

    fn package(&self) -> &[u8] {
        self.package
    }

    fn import(&self, alias: &[u8]) -> Option<&[u8]> {
        // SAFETY: C borrows the alias only during the lookup. A returned
        // package string is NUL-terminated and lives in the file arena.
        let package = unsafe { (self.import?)(self.native, alias.into()) };
        (!package.is_null())
            .then(|| unsafe { CStr::from_ptr(package) }.to_bytes())
    }

    fn type_parameter(&self, name: &[u8]) -> bool {
        let mut row = self.type_params;
        if row.is_null() {
            return false;
        }
        // SAFETY: C lends a NULL-terminated array of live C strings.
        unsafe {
            while !(*row).is_null() {
                if CStr::from_ptr(*row).to_bytes() == name {
                    return true;
                }
                row = row.add(1);
            }
        }
        false
    }

    fn push_type(&self, ty: Type<'_, TypeId>) -> TypeId {
        let mut name = &[][..];
        let mut package = &[][..];
        let mut detail = 0;
        let one;
        let two;
        let (kind, children) = match ty {
            Type::Unknown => (Kind::Unknown, &[][..]),
            Type::Builtin(value) => {
                name = value;
                (Kind::Builtin, &[][..])
            }
            Type::Named {
                package: scope,
                name: value,
            } => {
                package = scope;
                name = value;
                (Kind::Named, &[][..])
            }
            Type::Pointer(item) => {
                one = [item];
                (Kind::Pointer, &one[..])
            }
            Type::Slice(item) => {
                one = [item];
                (Kind::Slice, &one[..])
            }
            Type::Map(key, value) => {
                two = [key, value];
                (Kind::Map, &two[..])
            }
            Type::Channel(item, flow) => {
                one = [item];
                detail = match flow {
                    Channel::Both => 0,
                    Channel::Send => 1,
                    Channel::Receive => 2,
                };
                (Kind::Channel, &one[..])
            }
            Type::Function { children, params } => {
                detail = params as c_int;
                (Kind::Function, children)
            }
            Type::TypeParameter(value) => {
                name = value;
                (Kind::TypeParameter, &[][..])
            }
            Type::Interface => (Kind::Interface, &[][..]),
            Type::Struct => (Kind::Struct, &[][..]),
            Type::Tuple(items) => (Kind::Tuple, items),
        };
        // SAFETY: The C constructor copies names and child arrays before it
        // returns. Child type pointers belong to this file's live arena.
        unsafe {
            (self.make_type)(
                self.native,
                kind,
                package.into(),
                name.into(),
                children.as_ptr(),
                children.len(),
                detail,
            )
        }
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_go_parse_type(
    root: RawNode,
    source: Bytes,
    package: *const c_char,
    native: *mut c_void,
    import: Option<Import>,
    type_params: *const *const c_char,
    make_type: MakeType,
) -> TypeId {
    // SAFETY: C lends source, package name, and the tree through the call.
    let source = unsafe { std::slice::from_raw_parts(source.data, source.len) };
    let package = unsafe { CStr::from_ptr(package) }.to_bytes();
    let mode = if import.is_some() {
        TypeMode::File
    } else {
        TypeMode::Signature
    };
    GoTypeFacts::extract(Node(root), mode).resolve(
        source,
        &NativeContext {
            package,
            native,
            import,
            type_params,
            make_type,
        },
    )
}
