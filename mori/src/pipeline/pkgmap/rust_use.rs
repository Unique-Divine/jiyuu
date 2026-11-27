//! Resolve Rust use paths within the crate that owns the importer or the
//! workspace member that the path names.

use super::graph::*;
use super::resolve::{before, module_prefixes, split_last, targetable};
use super::{PATH_LIMIT, dirname};
use std::cmp::Ordering;

fn ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80
}

fn space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

fn skip_space(path: &[u8]) -> &[u8] {
    path.iter()
        .position(|&byte| !space(byte))
        .map_or(b"", |start| &path[start..])
}

fn trim_path(mut path: &[u8]) -> &[u8] {
    while let Some((&byte, rest)) = path.split_last() {
        if !space(byte) && byte != b':' {
            break;
        }
        path = rest;
    }
    path
}

fn path_text(path: &[u8]) -> Vec<u8> {
    let mut path = skip_space(path);
    if let Some(rest) = path
        .strip_prefix(b"pub")
        .filter(|tail| !tail.first().copied().is_some_and(ident))
    {
        path = skip_space(rest);
        if path.starts_with(b"(")
            && let Some(end) = path.iter().position(|&byte| byte == b')')
        {
            path = skip_space(&path[end + 1..]);
        }
    }
    if let Some(rest) = path
        .strip_prefix(b"use")
        .filter(|tail| tail.first().copied().is_some_and(space))
    {
        path = skip_space(rest);
    }
    let Some(brace) = path.iter().position(|&byte| byte == b'{') else {
        return path.to_vec();
    };
    let member = skip_space(&path[brace + 1..])
        .split(|byte| matches!(byte, b',' | b'}' | b'{'))
        .next()
        .unwrap_or_default();
    let member = trim_path(member);
    if member.is_empty() || member == b"self" {
        return path[..brace].to_vec();
    }
    [&path[..brace], member].concat()
}

struct UsePath<'a> {
    segments: Vec<&'a [u8]>,
    global: bool,
    glob: bool,
}

impl<'a> UsePath<'a> {
    fn parse(path: &'a [u8]) -> Option<Self> {
        if path.len() > PATH_LIMIT {
            return None;
        }
        let path = trim_path(skip_space(before(path, b" as ")));
        let (path, glob) = path
            .strip_suffix(b"*")
            .map_or((path, false), |tail| (trim_path(tail), true));
        let (mut path, global) = path
            .strip_prefix(b"::")
            .map_or((path, false), |rest| (rest, true));
        let mut segments = Vec::new();
        while !path.is_empty() {
            path = skip_space(path);
            path = path.strip_prefix(b"r#").unwrap_or(path);
            let end = path
                .iter()
                .position(|&byte| !ident(byte))
                .unwrap_or(path.len());
            if end == 0 || segments.len() == 64 {
                return None;
            }
            segments.push(&path[..end]);
            let remaining = skip_space(&path[end..]);
            path = if remaining.is_empty() {
                remaining
            } else {
                remaining.strip_prefix(b"::")?
            };
        }
        (!segments.is_empty()).then_some(Self {
            segments,
            global,
            glob,
        })
    }
}

fn unbounded_join(root: &[u8], path: &[u8]) -> Vec<u8> {
    if root.is_empty() {
        path.to_vec()
    } else {
        [root, b"/", path].concat()
    }
}

fn crate_dir<'a>(res: &Resolution<'_>, mut path: &'a [u8]) -> Option<&'a [u8]> {
    if path.len() > PATH_LIMIT {
        return None;
    }
    loop {
        let parent = split_last(path, b'/');
        path = parent.map_or(b"", |(dir, _)| dir);
        let manifest = unbounded_join(path, b"Cargo.toml");
        if manifest.len() <= PATH_LIMIT && res.file(&manifest).is_some() {
            return Some(path);
        }
        parent?;
    }
}

fn file_module<'a>(path: &'a [u8], root: &[u8]) -> Option<&'a [u8]> {
    let relative = if root.is_empty() {
        path
    } else {
        path.strip_prefix(root)?.strip_prefix(b"/")?
    };
    let module = relative.strip_suffix(b".rs")?;
    if module.is_empty() || module.len() > PATH_LIMIT {
        return None;
    }
    if matches!(module, b"lib" | b"main" | b"mod") {
        return Some(b"");
    }
    Some(match module.strip_suffix(b"/mod") {
        Some(stem) if !stem.is_empty() => stem,
        _ => module,
    })
}

fn workspace_root<'a>(res: &Resolution<'a>, name: &[u8]) -> Option<&'a [u8]> {
    let map = unsafe { pkgmap() }?;
    let qn = lookup(map, name).or_else(|| {
        if name.len() > 255 || !name.contains(&b'_') {
            return None;
        }
        lookup(map, &super::resolve::replace(name, b'_', b'-'))
    })?;
    let path = res.graph.qn(qn)?.path()?;
    if !path.ends_with(b".rs") {
        return None;
    }
    let (dir, _) = split_last(path, b'/')?;
    (dir.len() <= PATH_LIMIT).then_some(dir)
}

fn child(res: &Resolution<'_>, root: &[u8], own: &[u8], name: &[u8]) -> bool {
    let dir = unbounded_join(root, own);
    if dir.is_empty() || dir.len() > PATH_LIMIT {
        return false;
    }
    [b".rs".as_slice(), b"/mod.rs"].into_iter().any(|tail| {
        let path = unbounded_join(&dir, &[name, tail].concat());
        path.len() <= PATH_LIMIT && res.file(&path).is_some()
    })
}

struct Candidate<'a> {
    name: &'a [u8],
    module_len: usize,
}

fn choose<'a>(
    res: &Resolution<'a>,
    root: &[u8],
    want: &[u8],
    name: &[u8],
    type_only: bool,
) -> Option<(&'a Node, usize)> {
    fn modules(path: &[u8]) -> impl Iterator<Item = &[u8]> {
        path.strip_suffix(b"/")
            .unwrap_or(path)
            .split(|&byte| byte == b'/')
            .take(if path.is_empty() { 0 } else { usize::MAX })
    }
    let wanted = modules(want).count();
    let mut best: Option<(&Node, usize)> = None;
    for node in nodes(res.graph.name(name)) {
        if node.qn().is_none()
            || !targetable(node)
            || (type_only && !node.type_like())
            || node.generated()
            || !node.other_than(res.source_qn)
        {
            continue;
        }
        let Some(module) = node.path().and_then(|path| file_module(path, root))
        else {
            continue;
        };
        let common = modules(module)
            .zip(modules(want))
            .take_while(|(actual, expected)| actual == expected)
            .count();
        if common != wanted {
            continue;
        }
        let extra = modules(module).count() - common;
        if best.is_none_or(|(previous, fewer)| {
            extra
                .cmp(&fewer)
                .then_with(|| node.qn().cmp(&previous.qn()))
                == Ordering::Less
        }) {
            best = Some((node, extra));
        }
    }
    best
}

// None means no crate-scoped policy applies. Some(None) is a handled miss:
// an external crate or a failed lookup inside the named crate.
pub(super) fn resolve<'a>(
    res: &Resolution<'a>,
    source: &[u8],
    path: &[u8],
) -> Option<Option<&'a Node>> {
    let text = path_text(path);
    let use_path = UsePath::parse(&text)?;
    let package = crate_dir(res, source)?;
    let crate_prefix = package.len() + usize::from(!package.is_empty());
    let slash = source
        .get(crate_prefix..)?
        .iter()
        .position(|&byte| byte == b'/');
    let mut root =
        slash.map_or(package, |slash| &source[..crate_prefix + slash]);
    let own = match slash {
        Some(_) => file_module(source, root)?,
        None => b"",
    };

    let mut segments = use_path.segments.as_slice();
    let mut base = b"".as_slice();
    match (use_path.global, segments[0]) {
        (false, b"crate") => segments = &segments[1..],
        (false, b"self") => {
            base = own;
            segments = &segments[1..];
        }
        (false, b"super") => {
            base = own;
            while segments.first() == Some(&b"super".as_slice()) {
                if base.is_empty() {
                    return Some(None);
                }
                base = dirname(base);
                segments = &segments[1..];
            }
        }
        (false, head) if child(res, root, own, head) => base = own,
        (_, head) => {
            let Some(member) = workspace_root(res, head) else {
                return Some(None);
            };
            root = member;
            segments = &segments[1..];
        }
    }

    if !use_path.glob && segments.is_empty() {
        return Some(None);
    }
    let mut module = base.to_vec();
    let mut choices = Vec::new();
    for name in segments {
        choices.push(Candidate {
            name,
            module_len: module.len(),
        });
        if !module.is_empty() {
            module.push(b'/');
        }
        module.extend_from_slice(name);
        if module.len() > PATH_LIMIT {
            return Some(None);
        }
    }
    let mut choices = choices.into_iter().rev().take(8);
    let item = if use_path.glob {
        None
    } else {
        choices.next().and_then(|cand| {
            choose(res, root, &module[..cand.module_len], cand.name, false)
        })
    };
    if let Some((node, 0)) = item {
        return Some(Some(node));
    }
    if !module.is_empty() {
        let work = unbounded_join(root, &module);
        if work.len() <= PATH_LIMIT
            && let Some(node) =
                module_prefixes(res, &work, work.len(), true, false)
        {
            return Some(Some(node));
        }
    }
    Some(item.map(|(node, _)| node).or_else(|| {
        choices.find_map(|cand| {
            choose(res, root, &module[..cand.module_len], cand.name, true)
                .map(|(node, _)| node)
        })
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_first_group_member_and_strips_aliases() {
        let cases: &[(&[u8], &[&[u8]])] = &[
            (b"use crate::ops::{add, mul}", &[b"crate", b"ops", b"add"]),
            (
                b"use crate::util::helper as h",
                &[b"crate", b"util", b"helper"],
            ),
            (
                b"pub use crate::util::helper",
                &[b"crate", b"util", b"helper"],
            ),
        ];
        for &(input, expected) in cases {
            let path = path_text(input);
            let parsed = UsePath::parse(&path).unwrap();
            assert_eq!(parsed.segments, expected);
            assert!(!parsed.global && !parsed.glob);
        }
    }

    #[test]
    fn retains_the_module_named_by_a_glob() {
        let path = path_text(b"use crate::ops::*");
        let parsed = UsePath::parse(&path).unwrap();
        assert_eq!(parsed.segments, [b"crate".as_slice(), b"ops"]);
        assert!(parsed.glob);
    }
}
