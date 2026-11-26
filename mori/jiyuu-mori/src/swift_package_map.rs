//! Literal SwiftPM target discovery for `pass_pkgmap.c`.

use std::ffi::{CStr, c_char, c_void};
use std::slice;

const PATH_BUFFER: usize = 1024;
const TARGETS: [(&[u8], &[u8]); 5] = [
    (b".target(", b"Sources"),
    (b".executableTarget(", b"Sources"),
    (b".testTarget(", b"Tests"),
    (b".macro(", b"Sources"),
    (b".plugin(", b"Plugins"),
];

/// Return byte offsets outside comments and ordinary quoted strings.
fn code_offsets(source: &[u8]) -> impl Iterator<Item = usize> + '_ {
    let mut cursor = 0;
    std::iter::from_fn(move || {
        while cursor < source.len() {
            let start = cursor;
            let rest = &source[cursor..];
            if rest.starts_with(b"//") {
                cursor += rest
                    .iter()
                    .position(|&byte| byte == b'\n')
                    .unwrap_or(rest.len());
            } else if rest.starts_with(b"/*") {
                cursor += 2;
                let mut depth = 1;
                while depth > 0 && cursor < source.len() {
                    match &source[cursor..] {
                        [b'/', b'*', ..] => {
                            depth += 1;
                            cursor += 2;
                        }
                        [b'*', b'/', ..] => {
                            depth -= 1;
                            cursor += 2;
                        }
                        _ => cursor += 1,
                    }
                }
            } else if rest[0] == b'"' {
                cursor += 1;
                while cursor < source.len() {
                    match &source[cursor..] {
                        [b'\\', _, ..] => cursor += 2,
                        [b'"', ..] => {
                            cursor += 1;
                            break;
                        }
                        _ => cursor += 1,
                    }
                }
            } else {
                cursor += 1;
                return Some(start);
            }
        }
        None
    })
}

fn closing_paren(source: &[u8]) -> Option<usize> {
    let mut depth = 0;
    code_offsets(source).find(|&offset| match source[offset] {
        b'(' => {
            depth += 1;
            false
        }
        b')' => {
            depth -= 1;
            depth == 0
        }
        _ => false,
    })
}

fn argument<'a>(source: &'a [u8], label: &[u8]) -> Option<&'a [u8]> {
    let offset = code_offsets(source)
        .find(|&offset| source[offset..].starts_with(label))?;
    Some(&source[offset + label.len()..])
}

fn skip_horizontal_space(source: &[u8]) -> &[u8] {
    let mut rest = source;
    while let [b' ' | b'\t', tail @ ..] = rest {
        rest = tail;
    }
    rest
}

fn quoted_literal(source: &[u8]) -> Option<&[u8]> {
    let value = skip_horizontal_space(source).strip_prefix(b"\"")?;
    let mut cursor = 0;
    while cursor < value.len() {
        match &value[cursor..] {
            [b'\\', _, ..] => cursor += 2,
            [b'\n', ..] => return None,
            [b'"', tail @ ..] => {
                return match skip_horizontal_space(tail) {
                    [] | [b',', ..] => {
                        value[..cursor].split(|&byte| byte == 0).next()
                    }
                    _ => None,
                };
            }
            _ => cursor += 1,
        }
    }
    None
}

fn normalize_target_path(path: &[u8]) -> Option<Vec<u8>> {
    if path.starts_with(b"/")
        || path.iter().any(|&byte| matches!(byte, b'\\' | b':'))
    {
        return None;
    }
    let mut normalized = Vec::new();
    for component in path.split(|&byte| byte == b'/') {
        match component {
            b"" | b"." => {}
            b".." => {
                if normalized.is_empty() {
                    return None;
                }
                let parent = normalized
                    .iter()
                    .rposition(|&byte| byte == b'/')
                    .unwrap_or(0);
                normalized.truncate(parent);
            }
            _ => {
                // The reference reserves room for a separator and a NUL on every append.
                if normalized.len() + component.len() + 2 > PATH_BUFFER {
                    return None;
                }
                if !normalized.is_empty() {
                    normalized.push(b'/');
                }
                normalized.extend_from_slice(component);
            }
        }
    }
    Some(normalized)
}

fn target_path(call: &[u8], root: &[u8], name: &[u8]) -> Option<Vec<u8>> {
    let default_path;
    let path = match argument(call, b"path:") {
        Some(value) => quoted_literal(value)?,
        None => {
            if root.len() + 1 + name.len() >= PATH_BUFFER {
                return None;
            }
            default_path = [root, b"/", name].concat();
            &default_path
        }
    };
    normalize_target_path(path)
}

fn scan_targets(
    mut source: &[u8],
    manifest: &[u8],
    mut emit: impl FnMut(&[u8], &[u8]),
) {
    let manifest_dir = manifest
        .iter()
        .rposition(|&byte| byte == b'/')
        .map_or(&[][..], |slash| &manifest[..slash]);
    while let Some((open, root)) = code_offsets(source).find_map(|offset| {
        TARGETS.iter().find_map(|&(prefix, root)| {
            source[offset..]
                .starts_with(prefix)
                .then_some((offset + prefix.len() - 1, root))
        })
    }) {
        let Some(close) = closing_paren(&source[open..]) else {
            break;
        };
        let close = open + close;
        let call = &source[open..close];
        if let Some(name) = argument(call, b"name:").and_then(quoted_literal)
            && !name.is_empty()
            && let Some(path) = target_path(call, root, name)
        {
            if path.is_empty() {
                emit(name, manifest_dir);
            } else {
                let mut entry = manifest_dir.to_vec();
                if !entry.is_empty() {
                    entry.push(b'/');
                }
                entry.extend_from_slice(&path);
                // Match the reference's fixed buffer for non-empty suffixes.
                entry.truncate(PATH_BUFFER - 1);
                emit(name, &entry);
            }
        }
        // A nested dependency target belongs to the enclosing call.
        source = &source[close + 1..];
    }
}

type EmitTarget =
    unsafe extern "C" fn(*mut c_void, *const u8, usize, *const u8, usize);

/// The caller provides readable source bytes and a NUL-terminated manifest path.
/// The sink copies both byte spans before returning.
#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_swift_scan_targets(
    source: *const u8,
    source_len: usize,
    manifest: *const c_char,
    context: *mut c_void,
    emit: EmitTarget,
) {
    // SAFETY: The C adapter owns both inputs for the duration of this call.
    let source = unsafe { slice::from_raw_parts(source, source_len) };
    let manifest = unsafe { CStr::from_ptr(manifest) }.to_bytes();
    scan_targets(source, manifest, |name, entry| {
        // SAFETY: Both spans are readable until the synchronous sink returns.
        unsafe {
            emit(
                context,
                name.as_ptr(),
                name.len(),
                entry.as_ptr(),
                entry.len(),
            )
        };
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_only_literal_targets_and_keeps_declaration_order() {
        let source = br#"
            // .target(name: "Line")
            /* /* .target(name: "NestedComment") */ .target(name: "Comment") */
            let text = ".target(name: \"String\")"
            let package = Package(
                products: [.library(name: "Alias", targets: ["Core"])],
                dependencies: [.package(path: "../Dep")],
                targets: [
                    .target(name: "Core", dependencies: [.target(name: "Ref")],
                            path: "./Old/../Lib.v2"),
                    .testTarget(name: "CoreTests"),
                    .executableTarget(name: "Tool"),
                    .macro(name: "Macros"), .plugin(name: "Plugin"),
                    .target(name: "Root", path: "."),
                    .target(name: "Computed", path: "Sources/" + suffix),
                    .target(name: dynamic),
                    .target(name: "Absolute", path: "/tmp"),
                    .target(name: "Escape", path: "../Other"),
                    .target(name: "Drive", path: "C:/Other"),
                ])
            .target(
        "#;
        let mut targets = Vec::new();
        scan_targets(source, b"Nested/Package.swift", |name, entry| {
            targets.push((name.to_vec(), entry.to_vec()));
        });
        let expected: &[(&[u8], &[u8])] = &[
            (b"Core", b"Nested/Lib.v2"),
            (b"CoreTests", b"Nested/Tests/CoreTests"),
            (b"Tool", b"Nested/Sources/Tool"),
            (b"Macros", b"Nested/Sources/Macros"),
            (b"Plugin", b"Nested/Plugins/Plugin"),
            (b"Root", b"Nested"),
        ];
        assert_eq!(
            targets
                .iter()
                .map(|(name, entry)| (name.as_slice(), entry.as_slice()))
                .collect::<Vec<_>>(),
            expected
        );
    }

    #[test]
    fn preserves_byte_paths_and_buffer_limits() {
        let mut targets = Vec::new();
        scan_targets(
            b".target(name: \"\xff\", path: \"\")",
            b"Package.swift",
            |name, entry| {
                targets.push((name.to_vec(), entry.to_vec()));
            },
        );
        assert_eq!(targets, [(vec![0xff], vec![])]);

        let dir = vec![b'd'; PATH_BUFFER - 3];
        let manifest = [dir.as_slice(), b"/Package.swift"].concat();
        scan_targets(b".target(name: \"App\")", &manifest, |_, entry| {
            assert_eq!(entry, [dir.as_slice(), b"/S"].concat());
        });
        assert!(normalize_target_path(&[b'a'; PATH_BUFFER - 1]).is_none());
    }
}
