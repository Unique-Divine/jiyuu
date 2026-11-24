use std::ops::Range;

use super::{Invalid, MAX_BYTES, MAX_DEPTH, Result};

pub(super) fn is_quote(byte: u8) -> bool {
    matches!(byte, b'"' | b'\'')
}

fn unicode_line(rest: &[u8]) -> bool {
    matches!(rest, [0xe2, 0x80, 0xa8 | 0xa9, ..])
}

fn hex(bytes: &[u8], width: usize) -> Option<u32> {
    bytes.get(..width)?.iter().try_fold(0, |value, &byte| {
        Some(value * 16 + char::from(byte).to_digit(16)?)
    })
}

fn identifier_byte(byte: u8, first: bool) -> bool {
    byte.is_ascii_alphabetic()
        || matches!(byte, b'_' | b'$' | 0x80..=0xff)
        || (!first && byte.is_ascii_digit())
}

pub(super) struct Parser<'a> {
    text: &'a [u8],
    pos: usize,
    strict: bool,
}

impl<'a> Parser<'a> {
    fn rest(&self) -> &'a [u8] {
        &self.text[self.pos..]
    }

    fn peek(&self) -> Option<u8> {
        self.rest().first().copied()
    }

    fn take(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.pos += 1;
            return true;
        }
        false
    }

    fn expect(&mut self, byte: u8) -> Result<()> {
        self.take(byte).then_some(()).ok_or(Invalid)
    }

    fn trivia(&mut self) -> Result<()> {
        loop {
            self.pos += match self.rest() {
                [b' ' | b'\t' | b'\r' | b'\n', ..] => 1,
                _ if self.strict => return Ok(()),
                [0x0b | 0x0c, ..] => 1,
                [0xc2, 0xa0, ..] => 2,
                [0xef, 0xbb, 0xbf, ..] => 3,
                rest if unicode_line(rest) => 3,
                [b'/', b'/', ..] => {
                    self.pos += 2;
                    while !self.rest().is_empty()
                        && !matches!(self.peek(), Some(b'\r' | b'\n'))
                        && !unicode_line(self.rest())
                    {
                        self.pos += 1;
                    }
                    continue;
                }
                [b'/', b'*', rest @ ..] => {
                    rest.windows(2)
                        .position(|pair| pair == b"*/")
                        .ok_or(Invalid)?
                        + 4
                }
                _ => return Ok(()),
            };
        }
    }

    fn string(&mut self) -> Result<()> {
        let quote = self.peek().ok_or(Invalid)?;
        self.pos += 1;
        while let Some(byte) = self.peek() {
            if byte == quote {
                self.pos += 1;
                return Ok(());
            }
            if byte < 0x20 || (!self.strict && unicode_line(self.rest())) {
                return Err(Invalid);
            }
            self.pos += 1;
            if byte != b'\\' {
                continue;
            }
            if !self.strict && unicode_line(self.rest()) {
                self.pos += 3;
                continue;
            }
            let escaped = self.peek().ok_or(Invalid)?;
            self.pos += 1;
            match escaped {
                b'u' => {
                    hex(self.rest(), 4).ok_or(Invalid)?;
                    self.pos += 4;
                }
                _ if self.strict => {
                    if !b"\"\\/bfnrt".contains(&escaped) {
                        return Err(Invalid);
                    }
                }
                b'x' => {
                    hex(self.rest(), 2).ok_or(Invalid)?;
                    self.pos += 2;
                }
                b'\r' => {
                    self.take(b'\n');
                }
                b'\n' => {}
                b'0' if self
                    .peek()
                    .is_some_and(|next| next.is_ascii_digit()) =>
                {
                    return Err(Invalid);
                }
                b'1'..=b'9' | 0..=0x1f => return Err(Invalid),
                _ => {}
            }
        }
        Err(Invalid)
    }

    fn key(&mut self) -> Result<Range<usize>> {
        let start = self.pos;
        if self.peek() == Some(b'"')
            || (!self.strict && self.peek() == Some(b'\''))
        {
            self.string()?;
        } else if !self.strict {
            while let Some(byte) = self.peek() {
                let first = self.pos == start;
                if identifier_byte(byte, first) {
                    self.pos += 1;
                } else if let Some(code) = self
                    .rest()
                    .strip_prefix(b"\\u")
                    .and_then(|rest| hex(rest, 4))
                    && char::from_u32(code).is_some()
                    && (code >= 0x80 || identifier_byte(code as u8, first))
                {
                    self.pos += 6;
                } else {
                    break;
                }
            }
        }
        (self.pos != start)
            .then_some(start..self.pos)
            .ok_or(Invalid)
    }

    fn digits(&mut self, radix: u32) -> bool {
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|byte| char::from(byte).is_digit(radix))
        {
            self.pos += 1;
        }
        self.pos != start
    }

    fn word(&mut self, word: &[u8]) -> bool {
        if self.rest().starts_with(word) {
            self.pos += word.len();
            return true;
        }
        false
    }

    fn number(&mut self) -> Result<()> {
        if !self.take(b'-') && !self.strict {
            self.take(b'+');
        }
        if !self.strict {
            if self.word(b"Infinity") || self.word(b"NaN") {
                return Ok(());
            }
            if self.word(b"0x") || self.word(b"0X") {
                return self.digits(16).then_some(()).ok_or(Invalid);
            }
        }
        let integer = self.take(b'0') || self.digits(10);
        if self.strict && !integer {
            return Err(Invalid);
        }
        let fraction = if self.take(b'.') {
            let digits = self.digits(10);
            if self.strict && !digits {
                return Err(Invalid);
            }
            digits
        } else {
            false
        };
        if !integer && !fraction {
            return Err(Invalid);
        }
        if self.take(b'e') || self.take(b'E') {
            if !self.take(b'+') {
                self.take(b'-');
            }
            if !self.digits(10) {
                return Err(Invalid);
            }
        }
        Ok(())
    }

    fn value(&mut self, depth: usize) -> Result<Range<usize>> {
        let start = self.pos;
        match self.peek() {
            Some(b'{' | b'[') if depth < MAX_DEPTH => {
                let container = self.peek().ok_or(Invalid)?;
                self.container(container, depth, |_| {})?;
            }
            Some(b'"') => self.string()?,
            Some(b'\'') if !self.strict => self.string()?,
            _ if self.word(b"true")
                || self.word(b"false")
                || self.word(b"null") => {}
            _ => self.number()?,
        }
        Ok(start..self.pos)
    }

    fn container(
        &mut self,
        open: u8,
        depth: usize,
        mut visit: impl FnMut(Member),
    ) -> Result<usize> {
        self.expect(open)?;
        let close = if open == b'{' { b'}' } else { b']' };
        self.trivia()?;
        if self.take(close) {
            return Ok(self.pos - 1);
        }
        let mut previous_comma = None;
        loop {
            let key = if open == b'{' {
                let key = self.key()?;
                self.trivia()?;
                self.expect(b':')?;
                self.trivia()?;
                Some(key)
            } else {
                None
            };
            let value = self.value(depth + 1)?;
            self.trivia()?;
            let end = self.pos;
            let comma = self.take(b',').then_some(end);
            visit(Member {
                key,
                value,
                comma,
                previous_comma,
            });
            if comma.is_none() {
                self.expect(close)?;
                return Ok(end);
            }
            self.trivia()?;
            if !self.strict && self.take(close) {
                return Ok(self.pos - 1);
            }
            previous_comma = comma;
        }
    }
}

pub(super) fn validate_document(text: &[u8]) -> Result<usize> {
    Ok(validate(text, false)?.start)
}

pub(super) fn validate_entry(text: &[u8]) -> Result<&[u8]> {
    if text.len() > MAX_BYTES {
        return Err(Invalid);
    }
    Ok(&text[validate(text, true)?])
}

fn validate(text: &[u8], strict: bool) -> Result<Range<usize>> {
    std::str::from_utf8(text).map_err(|_| Invalid)?;
    let mut parser = Parser {
        text,
        pos: 0,
        strict,
    };
    parser.trivia()?;
    if !strict && parser.peek() != Some(b'{') {
        return Err(Invalid);
    }
    let value = parser.value(0)?;
    parser.trivia()?;
    (parser.pos == text.len()).then_some(value).ok_or(Invalid)
}

pub(super) struct Member {
    key: Option<Range<usize>>,
    pub value: Range<usize>,
    pub comma: Option<usize>,
    pub previous_comma: Option<usize>,
}

impl Member {
    pub fn start(&self) -> usize {
        self.key.as_ref().map_or(self.value.start, |key| key.start)
    }
}

#[derive(Default)]
pub(super) struct Scan {
    pub close: usize,
    pub first: Option<usize>,
    pub last_end: usize,
    pub last_comma: Option<usize>,
    pub count: usize,
    pub matches: usize,
    pub hit: Option<Member>,
}

pub(super) fn scan(
    text: &[u8],
    start: usize,
    open: u8,
    lookup: Option<&[u8]>,
) -> Result<Scan> {
    let mut parser = Parser {
        text,
        pos: start,
        strict: false,
    };
    let mut found = Scan::default();
    found.close = parser.container(open, 0, |member| {
        found.first.get_or_insert(member.start());
        found.last_end = member.value.end;
        found.last_comma = member.comma;
        found.count += 1;
        let token =
            &text[member.key.clone().unwrap_or_else(|| member.value.clone())];
        if let Some(lookup) = lookup
            && (open == b'{' || is_quote(token[0]))
            && string_equals(token, lookup)
        {
            found.matches += 1;
            if found.matches == 1 {
                found.hit = Some(member);
            }
        }
    })?;
    Ok(found)
}

// The reference matcher decodes ASCII line continuations only and permits
// escaped surrogate bytes when comparing a token against a raw C string.
fn string_equals(token: &[u8], mut target: &[u8]) -> bool {
    if !token.contains(&b'\\') {
        return unquote(token) == target;
    }
    decode(token, false, |bytes| {
        target = target.strip_prefix(bytes).ok_or(Invalid)?;
        Ok(())
    })
    .is_ok()
        && target.is_empty()
}

fn unquote(token: &[u8]) -> &[u8] {
    if is_quote(token[0]) {
        &token[1..token.len() - 1]
    } else {
        token
    }
}

pub(super) fn decoded_string(token: &[u8]) -> Result<Vec<u8>> {
    if !is_quote(token[0]) {
        return Err(Invalid);
    }
    let mut value = Vec::new();
    decode(token, true, |bytes| {
        value.extend_from_slice(bytes);
        Ok(())
    })?;
    Ok(value)
}

fn decode(
    token: &[u8],
    value: bool,
    mut emit: impl FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    let mut rest = unquote(token);
    while let Some(pos) = rest.iter().position(|&byte| byte == b'\\') {
        emit(&rest[..pos])?;
        let (&escaped, next) = rest[pos + 1..].split_first().ok_or(Invalid)?;
        rest = next;
        let code = match escaped {
            b'\r' => {
                rest = rest.strip_prefix(b"\n").unwrap_or(rest);
                continue;
            }
            b'\n' => continue,
            0xe2 if value && matches!(rest, [0x80, 0xa8 | 0xa9, ..]) => {
                rest = &rest[2..];
                continue;
            }
            b'b' => 8,
            b'f' => 12,
            b'n' => 10,
            b'r' => 13,
            b't' => 9,
            b'v' => 11,
            b'0' => 0,
            b'x' | b'u' => {
                let width = if escaped == b'x' { 2 } else { 4 };
                let mut code = hex(rest, width).ok_or(Invalid)?;
                rest = &rest[width..];
                if (0xd800..=0xdbff).contains(&code)
                    && let Some(low @ 0xdc00..=0xdfff) =
                        rest.strip_prefix(b"\\u").and_then(|tail| hex(tail, 4))
                {
                    code = 0x10000 + ((code - 0xd800) << 10) + low - 0xdc00;
                    rest = &rest[6..];
                }
                code
            }
            _ => u32::from(escaped),
        };
        match char::from_u32(code) {
            Some(ch) if !value || ch != '\0' => {
                emit(ch.encode_utf8(&mut [0; 4]).as_bytes())?
            }
            None if !value => emit(&[
                (0xe0 | (code >> 12)) as u8,
                (0x80 | ((code >> 6) & 0x3f)) as u8,
                (0x80 | (code & 0x3f)) as u8,
            ])?,
            _ => return Err(Invalid),
        }
    }
    emit(rest)
}
