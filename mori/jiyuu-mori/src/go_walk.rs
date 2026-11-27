//! Walk Go scopes and produce typed invocation and callable-value occurrences.

use std::ffi::CStr;

use super::bindings::{binding_name, statement, values};
use super::members;
use super::native::{File, TypeView};
use super::{Node, builtin_call, evaluate, name, parse, qualified, spelling};

const INVOCATION: u32 = 0;
const REFERENCE: u32 = 1;

fn leaf(qualified_name: &CStr) -> &[u8] {
    qualified_name
        .to_bytes()
        .rsplit(|&byte| byte == b'.')
        .next()
        .unwrap_or_default()
}

struct Selector<'file, 'tree> {
    operand: Node<'tree>,
    member: &'file CStr,
    import: Option<(&'file CStr, &'file CStr)>,
}

fn selector<'file, 'tree>(
    file: &File<'file>,
    node: Node<'tree>,
) -> Option<Selector<'file, 'tree>> {
    let operand = node.field(c"operand")?;
    let member = name(file, node.field(c"field")?)?;
    let imported = if operand.kind() == c"identifier" {
        name(file, operand)
            .and_then(|alias| file.import(alias).map(|package| (alias, package)))
    } else {
        None
    };
    Some(Selector {
        operand,
        member,
        import: imported,
    })
}

/// An exact callable value requires a direct method or a lexical binding.
/// Promoted selectors may be ambiguous, so they remain ordinary values.
pub(super) fn exact_callable<'file>(
    file: &File<'file>,
    mut node: Node<'_>,
) -> Option<&'file CStr> {
    while node.kind() == c"parenthesized_expression" {
        let mut children = node.named_children();
        let inner = children.next()?;
        if children.next().is_some() {
            return None;
        }
        node = inner;
    }
    match node.kind().to_bytes() {
        b"identifier" => {
            let name = name(file, node)?;
            if file.scope_lookup(name).is_some() {
                file.scope_callable(name)
            } else {
                file.symbol(file.package, name)?.qualified_name
            }
        }
        b"selector_expression" => {
            let Selector {
                operand,
                member,
                import,
            } = selector(file, node)?;
            if let Some((_, package)) = import {
                return file.symbol(package, member)?.qualified_name;
            }
            let recv = evaluate(file, Some(operand), 0);
            let base = match file.view(recv) {
                TypeView::Pointer(elem) => elem,
                _ => recv,
            };
            match file.view(base) {
                TypeView::Named(ty) => {
                    file.direct_method(ty, member)?.qualified_name
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn unresolved(
    file: &File<'_>,
    site: Node<'_>,
    name: Option<&CStr>,
    reason: &'static CStr,
) {
    file.emit(
        site,
        name.unwrap_or(c"?"),
        c"lsp_unresolved",
        0.,
        Some(reason),
        INVOCATION,
    );
}

fn references(file: &File<'_>, call: Node<'_>) {
    let Some(args) = call.field(c"arguments") else {
        return;
    };
    for arg in args.named_children().filter(|node| {
        matches!(
            node.kind().to_bytes(),
            b"identifier" | b"selector_expression" | b"parenthesized_expression"
        )
    }) {
        let target = exact_callable(file, arg);
        let source = name(file, arg);
        if let Some(target) = target {
            let join = source.filter(|source| source.to_bytes() != leaf(target));
            file.emit(
                arg,
                target,
                c"lsp_callable_value_reference",
                0.97,
                join,
                REFERENCE,
            );
        } else if arg.kind() == c"identifier"
            && let Some(source) = source
            && file.scope_lookup(source).is_none()
        {
            file.emit(
                arg,
                source,
                c"lsp_unresolved",
                0.,
                Some(c"callable_value_not_in_registry"),
                REFERENCE,
            );
        }
    }
}

fn invoke_selector(file: &File<'_>, selector_node: Node<'_>, site: Node<'_>) {
    let Some(Selector {
        operand,
        member,
        import,
    }) = selector(file, selector_node)
    else {
        return;
    };
    if let Some((alias, package)) = import {
        if let Some(target) =
            file.symbol(package, member).and_then(|f| f.qualified_name)
        {
            file.emit(site, target, c"lsp_direct", 0.95, None, INVOCATION);
        } else {
            unresolved(
                file,
                site,
                qualified(file, alias, member),
                c"symbol_not_in_registry",
            );
        }
        return;
    }

    let recv = evaluate(file, Some(operand), 0);
    let base = match file.view(recv) {
        TypeView::Pointer(elem) => elem,
        _ => recv,
    };
    let iface = match file.view(base) {
        TypeView::Named(ty) => {
            let (qualified_ty, registered) = members::receiver(file, ty);
            if registered.is_some_and(|ty| ty.is_interface()) {
                Some(registered.and_then(|ty| ty.name()))
            } else {
                if let Some(method) =
                    members::method(file, qualified_ty, member, 0)
                {
                    let strategy = if method
                        .receiver_type
                        .is_some_and(|recv| recv != qualified_ty)
                    {
                        c"lsp_embed_dispatch"
                    } else {
                        c"lsp_type_dispatch"
                    };
                    if let Some(target) = method.qualified_name {
                        file.emit(
                            site, target, strategy, 0.95, None, INVOCATION,
                        );
                    }
                } else {
                    unresolved(
                        file,
                        site,
                        qualified(file, ty, member),
                        c"method_not_found",
                    );
                }
                None
            }
        }
        TypeView::Interface => Some(None),
        _ => {
            if matches!(file.view(recv), TypeView::Unknown) {
                let operand = name(file, operand).unwrap_or(c"?");
                unresolved(
                    file,
                    site,
                    qualified(file, operand, member),
                    c"unknown_receiver_type",
                );
            }
            None
        }
    };

    if let Some(iface) = iface {
        if let Some(concrete) = iface.and_then(|iface| {
            members::concrete_interface_method(file, iface, member)
        }) {
            if let Some(target) = concrete.qualified_name {
                file.emit(
                    site,
                    target,
                    c"lsp_interface_resolve",
                    0.95,
                    None,
                    INVOCATION,
                );
            }
        } else if let Some(target) =
            qualified(file, iface.unwrap_or(c"interface"), member)
        {
            file.emit(
                site,
                target,
                c"lsp_interface_dispatch",
                0.85,
                None,
                INVOCATION,
            );
        }
    }
}

fn invocation(file: &File<'_>, node: Node<'_>) {
    references(file, node);
    let Some(target) = node.field(c"function") else {
        return;
    };
    match target.kind().to_bytes() {
        b"selector_expression" => invoke_selector(file, target, node),
        b"identifier" => {
            let Some(name) =
                name(file, target).filter(|name| !builtin_call(name.to_bytes()))
            else {
                return;
            };
            if file.scope_lookup(name).is_some() {
                if let Some(target) = file.scope_callable(name) {
                    file.emit(
                        node,
                        target,
                        c"lsp_callable_alias",
                        0.97,
                        Some(name),
                        INVOCATION,
                    );
                } else {
                    unresolved(
                        file,
                        node,
                        Some(name),
                        c"lexical_value_not_exact_callable",
                    );
                }
            } else if let Some(function) = file.symbol(file.package, name) {
                if let Some(target) = function.qualified_name {
                    file.emit(
                        node,
                        target,
                        c"lsp_direct",
                        0.95,
                        None,
                        INVOCATION,
                    );
                }
            } else {
                unresolved(file, node, Some(name), c"function_not_in_registry");
            }
        }
        _ => {}
    }
}

fn invalidate_aliases(file: &File<'_>, node: Node<'_>, depth: usize) {
    if depth > 64 {
        return;
    }
    match node.kind().to_bytes() {
        // Closure assignments do not join the current scope.
        b"func_literal" => {}
        b"assignment_statement" => {
            if let Some(left) = node.field(c"left") {
                for target in
                    values(left).filter(|node| node.kind() == c"identifier")
                {
                    if let Some(name) = name(file, target)
                        && file.scope_callable(name).is_some()
                    {
                        file.clear_callable(name);
                    }
                }
            }
        }
        _ => {
            for child in node.named_children() {
                invalidate_aliases(file, child, depth + 1);
            }
        }
    }
}

fn case_type(node: Node<'_>) -> bool {
    matches!(
        node.kind().to_bytes(),
        b"type_identifier" | b"qualified_type" | b"pointer_type" | b"slice_type"
    )
}

fn type_switch(file: &File<'_>, node: Node<'_>) {
    let mut switch_name = None;
    let mut assigned = false;
    for child in node.children() {
        if !assigned
            && child.kind() == c"expression_list"
            && let Some(ident) = child
                .named_children()
                .next()
                .filter(|n| n.kind() == c"identifier")
        {
            switch_name = name(file, ident);
        }
        if !child.is_named() && spelling(file, child) == b":=" {
            assigned = true;
        }
        if assigned && child.kind() == c"identifier" {
            evaluate(file, Some(child), 0);
        }
    }
    for branch in node
        .named_children()
        .filter(|node| node.kind() == c"type_case")
    {
        let _scope = file.push_scope();
        if let Some(name) = switch_name
            && let Some(ty) =
                branch.named_children().find(|node| case_type(*node))
        {
            file.bind(name, parse(file, Some(ty)));
        }
        for child in branch.named_children().filter(|node| !case_type(*node)) {
            file.walk(child);
        }
    }
}

fn receive(file: &File<'_>, node: Node<'_>) {
    let Some(right) = node.field(c"right") else {
        return;
    };
    let ty = evaluate(file, Some(right), 0);
    let received = match file.view(ty) {
        TypeView::Channel(elem) => elem,
        _ => ty,
    };
    if let Some(left) = node.field(c"left") {
        for (position, target) in values(left).enumerate() {
            if let Some(name) = binding_name(file, target) {
                file.bind(
                    name,
                    if position == 0 {
                        received
                    } else {
                        file.builtin(c"bool")
                    },
                );
            }
        }
    }
}

fn select(file: &File<'_>, node: Node<'_>) {
    for branch in node.named_children().filter(|node| {
        matches!(
            node.kind().to_bytes(),
            b"communication_case" | b"default_case"
        )
    }) {
        let _scope = file.push_scope();
        for child in branch.named_children() {
            if child.kind() == c"receive_statement" {
                receive(file, child);
            } else {
                file.walk(child);
            }
        }
    }
}

pub(super) fn resolve(file: &File<'_>, node: Node<'_>) {
    statement(file, node);
    let kind = node.kind().to_bytes();
    let scoped_statement = matches!(
        kind,
        b"if_statement" | b"for_statement" | b"expression_switch_statement"
    );
    if scoped_statement
        || matches!(kind, b"type_switch_statement" | b"select_statement")
    {
        invalidate_aliases(file, node, 0);
    }

    match kind {
        b"call_expression" => invocation(file, node),
        b"type_switch_statement" => return type_switch(file, node),
        b"select_statement" => return select(file, node),
        _ => {}
    }

    let _scope =
        (scoped_statement || kind == b"block").then(|| file.push_scope());
    if scoped_statement && let Some(initializer) = node.field(c"initializer") {
        statement(file, initializer);
    }
    if kind == b"for_statement"
        && let Some(range) = node
            .named_children()
            .find(|node| node.kind() == c"range_clause")
    {
        statement(file, range);
    }
    for child in node.children() {
        file.walk(child);
    }
}
