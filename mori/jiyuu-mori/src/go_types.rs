//! Go type syntax and file-local qualification for the typed resolver.
//!
//! Keep the syntax facts with the file's source bytes. Resolve them with the
//! import bindings for the current pass: source spellings during extraction,
//! and project package names during cross-file resolution.

use std::ops::Range;

const MAX_RESULTS: usize = 16;
const MAX_SIGNATURE_TYPES: usize = 15;

pub(crate) trait TypeTree: Copy {
    fn kind(self) -> &'static [u8];
    fn span(self) -> Range<usize>;
    fn field(self, name: &'static std::ffi::CStr) -> Option<Self>;
    fn children(self) -> impl Iterator<Item = Self>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Channel {
    Both,
    Send,
    Receive,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Type<'a, T> {
    Unknown,
    Builtin(&'a [u8]),
    Named { package: &'a [u8], name: &'a [u8] },
    Pointer(T),
    Slice(T),
    Map(T, T),
    Channel(T, Channel),
    Function { children: &'a [T], params: usize },
    TypeParameter(&'a [u8]),
    Interface,
    Struct,
    Tuple(&'a [T]),
}

/// File-local name lookup and type storage. The production adapter borrows the
/// native file context. A Rust project resolver can supply the file's resolved
/// imports from its generation's project context.
pub(crate) trait TypeContext {
    type Id: Copy;

    fn package(&self) -> &[u8];
    fn import(&self, alias: &[u8]) -> Option<&[u8]>;
    fn type_parameter(&self, _name: &[u8]) -> bool {
        false
    }
    fn push_type(&self, ty: Type<'_, Self::Id>) -> Self::Id;
}

enum Fact {
    Unknown,
    Ident(Range<usize>),
    Qualified {
        alias: Range<usize>,
        name: Range<usize>,
    },
    Pointer,
    Slice,
    Map,
    Channel(Range<usize>),
    Function {
        params: usize,
        returns: usize,
    },
    Interface,
    Struct,
    Tuple(usize),
}

/// Owned postorder syntax facts. Spans refer to the source in the enclosing
/// file facts. They contain no pointer to a parser, registry, or arena.
pub(crate) struct GoTypeFacts {
    facts: Vec<Fact>,
    mode: TypeMode,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TypeMode {
    File,
    Signature,
}

impl GoTypeFacts {
    pub(crate) fn extract(root: impl TypeTree, mode: TypeMode) -> Self {
        enum Work<N> {
            Visit(Option<N>),
            Emit(Fact),
        }

        let mut work = Vec::new();
        let mut facts = Vec::new();
        let mut current = Some(Work::Visit(Some(root)));
        while let Some(item) = current.take().or_else(|| work.pop()) {
            let node = match item {
                Work::Visit(Some(node)) => node,
                Work::Visit(None) => {
                    facts.push(Fact::Unknown);
                    continue;
                }
                Work::Emit(fact) => {
                    facts.push(fact);
                    continue;
                }
            };

            let kind = node.kind();
            let fact = match kind {
                b"type_identifier" => Fact::Ident(node.span()),
                b"qualified_type" => {
                    match (node.field(c"package"), node.field(c"name")) {
                        (Some(alias), Some(name)) => Fact::Qualified {
                            alias: alias.span(),
                            name: name.span(),
                        },
                        _ => Fact::Unknown,
                    }
                }
                b"pointer_type" => match node.children().last() {
                    Some(child) => {
                        work.push(Work::Emit(Fact::Pointer));
                        current = Some(Work::Visit(Some(child)));
                        continue;
                    }
                    None => Fact::Unknown,
                },
                b"slice_type" | b"array_type" | b"channel_type" => {
                    let (field, fact) = if kind == b"channel_type" {
                        (c"value", Fact::Channel(node.span()))
                    } else {
                        (c"element", Fact::Slice)
                    };
                    work.push(Work::Emit(fact));
                    current = Some(Work::Visit(
                        node.field(field).or_else(|| node.children().last()),
                    ));
                    continue;
                }
                b"map_type" => {
                    work.push(Work::Emit(Fact::Map));
                    work.push(Work::Visit(node.field(c"value")));
                    current = Some(Work::Visit(node.field(c"key")));
                    continue;
                }
                b"parenthesized_type" | b"type_elem" => {
                    current = Some(Work::Visit(node.children().next()));
                    continue;
                }
                b"generic_type" => {
                    current = Some(Work::Visit(node.field(c"type")));
                    continue;
                }
                b"function_type" => {
                    if mode == TypeMode::File {
                        Fact::Function {
                            params: 0,
                            returns: 0,
                        }
                    } else {
                        let mut children = [None; 2 * MAX_SIGNATURE_TYPES];
                        let (params, returns) =
                            signature_children(node, &mut children);
                        work.push(Work::Emit(Fact::Function {
                            params,
                            returns,
                        }));
                        work.extend(
                            children[..params + returns]
                                .iter()
                                .rev()
                                .copied()
                                .map(Work::Visit),
                        );
                        continue;
                    }
                }
                b"interface_type" => Fact::Interface,
                b"struct_type" if mode == TypeMode::File => Fact::Struct,
                b"parameter_list" if mode == TypeMode::File => {
                    let children = node.children().filter_map(|child| {
                        if child.kind() == b"parameter_declaration" {
                            child.field(c"type")
                        } else {
                            Some(child)
                        }
                    });
                    let mut results = [None; MAX_RESULTS];
                    let mut count = 0;
                    for (result, child) in results.iter_mut().zip(children) {
                        *result = Some(child);
                        count += 1;
                    }
                    if count == 0 {
                        Fact::Unknown
                    } else {
                        if count > 1 {
                            work.push(Work::Emit(Fact::Tuple(count)));
                        }
                        work.extend(
                            results[1..count]
                                .iter()
                                .rev()
                                .copied()
                                .map(Work::Visit),
                        );
                        current = Some(Work::Visit(results[0]));
                        continue;
                    }
                }
                _ => Fact::Unknown,
            };
            facts.push(fact);
        }
        Self { facts, mode }
    }

    pub(crate) fn resolve<C: TypeContext>(
        &self,
        source: &[u8],
        ctx: &C,
    ) -> C::Id {
        let text = |span: &Range<usize>| {
            let bytes = source.get(span.clone())?;
            // C-string consumers in the original resolver stop at NUL.
            bytes.split(|byte| *byte == 0).next()
        };
        let mut values = Vec::new();
        for (i, fact) in self.facts.iter().enumerate() {
            let mut consumed = 0;
            let ty = match fact {
                Fact::Unknown => Type::Unknown,
                Fact::Ident(span) => match text(span) {
                    Some(name)
                        if self.mode == TypeMode::Signature
                            && ctx.type_parameter(name) =>
                    {
                        Type::TypeParameter(name)
                    }
                    Some(name) if is_builtin(name, self.mode) => {
                        Type::Builtin(name)
                    }
                    Some(name) => Type::Named {
                        package: ctx.package(),
                        name,
                    },
                    None => Type::Unknown,
                },
                Fact::Qualified { alias, name } => {
                    let package =
                        text(alias).and_then(|alias| match self.mode {
                            TypeMode::File => ctx.import(alias),
                            TypeMode::Signature => Some(alias),
                        });
                    match (package, text(name)) {
                        (Some(package), Some(name)) => {
                            Type::Named { package, name }
                        }
                        _ => Type::Unknown,
                    }
                }
                Fact::Pointer => Type::Pointer(values.pop().unwrap()),
                Fact::Slice => Type::Slice(values.pop().unwrap()),
                Fact::Map => {
                    let value = values.pop().unwrap();
                    Type::Map(values.pop().unwrap(), value)
                }
                Fact::Channel(span) => Type::Channel(
                    values.pop().unwrap(),
                    channel(source.get(span.clone()).unwrap_or_default()),
                ),
                Fact::Function { params, returns } => {
                    consumed = params + returns;
                    Type::Function {
                        children: &values[values.len() - consumed..],
                        params: *params,
                    }
                }
                Fact::Interface => Type::Interface,
                Fact::Struct => Type::Struct,
                Fact::Tuple(count) => {
                    consumed = *count;
                    Type::Tuple(&values[values.len() - consumed..])
                }
            };
            let value = ctx.push_type(ty);
            values.truncate(values.len() - consumed);
            if i + 1 == self.facts.len() {
                return value;
            }
            values.push(value);
        }
        ctx.push_type(Type::Unknown)
    }
}

fn signature_children<N: TypeTree>(
    node: N,
    children: &mut [Option<N>; 2 * MAX_SIGNATURE_TYPES],
) -> (usize, usize) {
    let mut params = 0;
    if let Some(list) = node.field(c"parameters") {
        for child in list
            .children()
            .filter(|child| child.kind() == b"parameter_declaration")
            .filter_map(|child| child.field(c"type"))
            .take(MAX_SIGNATURE_TYPES)
        {
            children[params] = Some(child);
            params += 1;
        }
    }
    let mut returns = 0;
    if let Some(result) = node.field(c"result") {
        if result.kind() == b"parameter_list" {
            for child in result.children().take(MAX_SIGNATURE_TYPES) {
                children[params + returns] =
                    Some(child.field(c"type").unwrap_or(child));
                returns += 1;
            }
        } else {
            children[params] = Some(result);
            returns = 1;
        }
    }
    (params, returns)
}

fn is_builtin(name: &[u8], mode: TypeMode) -> bool {
    if matches!(name, b"complex64" | b"complex128") {
        return mode == TypeMode::File;
    }
    matches!(
        name,
        b"int"
            | b"int8"
            | b"int16"
            | b"int32"
            | b"int64"
            | b"uint"
            | b"uint8"
            | b"uint16"
            | b"uint32"
            | b"uint64"
            | b"float32"
            | b"float64"
            | b"string"
            | b"bool"
            | b"byte"
            | b"rune"
            | b"error"
            | b"uintptr"
            | b"any"
    )
}

fn channel(text: &[u8]) -> Channel {
    if text.starts_with(b"chan<-") || text.starts_with(b"chan <-") {
        Channel::Send
    } else if text.starts_with(b"<-chan") || text.starts_with(b"<- chan") {
        Channel::Receive
    } else {
        Channel::Both
    }
}
