//! C# document-link resolution and incremental scope rules.

mod usings;

use std::ffi::{CStr, c_char, c_int};
use std::ptr;
use std::slice;

const MAX_SEGMENTS: usize = 16;
const MAX_PARAMS: usize = 24;
const NAME_CAPACITY: usize = 513;
const PARAM_CAPACITY: usize = 256;
const REF_CAPACITY: usize = 1024;
const OP_CAPACITY: usize = 16;
const NO_ARITY: c_int = -1;

#[repr(C)]
struct Segment {
    name: [u8; NAME_CAPACITY],
    arity: c_int,
    args: *const u8,
    args_len: usize,
}

#[repr(C)]
struct Reference {
    segments: [Segment; MAX_SEGMENTS],
    segment_count: c_int,
    has_params: bool,
    params: [[u8; PARAM_CAPACITY]; MAX_PARAMS],
    param_count: c_int,
    signature: [u8; MAX_PARAMS * PARAM_CAPACITY],
    unknown_signature: bool,
    doc_id: u8,
    global: bool,
    operator: bool,
    operator_name: [u8; OP_CAPACITY],
    maybe_indexer: bool,
    keyword: bool,
}

unsafe extern "C" {
    fn atoi(text: *const c_char) -> c_int;
    fn cbm_doclink_cs_norm_type(
        text: *const c_char,
        len: usize,
        out: *mut c_char,
        capacity: usize,
    ) -> usize;
}

// C's isspace in the C locale also includes vertical tab.
fn space(byte: u8) -> bool {
    byte.is_ascii_whitespace() || byte == b'\x0b'
}

fn trim_start(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|&byte| !space(byte))
        .unwrap_or(bytes.len());
    &bytes[start..]
}

fn trim_end(bytes: &[u8], discard: impl Fn(u8) -> bool) -> &[u8] {
    let end = bytes
        .iter()
        .rposition(|&byte| !discard(byte))
        .map_or(0, |i| i + 1);
    &bytes[..end]
}

fn trim(bytes: &[u8]) -> &[u8] {
    trim_end(trim_start(bytes), space)
}

fn open(byte: u8) -> bool {
    b"<{[(".contains(&byte)
}

fn close(byte: u8) -> bool {
    b">}])".contains(&byte)
}

fn depth_change(byte: u8) -> i32 {
    i32::from(open(byte)) - i32::from(close(byte))
}

fn group_end(bytes: &[u8], start: usize) -> usize {
    let mut depth = 0;
    for (index, &byte) in bytes.iter().enumerate().skip(start) {
        depth += depth_change(byte);
        if depth == 0 {
            return index + 1;
        }
    }
    bytes.len()
}

// Split on top-level delimiters. The C grammar counts bracket depth across all
// bracket kinds. Preserve that rule for persisted scope inputs.
fn parts(
    bytes: &[u8],
    delimiter: u8,
    mut visit: impl FnMut(&[u8]) -> bool,
) -> bool {
    let mut depth = 0;
    let mut start = 0;
    for (index, byte) in bytes.iter().copied().enumerate() {
        if byte == delimiter && depth == 0 {
            if !visit(&bytes[start..index]) {
                return false;
            }
            start = index + 1;
        }
        depth += depth_change(byte);
        if depth < 0 {
            return false;
        }
    }
    depth == 0 && visit(&bytes[start..])
}

fn count_arguments(bytes: &[u8]) -> c_int {
    if bytes.iter().all(|&byte| space(byte)) {
        return 0;
    }
    let mut items = 1;
    let mut depth = 0;
    for &byte in bytes {
        items += c_int::from(byte == b',' && depth == 0);
        depth += depth_change(byte);
    }
    items
}

fn identifier(bytes: &[u8]) -> bool {
    if matches!(bytes, b"#ctor" | b"#cctor") {
        return true;
    }
    bytes.split_first().is_some_and(|(&first, rest)| {
        (first.is_ascii_alphabetic() || first == b'_' || first >= 128)
            && rest.iter().all(|&byte| {
                byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 128
            })
    })
}

fn write_string(buffer: &mut [u8], bytes: &[u8]) {
    buffer[..bytes.len()].copy_from_slice(bytes);
    buffer[bytes.len()] = 0;
}

// Keep the C integer conversion on the host. Only the optional sign and the
// first significant digits matter, since 30 digits overflow every host long.
fn metadata_arity(bytes: &[u8]) -> c_int {
    let bytes = trim_start(bytes);
    let mut number = [0u8; 32];
    let (digits, start) = match bytes.split_first() {
        Some((&sign @ (b'+' | b'-'), rest)) => {
            number[0] = sign;
            (rest, 1)
        }
        _ => (bytes, 0),
    };
    let zeros = digits.iter().take_while(|&&byte| byte == b'0').count();
    for (index, &digit) in digits[zeros..]
        .iter()
        .take_while(|digit| digit.is_ascii_digit())
        .take(30)
        .enumerate()
    {
        number[start + index] = digit;
    }
    // SAFETY: number always has a trailing NUL.
    unsafe { atoi(number.as_ptr().cast()) }
}

fn segment(bytes: &[u8]) -> Option<Segment> {
    let bytes = trim(bytes);
    let name_end = bytes
        .iter()
        .position(|&byte| matches!(byte, b'`' | b'{' | b'<'))
        .unwrap_or(bytes.len());
    let name = &bytes[..name_end];
    let name = name.strip_prefix(b"@").unwrap_or(name);
    if name.len() >= NAME_CAPACITY || !identifier(name) {
        return None;
    }

    let mut result = Segment {
        name: [0; NAME_CAPACITY],
        arity: NO_ARITY,
        args: ptr::null(),
        args_len: 0,
    };
    write_string(&mut result.name, name);
    match bytes.get(name_end) {
        Some(b'`') => {
            let tail = &bytes[name_end..];
            let ticks = tail.iter().take_while(|&&byte| byte == b'`').count();
            result.arity = metadata_arity(&tail[ticks..]);
        }
        Some(_) => {
            let end = group_end(bytes, name_end);
            if end < bytes.len() {
                return None;
            }
            let args_start = name_end + 1;
            let args = &bytes[args_start..args_start.max(end - 1)];
            result.arity = count_arguments(args);
            result.args = args.as_ptr();
            result.args_len = args.len();
        }
        None => {}
    }
    Some(result)
}

fn path(bytes: &[u8], mut write: impl FnMut(usize, Segment)) -> Option<usize> {
    let mut count = 0;
    let valid = parts(bytes, b'.', |part| {
        if count == MAX_SEGMENTS {
            return false;
        }
        let Some(parsed) = segment(part) else {
            return false;
        };
        write(count, parsed);
        count += 1;
        true
    });
    valid.then_some(count)
}

fn keyword(name: &[u8]) -> Option<&'static [u8]> {
    Some(match name {
        b"int" => b"Int32",
        b"string" => b"String",
        b"object" => b"Object",
        b"bool" => b"Boolean",
        b"byte" => b"Byte",
        b"sbyte" => b"SByte",
        b"short" => b"Int16",
        b"ushort" => b"UInt16",
        b"uint" => b"UInt32",
        b"long" => b"Int64",
        b"ulong" => b"UInt64",
        b"float" => b"Single",
        b"double" => b"Double",
        b"decimal" => b"Decimal",
        b"char" => b"Char",
        b"nint" => b"IntPtr",
        b"nuint" => b"UIntPtr",
        b"void" => b"Void",
        _ => return None,
    })
}

fn metadata_operator(name: &[u8]) -> Option<&'static [u8]> {
    Some(match name {
        b"Addition" | b"UnaryPlus" | b"CheckedAddition" => b"+",
        b"Subtraction"
        | b"UnaryNegation"
        | b"CheckedSubtraction"
        | b"CheckedUnaryNegation" => b"-",
        b"Multiply" | b"CheckedMultiply" => b"*",
        b"Division" | b"CheckedDivision" => b"/",
        b"Modulus" => b"%",
        b"BitwiseAnd" => b"&",
        b"BitwiseOr" => b"|",
        b"ExclusiveOr" => b"^",
        b"LeftShift" => b"<<",
        b"RightShift" => b">>",
        b"UnsignedRightShift" => b">>>",
        b"Equality" => b"==",
        b"Inequality" => b"!=",
        b"LessThan" => b"<",
        b"GreaterThan" => b">",
        b"LessThanOrEqual" => b"<=",
        b"GreaterThanOrEqual" => b">=",
        b"LogicalNot" => b"!",
        b"OnesComplement" => b"~",
        b"Increment" | b"CheckedIncrement" => b"++",
        b"Decrement" | b"CheckedDecrement" => b"--",
        b"True" => b"true",
        b"False" => b"false",
        b"Implicit" => b"implicit",
        b"Explicit" | b"CheckedExplicit" => b"explicit",
        _ => return None,
    })
}

fn strip_word<'a>(bytes: &'a [u8], word: &[u8]) -> Option<&'a [u8]> {
    bytes.strip_prefix(word).filter(|tail| {
        tail.first()
            .is_none_or(|byte| !byte.is_ascii_alphanumeric() && *byte != b'_')
    })
}

fn skip_blanks(bytes: &[u8]) -> &[u8] {
    let start = bytes.iter().take_while(|&&byte| byte == b' ').count();
    &bytes[start..]
}

fn operator_at(bytes: &[u8]) -> Option<&[u8]> {
    let bytes = skip_blanks(bytes);
    for word in [b"implicit", b"explicit"] {
        if let Some(tail) = strip_word(bytes, word) {
            return strip_word(skip_blanks(tail), b"operator")
                .map(|_| word.as_slice());
        }
    }
    if let Some(tail) = strip_word(bytes, b"operator") {
        let tail = skip_blanks(tail);
        let token = strip_word(tail, b"checked").map_or(tail, skip_blanks);
        let max = token.len().min(OP_CAPACITY - 1);
        let end = token[..max]
            .iter()
            .position(|byte| matches!(byte, b'(' | b' '))
            .unwrap_or(max);
        // C's precision is zero when the token is empty.
        return Some(&token[..end]);
    }
    if let Some(tail) = strip_word(bytes, b"this") {
        let tail = skip_blanks(tail);
        return (tail.is_empty() || tail.starts_with(b"[")).then_some(b"this");
    }
    let metadata = bytes.strip_prefix(b"op_")?;
    if !metadata.first()?.is_ascii_uppercase() {
        return None;
    }
    let len = metadata
        .iter()
        .take_while(|byte| byte.is_ascii_alphanumeric())
        .count();
    metadata_operator(&metadata[..len])
}

fn written_operator(bytes: &[u8]) -> Option<(usize, &[u8])> {
    let mut depth = 0;
    for (index, &byte) in bytes.iter().enumerate() {
        if (index == 0 || (depth == 0 && bytes[index - 1] == b'.'))
            && let Some(operator) = operator_at(&bytes[index..])
        {
            return Some((index, operator));
        }
        depth += depth_change(byte);
    }
    None
}

fn slot_param(bytes: &[u8]) -> Option<&[u8]> {
    let bytes = trim_end(trim_start(bytes), |byte| space(byte) || byte == b'@');
    let ticks = bytes.iter().take_while(|&&byte| byte == b'`').count();
    if !(1..=2).contains(&ticks) || bytes.len() >= PARAM_CAPACITY {
        return None;
    }
    let tail = &bytes[ticks..];
    let digits = tail.iter().take_while(|byte| byte.is_ascii_digit()).count();
    (digits > 0 && tail[digits..].iter().all(|byte| b"[],*".contains(byte)))
        .then_some(bytes)
}

fn parse_params(reference: &mut Reference, bytes: &[u8]) -> bool {
    if bytes.iter().all(|&byte| space(byte)) {
        return true;
    }
    let mut written = 0;
    parts(bytes, b',', |part| {
        let index = reference.param_count as usize;
        if index == MAX_PARAMS {
            return false;
        }
        let param = &mut reference.params[index];
        let len = if let Some(slot) = slot_param(part) {
            write_string(param, slot);
            slot.len()
        } else {
            // SAFETY: the scanner takes exactly part.len() input bytes and
            // writes a terminated parameter within this fixed-size buffer.
            unsafe {
                cbm_doclink_cs_norm_type(
                    part.as_ptr().cast(),
                    part.len(),
                    param.as_mut_ptr().cast(),
                    param.len(),
                )
            }
        };
        reference.param_count += 1;
        reference.unknown_signature |= param[..len].contains(&b'?');
        if written > 0 {
            reference.signature[written] = b'|';
            written += 1;
        }
        write_string(&mut reference.signature[written..], &param[..len]);
        written += len;
        true
    })
}

fn rewrite_keyword(reference: &mut Reference, count: usize) {
    let first = &reference.segments[0];
    if reference.doc_id != 0 || count == MAX_SEGMENTS || first.arity > 0 {
        return;
    }
    let end = first.name.iter().position(|&byte| byte == 0).unwrap();
    let Some(system_name) = keyword(&first.name[..end]) else {
        return;
    };
    reference.segments[..=count].rotate_right(1);
    let namespace = &mut reference.segments[0];
    namespace.arity = NO_ARITY;
    write_string(&mut namespace.name, b"System");
    write_string(&mut reference.segments[1].name, system_name);
    reference.segment_count += 1;
    reference.keyword = true;
    reference.global = true;
}

fn parse_cref(raw: &[u8], reference: &mut Reference) -> bool {
    let mut bytes = trim(raw);
    if bytes.is_empty() || bytes.len() >= REF_CAPACITY {
        return false;
    }

    if let [kind, b':', _, ..] = bytes
        && b"TMPFENO!".contains(kind)
    {
        if *kind == b'!' {
            return false;
        }
        bytes = trim_start(&bytes[2..]);
        if *kind == b'O' {
            // DocFX overload groups carry a member name without a signature.
            bytes = bytes.split(|&byte| byte == b'(').next().unwrap();
        }
        reference.doc_id = *kind;
        reference.global = true;
    }
    if let Some(unqualified) = bytes.strip_prefix(b"global::") {
        bytes = unqualified;
        reference.global = true;
    }
    if let Some((start, operator)) = written_operator(bytes) {
        reference.operator = true;
        write_string(&mut reference.operator_name, operator);
        if start == 0 {
            return true;
        }
        let Some(count) = path(&bytes[..start - 1], |index, part| {
            reference.segments[index] = part;
        }) else {
            return false;
        };
        reference.segment_count = count as c_int;
        rewrite_keyword(reference, count);
        return true;
    }

    let mut depth = 0;
    let paren = bytes
        .iter()
        .position(|&byte| {
            if byte == b'(' && depth == 0 {
                return true;
            }
            depth += match byte {
                b'<' | b'{' | b'[' => 1,
                b'>' | b'}' | b']' => -1,
                _ => 0,
            };
            false
        })
        .unwrap_or(bytes.len());
    let Some(count) = path(&bytes[..paren], |index, part| {
        reference.segments[index] = part;
    }) else {
        return false;
    };
    reference.segment_count = count as c_int;
    if paren < bytes.len() {
        reference.has_params = true;
        let end = group_end(bytes, paren);
        if bytes[end - 1] != b')'
            || !bytes[end..].iter().all(|&byte| space(byte))
            || !parse_params(reference, &bytes[paren + 1..end - 1])
        {
            return false;
        }
        reference.maybe_indexer =
            reference.segments[count - 1].name.starts_with(b"Item\0");
    }
    rewrite_keyword(reference, count);
    true
}

// The C caller owns all input and output storage. Type argument spans borrow
// the original input through the synchronous lookup.
#[unsafe(no_mangle)]
unsafe extern "C" fn mori_cs_doclink_parse_name(
    input: *const u8,
    len: usize,
    segments: *mut Segment,
    count: *mut c_int,
    global: *mut bool,
) -> bool {
    // SAFETY: C passes a valid name span and a MAX_SEGMENTS output array.
    let bytes = unsafe { slice::from_raw_parts(input, len) };
    let stripped = bytes.strip_prefix(b"global::");
    let parsed = path(stripped.unwrap_or(bytes), |index, segment| unsafe {
        segments.add(index).write(segment);
    });
    unsafe {
        global.write(stripped.is_some());
        count.write(parsed.unwrap_or(0) as c_int);
    }
    parsed.is_some()
}

// The C caller supplies a writable Reference and a NUL-terminated cref, or
// NULL for a missing cref. A successful Reference borrows that same cref.
#[unsafe(no_mangle)]
unsafe extern "C" fn mori_cs_doclink_parse_cref(
    input: *const c_char,
    out: *mut Reference,
) -> bool {
    if input.is_null() {
        return false;
    }
    let raw = unsafe { CStr::from_ptr(input) }.to_bytes();
    // All fields admit zero, including the borrowed pointers in unused slots.
    unsafe { out.write_bytes(0, 1) };
    parse_cref(raw, unsafe { &mut *out })
}

// An unsupported type form beginning with a C# keyword still names a System
// type. Overlong input cannot be classified by its prefix.
#[unsafe(no_mangle)]
unsafe extern "C" fn mori_cs_doclink_unparsed_keyword(
    input: *const c_char,
) -> bool {
    if input.is_null() {
        return false;
    }
    let bytes = trim_start(unsafe { CStr::from_ptr(input) }.to_bytes());
    let head = bytes
        .split(|byte| b"({<.[?* \t".contains(byte))
        .next()
        .unwrap();
    bytes.len() < REF_CAPACITY && keyword(head).is_some()
}

fn scope_lines(scope: &[u8]) -> impl Iterator<Item = &[u8]> {
    scope
        .split(|&byte| byte == b'\n')
        .filter(|line| !line.is_empty())
}

fn scope_field(line: &[u8], field: usize) -> Option<&[u8]> {
    line.split(|&byte| byte == b'\t').nth(field)
}

fn local_using(line: &[u8]) -> bool {
    line.starts_with(b"U")
        && scope_field(line, 2)
            .is_some_and(|kind| !kind.is_empty() && !kind.contains(&b'g'))
}

fn has_bases(scope: &[u8]) -> bool {
    scope_lines(scope).any(|line| {
        line.starts_with(b"T")
            && scope_field(line, 8).is_some_and(|bases| !bases.is_empty())
    })
}

#[repr(i32)]
#[derive(Debug, PartialEq, Eq)]
enum ScopeDelta {
    Local = 0,
    Global = 1,
    Unrecordable = -1,
}

fn scope_delta(
    stored: &[u8],
    fresh: &[u8],
    isolated: bool,
    mut removed: impl FnMut(&[u8]) -> bool,
) -> ScopeDelta {
    if isolated {
        // MSBuild inputs set unit-wide usings. Rejected files have no scope
        // until their serialized input changes.
        return if stored == fresh {
            ScopeDelta::Local
        } else {
            ScopeDelta::Global
        };
    }
    // Preserve using order when it can change a base type other files see.
    if !scope_lines(stored)
        .filter(|line| local_using(line))
        .eq(scope_lines(fresh).filter(|line| local_using(line)))
        && (has_bases(stored) || has_bases(fresh))
    {
        return ScopeDelta::Global;
    }

    let mut fresh = scope_lines(fresh)
        .filter(|line| !local_using(line))
        .peekable();
    for line in scope_lines(stored).filter(|line| !local_using(line)) {
        if fresh.peek().copied() == Some(line) {
            fresh.next();
        } else if line.starts_with(b"M") {
            // A removed member can only affect references that target its
            // file or unresolved references that name it.
            let Some(name) =
                scope_field(line, 5).filter(|name| !name.is_empty())
            else {
                return ScopeDelta::Unrecordable;
            };
            if !removed(name) {
                return ScopeDelta::Unrecordable;
            }
        } else {
            return ScopeDelta::Global;
        }
    }
    if fresh.next().is_some() {
        ScopeDelta::Global
    } else {
        ScopeDelta::Local
    }
}

type RemovedName =
    unsafe extern "C" fn(*mut std::ffi::c_void, *const c_char, usize) -> bool;

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_cs_doclink_scope_delta(
    stored: *const c_char,
    fresh: *const c_char,
    isolated: bool,
    removed: Option<RemovedName>,
    data: *mut std::ffi::c_void,
) -> c_int {
    // SAFETY: the resolver dispatch checks both scope strings before calling
    // this hook. Callback names are spans borrowed from the stored scope.
    let stored = unsafe { CStr::from_ptr(stored) }.to_bytes();
    let fresh = unsafe { CStr::from_ptr(fresh) }.to_bytes();
    scope_delta(stored, fresh, isolated, |name| {
        removed.is_some_and(|record| unsafe {
            record(data, name.as_ptr().cast(), name.len())
        })
    }) as c_int
}
