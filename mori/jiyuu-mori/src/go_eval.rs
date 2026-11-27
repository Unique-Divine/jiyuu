//! Go expression types, including lexical, imported, and generic calls.

use std::ffi::CStr;
use std::ptr;

use crate::go_types::{TypeMode, is_builtin};
use crate::native_tree::BorrowedNode;

#[path = "go_eval_native.rs"]
mod native;
use native::{Callable, File, NameList, Signature, Type, TypeView};

#[path = "go_bindings.rs"]
mod bindings;
#[path = "go_members.rs"]
mod members;
#[path = "go_walk.rs"]
mod walk;

type Node<'tree> = BorrowedNode<'tree>;

const MAX_GENERIC_ARGS: usize = 16;
const MAX_CLOSURE_TYPES: usize = 15;

fn spelling<'file>(file: &File<'file>, node: Node<'_>) -> &'file [u8] {
    file.source.get(node.range()).unwrap_or_default()
}

fn name<'file>(file: &File<'file>, node: Node<'_>) -> Option<&'file CStr> {
    file.name(spelling(file, node))
}

fn first_operator<'file>(file: &File<'file>, node: Node<'_>) -> &'file [u8] {
    node.unnamed_children()
        .next()
        .map_or(&[], |operator| spelling(file, operator))
}

fn parse(file: &File<'_>, node: Option<Node<'_>>) -> Type {
    node.map_or_else(|| file.unknown(), |ty| file.parse_type(ty.as_raw()))
}

fn qualified<'file>(
    file: &File<'file>,
    package: &CStr,
    name: &CStr,
) -> Option<&'file CStr> {
    qualified_bytes(file, package.to_bytes(), name.to_bytes())
}

fn qualified_bytes<'file>(
    file: &File<'file>,
    package: &[u8],
    name: &[u8],
) -> Option<&'file CStr> {
    let mut text = Vec::new();
    text.try_reserve_exact(
        package.len().checked_add(name.len())?.checked_add(1)?,
    )
    .ok()?;
    text.extend_from_slice(package);
    text.push(b'.');
    text.extend_from_slice(name);
    file.name(&text)
}

fn package_member(file: &File<'_>, package: &CStr, name: &CStr) -> Type {
    if let Some(function) =
        file.symbol(package, name).filter(Callable::has_signature)
    {
        return function.signature;
    }
    qualified(file, package, name)
        .filter(|qn| file.has_named_type(qn))
        .map_or_else(|| file.unknown(), |qn| file.named(qn))
}

fn identifier(file: &File<'_>, node: Node<'_>) -> Type {
    let Some(name) = name(file, node) else {
        return file.unknown();
    };
    if let Some(ty) = file.scope_lookup(name) {
        return ty;
    }
    if let Some(function) = file
        .symbol(file.package, name)
        .filter(Callable::has_signature)
    {
        return function.signature;
    }
    if is_builtin(name.to_bytes(), TypeMode::File) {
        return file.named(name);
    }
    qualified(file, file.package, name)
        .filter(|qn| file.has_named_type(qn))
        .map_or_else(|| file.unknown(), |qn| file.named(qn))
}

fn selector(file: &File<'_>, node: Node<'_>, depth: usize) -> Type {
    let (Some(operand), Some(field)) =
        (node.field(c"operand"), node.field(c"field"))
    else {
        return file.unknown();
    };
    let Some(member) = name(file, field) else {
        return file.unknown();
    };
    if operand.kind() == c"identifier"
        && let Some(package) =
            name(file, operand).and_then(|alias| file.import(alias))
    {
        return package_member(file, package, member);
    }

    let receiver = evaluate(file, Some(operand), depth);
    let base = match file.view(receiver) {
        TypeView::Pointer(element) => element,
        _ => receiver,
    };
    if let TypeView::Named(qn) = file.view(base) {
        if let Some(method) =
            members::method(file, qn, member, 0).filter(Callable::has_signature)
        {
            return method.signature;
        }
        if let Some(field) = members::field(file, qn, member, 0)
            && !matches!(file.view(field), TypeView::Unknown)
        {
            return field;
        }
    }
    file.unknown()
}

fn builtin_call(name: &[u8]) -> bool {
    matches!(
        name,
        b"make"
            | b"new"
            | b"append"
            | b"len"
            | b"cap"
            | b"delete"
            | b"close"
            | b"copy"
            | b"panic"
            | b"recover"
            | b"print"
            | b"println"
            | b"complex"
            | b"real"
            | b"imag"
            | b"min"
            | b"max"
            | b"clear"
    )
}

fn builtin_return(
    file: &File<'_>,
    name: &[u8],
    args: Option<Node<'_>>,
    depth: usize,
) -> Type {
    let first = args.and_then(|args| args.named_children().next());
    match (name, first) {
        (b"make", Some(arg)) => parse(file, Some(arg)),
        (b"new", Some(arg)) => file.pointer(parse(file, Some(arg))),
        (b"append", Some(arg)) => evaluate(file, Some(arg), depth),
        (b"len" | b"cap", _) => file.builtin(c"int"),
        _ => file.unknown(),
    }
}

fn registry_callable<'file>(
    file: &File<'file>,
    callee: Node<'_>,
) -> Option<Callable<'file>> {
    let name = self::name(file, callee)?;
    let qn = qualified(file, file.package, name)?;
    file.function(qn).or_else(|| {
        if callee.kind() != c"selector_expression" {
            return None;
        }
        let alias = self::name(file, callee.field(c"operand")?)?;
        let member = self::name(file, callee.field(c"field")?)?;
        file.symbol(file.import(alias)?, member)
    })
}

fn explicit_args(
    file: &File<'_>,
    args: Node<'_>,
    names: NameList,
    inferred: &mut [Type; MAX_GENERIC_ARGS],
) -> Option<usize> {
    // The legacy Go resolver accepts at most 15 explicit type arguments.
    let mut count = 0;
    for (slot, arg) in inferred[..MAX_GENERIC_ARGS - 1]
        .iter_mut()
        .zip(args.named_children())
    {
        *slot = parse(file, Some(arg));
        count += 1;
    }
    (count > 0 && count == file.names(names).count()).then_some(count)
}

fn uses_parameter(file: &File<'_>, ty: Type) -> bool {
    let parameter = |ty| matches!(file.view(ty), TypeView::Parameter(_));
    match file.view(ty) {
        TypeView::Parameter(_) => true,
        TypeView::Slice(elem) | TypeView::Pointer(elem) => parameter(elem),
        TypeView::Map(key, value) => parameter(key) || parameter(value),
        _ => false,
    }
}

fn infer_args(
    file: &File<'_>,
    args: Option<Node<'_>>,
    names: NameList,
    signature: Signature,
    inferred: &mut [Type; MAX_GENERIC_ARGS],
    depth: usize,
) -> Option<usize> {
    let count = file.names(names).count();
    if count == 0
        || count > MAX_GENERIC_ARGS
        || !signature.returns().any(|ty| uses_parameter(file, ty))
    {
        return None;
    }
    let args = args?;
    for (param, arg) in signature.params().zip(args.named_children()) {
        let arg_type = evaluate(file, Some(arg), depth);
        unify(file, param, arg_type, names, &mut inferred[..count], depth);
    }
    inferred[..count]
        .iter()
        .all(|ty| !ty.is_null())
        .then_some(count)
}

fn unify(
    file: &File<'_>,
    pattern: Type,
    arg: Type,
    names: NameList,
    inferred: &mut [Type],
    depth: usize,
) {
    let arg_view = file.view(arg);
    if depth >= file.depth_limit || matches!(arg_view, TypeView::Unknown) {
        return;
    }
    match (file.view(pattern), arg_view) {
        (TypeView::Parameter(param), _) => {
            if let Some(slot) = file
                .names(names)
                .zip(inferred)
                .find_map(|(name, slot)| (name == param).then_some(slot))
                && slot.is_null()
            {
                *slot = arg;
            }
        }
        (TypeView::Slice(a), TypeView::Slice(b))
        | (TypeView::Pointer(a), TypeView::Pointer(b))
        | (TypeView::Channel(a), TypeView::Channel(b)) => {
            unify(file, a, b, names, inferred, depth + 1);
        }
        (TypeView::Map(ak, av), TypeView::Map(bk, bv)) => {
            unify(file, ak, bk, names, inferred, depth + 1);
            unify(file, av, bv, names, inferred, depth + 1);
        }
        (TypeView::Function(a), TypeView::Function(b)) => {
            for (a, b) in a
                .params()
                .zip(b.params())
                .chain(a.returns().zip(b.returns()))
            {
                unify(file, a, b, names, inferred, depth + 1);
            }
        }
        _ => {}
    }
}

fn call_return(
    file: &File<'_>,
    call: Node<'_>,
    callee: Node<'_>,
    signature: Signature,
    args: Option<Node<'_>>,
    depth: usize,
) -> Option<Type> {
    let mut returns = signature.returns();
    let first = returns.next()?;
    let second = returns.next();
    let type_args = call.field(c"type_arguments");
    let mut inferred = [ptr::null(); MAX_GENERIC_ARGS];
    let binding = registry_callable(file, callee)
        .filter(|function| !function.type_params.is_null())
        .and_then(|function| {
            let names = function.type_params;
            let count = match type_args {
                Some(type_args) => {
                    explicit_args(file, type_args, names, &mut inferred)
                }
                None => infer_args(
                    file,
                    args,
                    names,
                    signature,
                    &mut inferred,
                    depth,
                ),
            }?;
            Some((names, &inferred[..count]))
        });
    let substitute = |ty| {
        binding.map_or(ty, |(names, args)| file.substitute(ty, names, args))
    };
    Some(match second {
        Some(second) => {
            let values: Vec<_> = [first, second]
                .into_iter()
                .chain(returns)
                .map(substitute)
                .collect();
            file.tuple(&values)
        }
        None => substitute(first),
    })
}

fn call(file: &File<'_>, node: Node<'_>, depth: usize) -> Type {
    let Some(callee) = node.field(c"function") else {
        return file.unknown();
    };
    let args = node.field(c"arguments");
    if callee.kind() == c"identifier"
        && let Some(name) = name(file, callee)
        && builtin_call(name.to_bytes())
    {
        return builtin_return(file, name.to_bytes(), args, depth);
    }
    let ty = evaluate(file, Some(callee), depth);
    match file.view(ty) {
        TypeView::Function(signature) => {
            if let Some(returns) =
                call_return(file, node, callee, signature, args, depth)
            {
                return returns;
            }
        }
        TypeView::Named(_) => return ty,
        _ => {}
    }
    match callee.kind().to_bytes() {
        b"slice_type" | b"array_type" | b"map_type" | b"pointer_type"
        | b"channel_type" => parse(file, Some(callee)),
        _ => file.unknown(),
    }
}

fn unary(file: &File<'_>, node: Node<'_>, depth: usize) -> Type {
    let Some(operand) = node.field(c"operand") else {
        return file.unknown();
    };
    match first_operator(file, node) {
        b"&" => file.pointer(evaluate(file, Some(operand), depth)),
        b"*" => file.deref(evaluate(file, Some(operand), depth)),
        b"<-" => match file.view(evaluate(file, Some(operand), depth)) {
            TypeView::Channel(element) => element,
            _ => file.unknown(),
        },
        b"!" => file.builtin(c"bool"),
        _ => file.unknown(),
    }
}

fn parameters(node: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    node.field(c"parameters")
        .into_iter()
        .flat_map(BorrowedNode::named_children)
        .filter(|node| node.kind() == c"parameter_declaration")
}

fn closure(file: &File<'_>, node: Node<'_>) -> Type {
    if let Some(body) = node.field(c"body") {
        let _scope = file.push_scope();
        for param in parameters(node) {
            let ty = parse(file, param.field(c"type"));
            for ident in param
                .named_children()
                .filter(|node| node.kind() == c"identifier")
            {
                if let Some(name) =
                    name(file, ident).filter(|name| *name != c"_")
                {
                    file.bind(name, ty);
                }
            }
        }
        file.walk(body);
    }

    let mut params = [ptr::null(); MAX_CLOSURE_TYPES];
    let mut param_count = 0;
    for (slot, ty) in params
        .iter_mut()
        .zip(parameters(node).filter_map(|p| p.field(c"type")))
    {
        *slot = parse(file, Some(ty));
        param_count += 1;
    }
    let mut returns = [ptr::null(); MAX_CLOSURE_TYPES];
    let mut return_count = 0;
    if let Some(result) = node.field(c"result") {
        if result.kind() == c"parameter_list" {
            for (slot, row) in returns.iter_mut().zip(result.named_children()) {
                *slot = parse(file, Some(row.field(c"type").unwrap_or(row)));
                return_count += 1;
            }
        } else {
            returns[0] = parse(file, Some(result));
            return_count = 1;
        }
    }
    file.func(&params[..param_count], &returns[..return_count])
}

fn evaluate(file: &File<'_>, node: Option<Node<'_>>, depth: usize) -> Type {
    let Some(node) = node.filter(|_| depth < file.depth_limit) else {
        return file.unknown();
    };
    let depth = depth + 1;
    match node.kind().to_bytes() {
        b"identifier" => identifier(file, node),
        b"selector_expression" => selector(file, node, depth),
        b"call_expression" => call(file, node, depth),
        b"composite_literal" | b"type_assertion_expression" => {
            parse(file, node.field(c"type"))
        }
        b"unary_expression" => unary(file, node, depth),
        b"index_expression" => {
            match file.view(evaluate(file, node.field(c"operand"), depth)) {
                TypeView::Map(_, value) | TypeView::Slice(value) => value,
                _ => file.unknown(),
            }
        }
        b"parenthesized_expression" => {
            evaluate(file, node.named_children().next(), depth)
        }
        b"binary_expression"
            if matches!(
                first_operator(file, node),
                b"==" | b"!=" | b"<" | b">" | b"<=" | b">=" | b"&&" | b"||"
            ) =>
        {
            file.builtin(c"bool")
        }
        b"binary_expression" => evaluate(file, node.field(c"left"), depth),
        b"slice_expression" => evaluate(file, node.field(c"operand"), depth),
        b"interpreted_string_literal" | b"raw_string_literal" => {
            file.builtin(c"string")
        }
        b"int_literal" => file.builtin(c"int"),
        b"float_literal" => file.builtin(c"float64"),
        b"true" | b"false" => file.builtin(c"bool"),
        b"func_literal" => closure(file, node),
        _ => file.unknown(),
    }
}
