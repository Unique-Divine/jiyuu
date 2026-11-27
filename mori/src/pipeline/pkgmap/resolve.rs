//! Import resolution in the pipeline's precedence order. Bounds mirror the
//! producer's path and symbol buffers so fallback targets stay deterministic.

use super::graph::*;
use super::{
    HashTable, PATH_LIMIT, c_bytes, cbm_pipeline_fqn_module, dirname, duplicate,
    join, limit,
};
use std::ffi::{CStr, c_char, c_uint};
use std::ptr::null;

pub(super) fn module_qn(res: &ModuleContext<'_>, path: &[u8]) -> *mut c_char {
    if let Some(relative) = res.relative(path) {
        // The relative resolver already stripped an explicit JS/TS extension.
        return take_text(res.folder(relative.bytes()));
    }
    if let Some(source) = res.source.filter(|_| !res.aliases.is_null()) {
        let aliases = unsafe {
            cbm_path_alias_find_for_file(res.aliases, source.as_ptr())
        };
        if !aliases.is_null() {
            let aliased = with_c_string(path, |path| {
                Text::new(unsafe {
                    cbm_path_alias_resolve(aliases, path.as_ptr())
                })
            });
            if let Some(aliased) = aliased {
                return take_text(fqn_module(res.project, aliased.bytes()));
            }
        }
    }

    if let Some(map) = unsafe { pkgmap() } {
        if let Some(qn) = lookup(map, path) {
            return duplicate(qn);
        }
        for &sep in b"/.\\" {
            for (cut, _) in path
                .iter()
                .enumerate()
                .rev()
                .filter(|&(i, &byte)| i > 0 && byte == sep)
            {
                let key = if sep == b'\\' {
                    &path[..(cut + 1).min(PATH_LIMIT)]
                } else {
                    &path[..cut]
                };
                let Some(base) = lookup(map, key) else {
                    continue;
                };
                let tail = &path[cut + 1..];
                return match sep {
                    b'/' => duplicate(&limit(
                        [base, b".", &replace(tail, b'/', b'.')].concat(),
                    )),
                    b'.' => take_text(fqn_module(
                        res.project,
                        &limit(
                            [base, b"/", &replace(tail, b'.', b'/')].concat(),
                        ),
                    )),
                    _ => take_text(fqn_module(
                        res.project,
                        &replace(&join(base, tail), b'\\', b'/'),
                    )),
                };
            }
        }
    }
    take_text(fqn_module(res.project, path))
}

pub(super) fn replace(source: &[u8], old: u8, new: u8) -> Vec<u8> {
    source
        .iter()
        .map(|&byte| if byte == old { new } else { byte })
        .collect()
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn cbm_pipeline_resolve_module(
    ctx: *const Context,
    source: *const c_char,
    path: *const c_char,
) -> *mut c_char {
    let (modules, _) = unsafe { ModuleContext::read(ctx, source) };
    match unsafe { c_bytes(path) } {
        Some(path) if !ctx.is_null() => module_qn(&modules, path),
        _ => unsafe { cbm_pipeline_fqn_module(modules.project, path) },
    }
}

pub(super) fn targetable(node: &Node) -> bool {
    matches!(
        node.label(),
        Some(
            b"Class"
                | b"Interface"
                | b"Function"
                | b"Method"
                | b"Module"
                | b"Struct"
                | b"Enum"
                | b"Trait"
                | b"Type"
                | b"File"
        )
    )
}

pub(super) fn module_file(node: &Node) -> bool {
    matches!(node.label(), Some(b"Module" | b"File"))
}

fn with_extension(path: &[u8], suffixes: &[&[u8]]) -> bool {
    suffixes
        .iter()
        .any(|suffix| path.len() > suffix.len() && path.ends_with(suffix))
}

fn resolve_header<'a>(
    res: &Resolution<'a>,
    source: &[u8],
    path: &[u8],
) -> Option<&'a Node> {
    if !with_extension(
        source,
        &[
            b".c", b".cc", b".cpp", b".cxx", b".c++", b".h", b".hh", b".hpp",
            b".hxx",
        ],
    ) || !with_extension(
        path,
        &[b".h", b".hh", b".hpp", b".hxx", b".inc", b".inl", b".ipp"],
    ) {
        return None;
    }
    let path = path.strip_prefix(b"./").unwrap_or(path);
    exact_file(res, path)
        .or_else(|| exact_file(res, &join(dirname(source), path)))
}

fn exact_file<'a>(res: &Resolution<'a>, path: &[u8]) -> Option<&'a Node> {
    fn rank(node: &Node) -> (bool, usize, &[u8]) {
        let path = node.path().unwrap_or_default();
        let depth = path
            .iter()
            .filter(|byte| matches!(byte, b'/' | b'\\'))
            .count();
        (node.label() != Some(b"File"), depth, path)
    }

    let leaf = leaf(path);
    if leaf.is_empty() {
        return None;
    }
    let stem = &leaf[..leaf.len().min(PATH_LIMIT)];
    let stem = match stem.iter().rposition(|&byte| byte == b'.') {
        Some(dot) if dot > 0 => &stem[..dot],
        _ => stem,
    };
    let mut best: Option<&Node> = None;
    for node in [stem, leaf]
        .into_iter()
        .flat_map(|name| nodes(res.graph.name(name)))
    {
        if !targetable(node)
            || !node.other_than(res.source_qn)
            || !node
                .path()
                .is_some_and(|candidate| candidate.ends_with(path))
        {
            continue;
        }
        if best.is_none_or(|best| {
            rank(node) < rank(best)
                || (rank(node) == rank(best) && qn_before(node, best))
        }) {
            best = Some(node);
        }
    }
    best
}

fn qn_before(first: &Node, second: &Node) -> bool {
    matches!((first.qn(), second.qn()), (Some(a), Some(b)) if a < b)
}

fn explicit_relative(path: &[u8]) -> bool {
    path.starts_with(b"./") || path.starts_with(b"../")
}

fn sibling<'a>(
    res: &Resolution<'a>,
    source: &[u8],
    path: &[u8],
    php: bool,
) -> Option<&'a Node> {
    if path.is_empty() {
        return None;
    }
    let dir = dirname(source);
    let base = path.strip_prefix(b"./").unwrap_or(path);
    let (subdir, filename) = split_last(base, b'/').unwrap_or((b"", base));
    let scss = (!filename.starts_with(b"_")).then(|| {
        let partial = [b"_", filename, b".scss"].concat();
        let partial = if subdir.is_empty() {
            partial
        } else {
            [subdir, b"/", &partial].concat()
        };
        join(dir, &partial)
    });

    [
        Some(join(dir, base)),
        scss,
        Some(join(dir, &[base, b"/meson.build"].concat())),
        (base.contains(&b'/')
            && !filename.is_empty()
            && !explicit_relative(path))
        .then(|| join(dir, filename)),
    ]
    .into_iter()
    .flatten()
    .find_map(|candidate| {
        res.module_at(&candidate).filter(|node| {
            (!php || node.php())
                && targetable(node)
                && node.other_than(res.source_qn)
        })
    })
}

fn kustomize<'a>(res: &Resolution<'a>, path: &[u8]) -> Option<&'a Node> {
    if path.is_empty()
        || [b"://".as_slice(), b"?ref=", b".git/"]
            .into_iter()
            .any(|marker| contains(path, marker))
        || [
            b"github.com/".as_slice(),
            b"gitlab.com/",
            b"bitbucket.org/",
            b"git@",
        ]
        .into_iter()
        .any(|prefix| path.starts_with(prefix))
    {
        return None;
    }
    let rel = limit(
        [
            if explicit_relative(path) {
                b"".as_slice()
            } else {
                b"./"
            },
            path,
        ]
        .concat(),
    );
    let path = res.modules.relative(&rel)?;
    if let Some(module) = res
        .module_at(path.bytes())
        .filter(|node| module_file(node) && node.path() == Some(path.bytes()))
    {
        return Some(module);
    }
    let folder = res.modules.folder(path.bytes())?;
    let is_folder = |node: &&Node| node.label() == Some(b"Folder");
    res.graph.qn(folder.bytes()).filter(is_folder).or_else(|| {
        let marked = [folder.bytes(), b"/"].concat();
        if marked.len() > PATH_LIMIT {
            return None;
        }
        res.graph.qn(&marked).filter(is_folder)
    })
}

pub(super) fn split_last(bytes: &[u8], sep: u8) -> Option<(&[u8], &[u8])> {
    let cut = bytes.iter().rposition(|&byte| byte == sep)?;
    Some((&bytes[..cut], &bytes[cut + 1..]))
}

pub(super) fn find(bytes: &[u8], needle: &[u8]) -> Option<usize> {
    bytes.windows(needle.len()).position(|part| part == needle)
}

pub(super) fn contains(bytes: &[u8], needle: &[u8]) -> bool {
    find(bytes, needle).is_some()
}

pub(super) fn before<'a>(bytes: &'a [u8], needle: &[u8]) -> &'a [u8] {
    &bytes[..find(bytes, needle).unwrap_or(bytes.len())]
}

pub(super) fn truncate_path(path: &[u8]) -> &[u8] {
    &path[..path.len().min(PATH_LIMIT)]
}

fn decorate_free(path: &[u8]) -> &[u8] {
    before(before(truncate_path(path), b"{"), b" as ")
}

fn trim_end(mut input: &[u8], trim: impl Fn(u8) -> bool) -> &[u8] {
    while let Some((&byte, rest)) = input.split_last() {
        if !trim(byte) {
            break;
        }
        input = rest;
    }
    input
}

fn symbol_tail(path: &[u8]) -> &[u8] {
    path.rsplit(|byte| matches!(byte, b'.' | b'/' | b'\\' | b':'))
        .next()
        .unwrap_or_default()
}

fn symbol(path: &[u8]) -> Option<&[u8]> {
    let path = truncate_path(path);
    if let Some(brace) = find(path, b"{") {
        let first = &path[brace + 1..];
        let first = first
            .split(|byte| matches!(byte, b',' | b'}'))
            .next()
            .unwrap_or_default();
        let first = first
            .iter()
            .position(|byte| !matches!(byte, b' ' | b'\t'))
            .map_or(b"".as_slice(), |start| &first[start..]);
        let first = trim_end(first, |byte| matches!(byte, b' ' | b'\t'));
        if !first.is_empty() && first != b"self" {
            return Some(&first[..first.len().min(255)]);
        }
    }
    let trimmed = trim_end(decorate_free(path), |byte| {
        matches!(byte, b'*' | b':' | b'.' | b'/' | b'\\' | b' ')
    });
    let last = symbol_tail(trimmed);
    valid_symbol(last).then_some(&last[..last.len().min(255)])
}

fn valid_symbol(name: &[u8]) -> bool {
    !name.is_empty() && name != b"*"
}

fn namespace_key(path: &[u8]) -> Vec<u8> {
    path.iter()
        .map(|&byte| match byte {
            b'\\' | b':' | b'/' => b'.',
            _ => byte,
        })
        .collect()
}

pub(super) fn namespace_nodes<'a>(
    graph: &'a Buffer,
    bucket: Option<&[u8]>,
) -> impl Iterator<Item = &'a Node> {
    bucket
        .into_iter()
        .flat_map(|bucket| bucket.split(|&byte| byte == b'\n'))
        .filter(|row| !row.is_empty())
        .filter(|row| row.len() <= PATH_LIMIT)
        .filter_map(|row| graph.qn(row))
}

fn namespace_target<'a>(
    res: &Resolution<'a>,
    map: &HashTable,
    path: &[u8],
    php: bool,
) -> Option<&'a Node> {
    let key = namespace_key(decorate_free(path));
    let key = trim_end(&key, |byte| matches!(byte, b'*' | b'.' | b' '));
    let mut collapsed = Vec::with_capacity(key.len());
    for &byte in key {
        if byte != b'.' || collapsed.last() != Some(&b'.') {
            collapsed.push(byte);
        }
    }
    let mut prefix = collapsed.as_slice();
    loop {
        if let Some(node) = namespace_nodes(res.graph, lookup(map, prefix))
            .find(|node| (!php || node.php()) && node.other_than(res.source_qn))
        {
            return Some(node);
        }
        prefix = split_last(prefix, b'.')?.0;
    }
}

pub(super) fn module_prefixes<'a>(
    res: &Resolution<'a>,
    mut work: &[u8],
    keep: usize,
    rust: bool,
    php: bool,
) -> Option<&'a Node> {
    let valid = |node: &&Node| {
        (!php || node.php())
            && targetable(node)
            && node.other_than(res.source_qn)
    };
    loop {
        if rust {
            if let Some(node) = res.module_at(work).filter(valid) {
                return Some(node);
            }
            if work.len() + 4 <= PATH_LIMIT
                && let Some(node) =
                    res.module_at(&[work, b"/mod"].concat()).filter(valid)
            {
                return Some(node);
            }
        } else if let Some(node) = res.module(work).filter(valid) {
            return Some(node);
        }
        work = split_last(work, b'/')?.0;
        if work.len() < keep {
            return None;
        }
    }
}

fn symbol_fallback<'a>(
    res: &Resolution<'a>,
    imp: &Import,
    path: &[u8],
    lang: Language,
    php: bool,
) -> Option<&'a Node> {
    let primary =
        symbol(path).or_else(|| imp.local().filter(|name| valid_symbol(name)));
    let norm = namespace_key(decorate_free(path));
    let mut names: Vec<&[u8]> = primary
        .into_iter()
        .filter(|name| valid_symbol(name))
        .collect();
    for part in norm.rsplit(|&byte| byte == b'.') {
        if names.len() == 8 {
            break;
        }
        if valid_symbol(part) && !names.contains(&part) {
            names.push(part);
        }
    }
    let py_external =
        lang == Language::Python && !super::python::module_in_project(res, path);
    for (index, name) in names.into_iter().enumerate() {
        let mut best: Option<&Node> = None;
        for node in nodes(res.graph.name(name)) {
            if (php && !node.php())
                || !targetable(node)
                || super::python::local_definition(res.graph, node)
                || node.generated()
                || !node.other_than(res.source_qn)
                || (py_external
                    && super::python::reject_external_hit(
                        imp,
                        name,
                        index > 0,
                        node,
                    ))
            {
                continue;
            }
            if best.is_none_or(|best| qn_before(node, best)) {
                best = Some(node);
            }
        }
        if best.is_some() {
            return best;
        }
    }
    None
}

fn generic<'a>(
    res: &Resolution<'a>,
    imp: &Import,
    source: &[u8],
    path: &[u8],
    namespaces: Option<&HashTable>,
    lang: Language,
    php: bool,
) -> Option<&'a Node> {
    if let Some(target) = res.module(path).filter(|node| !php || node.php()) {
        if module_file(target)
            && let (Some(qn), Some(sym)) =
                (target.qn(), symbol(path).filter(|name| valid_symbol(name)))
            && symbol_tail(qn) != sym
        {
            let member = [qn, b".", sym].concat();
            if let Some(node) = res
                .graph
                .qn(&member[..member.len().min(511)])
                .filter(|node| {
                    (!php || node.php())
                        && targetable(node)
                        && !module_file(node)
                })
            {
                return Some(node);
            }
        }
        return Some(target);
    }

    if lang == Language::Rust
        && !php
        && res.modules.source.is_some()
        && let Some(scoped) = super::rust_use::resolve(res, source, path)
    {
        return scoped;
    }

    if lang != Language::Go
        && let Some(target) = sibling(res, source, path, php)
    {
        return Some(target);
    }
    if res.modules.source.is_some()
        && source.len() <= PATH_LIMIT
        && explicit_relative(path)
        && let Some(node) = rel_join(dirname(source), path)
            .and_then(|rel| res.file(&rel))
            .filter(|node| !php || node.php())
    {
        return Some(node);
    }

    if let Some(target) =
        namespaces.and_then(|map| namespace_target(res, map, path, php))
    {
        return Some(target);
    }

    if lang != Language::Go
        && let Some(target) = symbol_fallback(res, imp, path, lang, php)
    {
        return Some(target);
    }

    let mut clean = Vec::new();
    for &byte in decorate_free(path) {
        match byte {
            b':' if clean.last() != Some(&b'/') => clean.push(b'/'),
            b':' | b'*' | b' ' => {}
            _ => clean.push(byte),
        }
    }
    let clean = trim_end(&clean, |byte| byte == b'/');
    let body = [b"crate/".as_slice(), b"self/", b"super/"]
        .into_iter()
        .find_map(|prefix| clean.strip_prefix(prefix))
        .unwrap_or(clean);
    if body.is_empty() {
        None
    } else {
        module_prefixes(res, body, 0, false, php)
    }
}

pub(super) fn import_node<'a>(
    res: &Resolution<'a>,
    imp: &Import,
    map: Option<&HashTable>,
) -> Option<&'a Node> {
    use super::php::{UseShape, declared_use, psr4_class, use_shape};

    let path = imp.path()?;
    let source = res.modules.source.map_or(b"".as_slice(), CStr::to_bytes);
    let lang = language(leaf(source));
    if lang == Language::Kustomize {
        return kustomize(res, path);
    }
    if let Some(target) = resolve_header(res, source, path) {
        return Some(target);
    }
    if let Some(outcome) = psr4_class(res.graph, res.source_qn, path, imp.kind) {
        return outcome;
    }

    let shape = use_shape(lang, path);
    if let Some(map) = map.filter(|_| shape == UseShape::Namespaced) {
        return declared_use(res, path, map);
    }
    generic(res, imp, source, path, map, lang, shape != UseShape::None)
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn cbm_pipeline_resolve_import_node(
    ctx: *const Context,
    source: *const c_char,
    source_qn: *const c_char,
    imp: *const Import,
    map: *const HashTable,
) -> *const Node {
    unsafe {
        let Some(res) = Resolution::new(ctx, source, source_qn) else {
            return null();
        };
        ptr(imp
            .as_ref()
            .and_then(|imp| import_node(&res, imp, map.as_ref())))
    }
}

#[unsafe(no_mangle)]
pub(crate) extern "C" fn cbm_import_symbol_fallback_allowed(
    language: c_uint,
) -> bool {
    // CBM_LANG_GO is the first variant of CBMLanguage.
    language != 0
}
