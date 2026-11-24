//! The editor reads only the TOML forms it needs to identify. Raw source
//! ranges are retained so foreign content can be copied byte for byte.

use std::ops::Range;

use super::{Error, MAX_BYTES, Result};

const BOM: &[u8] = b"\xef\xbb\xbf";

pub(super) fn payload_start(data: &[u8]) -> usize {
    if data.starts_with(BOM) { BOM.len() } else { 0 }
}

pub(super) fn blank(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t')
}

pub(super) fn bare(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}

pub(super) fn trim_start(data: &[u8]) -> &[u8] {
    let first = data
        .iter()
        .position(|&byte| !blank(byte))
        .unwrap_or(data.len());
    &data[first..]
}

fn trim(data: &[u8]) -> &[u8] {
    let data = trim_start(data);
    let end = data
        .iter()
        .rposition(|&byte| !blank(byte))
        .map_or(0, |i| i + 1);
    &data[..end]
}

pub(super) fn text_is_safe(data: &[u8], multiline: bool) -> bool {
    std::str::from_utf8(data).is_ok()
        && data.iter().enumerate().all(|(i, &byte)| match byte {
            b'\r' => multiline && data.get(i + 1) == Some(&b'\n'),
            b'\n' | b'\t' => multiline,
            0..=31 | 127 => false,
            _ => true,
        })
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Line {
    pub start: usize,
    pub content_end: usize,
    pub end: usize,
}

impl Line {
    pub fn content(self, data: &[u8]) -> &[u8] {
        &data[self.start..self.content_end]
    }

    pub fn raw(self, data: &[u8]) -> &[u8] {
        &data[self.start..self.end]
    }

    pub fn edit_start(self, data: &[u8]) -> usize {
        if self.start == 0 {
            payload_start(self.content(data))
        } else {
            self.start
        }
    }

    pub fn trimmed(self, data: &[u8]) -> &[u8] {
        trim_start(self.content(data))
    }

    pub fn significant(self, data: &[u8]) -> bool {
        self.trimmed(data).first().is_some_and(|&byte| byte != b'#')
    }
}

pub(super) fn lines(
    data: &[u8],
    range: Range<usize>,
) -> impl Iterator<Item = Line> + '_ {
    let mut start = range.start;
    data[range]
        .split_inclusive(|&byte| byte == b'\n')
        .map(move |raw| {
            let end = start + raw.len();
            let content = raw.strip_suffix(b"\n").unwrap_or(raw);
            let content = content.strip_suffix(b"\r").unwrap_or(content);
            let line = Line {
                start,
                content_end: start + content.len(),
                end,
            };
            start = end;
            line
        })
}

pub(super) fn newline(data: &[u8]) -> &'static [u8] {
    match data.iter().position(|&byte| byte == b'\n') {
        Some(i) if i > 0 && data[i - 1] == b'\r' => b"\r\n",
        _ => b"\n",
    }
}

fn quote(byte: u8) -> bool {
    matches!(byte, b'"' | b'\'')
}

fn triple(data: &[u8], pos: usize, quote: u8) -> bool {
    data.get(pos..pos + 3) == Some(&[quote; 3])
}

#[derive(Default)]
pub(super) struct Strings(Option<u8>);

impl Strings {
    pub fn in_multiline(&self) -> bool {
        self.0.is_some()
    }

    pub fn finish(&self) -> Result<()> {
        if self.in_multiline() {
            Err(Error)
        } else {
            Ok(())
        }
    }

    pub fn scan(&mut self, data: &[u8], line: Line) -> Result<()> {
        let content = &data[line.edit_start(data)..line.content_end];
        let mut pos = 0;
        while let Some(&byte) = content.get(pos) {
            if let Some(delimiter) = self.0 {
                if delimiter == b'"' && byte == b'\\' {
                    pos += 2;
                } else if triple(content, pos, delimiter) {
                    self.0 = None;
                    pos += 3;
                } else {
                    pos += 1;
                }
            } else if byte == b'#' {
                break;
            } else if quote(byte) {
                if triple(content, pos, byte) {
                    self.0 = Some(byte);
                    pos += 3;
                } else {
                    pos = quoted_end(content, pos).ok_or(Error)?;
                }
            } else {
                pos += 1;
            }
        }
        Ok(())
    }
}

pub(super) fn validate_strings(data: &[u8]) -> Result<()> {
    let mut strings = Strings::default();
    for line in lines(data, 0..data.len()) {
        strings.scan(data, line)?;
    }
    strings.finish()
}

fn quoted_end(data: &[u8], start: usize) -> Option<usize> {
    let delimiter = data[start];
    let mut pos = start + 1;
    while let Some(&byte) = data.get(pos) {
        if byte == delimiter {
            return Some(pos + 1);
        }
        pos += if delimiter == b'"' && byte == b'\\' {
            2
        } else {
            1
        };
    }
    None
}

pub(super) fn parse_string(data: &[u8]) -> Result<(Vec<u8>, usize)> {
    let delimiter = *data.first().filter(|&&byte| quote(byte)).ok_or(Error)?;
    if triple(data, 0, delimiter) {
        return Err(Error);
    }
    let mut decoded = Vec::new();
    let mut pos = 1;
    while let Some(&byte) = data.get(pos) {
        pos += 1;
        if byte == delimiter {
            crate::ffi_output::bytes(&decoded);
            return Ok((decoded, pos));
        }
        if decoded.len() == MAX_BYTES
            || (byte < 32 && byte != b'\t')
            || byte == 127
        {
            return Err(Error);
        }
        if delimiter == b'\'' || byte != b'\\' {
            decoded.push(byte);
            continue;
        }
        let escape = *data.get(pos).ok_or(Error)?;
        pos += 1;
        let byte = match escape {
            b'b' => 8,
            b't' => b'\t',
            b'n' => b'\n',
            b'f' => 12,
            b'r' => b'\r',
            b'"' | b'\\' => escape,
            b'u' | b'U' => {
                let digits = if escape == b'u' { 4 } else { 8 };
                let mut code = 0;
                for &digit in data.get(pos..pos + digits).ok_or(Error)? {
                    code = code * 16
                        + char::from(digit).to_digit(16).ok_or(Error)?;
                }
                let character = char::from_u32(code)
                    .filter(|&ch| ch != '\0')
                    .ok_or(Error)?;
                decoded.extend_from_slice(
                    character.encode_utf8(&mut [0; 4]).as_bytes(),
                );
                pos += digits;
                continue;
            }
            _ => return Err(Error),
        };
        decoded.push(byte);
    }
    Err(Error)
}

pub(super) type Key = Vec<Vec<u8>>;

pub(super) fn single(key: &Key, name: &[u8]) -> bool {
    key.len() == 1 && key[0] == name
}

pub(super) fn overlaps(left: &Key, right: &Key) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

pub(super) fn parse_key(mut data: &[u8]) -> Result<Key> {
    let mut key = Key::new();
    let mut encoded_size = 0;
    while !data.is_empty() {
        data = trim_start(data);
        let first = *data.first().filter(|_| key.len() < 64).ok_or(Error)?;
        let (segment, used) = if quote(first) {
            parse_string(data)?
        } else {
            let used = data.iter().take_while(|&&byte| bare(byte)).count();
            if used == 0 {
                return Err(Error);
            }
            (data[..used].to_vec(), used)
        };
        encoded_size += segment.len() + 1;
        if encoded_size > MAX_BYTES {
            return Err(Error);
        }
        crate::ffi_output::bytes(&segment);
        key.push(segment);
        data = trim_start(&data[used..]);
        if data.is_empty() {
            break;
        }
        data = data.strip_prefix(b".").ok_or(Error)?;
    }
    if key.is_empty() { Err(Error) } else { Ok(key) }
}

pub(super) struct Header {
    pub array: bool,
    pub key: Key,
}

pub(super) fn header(data: &[u8], line: Line) -> Result<Option<Header>> {
    let content = trim_start(&data[line.edit_start(data)..line.content_end]);
    let Some(after_open) = content.strip_prefix(b"[") else {
        return Ok(None);
    };
    let array = after_open.starts_with(b"[");
    let key = &after_open[usize::from(array)..];
    let closing: &[u8] = if array { b"]]" } else { b"]" };
    let mut pos = 0;
    while let Some(&byte) = key.get(pos) {
        if quote(byte) {
            pos = quoted_end(key, pos).ok_or(Error)?;
        } else if let Some(tail) = key[pos..].strip_prefix(closing) {
            let tail = trim_start(tail);
            if !tail.is_empty() && !tail.starts_with(b"#") {
                return Err(Error);
            }
            return Ok(Some(Header {
                array,
                key: parse_key(trim(&key[..pos]))?,
            }));
        } else {
            pos += 1;
        }
    }
    Err(Error)
}

pub(super) struct Assignment<'a> {
    pub key: Key,
    pub value: &'a [u8],
    pub multiline: bool,
    pub trailing_comment: bool,
}

impl Assignment<'_> {
    pub fn string_value(&self) -> Result<Vec<u8>> {
        if self.multiline {
            return Err(Error);
        }
        let (decoded, used) = parse_string(self.value)?;
        if used == self.value.len() {
            Ok(decoded)
        } else {
            Err(Error)
        }
    }
}

pub(super) fn assignment(
    data: &[u8],
    line: Line,
) -> Result<Option<Assignment<'_>>> {
    let content = trim_start(&data[line.edit_start(data)..line.content_end]);
    let mut pos = 0;
    while let Some(&byte) = content.get(pos) {
        match byte {
            b'#' => return Ok(None),
            b'=' => break,
            b'"' | b'\'' => match quoted_end(content, pos) {
                Some(end) => {
                    pos = end;
                    continue;
                }
                None => return Ok(None),
            },
            _ => {}
        }
        pos += 1;
    }
    if pos == content.len() {
        return Ok(None);
    }
    let key = parse_key(trim(&content[..pos]))?;
    let value = trim_start(&content[pos + 1..]);
    pos = 0;
    let mut multiline = false;
    let mut trailing_comment = false;
    while let Some(&byte) = value.get(pos) {
        if byte == b'#' {
            trailing_comment = true;
            break;
        }
        if quote(byte) {
            if triple(value, pos, byte) {
                multiline = true;
                pos = value.len();
                break;
            }
            pos = quoted_end(value, pos).unwrap_or(value.len());
        } else {
            pos += 1;
        }
    }
    let value = trim(&value[..pos]);
    if value.is_empty() {
        return Err(Error);
    }
    Ok(Some(Assignment {
        key,
        value,
        multiline,
        trailing_comment,
    }))
}
