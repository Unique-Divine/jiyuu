//! Literal manifest readers. The order of returned rows is part of package
//! precedence, including duplicate Composer keys and Python's src-layout entry.

use super::json::Document;
use super::{c_bytes, dirname, join, limit, trimmed_c_string};
use std::ffi::{CStr, c_char, c_int};

#[derive(Clone, Copy)]
pub(super) enum Manifest {
    Node,
    Go,
    Cargo,
    Python,
    Composer,
    Dart,
    Maven,
    Gradle,
    Elixir,
    Ruby,
    Swift,
}

pub(super) struct Entry {
    pub name: Vec<u8>,
    pub path: Vec<u8>,
}

impl Manifest {
    pub fn detect(name: &[u8]) -> Option<Self> {
        Some(match name {
            b"package.json" => Self::Node,
            b"go.mod" => Self::Go,
            b"Cargo.toml" => Self::Cargo,
            b"pyproject.toml" => Self::Python,
            b"composer.json" => Self::Composer,
            b"pubspec.yaml" => Self::Dart,
            b"pom.xml" => Self::Maven,
            b"build.gradle" | b"build.gradle.kts" => Self::Gradle,
            b"mix.exs" => Self::Elixir,
            b"Package.swift" => Self::Swift,
            name if name.ends_with(b".gemspec") => Self::Ruby,
            _ => return None,
        })
    }

    pub fn parse(self, path: &[u8], source: &[u8]) -> Vec<Entry> {
        let dir = dirname(path);
        let mut entries = Vec::new();
        let mut push = |name: &[u8], path: Vec<u8>| {
            entries.push(Entry {
                name: trimmed_c_string(name).to_vec(),
                path,
            })
        };
        match self {
            Self::Node => {
                if let Some(doc) = Document::read(source) {
                    let root = doc.root();
                    if let Some(name) = root
                        .get(c"name")
                        .string()
                        .filter(|name| !name.is_empty())
                    {
                        let exports = root.get(c"exports").get(c".");
                        let entry = exports
                            .string()
                            .or_else(|| {
                                [c"import", c"default", c"require"]
                                    .into_iter()
                                    .find_map(|key| exports.get(key).string())
                            })
                            .or_else(|| {
                                [c"main", c"module"]
                                    .into_iter()
                                    .find_map(|key| root.get(key).string())
                            })
                            .unwrap_or(b"src/index.ts");
                        if !entry.is_empty() {
                            let mut entry = join(
                                dir,
                                entry.strip_prefix(b"./").unwrap_or(entry),
                            );
                            if let Some(dot) = entry
                                .iter()
                                .rposition(|&byte| byte == b'.' || byte == b'/')
                                .filter(|&position| entry[position] == b'.')
                            {
                                entry.truncate(dot);
                            }
                            push(name, entry);
                        }
                    }
                }
            }
            Self::Go => {
                let module = line_value(source, b"module ")
                    .and_then(|value| token(trim_horizontal(value), b"\n\r "));
                if let Some(name) = module {
                    push(name, dir.to_vec());
                }
            }
            Self::Cargo => {
                if let Some(name) = toml_name(source, b"[package]") {
                    push(name, join(dir, b"src/lib"));
                }
            }
            Self::Python => {
                if let Some(name) = toml_name(source, b"[project]") {
                    let name = trimmed_c_string(name)
                        .iter()
                        .map(|&byte| if byte == b'-' { b'_' } else { byte })
                        .collect::<Vec<_>>();
                    push(
                        &name,
                        join(
                            dir,
                            &limit([b"src/", &name[..], b"/__init__"].concat()),
                        ),
                    );
                    push(
                        &name,
                        join(dir, &limit([&name[..], b"/__init__"].concat())),
                    );
                }
            }
            Self::Composer => {
                if let Some(doc) = Document::read(source) {
                    let root = doc.root();
                    if let Some(name) = root
                        .get(c"name")
                        .string()
                        .filter(|name| !name.is_empty())
                    {
                        push(name, dir.to_vec());
                    }
                    for section in [c"autoload", c"autoload-dev"] {
                        for (key, value) in
                            root.get(section).get(c"psr-4").pairs()
                        {
                            if let (Some(key), Some(mut location)) =
                                (key.string(), value.string())
                            {
                                while let Some(rest) =
                                    location.strip_prefix(b"./")
                                {
                                    location = rest;
                                }
                                if location == b"." {
                                    location = b"";
                                }
                                let mut entry = join(dir, location);
                                if entry.last() == Some(&b'/') {
                                    entry.pop();
                                }
                                push(key, entry);
                            }
                        }
                    }
                }
            }
            Self::Dart => {
                if let Ok(len) = c_int::try_from(source.len()) {
                    // The shared YAML parser owns the scalar until the document is freed.
                    let doc =
                        unsafe { cbm_yaml_parse(source.as_ptr().cast(), len) };
                    if !doc.is_null() {
                        let name =
                            unsafe { cbm_yaml_get_str(doc, c"name".as_ptr()) };
                        if let Some(name) = unsafe { c_bytes(name) }
                            .filter(|name| !name.is_empty())
                        {
                            push(name, join(dir, b"lib"));
                        }
                        unsafe { cbm_yaml_free(doc) };
                    }
                }
            }
            Self::Maven => {
                if let (Some(group), Some(artifact)) = (
                    pom_tag(source, b"<groupId>"),
                    pom_tag(source, b"<artifactId>"),
                ) {
                    let group = trimmed_c_string(group);
                    let artifact = trimmed_c_string(artifact);
                    let entry = join(dir, b"src/main/java");
                    push(
                        &limit([group, b".", artifact].concat()),
                        entry.clone(),
                    );
                    push(group, entry);
                }
            }
            Self::Gradle => {
                if let Some(group) =
                    line_value(source, b"group").and_then(quoted)
                {
                    push(group, join(dir, b"src/main/java"));
                }
            }
            Self::Elixir => {
                if let Some(name) = line_value(source, b"app:")
                    .and_then(|value| trim_horizontal(value).strip_prefix(b":"))
                    .and_then(|value| token(value, b",\n )"))
                {
                    let name = trimmed_c_string(name);
                    push(
                        &limit(name.to_vec()),
                        join(dir, &[b"lib/", name].concat()),
                    );
                }
            }
            Self::Ruby => {
                let source = trimmed_c_string(source);
                if let Some(name) = source
                    .windows(5)
                    .enumerate()
                    .filter(|(_, window)| *window == b".name")
                    .find_map(|(index, _)| quoted(&source[index + 5..]))
                {
                    push(name, join(dir, &[b"lib/", name].concat()));
                }
            }
            Self::Swift => crate::swift_package_map::scan_targets(
                source,
                path,
                |name, entry| {
                    push(name, entry.to_vec());
                },
            ),
        }
        entries
    }
}

unsafe extern "C" {
    fn cbm_yaml_parse(text: *const c_char, len: c_int) -> *mut std::ffi::c_void;
    fn cbm_yaml_get_str(
        doc: *const std::ffi::c_void,
        key: *const c_char,
    ) -> *const c_char;
    fn cbm_yaml_free(doc: *mut std::ffi::c_void);
}

fn trim_horizontal(source: &[u8]) -> &[u8] {
    let start = source
        .iter()
        .position(|byte| !matches!(byte, b' ' | b'\t'))
        .unwrap_or(source.len());
    &source[start..]
}

fn token<'a>(value: &'a [u8], delimiters: &[u8]) -> Option<&'a [u8]> {
    let end = value
        .iter()
        .position(|byte| delimiters.contains(byte))
        .unwrap_or(value.len());
    (end != 0).then_some(&value[..end])
}

fn line_value<'a>(mut source: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    loop {
        if let Some(value) = trim_horizontal(source).strip_prefix(prefix) {
            return Some(value);
        }
        source = &source[source.iter().position(|&byte| byte == b'\n')? + 1..];
    }
}

fn quoted(source: &[u8]) -> Option<&[u8]> {
    let start = source
        .iter()
        .position(|byte| !matches!(byte, b' ' | b'\t' | b'='))?;
    let (&quote, rest) = source[start..].split_first()?;
    if !matches!(quote, b'\'' | b'"') {
        return None;
    }
    let end = rest
        .iter()
        .position(|&byte| byte == quote || byte == b'\n')?;
    (rest[end] == quote).then_some(&rest[..end])
}

fn toml_name<'a>(source: &'a [u8], section: &[u8]) -> Option<&'a [u8]> {
    let mut tail = line_value(source, section)?;
    loop {
        let value = trim_horizontal(tail);
        if value.starts_with(b"[") {
            return None;
        }
        if let Some(name) = value
            .strip_prefix(b"name ")
            .or_else(|| value.strip_prefix(b"name="))
        {
            return quoted(name);
        }
        tail = &tail[tail.iter().position(|&byte| byte == b'\n')? + 1..];
    }
}

fn pom_tag<'a>(source: &'a [u8], tag: &[u8]) -> Option<&'a [u8]> {
    let mut parent = false;
    for index in 0..source.len() {
        let rest = &source[index..];
        if rest.starts_with(b"<parent>") {
            parent = true;
        }
        if rest.starts_with(b"</parent>") {
            parent = false;
        }
        if !parent && let Some(content) = rest.strip_prefix(tag) {
            return token(content, b"<");
        }
    }
    None
}

// Keep CStr at the boundary. Package paths and names can contain non-UTF-8 bytes.
pub(super) fn basename(path: &CStr) -> &[u8] {
    path.to_bytes()
        .rsplit(|&byte| byte == b'/')
        .next()
        .unwrap_or_default()
}
