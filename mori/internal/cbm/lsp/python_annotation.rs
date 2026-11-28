//! Parse and resolve Python annotation text for registry and lexical callers.

#[derive(Clone, Copy)]
enum Mode<'a> {
    Registry(Option<&'a [u8]>),
    Scope(Option<&'a [u8]>),
}

trait Types {
    type Type: Copy;

    fn unknown(&self) -> Self::Type;
    fn builtin(&self, name: &[u8]) -> Self::Type;
    fn named(&self, name: &[u8]) -> Self::Type;
    fn optional(&self, child: Self::Type) -> Self::Type;
    fn union(&self, members: &[Self::Type]) -> Self::Type;
    fn template(&self, name: &[u8], args: &[Self::Type]) -> Self::Type;
    fn scope(&self, name: &[u8], enclosing: bool) -> Option<Self::Type>;
    fn registered(&self, name: &[u8]) -> bool;
}

fn trim(mut text: &[u8], tabs: bool) -> &[u8] {
    let whitespace = |byte: u8| byte == b' ' || (tabs && byte == b'\t');
    while text.first().is_some_and(|&byte| whitespace(byte)) {
        text = &text[1..];
    }
    while text.last().is_some_and(|&byte| whitespace(byte)) {
        text = &text[..text.len() - 1];
    }
    text
}

struct Parts<'a> {
    values: Vec<&'a [u8]>,
    separated: bool,
    complete: bool,
}

fn parts(text: &[u8], separator: u8, limit: usize) -> Parts<'_> {
    let mut values = Vec::new();
    let mut start = 0;
    let mut depth = 0isize;
    let mut push = |part| {
        let part = trim(part, true);
        if !part.is_empty() && values.len() < limit {
            values.push(part);
        }
    };
    for (end, &byte) in text.iter().enumerate() {
        match byte {
            b'[' | b'(' | b'{' => depth += 1,
            b']' | b')' | b'}' => depth -= 1,
            _ if byte == separator && depth == 0 => {
                push(&text[start..end]);
                start = end + 1;
            }
            _ => {}
        }
    }
    let complete = depth == 0;
    if complete {
        push(&text[start..]);
    }
    Parts {
        values,
        separated: start != 0,
        complete,
    }
}

fn wrapper(name: &[u8]) -> bool {
    matches!(
        name,
        b"ClassVar"
            | b"Final"
            | b"InitVar"
            | b"ReadOnly"
            | b"Required"
            | b"NotRequired"
            | b"Annotated"
            | b"Mapped"
            | b"WriteOnlyMapped"
            | b"DynamicMapped"
            | b"Type"
            | b"type"
            | b"typing.ClassVar"
            | b"typing.Final"
            | b"typing.Annotated"
            | b"typing.Required"
            | b"typing.NotRequired"
            | b"typing.ReadOnly"
            | b"typing.Type"
            | b"typing_extensions.ClassVar"
            | b"typing_extensions.Final"
            | b"typing_extensions.Annotated"
            | b"typing_extensions.Required"
            | b"typing_extensions.NotRequired"
            | b"typing_extensions.ReadOnly"
            | b"dataclasses.InitVar"
    )
}

fn builtin(name: &[u8]) -> bool {
    matches!(
        name,
        b"int"
            | b"str"
            | b"bool"
            | b"float"
            | b"bytes"
            | b"None"
            | b"complex"
            | b"bytearray"
            | b"object"
            | b"type"
    )
}

fn typing_name(name: &[u8]) -> bool {
    matches!(
        name,
        b"Self"
            | b"Any"
            | b"None"
            | b"True"
            | b"False"
            | b"Iterator"
            | b"Iterable"
            | b"Generator"
            | b"AsyncIterator"
            | b"AsyncIterable"
            | b"Sequence"
            | b"MutableSequence"
            | b"Mapping"
            | b"MutableMapping"
            | b"Collection"
            | b"Container"
            | b"Reversible"
            | b"Optional"
            | b"Union"
            | b"Callable"
            | b"Awaitable"
            | b"Coroutine"
            | b"TypeVar"
            | b"ParamSpec"
    )
}

fn leaf<T: Types>(types: &T, mode: Mode<'_>, text: &[u8]) -> T::Type {
    if let Mode::Scope(module) = mode {
        if let Some(bound) =
            types.scope(text, false).or_else(|| types.scope(text, true))
        {
            return bound;
        }
        if let Some(module) = module {
            let local = [module, b".", text].concat();
            if types.registered(&local) {
                return types.named(&local);
            }
        }
    }
    if builtin(text) {
        return types.builtin(text);
    }
    if let Mode::Registry(Some(module)) = mode
        && !text.contains(&b'.')
        && !typing_name(text)
    {
        return types.named(&[module, b".", text].concat());
    }
    types.named(text)
}

fn resolve<T: Types>(types: &T, mode: Mode<'_>, mut text: &[u8]) -> T::Type {
    loop {
        if text.is_empty() {
            return types.unknown();
        }
        if text.len() >= 2
            && matches!(text[0], b'\'' | b'"')
            && text.last() == text.first()
        {
            text = &text[1..text.len() - 1];
            continue;
        }
        if text.last() == Some(&b']')
            && let Some(open) = text.iter().position(|&byte| byte == b'[')
        {
            let base = trim(&text[..open], false);
            let parts = parts(&text[open + 1..text.len() - 1], b',', usize::MAX);
            let args: Vec<_> = parts
                .values
                .into_iter()
                .map(|arg| resolve(types, mode, arg))
                .collect();
            let Some(&first) = args.first() else {
                text = base;
                continue;
            };
            return match base {
                b"Optional" | b"typing.Optional" => types.optional(first),
                b"Union" | b"typing.Union" => types.union(&args),
                _ if wrapper(base) => first,
                _ => types.template(base, &args),
            };
        }
        if text.contains(&b'|') {
            let union = parts(text, b'|', 4);
            // Registry text lost the separator flag at the terminating
            // delimiter. Lexical annotations recognize balanced PEP 604.
            let recognize = union.separated
                && (matches!(mode, Mode::Scope(_)) || !union.complete);
            if recognize && union.values.len() >= 2 {
                let members: Vec<_> = union
                    .values
                    .into_iter()
                    .map(|part| resolve(types, mode, part))
                    .collect();
                return types.union(&members);
            }
        }
        return leaf(types, mode, text);
    }
}

#[cfg(feature = "native-engine-abi")]
mod abi {
    use super::*;
    use std::ffi::{CStr, CString, c_char, c_int, c_void};
    use std::ptr;

    type Type = *const c_void;

    #[repr(i32)]
    enum Kind {
        Unknown = 0,
        Builtin = 1,
        Named = 2,
        Optional = 3,
        Union = 4,
        Template = 5,
    }

    #[repr(C)]
    struct Native {
        build: unsafe extern "C" fn(
            *mut c_void,
            c_int,
            *const c_char,
            usize,
            *const Type,
            usize,
        ) -> Type,
        scope: unsafe extern "C" fn(
            *mut c_void,
            *const c_char,
            usize,
            bool,
        ) -> Type,
        registered:
            unsafe extern "C" fn(*mut c_void, *const c_char, usize) -> bool,
    }

    struct Sink<'a> {
        context: *mut c_void,
        native: &'a Native,
    }

    impl Sink<'_> {
        fn build(&self, kind: Kind, name: Option<&[u8]>, args: &[Type]) -> Type {
            let name = match name.map(CString::new).transpose() {
                Ok(name) => name,
                Err(_) => return self.unknown(),
            };
            let (text, length) =
                name.as_deref().map_or((ptr::null(), 0), |name| {
                    (name.as_ptr(), name.to_bytes_with_nul().len())
                });
            // SAFETY: Name and argument storage are borrowed by the constructor;
            // the host copies them into its own arena before returning.
            unsafe {
                (self.native.build)(
                    self.context,
                    kind as c_int,
                    text,
                    length,
                    args.as_ptr(),
                    args.len(),
                )
            }
        }
    }

    impl Types for Sink<'_> {
        type Type = Type;

        fn unknown(&self) -> Type {
            self.build(Kind::Unknown, None, &[])
        }

        fn builtin(&self, name: &[u8]) -> Type {
            self.build(Kind::Builtin, Some(name), &[])
        }

        fn named(&self, name: &[u8]) -> Type {
            self.build(Kind::Named, Some(name), &[])
        }

        fn optional(&self, child: Type) -> Type {
            self.build(Kind::Optional, None, &[child])
        }

        fn union(&self, members: &[Type]) -> Type {
            self.build(Kind::Union, None, members)
        }

        fn template(&self, name: &[u8], args: &[Type]) -> Type {
            self.build(Kind::Template, Some(name), args)
        }

        fn scope(&self, name: &[u8], enclosing: bool) -> Option<Type> {
            let name = CString::new(name).ok()?;
            // SAFETY: The lookup borrows the terminated name until return.
            let found = unsafe {
                (self.native.scope)(
                    self.context,
                    name.as_ptr(),
                    name.as_bytes_with_nul().len(),
                    enclosing,
                )
            };
            (!found.is_null()).then_some(found)
        }

        fn registered(&self, name: &[u8]) -> bool {
            let Ok(name) = CString::new(name) else {
                return false;
            };
            // SAFETY: The lookup borrows the terminated name until return.
            unsafe {
                (self.native.registered)(
                    self.context,
                    name.as_ptr(),
                    name.as_bytes_with_nul().len(),
                )
            }
        }
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn mori_py_resolve_annotation(
        context: *mut c_void,
        native: *const Native,
        ann: *const c_char,
        module: *const c_char,
        scoped: bool,
    ) -> Type {
        // SAFETY: The host lends both strings and the callbacks for this call.
        let text = |p: *const c_char| {
            (!p.is_null()).then(|| unsafe { CStr::from_ptr(p).to_bytes() })
        };
        let types = Sink {
            context,
            native: unsafe { &*native },
        };
        let module = text(module);
        let mode = if scoped {
            Mode::Scope(module)
        } else {
            Mode::Registry(module)
        };
        resolve(&types, mode, text(ann).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Typeset {
        names: RefCell<Vec<String>>,
    }

    impl Typeset {
        fn keep(&self, name: String) -> usize {
            let mut names = self.names.borrow_mut();
            names.push(name);
            names.len() - 1
        }

        fn print(&self, name: usize) -> String {
            self.names.borrow()[name].clone()
        }

        fn joined(&self, members: &[usize]) -> String {
            let names = self.names.borrow();
            members
                .iter()
                .map(|&member| names[member].as_str())
                .collect::<Vec<_>>()
                .join(",")
        }
    }

    impl Types for Typeset {
        type Type = usize;

        fn unknown(&self) -> usize {
            self.keep("?".to_owned())
        }

        fn builtin(&self, name: &[u8]) -> usize {
            self.keep(format!("builtin({})", String::from_utf8_lossy(name)))
        }

        fn named(&self, name: &[u8]) -> usize {
            self.keep(String::from_utf8_lossy(name).into_owned())
        }

        fn optional(&self, child: usize) -> usize {
            self.keep(format!("optional({})", self.print(child)))
        }

        fn union(&self, members: &[usize]) -> usize {
            self.keep(format!("union({})", self.joined(members)))
        }

        fn template(&self, name: &[u8], args: &[usize]) -> usize {
            self.keep(format!(
                "{}[{}]",
                String::from_utf8_lossy(name),
                self.joined(args)
            ))
        }

        fn scope(&self, name: &[u8], enclosing: bool) -> Option<usize> {
            match (name, enclosing) {
                (b"Foo", false) => Some(self.keep("imported.Foo".to_owned())),
                (b"Foo", true) => Some(self.keep("enclosing.Foo".to_owned())),
                (b"Local", true) => {
                    Some(self.keep("enclosing.Local".to_owned()))
                }
                _ => None,
            }
        }

        fn registered(&self, name: &[u8]) -> bool {
            name == b"p.mod.Bar" || name == b"p.mod.int"
        }
    }

    fn check(mode: Mode<'_>, text: &[u8], expected: &str) {
        let types = Typeset::default();
        assert_eq!(types.print(resolve(&types, mode, text)), expected);
    }

    #[test]
    fn forward_references_and_nested_wrappers() {
        let mode = Mode::Registry(Some(b"p.mod"));
        check(mode, b"'Foo'", "p.mod.Foo");
        check(
            mode,
            b"typing.ClassVar[typing.Optional[list[Foo]]]",
            "optional(list[p.mod.Foo])",
        );
        check(
            mode,
            b"typing_extensions.Annotated[dict[str, tuple[Foo, int]], metadata(x, y)]",
            "dict[builtin(str),tuple[p.mod.Foo,builtin(int)]]",
        );
        check(mode, b"Type[Foo]", "p.mod.Foo");
        check(mode, b"Optional[]", "Optional");
    }

    #[test]
    fn lexical_binding_precedes_registry_and_builtins() {
        let mode = Mode::Scope(Some(b"p.mod"));
        check(mode, b"'Foo'", "imported.Foo");
        check(mode, b"Local", "enclosing.Local");
        check(mode, b"Bar", "p.mod.Bar");
        check(mode, b"int", "p.mod.int");
        check(mode, b"None", "builtin(None)");
        check(mode, b"Unresolved", "Unresolved");
        check(Mode::Registry(Some(b"p.mod")), b"int", "builtin(int)");
        check(Mode::Registry(Some(b"p.mod")), b"Self", "Self");
    }

    #[test]
    fn pipe_annotations_preserve_caller_modes_and_member_order() {
        check(
            Mode::Registry(Some(b"p.mod")),
            b"Foo | None",
            "p.mod.Foo | None",
        );
        check(
            Mode::Scope(Some(b"p.mod")),
            b"Foo | None | X | Y | Z",
            "union(imported.Foo,builtin(None),X,Y)",
        );
        check(
            Mode::Registry(Some(b"p.mod")),
            b"Foo | Bar | Incomplete[",
            "union(p.mod.Foo,p.mod.Bar)",
        );
    }
}
