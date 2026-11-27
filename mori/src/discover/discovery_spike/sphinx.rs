use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

const CONFIG_BYTES: u64 = 65_536;
const SUFFIX_COUNT: usize = 4;
const SUFFIX_BUFFER_BYTES: usize = 16;

#[derive(Default)]
pub(super) struct Suffixes(Vec<Vec<u8>>);

impl Suffixes {
    pub(super) fn read(directory: &Path) -> Option<Self> {
        let path = directory.join("conf.py");
        if !fs::metadata(&path).ok()?.is_file() {
            return None;
        }
        let mut bytes = Vec::new();
        let mut reader = File::open(path).ok()?.take(CONFIG_BYTES);
        // C inspects the bytes read even if a later read fails.
        let _ = reader.read_to_end(&mut bytes);
        Some(Self::parse(&bytes))
    }

    fn parse(mut bytes: &[u8]) -> Self {
        loop {
            if let Some(rhs) = bytes
                .strip_prefix(b"source_suffix")
                .map(skip_horizontal_space)
                .and_then(|tail| tail.strip_prefix(b"="))
            {
                return Self::parse_value(skip_horizontal_space(rhs));
            }
            let Some(end) = bytes.iter().position(|&byte| byte == b'\n') else {
                return Self::default();
            };
            bytes = &bytes[end + 1..];
        }
    }

    fn parse_value(value: &[u8]) -> Self {
        let close = match value.first() {
            Some(b'[') => b']',
            Some(b'(') => b')',
            Some(b'{') => b'}',
            _ => b'\n',
        };
        let end =
            value
                .iter()
                .position(|&byte| byte == close)
                .unwrap_or_else(|| {
                    value
                        .iter()
                        .position(|&byte| byte == b'\n')
                        .unwrap_or(value.len())
                });
        let dictionary = value.first() == Some(&b'{');
        let mut body = &value[..end];
        let mut suffixes = Self::default();
        while !body.is_empty() && suffixes.0.len() < SUFFIX_COUNT {
            let Some((suffix, rest)) = quoted(body) else {
                body = &body[1..];
                continue;
            };
            body = rest;
            if dictionary {
                body = skip_bytes(body, |byte| matches!(byte, b' ' | b':'));
                let Some((kind, rest)) = quoted(body) else {
                    continue;
                };
                body = rest;
                if kind != b"restructuredtext" {
                    continue;
                }
            }
            if suffix.len() >= 2
                && suffix.len() < SUFFIX_BUFFER_BYTES
                && suffix != b".rst"
                && suffix.strip_prefix(b".").is_some_and(|tail| {
                    tail.iter().all(|&byte| {
                        byte.is_ascii_alphanumeric()
                            || matches!(byte, b'_' | b'-')
                    })
                })
            {
                suffixes.0.push(suffix.to_vec());
            }
        }
        suffixes
    }

    pub(super) fn matches(&self, name: &[u8]) -> bool {
        self.0
            .iter()
            .any(|suffix| name.len() > suffix.len() && name.ends_with(suffix))
    }
}

fn skip_horizontal_space(bytes: &[u8]) -> &[u8] {
    skip_bytes(bytes, |byte| matches!(byte, b' ' | b'\t'))
}

fn skip_bytes(bytes: &[u8], skip: impl Fn(u8) -> bool) -> &[u8] {
    let start = bytes
        .iter()
        .position(|&byte| !skip(byte))
        .unwrap_or(bytes.len());
    &bytes[start..]
}

fn quoted(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let (&delimiter, body) = bytes.split_first()?;
    if !matches!(delimiter, b'\'' | b'"') {
        return None;
    }
    let end = body.iter().position(|&byte| byte == delimiter)?;
    Some((&body[..end], &body[end + 1..]))
}
