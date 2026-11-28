//! The part of yyjson's reader and writer the C decorator tags pass relied
//! on: objects keep member order and duplicate keys, numbers keep yyjson's
//! integer and real kinds, and the output is yyjson's compact form.

enum Value {
    Null,
    Bool(bool),
    Unsigned(u64),
    Signed(i64),
    Real(f64),
    Str(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

// Nesting past this depth reads as invalid instead of risking the stack.
const DEPTH_MAX: usize = 4096;

/// A strict RFC 8259 reader with yyjson's default number kinds: an integer
/// literal is unsigned or signed when it fits 64 bits (`-0` is the integer
/// 0) and a real otherwise; reals round to nearest and must be finite.
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_space(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.peek() {
            self.pos += 1;
        }
    }

    fn eat(&mut self, literal: &[u8]) -> Option<()> {
        let end = self.pos.checked_add(literal.len())?;
        (self.bytes.get(self.pos..end)? == literal).then(|| self.pos = end)
    }

    fn value(&mut self, depth: usize) -> Option<Value> {
        if depth > DEPTH_MAX {
            return None;
        }
        self.skip_space();
        match self.peek()? {
            b'n' => self.eat(b"null").map(|()| Value::Null),
            b't' => self.eat(b"true").map(|()| Value::Bool(true)),
            b'f' => self.eat(b"false").map(|()| Value::Bool(false)),
            b'"' => self.string().map(Value::Str),
            b'[' => {
                self.pos += 1;
                let mut items = Vec::new();
                self.skip_space();
                if self.peek()? == b']' {
                    self.pos += 1;
                    return Some(Value::Array(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    self.skip_space();
                    match self.peek()? {
                        b',' => self.pos += 1,
                        b']' => {
                            self.pos += 1;
                            return Some(Value::Array(items));
                        }
                        _ => return None,
                    }
                }
            }
            b'{' => {
                self.pos += 1;
                let mut members = Vec::new();
                self.skip_space();
                if self.peek()? == b'}' {
                    self.pos += 1;
                    return Some(Value::Object(members));
                }
                loop {
                    self.skip_space();
                    if self.peek()? != b'"' {
                        return None;
                    }
                    let key = self.string()?;
                    self.skip_space();
                    self.eat(b":")?;
                    members.push((key, self.value(depth + 1)?));
                    self.skip_space();
                    match self.peek()? {
                        b',' => self.pos += 1,
                        b'}' => {
                            self.pos += 1;
                            return Some(Value::Object(members));
                        }
                        _ => return None,
                    }
                }
            }
            b'-' | b'0'..=b'9' => self.number(),
            _ => None,
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.pos;
        while let Some(b'0'..=b'9') = self.peek() {
            self.pos += 1;
        }
        self.pos - start
    }

    fn number(&mut self) -> Option<Value> {
        let start = self.pos;
        let negative = self.peek() == Some(b'-');
        if negative {
            self.pos += 1;
        }
        let int_start = self.pos;
        let int_len = self.digits();
        if int_len == 0 || (int_len > 1 && self.bytes[int_start] == b'0') {
            return None;
        }
        let mut real = false;
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if self.digits() == 0 {
                return None;
            }
            real = true;
        }
        if let Some(b'e' | b'E') = self.peek() {
            self.pos += 1;
            if let Some(b'+' | b'-') = self.peek() {
                self.pos += 1;
            }
            if self.digits() == 0 {
                return None;
            }
            real = true;
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos]).ok()?;
        if !real {
            let magnitude = &self.bytes[int_start..self.pos];
            let magnitude = std::str::from_utf8(magnitude).ok()?;
            if let Ok(n) = magnitude.parse::<u64>() {
                if !negative {
                    return Some(Value::Unsigned(n));
                }
                if n <= 1 << 63 {
                    return Some(Value::Signed(0i64.wrapping_sub_unsigned(n)));
                }
            }
        }
        let x: f64 = text.parse().ok()?;
        x.is_finite().then_some(Value::Real(x))
    }

    fn hex4(&mut self) -> Option<u32> {
        let end = self.pos.checked_add(4)?;
        let hex = std::str::from_utf8(self.bytes.get(self.pos..end)?).ok()?;
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        self.pos = end;
        u32::from_str_radix(hex, 16).ok()
    }

    fn string(&mut self) -> Option<String> {
        self.pos += 1;
        let mut out = Vec::new();
        loop {
            let byte = self.peek()?;
            self.pos += 1;
            match byte {
                b'"' => return String::from_utf8(out).ok(),
                0x00..=0x1f => return None,
                b'\\' => {
                    let escape = self.peek()?;
                    self.pos += 1;
                    let simple = match escape {
                        b'"' => b'"',
                        b'\\' => b'\\',
                        b'/' => b'/',
                        b'b' => 0x08,
                        b'f' => 0x0c,
                        b'n' => b'\n',
                        b'r' => b'\r',
                        b't' => b'\t',
                        b'u' => {
                            let unit = self.hex4()?;
                            let code = match unit {
                                0xd800..=0xdbff => {
                                    self.eat(b"\\u")?;
                                    let low = self.hex4()?;
                                    if !(0xdc00..=0xdfff).contains(&low) {
                                        return None;
                                    }
                                    0x10000
                                        + ((unit - 0xd800) << 10)
                                        + (low - 0xdc00)
                                }
                                0xdc00..=0xdfff => return None,
                                _ => unit,
                            };
                            let c = char::from_u32(code)?;
                            out.extend_from_slice(
                                c.encode_utf8(&mut [0; 4]).as_bytes(),
                            );
                            continue;
                        }
                        _ => return None,
                    };
                    out.push(simple);
                }
                _ => out.push(byte),
            }
        }
    }
}

fn parse(bytes: &[u8]) -> Option<Value> {
    let mut reader = Reader { bytes, pos: 0 };
    let value = reader.value(0)?;
    reader.skip_space();
    (reader.pos == bytes.len()).then_some(value)
}

/// The strings of the first `decorators` member, each cut at its first NUL
/// as the C string copy did, or `None` when the member is missing, not an
/// array, or holds no strings.
pub(super) fn decorators(props: &[u8]) -> Option<Vec<Vec<u8>>> {
    let Value::Object(members) = parse(props)? else {
        return None;
    };
    let (_, value) = members.iter().find(|(key, _)| key == "decorators")?;
    let Value::Array(items) = value else {
        return None;
    };
    let strings: Vec<Vec<u8>> = items
        .iter()
        .filter_map(|item| match item {
            Value::Str(s) => {
                let bytes = s.as_bytes();
                let end = bytes.iter().position(|&b| b == 0);
                Some(bytes[..end.unwrap_or(bytes.len())].to_vec())
            }
            _ => None,
        })
        .collect();
    (!strings.is_empty()).then_some(strings)
}

/// `props` without its `decorator_tags` members and with `tags` appended,
/// or `None` where yyjson refused to write: a tag that is not UTF-8.
pub(super) fn with_tags(props: &[u8], tags: &[Vec<u8>]) -> Option<Vec<u8>> {
    let members = match parse(props) {
        Some(Value::Object(members)) => members,
        _ => Vec::new(),
    };
    let mut out = vec![b'{'];
    for (key, value) in members.iter().filter(|(key, _)| key != "decorator_tags")
    {
        write_str(&mut out, key.as_bytes());
        out.push(b':');
        write_value(&mut out, value);
        out.push(b',');
    }
    write_str(&mut out, b"decorator_tags");
    out.extend_from_slice(b":[");
    for (i, tag) in tags.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        std::str::from_utf8(tag).ok()?;
        write_str(&mut out, tag);
    }
    out.extend_from_slice(b"]}");
    Some(out)
}

fn write_value(out: &mut Vec<u8>, value: &Value) {
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(true) => out.extend_from_slice(b"true"),
        Value::Bool(false) => out.extend_from_slice(b"false"),
        Value::Unsigned(n) => out.extend_from_slice(n.to_string().as_bytes()),
        Value::Signed(n) => out.extend_from_slice(n.to_string().as_bytes()),
        Value::Real(x) => write_real(out, *x),
        Value::Str(s) => write_str(out, s.as_bytes()),
        Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_value(out, item);
            }
            out.push(b']');
        }
        Value::Object(members) => {
            out.push(b'{');
            for (i, (key, value)) in members.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_str(out, key.as_bytes());
                out.push(b':');
                write_value(out, value);
            }
            out.push(b'}');
        }
    }
}

// yyjson's default escapes: short forms where JSON has them, uppercase hex
// for the other control bytes, everything else (`/`, DEL, UTF-8) as is.
fn write_str(out: &mut Vec<u8>, bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    out.push(b'"');
    for &byte in bytes {
        match byte {
            b'"' => out.extend_from_slice(br#"\""#),
            b'\\' => out.extend_from_slice(br"\\"),
            b'\n' => out.extend_from_slice(br"\n"),
            b'\r' => out.extend_from_slice(br"\r"),
            b'\t' => out.extend_from_slice(br"\t"),
            0x08 => out.extend_from_slice(br"\b"),
            0x0c => out.extend_from_slice(br"\f"),
            0x00..=0x1f => out.extend_from_slice(&[
                b'\\',
                b'u',
                b'0',
                b'0',
                HEX[usize::from(byte >> 4)],
                HEX[usize::from(byte & 0xf)],
            ]),
            _ => out.push(byte),
        }
    }
    out.push(b'"');
}

/// A finite real the way yyjson writes it: the shortest digits that read
/// back to the same value, laid out like ECMAScript's Number#toString, with
/// the sign of -0.0, a `.0` on whole numbers, and no `+` in exponents.
fn write_real(out: &mut Vec<u8>, x: f64) {
    if x.is_sign_negative() {
        out.push(b'-');
    }
    let x = x.abs();
    if x == 0.0 {
        out.extend_from_slice(b"0.0");
        return;
    }
    // `{:e}` gives the shortest round-trip digits as d.ddd and an exponent.
    let sci = format!("{x:e}");
    let (mantissa, exponent) = sci.split_once('e').expect("{:e} has an e");
    let digits: Vec<u8> = mantissa.bytes().filter(|&b| b != b'.').collect();
    let exponent: i32 = exponent.parse().expect("{:e} exponent is an integer");
    // The decimal point's offset from the first digit.
    let dot = exponent + 1;
    let len = digits.len() as i32;
    if -6 < dot && dot <= 21 {
        if dot <= 0 {
            out.extend_from_slice(b"0.");
            out.extend(std::iter::repeat_n(b'0', (-dot) as usize));
            out.extend_from_slice(&digits);
        } else if len <= dot {
            out.extend_from_slice(&digits);
            out.extend(std::iter::repeat_n(b'0', (dot - len) as usize));
            out.extend_from_slice(b".0");
        } else {
            out.extend_from_slice(&digits[..dot as usize]);
            out.push(b'.');
            out.extend_from_slice(&digits[dot as usize..]);
        }
    } else {
        out.push(digits[0]);
        if len > 1 {
            out.push(b'.');
            out.extend_from_slice(&digits[1..]);
        }
        out.push(b'e');
        out.extend_from_slice((dot - 1).to_string().as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::{decorators, with_tags, write_real};

    fn real(x: f64) -> String {
        let mut out = Vec::new();
        write_real(&mut out, x);
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn writes_reals_like_yyjson() {
        assert_eq!(real(0.0), "0.0");
        assert_eq!(real(-0.0), "-0.0");
        assert_eq!(real(1.0), "1.0");
        assert_eq!(real(1.5), "1.5");
        assert_eq!(real(-2.25), "-2.25");
        assert_eq!(real(100.0), "100.0");
        assert_eq!(real(0.1), "0.1");
        assert_eq!(real(0.000001), "0.000001");
        assert_eq!(real(1e-7), "1e-7");
        assert_eq!(real(1.25e-7), "1.25e-7");
        assert_eq!(real(1e20), "100000000000000000000.0");
        assert_eq!(real(1e21), "1e21");
        assert_eq!(real(1.5e300), "1.5e300");
        assert_eq!(real(18446744073709551616.0), "18446744073709552000.0");
        assert_eq!(real(5e-324), "5e-324");
        assert_eq!(real(123456.789), "123456.789");
    }

    #[test]
    fn reads_the_first_decorators_member() {
        let got = decorators(
            br#"{"decorators":["@a\u0000b",1,"@c"],"decorators":["@x"]}"#,
        );
        assert_eq!(got, Some(vec![b"@a".to_vec(), b"@c".to_vec()]));
        assert_eq!(decorators(br#"{"decorators":"@a"}"#), None);
        assert_eq!(decorators(br#"{"decorators":[1,null]}"#), None);
        assert_eq!(decorators(br#"{"decorators":[]}"#), None);
        assert_eq!(decorators(br#"["decorators"]"#), None);
        assert_eq!(decorators(b"{"), None);
    }

    #[test]
    fn refuses_a_tag_that_is_not_utf8() {
        assert_eq!(with_tags(b"{}", &[b"caf\xc3".to_vec()]), None);
        assert_eq!(
            with_tags(
                br#"{ "a" : "\u00e9\/\u001f" , "decorator_tags":1, "n":-0, "r":1E2 }"#,
                &[b"x".to_vec()]
            ),
            Some(
                "{\"a\":\"\u{e9}/\\u001F\",\"n\":0,\"r\":100.0,\"decorator_tags\":[\"x\"]}"
                    .as_bytes()
                    .to_vec()
            )
        );
    }
}
