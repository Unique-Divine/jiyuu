//! Go method sets, type aliases, and promoted fields and methods.

use std::ffi::CStr;

use super::native::{Callable, File, RegisteredType, Type};
use super::qualified_bytes;

const METHOD_DEPTH: usize = 16;
const FIELD_DEPTH: usize = 5;

/// A cross-file field type may still contain the declaring file's import
/// spelling. Resolve the last matching segment through the caller's imports.
pub(super) fn requalify<'file>(
    file: &File<'file>,
    ty: &CStr,
) -> Option<&'file CStr> {
    let ty = ty.to_bytes();
    for (alias, package) in file.imports() {
        let alias = alias.to_bytes();
        if alias.is_empty() || alias.contains(&b'.') {
            continue;
        }
        let suffix = ty
            .iter()
            .enumerate()
            .rev()
            .filter(|&(_, &byte)| byte == b'.')
            .find_map(|(start, _)| {
                ty[start + 1..].strip_prefix(alias)?.strip_prefix(b".")
            });
        if let Some(suffix) = suffix {
            return qualified_bytes(file, package.to_bytes(), suffix);
        }
    }
    None
}

pub(super) fn receiver<'file>(
    file: &File<'file>,
    ty: &'file CStr,
) -> (&'file CStr, Option<RegisteredType<'file>>) {
    if let Some(registered) = file.registry_type(ty) {
        return (ty, Some(registered));
    }
    if let Some(alternate) = requalify(file, ty)
        && let Some(registered) = file.registry_type(alternate)
    {
        return (alternate, Some(registered));
    }
    (ty, None)
}

pub(super) fn method<'file>(
    file: &File<'file>,
    ty: &CStr,
    name: &CStr,
    depth: usize,
) -> Option<Callable<'file>> {
    if depth > METHOD_DEPTH {
        return None;
    }
    if let Some(method) = file.direct_method(ty, name) {
        return Some(method);
    }
    if depth == 0
        && let Some(alternate) = requalify(file, ty)
        && let Some(method) = method(file, alternate, name, depth + 1)
    {
        return Some(method);
    }

    let registered = file.registry_type(ty)?;
    if let Some(aliased) = registered.alias()
        && let Some(method) = method(file, aliased, name, depth + 1)
    {
        return Some(method);
    }
    registered
        .embeds()
        .find_map(|embedded| method(file, embedded, name, depth + 1))
}

pub(super) fn field(
    file: &File<'_>,
    ty: &CStr,
    name: &CStr,
    depth: usize,
) -> Option<Type> {
    if depth > FIELD_DEPTH {
        return None;
    }
    let registered = file.registry_type(ty).or_else(|| {
        (depth == 0)
            .then(|| requalify(file, ty).and_then(|ty| file.registry_type(ty)))
            .flatten()
    })?;
    if let Some(aliased) = registered.alias() {
        return field(file, aliased, name, depth + 1);
    }
    registered.field(name).or_else(|| {
        registered
            .embeds()
            .find_map(|embedded| field(file, embedded, name, depth + 1))
    })
}

pub(super) fn concrete_interface_method<'file>(
    file: &File<'file>,
    interface: &CStr,
    member: &CStr,
) -> Option<Callable<'file>> {
    let registered = file.registry_type(interface)?;
    let methods = registered.method_names();
    if methods.is_null() || file.names(methods).next().is_none() {
        return None;
    }
    let project_interface = interface.to_bytes().contains(&b'/');
    let mut implementation = None;
    let mut ambiguous = false;
    file.for_each_type(|candidate| {
        if candidate.is_interface() || candidate.alias().is_some() {
            return true;
        }
        let Some(name) = candidate.name() else {
            return true;
        };
        if project_interface && !name.to_bytes().contains(&b'/') {
            return true;
        }
        if file
            .names(methods)
            .all(|member| file.direct_method(name, member).is_some())
        {
            if implementation.is_some() {
                ambiguous = true;
                return false;
            }
            implementation = Some(name);
        }
        true
    });
    if ambiguous {
        return None;
    }
    file.direct_method(implementation?, member)
}
