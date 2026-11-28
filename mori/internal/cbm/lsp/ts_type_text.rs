//! Type text shared by TS annotations, cross-file signatures, and JavaScript JSDoc.
//! The parser borrows source bytes and emits types through a sink. The linked
//! resolver uses its existing arena as the sink while it still owns the registry.

use std::ffi::c_long;

const MAX_MEMBERS: usize = 15;

#[derive(Debug, PartialEq, Eq)]
enum Name<'a> {
    Bare(&'a [u8]),
    Qualified(&'a [u8], &'a [u8]),
}

#[derive(Debug, PartialEq, Eq)]
enum Param<'a> {
    Explicit(&'a [u8]),
    Positional(usize),
}

#[derive(Debug, PartialEq, Eq)]
enum Shape<'a> {
    Unknown,
    Named(Name<'a>),
    Builtin(&'a [u8]),
    This,
    Infer(&'a [u8]),
    Typeof(&'a [u8]),
    Keyof,
    Conditional,
    Function(Vec<Param<'a>>),
    Tuple,
    Union,
    Intersection,
    Template(Name<'a>),
}

struct Plan<'a> {
    shape: Shape<'a>,
    parts: Vec<&'a [u8]>,
}

impl<'a> Plan<'a> {
    fn leaf(shape: Shape<'a>) -> Self {
        Self {
            shape,
            parts: Vec::new(),
        }
    }
}

fn trim_start(text: &[u8], matches: impl Fn(u8) -> bool) -> &[u8] {
    &text[text.iter().position(|&b| !matches(b)).unwrap_or(text.len())..]
}

fn trim_end(text: &[u8], matches: impl Fn(u8) -> bool) -> &[u8] {
    &text[..text.iter().rposition(|&b| !matches(b)).map_or(0, |i| i + 1)]
}

fn horizontal(b: u8) -> bool {
    matches!(b, b' ' | b'\t')
}

fn whitespace(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n')
}

// Each caller uses the same delimiter set as the C parser for that construction.
fn top_level<'a>(
    bytes: impl IntoIterator<Item = (usize, u8)> + 'a,
    open: &'a [u8],
    close: &'a [u8],
) -> impl Iterator<Item = (usize, u8)> + 'a {
    let mut depth = 0isize;
    bytes.into_iter().filter(move |&(_, b)| {
        if open.contains(&b) {
            depth += 1;
        } else if close.contains(&b) {
            depth -= 1;
        }
        depth == 0
    })
}

fn comma_parts<'a>(
    text: &'a [u8],
    open: &'a [u8],
    close: &'a [u8],
) -> impl Iterator<Item = &'a [u8]> + 'a {
    let bytes = text.iter().copied().chain(*b",").enumerate();
    let mut start = 0;
    top_level(bytes, open, close)
        .filter(|&(_, b)| b == b',')
        .map(move |(i, _)| {
            let part = &text[start..i];
            start = i + 1;
            part
        })
}

fn function_plan<'a>(text: &'a [u8], raw: &'a [u8]) -> Plan<'a> {
    let Some((close, b')')) =
        top_level(text.iter().copied().enumerate(), b"(", b")").next()
    else {
        return Plan::leaf(Shape::Unknown);
    };
    let after = trim_start(&text[close + 1..], horizontal);
    if !after.starts_with(b"=>") {
        return Plan::leaf(Shape::Unknown);
    }

    // Charge and parse the return before parsing the parameters, as in C.
    let mut parts = vec![&raw[text.len() - after.len() + 2..]];
    let mut names = Vec::new();
    for part in comma_parts(&text[1..close], b"(<[", b")>]")
        .filter(|part| !part.is_empty())
        .take(MAX_MEMBERS)
    {
        if let Some(colon) = part.iter().position(|&b| b == b':') {
            let name = trim_start(&part[..colon], horizontal);
            let name = trim_end(name, |b| matches!(b, b' ' | b'?'));
            names.push(Param::Explicit(name));
            parts.push(&part[colon + 1..]);
        } else {
            names.push(Param::Positional(names.len()));
            parts.push(part);
        }
    }
    Plan {
        shape: Shape::Function(names),
        parts,
    }
}

fn conditional_parts(text: &[u8]) -> Option<Vec<&[u8]>> {
    let extends = b" extends ";
    let question = b" ? ";
    let colon = b" : ";
    let mut spaces = top_level(text.iter().copied().enumerate(), b"<[(", b">])")
        .filter_map(|(i, b)| (i > 0 && b == b' ').then_some(i));
    let check_end = spaces.find(|&i| text[i..].starts_with(extends))?;
    let bound_start = check_end + extends.len();
    let bound_end =
        spaces.find(|&i| i >= bound_start && text[i..].starts_with(question))?;
    let yes_start = bound_end + question.len();
    let yes_end =
        spaces.find(|&i| i >= yes_start && text[i..].starts_with(colon))?;
    Some(vec![
        &text[..check_end],
        &text[bound_start..bound_end],
        &text[yes_start..yes_end],
        &text[yes_end + colon.len()..],
    ])
}

fn compound_plan(text: &[u8]) -> Option<Plan<'_>> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut union = false;
    for (i, b) in top_level(text.iter().copied().enumerate(), b"<[", b">]") {
        if matches!(b, b'|' | b'&') && i > 0 && horizontal(text[i - 1]) {
            if parts.len() < MAX_MEMBERS {
                parts.push(&text[start..i]);
            }
            union |= b == b'|';
            start = i + 1;
        }
    }
    if parts.is_empty() {
        return None;
    }
    if parts.len() < MAX_MEMBERS {
        parts.push(&text[start..]);
    }
    Some(Plan {
        shape: if union {
            Shape::Union
        } else {
            Shape::Intersection
        },
        parts,
    })
}

const BUILTINS: &[&[u8]] = &[
    b"string",
    b"number",
    b"boolean",
    b"bigint",
    b"any",
    b"unknown",
    b"void",
    b"never",
    b"null",
    b"undefined",
    b"object",
    b"symbol",
];

const STDLIB: &[&[u8]] = &[
    b"Array",
    b"Promise",
    b"Map",
    b"Set",
    b"WeakMap",
    b"WeakSet",
    b"ReadonlyArray",
    b"Iterable",
    b"Iterator",
    b"Generator",
    b"AsyncIterable",
    b"AsyncIterator",
    b"AsyncGenerator",
    b"Object",
    b"String",
    b"Number",
    b"Boolean",
    b"BigInt",
    b"Symbol",
    b"Function",
    b"Date",
    b"RegExp",
    b"Error",
    b"NodeList",
    b"HTMLCollection",
];

const STDLIB_GENERIC: &[&[u8]] = &[
    b"Partial",
    b"Required",
    b"Readonly",
    b"Pick",
    b"Omit",
    b"Record",
    b"Exclude",
    b"Extract",
    b"NonNullable",
    b"Parameters",
    b"ReturnType",
    b"Awaited",
    b"ThisType",
    b"InstanceType",
    b"ConstructorParameters",
    b"Uppercase",
    b"Lowercase",
    b"Capitalize",
    b"Uncapitalize",
];

const STDLIB_BARE: &[&[u8]] = &[
    b"Math",
    b"JSON",
    b"console",
    b"Element",
    b"HTMLElement",
    b"Document",
    b"Window",
    b"Node",
    b"EventTarget",
    b"Event",
    b"Response",
    b"Request",
    b"Headers",
    b"URL",
    b"URLSearchParams",
    b"FormData",
    b"Blob",
    b"File",
    b"JSX",
];

fn qualify<'a>(
    name: &'a [u8],
    module: Option<&'a [u8]>,
    extra_bare: &[&[u8]],
) -> Name<'a> {
    if let Some(module) = module
        && name.first().is_some_and(u8::is_ascii_uppercase)
        && !name.contains(&b'.')
        && !STDLIB.contains(&name)
        && !extra_bare.contains(&name)
    {
        Name::Qualified(module, name)
    } else {
        Name::Bare(name)
    }
}

fn plan<'a>(
    raw: &'a [u8],
    module: Option<&'a [u8]>,
    charge: &mut impl FnMut(usize) -> bool,
) -> Plan<'a> {
    let raw = trim_start(raw, whitespace);
    let raw = trim_start(raw.strip_prefix(b":").unwrap_or(raw), whitespace);
    let text = trim_end(raw, |b| whitespace(b) || matches!(b, b';' | b','));
    if text.is_empty() || !charge(text.len()) {
        return Plan::leaf(Shape::Unknown);
    }
    match text[0] {
        b'(' => return function_plan(text, raw),
        b'{' => return Plan::leaf(Shape::Unknown),
        _ if text == b"this" => return Plan::leaf(Shape::This),
        _ => {}
    }

    if let Some(parts) = conditional_parts(text) {
        return Plan {
            shape: Shape::Conditional,
            parts,
        };
    }

    // These operators borrow the untrimmed tail in the reference parser.
    if let Some(rest) = raw.strip_prefix(b"infer ").filter(|_| text.len() > 6) {
        let rest = trim_start(rest, horizontal);
        let end = rest
            .iter()
            .position(|&b| horizontal(b))
            .unwrap_or(rest.len());
        return Plan::leaf(Shape::Infer(&rest[..end]));
    }
    if let Some(rest) = raw.strip_prefix(b"keyof ").filter(|_| text.len() > 6) {
        return Plan {
            shape: Shape::Keyof,
            parts: vec![trim_start(rest, horizontal)],
        };
    }
    if let Some(rest) = raw.strip_prefix(b"typeof ").filter(|_| text.len() > 7) {
        return Plan::leaf(Shape::Typeof(trim_start(rest, horizontal)));
    }
    if text.len() >= 2 && text.starts_with(b"[") && text.ends_with(b"]") {
        return Plan {
            shape: Shape::Tuple,
            parts: comma_parts(&text[1..text.len() - 1], b"<[", b">]")
                .take(MAX_MEMBERS)
                .collect(),
        };
    }
    if let Some(plan) = compound_plan(text) {
        return plan;
    }
    if BUILTINS.contains(&text) {
        return Plan::leaf(Shape::Builtin(text));
    }
    if let Some(part) = text.strip_suffix(b"[]") {
        return Plan {
            shape: Shape::Template(Name::Bare(b"Array")),
            parts: vec![part],
        };
    }
    if text.len() > 2
        && text.ends_with(b">")
        && let Some((open, b'<')) =
            top_level(text.iter().copied().enumerate().rev(), b">", b"<").next()
        && open > 0
    {
        return Plan {
            shape: Shape::Template(qualify(
                &text[..open],
                module,
                STDLIB_GENERIC,
            )),
            parts: comma_parts(&text[open + 1..text.len() - 1], b"<", b">")
                .take(MAX_MEMBERS)
                .collect(),
        };
    }
    Plan::leaf(Shape::Named(qualify(text, module, STDLIB_BARE)))
}

// Called by ts_lsp.c in the linked Mori build. A negative value disables
// charging, including after the first overrun, as in the standalone parser.
fn charge_budget(budget: &mut c_long, len: usize) -> bool {
    if *budget < 0 {
        return true;
    }
    *budget -= 1 + (len / 16) as c_long;
    *budget >= 0
}

/// Visit children in the reference parser's order. Use a heap stack so deeply
/// nested generated types do not put one native stack frame per nesting level.
fn parse<T>(
    text: &[u8],
    module: Option<&[u8]>,
    mut charge: impl FnMut(usize) -> bool,
    mut emit: impl FnMut(Shape<'_>, &[T]) -> T,
) -> T {
    struct Frame<'a, T> {
        shape: Shape<'a>,
        pending: std::vec::IntoIter<&'a [u8]>,
        children: Vec<T>,
    }
    let mut stack: Vec<Frame<'_, T>> = Vec::new();
    let mut text = text;
    loop {
        let Plan { shape, parts } = plan(text, module, &mut charge);
        let mut parts = parts.into_iter();
        if let Some(first) = parts.next() {
            stack.push(Frame {
                shape,
                children: Vec::with_capacity(parts.len() + 1),
                pending: parts,
            });
            text = first;
            continue;
        }
        let mut result = emit(shape, &[]);
        loop {
            let Some(frame) = stack.last_mut() else {
                return result;
            };
            frame.children.push(result);
            if let Some(next) = frame.pending.next() {
                text = next;
                break;
            }
            let frame = stack.pop().expect("current frame");
            result = emit(frame.shape, &frame.children);
        }
    }
}

#[cfg(feature = "native-engine-abi")]
#[path = "ts_type_text_ffi.rs"]
mod ffi;

#[cfg(test)]
#[path = "ts_type_text_tests.rs"]
mod tests;
