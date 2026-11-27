//! Literal manifest readers. The order of returned rows is part of package
//! precedence, including duplicate Composer keys and Python's src-layout entry.

use super::json::Document;
use super::{PATH_LIMIT, c_bytes, dirname, trimmed_c_string};
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

// Keep the native path limit, including for unusually wide Composer maps.
struct EntryPath {
    bytes: [u8; PATH_LIMIT],
    len: usize,
}

impl EntryPath {
    fn from_parts(parts: &[&[u8]]) -> Self {
        let mut path = Self {
            bytes: [0; PATH_LIMIT],
            len: 0,
        };
        for part in parts {
            path.extend(part);
        }
        path
    }

    fn join(dir: &[u8], parts: &[&[u8]]) -> Self {
        let mut path = if dir.is_empty() {
            Self::from_parts(&[])
        } else {
            Self::from_parts(&[dir, b"/"])
        };
        for part in parts {
            path.extend(part);
        }
        path
    }

    fn extend(&mut self, bytes: &[u8]) {
        let count = bytes.len().min(PATH_LIMIT - self.len);
        self.bytes[self.len..self.len + count].copy_from_slice(&bytes[..count]);
        self.len += count;
    }

    fn bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
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

    pub fn parse(
        self,
        path: &[u8],
        source: &[u8],
        mut emit: impl FnMut(&[u8], &[u8]),
    ) {
        let dir = dirname(path);
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
                            let entry = EntryPath::join(
                                dir,
                                &[entry.strip_prefix(b"./").unwrap_or(entry)],
                            );
                            let bytes = entry.bytes();
                            let end = bytes
                                .iter()
                                .rposition(|&byte| byte == b'.' || byte == b'/')
                                .filter(|&position| bytes[position] == b'.')
                                .unwrap_or(bytes.len());
                            emit(name, &bytes[..end]);
                        }
                    }
                }
            }
            Self::Go => {
                let module = line_value(source, b"module ")
                    .and_then(|value| token(trim_horizontal(value), b"\n\r "));
                if let Some(name) = module {
                    emit(name, dir);
                }
            }
            Self::Cargo => {
                if let Some(name) = toml_name(source, b"[package]") {
                    emit(name, EntryPath::join(dir, &[b"src/lib"]).bytes());
                }
            }
            Self::Python => {
                if let Some(name) = toml_name(source, b"[project]") {
                    let name = trimmed_c_string(name);
                    let mut stem = EntryPath::from_parts(&[name]);
                    for byte in &mut stem.bytes[..stem.len] {
                        if *byte == b'-' {
                            *byte = b'_';
                        }
                    }
                    emit(
                        name,
                        EntryPath::join(
                            dir,
                            &[b"src/", stem.bytes(), b"/__init__"],
                        )
                        .bytes(),
                    );
                    emit(
                        name,
                        EntryPath::join(dir, &[stem.bytes(), b"/__init__"])
                            .bytes(),
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
                        emit(name, dir);
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
                                let entry = EntryPath::join(dir, &[location]);
                                let bytes = entry.bytes();
                                emit(
                                    key,
                                    bytes.strip_suffix(b"/").unwrap_or(bytes),
                                );
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
                            emit(name, EntryPath::join(dir, &[b"lib"]).bytes());
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
                    let entry = EntryPath::join(dir, &[b"src/main/java"]);
                    let package =
                        EntryPath::from_parts(&[group, b".", artifact]);
                    emit(package.bytes(), entry.bytes());
                    emit(group, entry.bytes());
                }
            }
            Self::Gradle => {
                if let Some(group) =
                    line_value(source, b"group").and_then(quoted)
                {
                    emit(
                        group,
                        EntryPath::join(dir, &[b"src/main/java"]).bytes(),
                    );
                }
            }
            Self::Elixir => {
                if let Some(name) = line_value(source, b"app:")
                    .and_then(|value| trim_horizontal(value).strip_prefix(b":"))
                    .and_then(|value| token(value, b",\n )"))
                {
                    let name = trimmed_c_string(name);
                    let atom = EntryPath::from_parts(&[name]);
                    emit(
                        atom.bytes(),
                        EntryPath::join(dir, &[b"lib/", name]).bytes(),
                    );
                }
            }
            Self::Ruby => {
                if let Some(name) = trimmed_c_string(source)
                    .windows(5)
                    .enumerate()
                    .filter(|(_, window)| *window == b".name")
                    .find_map(|(index, _)| quoted(&source[index + 5..]))
                {
                    emit(
                        name,
                        EntryPath::join(dir, &[b"lib/", trimmed_c_string(name)])
                            .bytes(),
                    );
                }
            }
            Self::Swift => {
                crate::swift_package_map::scan_targets(source, path, emit)
            }
        }
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
