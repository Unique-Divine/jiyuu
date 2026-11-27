//! Unit tests for Go type facts. Runs with `rustc --edition 2024 --test`.

#[path = "../src/go_types.rs"]
mod go_types;

use std::cell::RefCell;
use std::ffi::CStr;
use std::ops::Range;

use go_types::{Channel, GoTypeFacts, Type, TypeContext, TypeMode, TypeTree};

struct Node<'a> {
    kind: &'static [u8],
    span: Range<usize>,
    children: &'a [(&'static CStr, &'a Node<'a>)],
}

impl TypeTree for &Node<'_> {
    fn kind(self) -> &'static [u8] {
        self.kind
    }

    fn span(self) -> Range<usize> {
        self.span.clone()
    }

    fn field(self, name: &'static CStr) -> Option<Self> {
        self.children
            .iter()
            .find_map(|(field, node)| (*field == name).then_some(*node))
    }

    fn children(self) -> impl Iterator<Item = Self> {
        self.children.iter().map(|(_, node)| *node)
    }
}

fn node(kind: &'static [u8], span: Range<usize>) -> Node<'static> {
    Node {
        kind,
        span,
        children: &[],
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Resolved {
    Unknown,
    Builtin(Vec<u8>),
    Named(Vec<u8>),
    Pointer(usize),
    Slice(usize),
    Map(usize, usize),
    Channel(usize, Channel),
    Function { children: Vec<usize>, params: usize },
    TypeParameter(Vec<u8>),
    Interface,
    Struct,
    Tuple(Vec<usize>),
}

struct FileScope<'a> {
    imports: &'a [(&'a [u8], &'a [u8])],
    types: RefCell<Vec<Resolved>>,
}

impl TypeContext for FileScope<'_> {
    type Id = usize;

    fn package(&self) -> &[u8] {
        b"project.app"
    }

    fn import(&self, alias: &[u8]) -> Option<&[u8]> {
        self.imports
            .iter()
            .find_map(|&(name, package)| (name == alias).then_some(package))
    }

    fn type_parameter(&self, name: &[u8]) -> bool {
        matches!(name, b"T" | b"int")
    }

    fn push_type(&self, ty: Type<'_, usize>) -> usize {
        let ty = match ty {
            Type::Unknown => Resolved::Unknown,
            Type::Builtin(name) => Resolved::Builtin(name.into()),
            Type::Named { package, name } => {
                Resolved::Named([package, b".", name].concat())
            }
            Type::Pointer(item) => Resolved::Pointer(item),
            Type::Slice(item) => Resolved::Slice(item),
            Type::Map(key, value) => Resolved::Map(key, value),
            Type::Channel(item, flow) => Resolved::Channel(item, flow),
            Type::Function { children, params } => Resolved::Function {
                children: children.into(),
                params,
            },
            Type::TypeParameter(name) => Resolved::TypeParameter(name.into()),
            Type::Interface => Resolved::Interface,
            Type::Struct => Resolved::Struct,
            Type::Tuple(items) => Resolved::Tuple(items.into()),
        };
        let mut types = self.types.borrow_mut();
        let id = types.len();
        types.push(ty);
        id
    }
}

fn file_facts(root: impl TypeTree) -> GoTypeFacts {
    GoTypeFacts::extract(root, TypeMode::File)
}

fn resolve(
    facts: &GoTypeFacts,
    source: &[u8],
    imports: &[(&[u8], &[u8])],
) -> Vec<Resolved> {
    let ctx = FileScope {
        imports,
        types: RefCell::new(Vec::new()),
    };
    let id = facts.resolve(source, &ctx);
    let types = ctx.types.into_inner();
    assert_eq!(id, types.len() - 1);
    types
}

#[test]
fn owned_facts_resolve_with_local_and_cross_file_import_bindings() {
    let source = b"*api.Thing";
    let facts = {
        let alias = node(b"package_identifier", 1..4);
        let name = node(b"type_identifier", 5..10);
        let qualified = Node {
            kind: b"qualified_type",
            span: 1..10,
            children: &[(c"package", &alias), (c"name", &name)],
        };
        let pointer = Node {
            kind: b"pointer_type",
            span: 0..10,
            children: &[(c"", &qualified)],
        };
        file_facts(&pointer)
    };
    let local_imports: &[(&[u8], &[u8])] = &[(b"api", b"example.net/lib")];
    let project_imports: &[(&[u8], &[u8])] =
        &[(b"api", b"project.lib"), (b"api", b"project.duplicate")];
    assert_eq!(
        resolve(&facts, source, local_imports),
        [
            Resolved::Named(b"example.net/lib.Thing".into()),
            Resolved::Pointer(0)
        ],
    );
    let expected = [
        Resolved::Named(b"project.lib.Thing".into()),
        Resolved::Pointer(0),
    ];
    assert_eq!(resolve(&facts, source, project_imports), expected);
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..4)
            .map(|_| scope.spawn(|| resolve(&facts, source, project_imports)))
            .collect();
        for worker in workers {
            assert_eq!(worker.join().unwrap(), expected);
        }
    });
    assert_eq!(
        resolve(&facts, source, &[(b"ap", b"project.wrong")]),
        [Resolved::Unknown, Resolved::Pointer(0)],
    );
}

#[test]
fn retains_non_utf8_source_and_native_nul_boundary() {
    let source = b"Caf\xff\0recovered";
    let ident = node(b"type_identifier", 0..source.len());
    assert_eq!(
        resolve(&file_facts(&ident), source, &[]),
        [Resolved::Named(b"project.app.Caf\xff".into())],
    );
}

#[test]
fn container_and_wrapper_types_keep_native_shape() {
    let source = b"map[string][5]<- chan Item[T]";
    let builtin = node(b"type_identifier", 4..10);
    let name = node(b"type_identifier", 22..26);
    let generic = Node {
        kind: b"generic_type",
        span: 22..29,
        children: &[(c"type", &name)],
    };
    let wrapped = Node {
        kind: b"parenthesized_type",
        span: 22..29,
        children: &[(c"", &generic)],
    };
    let channel = Node {
        kind: b"channel_type",
        span: 14..29,
        children: &[(c"value", &wrapped)],
    };
    // Missing field metadata falls back to the last named child.
    let array = Node {
        kind: b"array_type",
        span: 11..29,
        children: &[(c"", &channel)],
    };
    let map = Node {
        kind: b"map_type",
        span: 0..29,
        children: &[(c"key", &builtin), (c"value", &array)],
    };
    assert_eq!(
        resolve(&file_facts(&map), source, &[]),
        [
            Resolved::Builtin(b"string".into()),
            Resolved::Named(b"project.app.Item".into()),
            Resolved::Channel(1, Channel::Receive),
            Resolved::Slice(2),
            Resolved::Map(0, 3),
        ],
    );
}

#[test]
fn tuple_results_skip_missing_declaration_types_and_keep_the_cap() {
    let ident = node(b"type_identifier", 0..3);
    let missing = node(b"parameter_declaration", 0..0);
    let valid = Node {
        kind: b"parameter_declaration",
        span: 0..3,
        children: &[(c"type", &ident)],
    };
    let one_result = [(c"", &missing), (c"", &valid)];
    let children: Vec<_> = std::iter::once((c"", &missing))
        .chain(std::iter::repeat_n((c"", &valid), 17))
        .collect();
    let make = |children| Node {
        kind: b"parameter_list",
        span: 0..3,
        children,
    };
    assert_eq!(
        resolve(&file_facts(&make(&one_result)), b"int", &[]),
        [Resolved::Builtin(b"int".into())],
    );
    let resolved = resolve(&file_facts(&make(&children)), b"int", &[]);
    assert_eq!(resolved.len(), 17);
    assert!(
        resolved[..16]
            .iter()
            .all(|row| *row == Resolved::Builtin(b"int".into()))
    );
    assert_eq!(resolved[16], Resolved::Tuple((0..16).collect()));
}

#[test]
fn missing_fields_and_opaque_types_keep_native_defaults() {
    for (kind, expected) in [
        (b"qualified_type".as_slice(), vec![Resolved::Unknown]),
        (b"pointer_type", vec![Resolved::Unknown]),
        (b"slice_type", vec![Resolved::Unknown, Resolved::Slice(0)]),
        (
            b"map_type",
            vec![Resolved::Unknown, Resolved::Unknown, Resolved::Map(0, 1)],
        ),
        (
            b"function_type",
            vec![Resolved::Function {
                children: vec![],
                params: 0,
            }],
        ),
        (b"interface_type", vec![Resolved::Interface]),
        (b"struct_type", vec![Resolved::Struct]),
    ] {
        assert_eq!(resolve(&file_facts(&node(kind, 0..0)), b"", &[]), expected);
    }
}

#[test]
fn signature_types_preserve_parameter_scope_and_literal_import_spelling() {
    let source = b"T int complex64 api.Thing";
    let type_parameter = node(b"type_identifier", 0..1);
    let shadowed_builtin = node(b"type_identifier", 2..5);
    let complex = node(b"type_identifier", 6..15);
    let alias = node(b"package_identifier", 16..19);
    let name = node(b"type_identifier", 20..25);
    let qualified = Node {
        kind: b"qualified_type",
        span: 16..25,
        children: &[(c"package", &alias), (c"name", &name)],
    };
    let param = Node {
        kind: b"parameter_declaration",
        span: 0..1,
        children: &[(c"type", &type_parameter)],
    };
    let variadic = Node {
        kind: b"variadic_parameter_declaration",
        span: param.span.clone(),
        ..param
    };
    let params = Node {
        kind: b"parameter_list",
        span: 0..1,
        children: &[(c"", &param), (c"", &variadic)],
    };
    let returns = Node {
        kind: b"parameter_list",
        span: 2..25,
        children: &[
            (c"", &shadowed_builtin),
            (c"", &complex),
            (c"", &qualified),
        ],
    };
    let function = Node {
        kind: b"function_type",
        span: 0..25,
        children: &[(c"parameters", &params), (c"result", &returns)],
    };
    let imports: &[(&[u8], &[u8])] = &[(b"api", b"project.api")];
    assert_eq!(
        resolve(
            &GoTypeFacts::extract(&function, TypeMode::Signature),
            source,
            imports
        ),
        [
            Resolved::TypeParameter(b"T".into()),
            Resolved::TypeParameter(b"int".into()),
            Resolved::Named(b"project.app.complex64".into()),
            Resolved::Named(b"api.Thing".into()),
            Resolved::Function {
                children: vec![0, 1, 2, 3],
                params: 1
            },
        ],
    );
    assert_eq!(
        resolve(&file_facts(&function), source, imports),
        [Resolved::Function {
            children: vec![],
            params: 0
        }],
    );
}
