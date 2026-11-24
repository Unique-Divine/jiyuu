//! Byte-oriented port of the managed YAML editor. Only the selected ranges
//! are rendered. This deliberately accepts the same YAML subset as the C editor.

mod file;
mod mapping;
mod sequence;
mod string_list;

use crate::ffi_output;

use std::{
    ffi::{CStr, c_char, c_int},
    ptr, slice,
};

type Result<T> = std::result::Result<T, ()>;
type Edit = Result<Change>;

const INPUT_MAX: usize = 16 * 1024 * 1024;
const OUTPUT_MAX: usize = 32 * 1024 * 1024;
const BLOCK_MAX: usize = 1024 * 1024;
const ITEM_MAX: usize = 64 * 1024;
const KEY_MAX: usize = 255;
const PATH_DEPTH_MAX: usize = 16;
const BOM: &[u8] = b"\xef\xbb\xbf";

enum Change {
    Keep,
    Foreign,
    Replace(Vec<u8>),
}

#[derive(Clone, Copy)]
struct Line {
    start: usize,
    text_end: usize,
    end: usize,
    indent: usize,
    blank: bool,
    comment: bool,
    opens: bool,
    continuation: bool,
}

impl Line {
    fn content(self) -> usize {
        self.start + self.indent
    }

    fn ignored(self) -> bool {
        self.blank || self.comment || self.continuation
    }
}

struct Document<'a> {
    data: &'a [u8],
    lines: Vec<Line>,
    eol: &'static [u8],
}

impl<'a> Document<'a> {
    fn parse(data: &'a [u8]) -> Result<Self> {
        let count = data.split_inclusive(|&b| b == b'\n').count();
        let mut lines = Vec::new();
        lines.try_reserve_exact(count).map_err(|_| ())?;
        let mut start = 0;
        for raw in data.split_inclusive(|&b| b == b'\n') {
            let text = line_text(raw);
            let indent = skip_spaces(text, 0, text.len());
            lines.push(Line {
                start,
                text_end: start + text.len(),
                end: start + raw.len(),
                indent,
                blank: indent == text.len(),
                comment: text.get(indent) == Some(&b'#'),
                opens: false,
                continuation: false,
            });
            start += raw.len();
        }
        let eol: &'static [u8] = if lines
            .first()
            .is_some_and(|line| line.end - line.text_end == 2)
        {
            b"\r\n"
        } else {
            b"\n"
        };
        if data.starts_with(BOM) {
            let first = &mut lines[0];
            first.start = BOM.len();
            first.blank = first.content() >= first.text_end;
        }
        let mut continuation = false;
        for line in &mut lines {
            line.continuation = continuation;
            if !continuation && (line.blank || line.comment) {
                continue;
            }
            let start = if continuation {
                line.start
            } else {
                line.content()
            };
            let mut scan = Scanner {
                pos: start,
                quote: if continuation { b'"' } else { 0 },
            };
            let mut previous = 0;
            while scan.pos < line.text_end {
                let pos = scan.pos;
                let byte = data[pos];
                match scan.next(data, line.text_end, Some(previous)) {
                    Err(()) => {
                        line.opens = true;
                        break;
                    }
                    Ok(None) => continue,
                    Ok(Some(_)) => {}
                }
                if comment_at(data, start, pos) {
                    break;
                }
                if byte != b' ' && byte != b'\t' {
                    previous = byte;
                }
            }
            continuation = line.opens;
        }
        require(!continuation)?;
        let doc = Self { data, lines, eol };
        doc.validate_root()?;
        Ok(doc)
    }

    fn validate_root(&self) -> Result<()> {
        let mut have_key = false;
        let mut open = false;
        for &line in &self.lines {
            if line.ignored() {
                continue;
            }
            if line.indent != 0 {
                require(have_key)?;
                continue;
            }
            let key = &self.data[line.start..line.text_end];
            require(!matches!(key, b"---" | b"..."))?;
            if sequence_dash(key) {
                require(open)?;
                continue;
            }
            require(!matches!(key[0], b'%' | b'?' | b':'))?;
            let colon = mapping_colon(self.data, line.start, line.text_end)?;
            require(decode(&self.data[line.start..colon])? != b"<<")?;
            have_key = true;
            open = tail_empty(self.data, colon, line.text_end);
        }
        Ok(())
    }

    fn match_key(
        &self,
        line: Line,
        indent: usize,
        key: &[u8],
    ) -> Result<Option<usize>> {
        if line.ignored() || line.indent != indent {
            return Ok(None);
        }
        let start = line.content();
        let text = &self.data[start..line.text_end];
        if text.is_empty() || sequence_dash(text) {
            return Ok(None);
        }
        let colon = mapping_colon(self.data, start, line.text_end)?;
        let raw = &self.data[start..trim_spaces(self.data, start, colon)];
        require(!raw.is_empty())?;
        if !matches!(raw[0], b'\'' | b'"') {
            return Ok((raw == key).then_some(colon));
        }
        if decode(raw)? != key {
            return Ok(None);
        }
        require(
            raw.len() == key.len() + 2
                && raw[raw.len() - 1] == raw[0]
                && &raw[1..raw.len() - 1] == key,
        )?;
        Ok(Some(colon))
    }

    fn find_key(
        &self,
        begin: usize,
        end: usize,
        indent: usize,
        key: &[u8],
    ) -> Result<Option<(usize, usize)>> {
        let mut found = None;
        for i in begin..end {
            if let Some(colon) = self.match_key(self.lines[i], indent, key)? {
                require(found.is_none())?;
                found = Some((i, colon));
            }
        }
        Ok(found)
    }

    fn offset(&self, index: usize) -> usize {
        self.lines
            .get(index)
            .map_or(self.data.len(), |line| line.start)
    }

    fn section_end(&self, header: usize, limit: usize) -> usize {
        let indent = self.lines[header].indent;
        (header + 1..limit)
            .find(|&i| {
                let line = self.lines[i];
                !line.ignored() && line.indent <= indent
            })
            .unwrap_or(limit)
    }

    fn splice(&self, start: usize, end: usize, replacement: &[u8]) -> Edit {
        let mut out = Vec::new();
        append(&mut out, &self.data[..start])?;
        append(&mut out, replacement)?;
        append(&mut out, &self.data[end..])?;
        Ok(Change::Replace(out))
    }

    fn prefix(&self, offset: usize) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        append(&mut out, &self.data[..offset])?;
        self.separator(&mut out)?;
        Ok(out)
    }

    fn separator(&self, out: &mut Vec<u8>) -> Result<()> {
        if !out.is_empty() && !out.ends_with(b"\n") {
            append(out, self.eol)?;
        }
        Ok(())
    }

    fn rewrite_empty_mapping_header(
        &self,
        line: Line,
        colon: usize,
        empty: bool,
    ) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        append(&mut out, &self.data[..=colon])?;
        if empty {
            append(&mut out, b" {}")?;
        }
        let comment = find_comment(self.data, colon + 1, line.text_end)?;
        if comment < line.text_end {
            append(&mut out, b" ")?;
        }
        append(&mut out, &self.data[comment..line.end])?;
        Ok(out)
    }
}

fn require(condition: bool) -> Result<()> {
    condition.then_some(()).ok_or(())
}

fn append(out: &mut Vec<u8>, bytes: &[u8]) -> Result<()> {
    require(bytes.len() <= OUTPUT_MAX - out.len())?;
    out.try_reserve(bytes.len()).map_err(|_| ())?;
    out.extend_from_slice(bytes);
    Ok(())
}

fn line_text(raw: &[u8]) -> &[u8] {
    let text = raw.strip_suffix(b"\n").unwrap_or(raw);
    text.strip_suffix(b"\r").unwrap_or(text)
}

fn validate_bytes(data: &[u8], allow_bom: bool) -> Result<()> {
    require(allow_bom || !data.starts_with(BOM))?;
    require(std::str::from_utf8(data).is_ok())?;
    for (i, &byte) in data.iter().enumerate() {
        require(match byte {
            b'\r' => data.get(i + 1) == Some(&b'\n'),
            b'\n' => true,
            _ => byte >= 0x20 && byte != 0x7f,
        })?;
    }
    Ok(())
}

fn validate_key(key: &[u8]) -> Result<()> {
    require(
        !key.is_empty()
            && key.len() <= KEY_MAX
            && key.iter().all(|&b| {
                matches!(b, b'_' | b'-' | b'.')
                    || unsafe { libc::isalnum(c_int::from(b)) } != 0
            }),
    )
}

fn validate_item(item: &[u8]) -> Result<()> {
    require(
        !item.is_empty()
            && item.len() <= ITEM_MAX
            && item.iter().all(|&b| b >= 0x20 && b != 0x7f)
            && std::str::from_utf8(item).is_ok(),
    )
}

fn skip_spaces(data: &[u8], mut start: usize, end: usize) -> usize {
    while start < end && data[start] == b' ' {
        start += 1;
    }
    start
}

fn trim_spaces(data: &[u8], start: usize, mut end: usize) -> usize {
    while end > start && data[end - 1] == b' ' {
        end -= 1;
    }
    end
}

fn sequence_dash(text: &[u8]) -> bool {
    text.first() == Some(&b'-') && matches!(text.get(1), None | Some(b' '))
}

fn comment_at(data: &[u8], start: usize, pos: usize) -> bool {
    data[pos] == b'#' && (pos == start || data[pos - 1] == b' ')
}

fn node_start(previous: u8) -> bool {
    matches!(previous, 0 | b':' | b'-')
}

struct Scanner {
    pos: usize,
    quote: u8,
}

impl Scanner {
    fn next(
        &mut self,
        data: &[u8],
        end: usize,
        previous: Option<u8>,
    ) -> Result<Option<u8>> {
        let byte = data[self.pos];
        self.pos += 1;
        match self.quote {
            b'\'' => {
                if byte == b'\'' {
                    if self.pos < end && data[self.pos] == b'\'' {
                        self.pos += 1;
                    } else {
                        self.quote = 0;
                    }
                }
            }
            b'"' => match byte {
                b'\\' => {
                    require(self.pos < end)?;
                    self.pos += 1;
                }
                b'"' => self.quote = 0,
                _ => {}
            },
            _ if matches!(byte, b'\'' | b'"')
                && previous.is_none_or(node_start) =>
            {
                self.quote = byte
            }
            _ => return Ok(Some(byte)),
        }
        Ok(None)
    }
}

fn find_comment(data: &[u8], start: usize, end: usize) -> Result<usize> {
    let mut scan = Scanner {
        pos: start,
        quote: 0,
    };
    while scan.pos < end {
        let pos = scan.pos;
        if scan.next(data, end, None)?.is_some() && comment_at(data, start, pos)
        {
            return Ok(pos);
        }
    }
    require(scan.quote == 0)?;
    Ok(end)
}

fn mapping_colon(data: &[u8], start: usize, end: usize) -> Result<usize> {
    let mut scan = Scanner {
        pos: start,
        quote: 0,
    };
    while scan.pos < end {
        let pos = scan.pos;
        if scan.next(data, end, None)? == Some(b':')
            && (pos + 1 == end || matches!(data[pos + 1], b' ' | b'#'))
        {
            require(pos != start)?;
            return Ok(pos);
        }
    }
    Err(())
}

fn check_indicators(
    data: &[u8],
    start: usize,
    mut end: usize,
    opens: bool,
    sequence: bool,
) -> Result<()> {
    if opens && end > start && data[end - 1] == b'\\' {
        end -= 1;
    }
    let mut scan = Scanner {
        pos: start,
        quote: 0,
    };
    let mut previous = 0;
    while scan.pos < end {
        let pos = scan.pos;
        let Some(byte) = scan.next(data, end, Some(previous))? else {
            previous = b'q';
            continue;
        };
        if comment_at(data, start, pos) {
            break;
        }
        require(
            !matches!(byte, b'{' | b'}') && !data[pos..end].starts_with(b"<<:"),
        )?;
        if node_start(previous) {
            require(
                !matches!(byte, b'&' | b'*')
                    && !(sequence && matches!(byte, b'[' | b']' | b'|' | b'>')),
            )?;
        }
        if !matches!(byte, b' ' | b'\t') {
            previous = byte;
        }
    }
    require(scan.quote == 0 || (scan.quote == b'"' && opens))
}

fn check_body_line(data: &[u8], line: Line, sequence: bool) -> Result<()> {
    let start = line.content();
    if let Ok(colon) = mapping_colon(data, start, line.text_end)
        && matches!(
            inline_value(data, colon, line.text_end),
            Ok((b"{}" | b"[]", _, _))
        )
    {
        return check_indicators(data, start, colon + 1, false, false);
    }
    check_indicators(data, start, line.text_end, line.opens, sequence)
}

fn inline_value(
    data: &[u8],
    colon: usize,
    end: usize,
) -> Result<(&[u8], usize, usize)> {
    let comment = find_comment(data, colon + 1, end)?;
    let start = skip_spaces(data, colon + 1, comment);
    let end = trim_spaces(data, start, comment);
    Ok((&data[start..end], start, end))
}

fn tail_empty(data: &[u8], colon: usize, end: usize) -> bool {
    let pos = skip_spaces(data, colon + 1, end);
    pos == end || data[pos] == b'#'
}

fn multiline(data: &[u8], colon: usize, end: usize) -> bool {
    let pos = skip_spaces(data, colon + 1, end);
    pos < end && matches!(data[pos], b'|' | b'>')
}

fn decode(raw: &[u8]) -> Result<Vec<u8>> {
    let start = skip_spaces(raw, 0, raw.len());
    let text = &raw[start..trim_spaces(raw, start, raw.len())];
    require(!text.is_empty() && text.len() <= ITEM_MAX)?;
    let mut value = Vec::new();
    value.try_reserve_exact(text.len()).map_err(|_| ())?;
    match text[0] {
        quote @ (b'\'' | b'"') => {
            require(text.len() >= 2 && text.last() == Some(&quote))?;
            let body = &text[1..text.len() - 1];
            let mut pos = 0;
            while pos < body.len() {
                let mut byte = body[pos];
                pos += 1;
                if quote == b'\'' {
                    if byte == quote {
                        require(body.get(pos) == Some(&quote))?;
                        pos += 1;
                    }
                } else {
                    require(byte != quote)?;
                    if byte == b'\\' {
                        byte = match body.get(pos).ok_or(())? {
                            b' ' => b' ',
                            b'/' => b'/',
                            b'"' => b'"',
                            b'\\' => b'\\',
                            b'x' => {
                                let high = (*body.get(pos + 1).ok_or(())?
                                    as char)
                                    .to_digit(16)
                                    .ok_or(())?;
                                let low = (*body.get(pos + 2).ok_or(())?
                                    as char)
                                    .to_digit(16)
                                    .ok_or(())?;
                                pos += 2;
                                ((high << 4) | low) as u8
                            }
                            // Escapes for control characters are outside the accepted subset.
                            _ => return Err(()),
                        };
                        pos += 1;
                    }
                    require(byte >= 0x20 && byte != 0x7f)?;
                }
                value.push(byte);
            }
        }
        _ => {
            require(!b"-?:,[]{}#&*!|>@`".contains(&text[0]))?;
            require(!text.windows(2).any(|w| matches!(w, b": " | b" #")))?;
            value.extend_from_slice(text);
        }
    }
    require(!value.is_empty() && std::str::from_utf8(&value).is_ok())?;
    Ok(value)
}

fn quote(out: &mut Vec<u8>, value: &[u8]) -> Result<()> {
    append(out, b"\"")?;
    for &byte in value {
        if matches!(byte, b'"' | b'\\') {
            append(out, b"\\")?;
        }
        append(out, &[byte])?;
    }
    append(out, b"\"")
}

// Each pointer is borrowed only for the synchronous native call.
unsafe fn argument<'a>(ptr: *const c_char, limit: usize) -> Result<&'a [u8]> {
    require(!ptr.is_null())?;
    for len in 0..=limit {
        if unsafe { *ptr.add(len) } == 0 {
            return Ok(unsafe { slice::from_raw_parts(ptr.cast(), len) });
        }
    }
    Err(())
}

fn edit(
    path: *const c_char,
    remove: bool,
    operation: impl FnOnce(&CStr) -> Result<c_int>,
) -> c_int {
    fn run(
        path: &CStr,
        remove: bool,
        operation: impl FnOnce(&CStr) -> Result<c_int>,
    ) -> Result<c_int> {
        if remove && file::absent(path) {
            return Ok(0);
        }
        require(!path.to_bytes().is_empty())?;
        let lock = file::Lock::acquire(path)?;
        let result = operation(path);
        lock.release()?;
        result
    }
    unsafe { argument(path, OUTPUT_MAX) }
        .and_then(|_| run(unsafe { CStr::from_ptr(path) }, remove, operation))
        .unwrap_or(-1)
}

fn apply(path: &CStr, transform: impl FnOnce(&[u8]) -> Edit) -> Result<c_int> {
    let before = file::read(path)?;
    match transform(&before.data)? {
        Change::Keep => Ok(0),
        Change::Foreign => Ok(1),
        Change::Replace(data) => {
            if data != before.data {
                file::publish(path, &before, &data)?;
            }
            Ok(0)
        }
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_yaml_encode_double_quoted_scalar(
    value: *const c_char,
    out: *mut *mut c_char,
) -> c_int {
    if out.is_null() {
        return -1;
    }
    unsafe { ffi_output::write(out, ptr::null_mut()) };
    let result: Result<c_int> = (|| {
        let value = unsafe { argument(value, ITEM_MAX) }?;
        validate_item(value)?;
        let mut encoded = Vec::new();
        quote(&mut encoded, value)?;
        // The C header promises storage released by the caller's free().
        let allocation =
            unsafe { libc::malloc(encoded.len() + 1) }.cast::<c_char>();
        require(!allocation.is_null())?;
        unsafe {
            ptr::copy_nonoverlapping(
                encoded.as_ptr().cast(),
                allocation,
                encoded.len(),
            );
            *allocation.add(encoded.len()) = 0;
            ffi_output::written(allocation, encoded.len() + 1);
            ffi_output::write(out, allocation);
        }
        Ok(0)
    })();
    result.unwrap_or(-1)
}
