//! Condition evaluation over the native evaluator's project-local property view.

use std::ffi::{CStr, c_char, c_int, c_void};

const MAX_GROUP_DEPTH: usize = 32;
const MAX_SIGNIFICANT_DIGITS: usize = 15;
const PATH_MARK: u8 = 1;

#[repr(i32)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Truth {
    False = 0,
    True = 1,
    Unknown = 2,
}

impl From<bool> for Truth {
    fn from(value: bool) -> Self {
        if value { Self::True } else { Self::False }
    }
}

impl Truth {
    fn not(self) -> Self {
        match self {
            Self::False => Self::True,
            Self::True => Self::False,
            Self::Unknown => Self::Unknown,
        }
    }

    fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::False, _) | (_, Self::False) => Self::False,
            (Self::True, Self::True) => Self::True,
            _ => Self::Unknown,
        }
    }

    fn or(self, other: Self) -> Self {
        match (self, other) {
            (Self::True, _) | (_, Self::True) => Self::True,
            (Self::False, Self::False) => Self::False,
            _ => Self::Unknown,
        }
    }
}

fn space(byte: u8) -> bool {
    byte.is_ascii_whitespace() || byte == b'\x0b'
}

fn trim_start(mut text: &[u8]) -> &[u8] {
    while text.first().is_some_and(|&byte| space(byte)) {
        text = &text[1..];
    }
    text
}

fn trim(text: &[u8]) -> &[u8] {
    let text = trim_start(text);
    let end = text.iter().rposition(|&byte| !space(byte));
    end.map_or(&[], |end| &text[..=end])
}

fn boolean(text: &[u8]) -> Option<bool> {
    if [b"true".as_slice(), b"on", b"yes"]
        .iter()
        .any(|word| text.eq_ignore_ascii_case(word))
    {
        Some(true)
    } else if [b"false".as_slice(), b"off", b"no"]
        .iter()
        .any(|word| text.eq_ignore_ascii_case(word))
    {
        Some(false)
    } else {
        None
    }
}

#[derive(PartialEq, Eq)]
struct Decimal<'a> {
    negative: bool,
    integer: &'a [u8],
    fraction: &'a [u8],
}

impl Decimal<'_> {
    fn digits(&self) -> usize {
        self.integer.len() + self.fraction.len()
    }
}

fn decimal(text: &[u8]) -> Option<Decimal<'_>> {
    let (negative, digits) = match text {
        [b'-', rest @ ..] => (true, rest),
        [b'+', rest @ ..] => (false, rest),
        _ => (false, text),
    };
    let dot = digits.iter().position(|&byte| byte == b'.');
    let (integer, fraction) =
        dot.map_or((digits, &[][..]), |i| (&digits[..i], &digits[i + 1..]));
    if (integer.is_empty() && fraction.is_empty())
        || !integer.iter().chain(fraction).all(u8::is_ascii_digit)
    {
        return None;
    }
    let integer = integer
        .iter()
        .position(|&byte| byte != b'0')
        .map_or(&[][..], |i| &integer[i..]);
    let fraction = fraction
        .iter()
        .rposition(|&byte| byte != b'0')
        .map_or(&[][..], |i| &fraction[..=i]);
    Some(Decimal {
        negative: negative && !(integer.is_empty() && fraction.is_empty()),
        integer,
        fraction,
    })
}

// An exponent or hexadecimal spelling might also convert to a number.
fn numberish(text: &[u8]) -> bool {
    matches!(text.first(), Some(b'0'..=b'9' | b'+' | b'-' | b'.'))
        && text
            .iter()
            .all(|&byte| byte.is_ascii_hexdigit() || b"xX.+-".contains(&byte))
}

fn equal_spans(left: &[u8], right: &[u8]) -> Truth {
    if left.eq_ignore_ascii_case(right) {
        return Truth::True;
    }
    let left_mark = left.first() == Some(&PATH_MARK);
    let right_mark = right.first() == Some(&PATH_MARK);
    if left[usize::from(left_mark)..].contains(&PATH_MARK)
        || right[usize::from(right_mark)..].contains(&PATH_MARK)
    {
        return Truth::Unknown;
    }
    if left_mark || right_mark {
        // The real repository path is unknown.
        return if (left_mark && right_mark)
            || left.is_empty()
            || right.is_empty()
        {
            Truth::False
        } else {
            Truth::Unknown
        };
    }
    if let (Some(left), Some(right)) = (boolean(left), boolean(right)) {
        return (left == right).into();
    }
    let left_decimal = decimal(left);
    let right_decimal = decimal(right);
    if let (Some(left), Some(right)) = (&left_decimal, &right_decimal) {
        return if left.digits() > MAX_SIGNIFICANT_DIGITS
            || right.digits() > MAX_SIGNIFICANT_DIGITS
        {
            Truth::Unknown // MSBuild compares doubles, which may round together.
        } else {
            (left == right).into()
        };
    }
    if (left_decimal.is_some() || numberish(left))
        && (right_decimal.is_some() || numberish(right))
    {
        return Truth::Unknown;
    }
    Truth::False
}

fn equal(left: &[u8], right: &[u8]) -> Truth {
    let written = equal_spans(left, right);
    if written == equal_spans(trim(left), trim(right)) {
        written
    } else {
        Truth::Unknown
    }
}

fn truth(text: &[u8]) -> Truth {
    match boolean(text) {
        Some(value) if boolean(trim(text)) == Some(value) => value.into(),
        _ => Truth::Unknown,
    }
}

type ReadOperand = unsafe extern "C" fn(
    *mut c_void,
    *const c_char,
    usize,
    bool,
) -> *const c_char;
type ResetOperands = unsafe extern "C" fn(*mut c_void);

struct Operands {
    context: *mut c_void,
    read: ReadOperand,
    reset: ResetOperands,
}

impl Operands {
    fn read(&self, text: &[u8], expand: bool) -> Option<&[u8]> {
        // SAFETY: The host copies this span and keeps every value readable
        // through the end of the comparison, including across further reads.
        let value = unsafe {
            (self.read)(self.context, text.as_ptr().cast(), text.len(), expand)
        };
        if value.is_null() {
            None
        } else {
            Some(unsafe { CStr::from_ptr(value) }.to_bytes())
        }
    }

    fn reset(&mut self) {
        // SAFETY: The native context is live. No operand borrows outlive reset.
        unsafe { (self.reset)(self.context) }
    }
}

// A syntax error invalidates the whole condition. Unknown is a valid value
// that still participates in three-valued and/or.
type Parsed<T> = Result<T, ()>;

struct Parser<'a> {
    rest: &'a [u8],
}

fn word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

impl<'a> Parser<'a> {
    fn ws(&mut self) {
        self.rest = trim_start(self.rest);
    }

    fn keyword(&mut self, word: &[u8]) -> bool {
        self.ws();
        if let Some(prefix) = self.rest.get(..word.len())
            && prefix.eq_ignore_ascii_case(word)
            && !self
                .rest
                .get(word.len())
                .is_some_and(|&byte| word_byte(byte))
        {
            self.rest = &self.rest[word.len()..];
            true
        } else {
            false
        }
    }

    fn skip_group(&mut self) -> Parsed<()> {
        let mut depth = 1usize;
        let mut quoted = false;
        for (i, &byte) in self.rest.iter().enumerate().skip(1) {
            if byte == b'\'' {
                quoted = !quoted;
            } else if !quoted {
                match byte {
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            self.rest = &self.rest[i + 1..];
                            return Ok(());
                        }
                    }
                    _ => {}
                }
            }
        }
        Err(())
    }

    fn operand<'host>(
        &mut self,
        host: &'host Operands,
    ) -> Parsed<Option<&'host [u8]>> {
        self.ws();
        let text = self.rest;
        if let Some(quoted) = text.strip_prefix(b"'") {
            let end = quoted.iter().position(|&byte| byte == b'\'').ok_or(())?;
            self.rest = &quoted[end + 1..];
            return Ok(host.read(&quoted[..end], true));
        }
        if text.starts_with(b"$(") {
            self.rest = &text[1..];
            self.skip_group()?;
            return Ok(host.read(&text[..text.len() - self.rest.len()], true));
        }
        let end = text
            .iter()
            .position(|&byte| !(word_byte(byte) || b".-+".contains(&byte)))
            .unwrap_or(text.len());
        if end == 0 {
            return Err(());
        }
        self.rest = &text[end..];
        self.ws();
        if self.rest.starts_with(b"(") {
            self.skip_group()?;
            return Ok(None); // Exists, HasTrailingSlash, etc.
        }
        Ok(host.read(&text[..end], false))
    }

    fn comparison(&mut self, host: &Operands) -> Parsed<Truth> {
        let left = self.operand(host)?;
        self.ws();
        match self.rest {
            [op @ (b'=' | b'!'), b'=', rest @ ..] => {
                self.rest = rest;
                let right = self.operand(host)?;
                let value = match (left, right) {
                    (Some(left), Some(right)) => equal(left, right),
                    _ => Truth::Unknown,
                };
                Ok(if *op == b'!' { value.not() } else { value })
            }
            [b'<' | b'>', rest @ ..] => {
                self.rest = rest.strip_prefix(b"=").unwrap_or(rest);
                self.operand(host)?;
                Ok(Truth::Unknown)
            }
            _ => Ok(left.map_or(Truth::Unknown, truth)),
        }
    }

    fn primary(&mut self, host: &mut Operands, depth: usize) -> Parsed<Truth> {
        self.ws();
        let mut negate = false;
        while self.rest.starts_with(b"!") && !self.rest.starts_with(b"!=") {
            negate = !negate;
            self.rest = trim_start(&self.rest[1..]);
        }
        let result = if let Some(rest) = self.rest.strip_prefix(b"(") {
            if depth == MAX_GROUP_DEPTH {
                return Err(());
            }
            self.rest = rest;
            let value = self.or(host, depth + 1)?;
            self.ws();
            self.rest = self.rest.strip_prefix(b")").ok_or(())?;
            value
        } else {
            let value = self.comparison(host);
            host.reset();
            value?
        };
        Ok(if negate { result.not() } else { result })
    }

    fn and(&mut self, host: &mut Operands, depth: usize) -> Parsed<Truth> {
        let mut value = self.primary(host, depth)?;
        while self.keyword(b"and") {
            // Evaluate both operands so the native cache records their reads.
            value = value.and(self.primary(host, depth)?);
        }
        Ok(value)
    }

    fn or(&mut self, host: &mut Operands, depth: usize) -> Parsed<Truth> {
        let mut value = self.and(host, depth)?;
        while self.keyword(b"or") {
            value = value.or(self.and(host, depth)?);
        }
        Ok(value)
    }
}

/// The condition is null or a NUL-terminated string valid for this call.
/// Callbacks accept a live context and must not retain input spans. Any
/// returned string remains valid until reset; the parser retains no pointers.
#[unsafe(no_mangle)]
unsafe extern "C" fn mori_msbuild_condition(
    condition: *const c_char,
    context: *mut c_void,
    read: ReadOperand,
    reset: ResetOperands,
) -> c_int {
    if condition.is_null() {
        return Truth::True as c_int;
    }
    // SAFETY: The native evaluator holds the immutable condition text.
    let mut parser = Parser {
        rest: unsafe { CStr::from_ptr(condition) }.to_bytes(),
    };
    parser.ws();
    if parser.rest.is_empty() {
        return Truth::True as c_int;
    }
    let mut host = Operands {
        context,
        read,
        reset,
    };
    let result = parser.or(&mut host, 0);
    parser.ws();
    if parser.rest.is_empty() {
        result.unwrap_or(Truth::Unknown) as c_int
    } else {
        Truth::Unknown as c_int
    }
}

/// Both arguments are non-null, NUL-terminated expanded operands.
#[unsafe(no_mangle)]
unsafe extern "C" fn mori_msbuild_equal(
    left: *const c_char,
    right: *const c_char,
) -> c_int {
    // SAFETY: Both native operands remain readable until this call returns.
    let (left, right) = unsafe { (CStr::from_ptr(left), CStr::from_ptr(right)) };
    equal(left.to_bytes(), right.to_bytes()) as c_int
}
