//! Package, function, and statement bindings for Go resolution.

use std::ffi::CStr;

use super::native::{File, TypeView};
use super::{Node, evaluate, name, parse};

fn binding_name<'file>(
    file: &File<'file>,
    node: Node<'_>,
) -> Option<&'file CStr> {
    (node.kind() == c"identifier")
        .then(|| name(file, node))
        .flatten()
        .filter(|name| *name != c"_")
}

fn first_value(node: Node<'_>) -> Node<'_> {
    if node.kind() == c"expression_list" {
        node.named_children().next().unwrap_or(node)
    } else {
        node
    }
}

fn values(node: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    let list = node.kind() == c"expression_list";
    (!list)
        .then_some(node)
        .into_iter()
        .chain(list.then(|| node.named_children()).into_iter().flatten())
}

fn only_value(node: Node<'_>) -> Option<Node<'_>> {
    let mut expressions = values(node);
    let first = expressions.next()?;
    expressions.next().is_none().then_some(first)
}

fn short_declaration(file: &File<'_>, left: Node<'_>, right: Node<'_>) {
    let single = only_value(right);
    let ty = single.map(|node| evaluate(file, Some(node), 0));
    let callable = single.and_then(|node| file.exact_callable(node.as_raw()));
    if left.kind() == c"expression_list" {
        for (index, target) in left.named_children().enumerate() {
            let Some(name) = binding_name(file, target) else {
                continue;
            };
            let binding_type = ty
                .and_then(|ty| {
                    file.tuple_element(ty, index).or((index == 0).then_some(ty))
                })
                .unwrap_or_else(|| file.unknown());
            file.bind_value(
                name,
                binding_type,
                (index == 0).then_some(callable).flatten(),
            );
        }
    } else if let (Some(name), Some(ty)) = (binding_name(file, left), ty) {
        file.bind_value(name, ty, callable);
    }
}

fn assignment(file: &File<'_>, left: Node<'_>, right: Node<'_>) {
    let paired = values(left).count() == values(right).count();
    let mut right = values(right);
    for target in values(left) {
        // Position matters even for blank identifiers and non-binding targets.
        let value = paired.then(|| right.next()).flatten();
        let Some(name) = binding_name(file, target) else {
            continue;
        };
        let ty = evaluate(file, value, 0);
        let callable = value.and_then(|node| file.exact_callable(node.as_raw()));
        // Unknown RHS calls clear a previous exact callable on this name.
        file.bind_value(name, ty, callable);
    }
}

fn declaration(file: &File<'_>, node: Node<'_>) {
    let is_const = node.kind() == c"const_spec";
    if is_const && node.field(c"name").is_none() {
        return;
    }
    let value = node.field(c"value");
    let ty = match node.field(c"type") {
        Some(ty) => parse(file, Some(ty)),
        None => evaluate(file, value.map(first_value), 0),
    };
    let callable = value
        .and_then(only_value)
        .and_then(|node| file.exact_callable(node.as_raw()));
    if is_const {
        if let Some(name) = node
            .field(c"name")
            .and_then(|name| binding_name(file, name))
        {
            file.bind_value(name, ty, callable);
        }
    } else {
        for name in node
            .named_children()
            .filter_map(|name| binding_name(file, name))
        {
            file.bind_value(name, ty, callable);
        }
    }
}

fn range(file: &File<'_>, node: Node<'_>) {
    let Some(container) = node.field(c"right") else {
        return;
    };
    let unknown = file.unknown();
    let container = evaluate(file, Some(container), 0);
    let (key, value) = match file.view(container) {
        TypeView::Slice(elem) => (file.builtin(c"int"), elem),
        TypeView::Map(key, value) => (key, value),
        TypeView::Channel(elem) => (unknown, elem),
        TypeView::Builtin(ty) if ty == c"string" => {
            (file.builtin(c"int"), file.builtin(c"rune"))
        }
        _ => (unknown, unknown),
    };
    if let Some(left) = node
        .field(c"left")
        .filter(|node| node.kind() == c"expression_list")
    {
        for (position, node) in left.named_children().enumerate() {
            if let Some(name) = binding_name(file, node) {
                file.bind(name, if position == 0 { key } else { value });
            }
        }
    }
}

pub(super) fn statement(file: &File<'_>, node: Node<'_>) {
    match node.kind().to_bytes() {
        b"short_var_declaration" | b"assignment_statement" => {
            let (Some(left), Some(right)) =
                (node.field(c"left"), node.field(c"right"))
            else {
                return;
            };
            if node.kind() == c"short_var_declaration" {
                short_declaration(file, left, right);
            } else {
                assignment(file, left, right);
            }
        }
        b"var_spec" | b"const_spec" => declaration(file, node),
        b"range_clause" => range(file, node),
        _ => {}
    }
}

fn bind_parameters(
    file: &File<'_>,
    list: Node<'_>,
    allow_variadic: bool,
    receiver: bool,
) {
    for parameter in list.named_children() {
        let variadic = parameter.kind() == c"variadic_parameter_declaration";
        if parameter.kind() != c"parameter_declaration"
            && !(allow_variadic && variadic)
        {
            continue;
        }
        let ty = parse(file, parameter.field(c"type"));
        let ty = if variadic && !ty.is_null() {
            file.slice(ty)
        } else {
            ty
        };
        let mut names = parameter
            .named_children()
            .filter(|node| node.kind() == c"identifier");
        if receiver {
            if let Some(name) =
                names.next().and_then(|id| binding_name(file, id))
            {
                file.bind(name, ty);
            }
        } else {
            for name in names.filter_map(|id| binding_name(file, id)) {
                file.bind(name, ty);
            }
        }
    }
}

fn function(file: &File<'_>, node: Node<'_>) {
    let Some(function_name) = node
        .field(c"name")
        .and_then(|id| name(file, id))
        .filter(|name| !name.is_empty())
    else {
        return;
    };
    file.enter_function(node.as_raw(), function_name);
    let _scope = file.push_scope();

    if let Some(params) = node.field(c"parameters") {
        bind_parameters(file, params, true, false);
    }
    if let Some(result) = node
        .field(c"result")
        .filter(|node| node.kind() == c"parameter_list")
    {
        bind_parameters(file, result, false, false);
    }
    if let Some(receiver) = node.field(c"receiver") {
        bind_parameters(file, receiver, false, true);
    }
    if let Some(body) = node.field(c"body") {
        file.walk(body.as_raw());
    }
}

pub(super) fn file(file: &File<'_>, root: Node<'_>) {
    // Package bindings precede function bodies, independent of source order.
    for declaration in root.named_children().filter(|node| {
        matches!(
            node.kind().to_bytes(),
            b"var_declaration" | b"const_declaration"
        )
    }) {
        for spec in declaration.named_children() {
            statement(file, spec);
        }
    }
    for declaration in root.named_children().filter(|node| {
        matches!(
            node.kind().to_bytes(),
            b"function_declaration" | b"method_declaration"
        )
    }) {
        function(file, declaration);
    }
}
