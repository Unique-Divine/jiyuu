//! Python import-chain checks shared by import, call, and base resolution.

use super::c_bytes;
use super::graph::*;
use super::resolve::{before, find, module_file, split_last, truncate_path};
use std::ffi::{c_char, c_uint};

fn path_matches(path: &[u8], name: &[u8], qn: &[u8]) -> bool {
    let path = before(truncate_path(path), b" as ");
    let path = path
        .iter()
        .position(|&byte| byte != b'.')
        .map_or(b"".as_slice(), |start| &path[start..]);
    let segments: Vec<_> = path
        .strip_suffix(b".")
        .unwrap_or(path)
        .split(|&byte| byte == b'.')
        .take(64)
        .collect();
    let Some(index) = segments.iter().rposition(|segment| *segment == name)
    else {
        return true;
    };
    if index == 0 {
        return true;
    }
    let Some((scope, _)) = split_last(qn, b'.') else {
        return false;
    };
    let mut enclosing = scope.split(|&byte| byte == b'.');
    segments[..index]
        .iter()
        .all(|part| enclosing.any(|segment| segment == *part))
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn cbm_python_import_path_matches_qn(
    path: *const c_char,
    name: *const c_char,
    qn: *const c_char,
) -> bool {
    match unsafe { (c_bytes(path), c_bytes(name), c_bytes(qn)) } {
        (Some(path), Some(name), Some(qn)) => path_matches(path, name, qn),
        _ => true,
    }
}

fn materialized(
    graph: &Buffer,
    project: *const c_char,
    rel: *const c_char,
    name: &[u8],
) -> bool {
    let Some(qn) = Text::new(unsafe {
        cbm_pipeline_fqn_compute(project, rel, c"__file__".as_ptr())
    }) else {
        return false;
    };
    let Some(edges) =
        graph.qn(qn.bytes()).and_then(|file| graph.imports(file.id))
    else {
        return false;
    };
    let needle = [b"\"local_name\":\"", name, b"\""].concat();
    let needle = &needle[..needle.len().min(255)];
    edges
        .iter()
        .filter_map(|&edge| unsafe { edge.as_ref() })
        .filter_map(|edge| unsafe { c_bytes(edge.properties) })
        .any(|props| find(props, needle).is_some())
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn cbm_python_import_binding_contradicts(
    imports: *const Imports,
    callee: *const c_char,
    qn: *const c_char,
    graph: *const Buffer,
    project: *const c_char,
    rel: *const c_char,
) -> bool {
    let (Some(imports), Some(callee), Some(qn)) = (unsafe {
        (
            imports.as_ref().and_then(Imports::rows),
            c_bytes(callee),
            c_bytes(qn),
        )
    }) else {
        return false;
    };
    if callee.is_empty() || qn.is_empty() {
        return false;
    }
    let root = before(callee, b".");
    let mut bound = false;
    for import in imports {
        let Some(path) = import.path().filter(|_| import.local() == Some(root))
        else {
            continue;
        };
        let full = truncate_path(path);
        let alias = find(full, b" as ");
        let full = before(full, b" as ");
        let split = split_last(full, b'.');
        let from = split.is_some_and(|(_, leaf)| leaf == root);
        let substituted;
        let full = if alias.is_none()
            && !from
            && full
                .strip_prefix(root)
                .is_some_and(|tail| tail.is_empty() || tail.starts_with(b"."))
        {
            truncate_path(callee)
        } else if alias.is_none()
            && split.is_none()
            && !full.starts_with(b".")
            && full != root
        {
            substituted = [path, &callee[root.len()..]].concat();
            truncate_path(&substituted)
        } else {
            full
        };
        let leaf = split_last(full, b'.').map_or(full, |(_, leaf)| leaf);
        if path_matches(full, leaf, qn)
            || (!rel.is_null()
                && unsafe { graph.as_ref() }.is_some_and(|graph| {
                    materialized(graph, project, rel, root)
                }))
        {
            return false;
        }
        bound = true;
    }
    bound
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn cbm_python_external_base_contradicts(
    lang: c_uint,
    imports: *const Imports,
    spelling: *const c_char,
    qn: *const c_char,
    graph: *const Buffer,
    project: *const c_char,
    rel: *const c_char,
) -> bool {
    // CBM_LANG_PYTHON follows CBM_LANG_GO.
    lang == 1
        && unsafe {
            cbm_python_import_binding_contradicts(
                imports, spelling, qn, graph, project, rel,
            )
        }
}

pub(super) fn module_in_project(res: &Resolution<'_>, path: &[u8]) -> bool {
    path.starts_with(b".")
        || split_last(before(truncate_path(path), b" as "), b'.')
            .is_some_and(|(module, _)| res.module(module).is_some())
}

fn plain_import(path: &[u8], local: Option<&[u8]>) -> bool {
    if path.starts_with(b".") || find(path, b" as ").is_some() {
        return false;
    }
    split_last(path, b'.').is_none_or(|(_, leaf)| {
        local == Some(before(path, b".")) && local != Some(leaf)
    })
}

pub(super) fn reject_external_hit(
    import: &Import,
    name: &[u8],
    enclosing: bool,
    node: &Node,
) -> bool {
    let Some(path) = import.path() else {
        return false;
    };
    !node.qn().is_none_or(|qn| path_matches(path, name, qn))
        || ((enclosing || plain_import(path, import.local()))
            && !module_file(node))
}

pub(super) fn local_definition(graph: &Buffer, node: &Node) -> bool {
    let (Some(path), Some(mut qn)) = (node.path(), node.qn()) else {
        return false;
    };
    let basename = path.rsplit(|&byte| byte == b'/').next().unwrap_or_default();
    if qn.is_empty() || qn.len() >= 512 || language(basename) != Language::Python
    {
        return false;
    }
    while let Some((scope, _)) = split_last(qn, b'.') {
        if graph.qn(scope).is_some_and(|outer| {
            outer.path() == Some(path)
                && matches!(outer.label(), Some(b"Function" | b"Method"))
        }) {
            return true;
        }
        qn = scope;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_matching_name_without_the_imports_module_chain() {
        assert!(!path_matches(
            b"unittest.mock.patch",
            b"patch",
            b"project.test_utils.patch",
        ));
        assert!(path_matches(
            b"..template_tests.utils.setup",
            b"setup",
            b"project.tests.template_tests.utils.setup",
        ));
    }
}
