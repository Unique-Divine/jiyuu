use std::ops::Range;

use super::parser::{Member, Scan, scan, validate_document};
use super::{Invalid, MAX_BYTES, Result};

pub(super) enum Edit<'a> {
    Upsert(&'a [u8]),
    ReplaceField { key: &'a [u8], raw: &'a [u8] },
    Remove,
    AddString(&'a [u8]),
    RemoveString(&'a [u8]),
}

pub(super) fn unique(scan: Scan) -> Result<Option<Member>> {
    if scan.matches > 1 {
        return Err(Invalid);
    }
    Ok(scan.hit)
}

pub(super) fn lookup(
    source: &[u8],
    path: &[&[u8]],
    key: &[u8],
) -> Result<Option<Range<usize>>> {
    if source.is_empty() {
        return Ok(None);
    }
    let mut start = validate_document(source)?;
    for component in path {
        let Some(hit) = unique(scan(source, start, b'{', Some(component))?)?
        else {
            return Ok(None);
        };
        start = hit.value.start;
    }
    Ok(unique(scan(source, start, b'{', Some(key))?)?.map(|hit| hit.value))
}

pub(super) fn update(
    source: &[u8],
    path: &[&[u8]],
    key: &[u8],
    operation: Edit<'_>,
) -> Result<Option<Vec<u8>>> {
    if source.is_empty() {
        return missing(&operation, |raw| {
            let mut fresh = b"{\n".to_vec();
            let style = Style {
                base: b"",
                unit: b"  ",
                close_indent_only: true,
                newline: b"\n",
            };
            fresh.extend_from_slice(style.unit);
            fresh.extend_from_slice(&build_member(path, key, raw, &style)?);
            fresh.extend_from_slice(b"\n}\n");
            Ok(fresh)
        });
    }

    let mut start = validate_document(source)?;
    for (index, component) in path.iter().enumerate() {
        let parent = scan(source, start, b'{', Some(component))?;
        if parent.matches > 1 {
            return Err(Invalid);
        }
        let Some(hit) = parent.hit.as_ref() else {
            return missing(&operation, |raw| {
                insert_member(source, start, &parent, &path[index..], key, raw)
            });
        };
        start = hit.value.start;
    }

    let object = scan(source, start, b'{', Some(key))?;
    if object.matches > 1 {
        return Err(Invalid);
    }
    let Some(hit) = object.hit.as_ref() else {
        return missing(&operation, |raw| {
            insert_member(source, start, &object, &[], key, raw)
        });
    };

    let result = match operation {
        Edit::Upsert(raw) => {
            if &source[hit.value.clone()] == raw {
                return Ok(None);
            }
            replace(source, hit.value.clone(), raw)?
        }
        Edit::ReplaceField { key, raw } => {
            let field = unique(scan(source, hit.value.start, b'{', Some(key))?)?
                .ok_or(Invalid)?;
            replace(source, field.value, raw)?
        }
        Edit::Remove => remove(source, hit)?,
        Edit::AddString(value) if &source[hit.value.clone()] == b"null" => {
            replace(source, hit.value.clone(), &quoted_array(value)?)?
        }
        Edit::AddString(value) | Edit::RemoveString(value) => {
            let array = scan(source, hit.value.start, b'[', Some(value))?;
            if array.matches > 1 {
                return Err(Invalid);
            }
            match (operation, array.hit.as_ref()) {
                (Edit::AddString(_), None) => insert(
                    source,
                    hit.value.start,
                    &array,
                    &quote(value)?,
                    &style(source, &array),
                )?,
                (Edit::RemoveString(_), Some(hit)) => remove(source, hit)?,
                _ => return Ok(None),
            }
        }
    };
    Ok(Some(result))
}

fn missing(
    operation: &Edit<'_>,
    write: impl FnOnce(&[u8]) -> Result<Vec<u8>>,
) -> Result<Option<Vec<u8>>> {
    match operation {
        Edit::Upsert(raw) => write(raw).map(Some),
        Edit::AddString(value) => write(&quoted_array(value)?).map(Some),
        Edit::Remove | Edit::RemoveString(_) => Ok(None),
        Edit::ReplaceField { .. } => Err(Invalid),
    }
}

fn quoted_array(value: &[u8]) -> Result<Vec<u8>> {
    let mut raw = quote(value)?;
    raw.insert(0, b'[');
    raw.push(b']');
    Ok(raw)
}

fn quote(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(bytes.len() + 2);
    out.push(b'"');
    for &byte in bytes {
        match byte {
            b'"' | b'\\' => out.extend_from_slice(&[b'\\', byte]),
            8 => out.extend_from_slice(b"\\b"),
            12 => out.extend_from_slice(b"\\f"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\t' => out.extend_from_slice(b"\\t"),
            0..=31 => {
                let hex = b"0123456789abcdef";
                out.extend_from_slice(&[
                    b'\\',
                    b'u',
                    b'0',
                    b'0',
                    hex[(byte >> 4) as usize],
                    hex[(byte & 15) as usize],
                ]);
            }
            _ => out.push(byte),
        }
        if out.len() >= MAX_BYTES {
            return Err(Invalid);
        }
    }
    out.push(b'"');
    Ok(out)
}

struct Style<'a> {
    base: &'a [u8],
    unit: &'a [u8],
    close_indent_only: bool,
    newline: &'a [u8],
}

fn is_line_end(byte: u8) -> bool {
    matches!(byte, b'\r' | b'\n')
}

fn is_indent(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t')
}

fn line_indent(source: &[u8], pos: usize) -> Option<&[u8]> {
    let start = source[..pos]
        .iter()
        .rposition(|&byte| is_line_end(byte))
        .map_or(0, |i| i + 1);
    let indent = &source[start..pos];
    indent.iter().all(|&byte| is_indent(byte)).then_some(indent)
}

fn style<'a>(source: &'a [u8], scan: &Scan) -> Style<'a> {
    let close = line_indent(source, scan.close);
    let base = close.unwrap_or_default();
    let indent = match scan.first {
        None => base,
        Some(first) => line_indent(source, first).unwrap_or_default(),
    };
    let unit = match scan
        .first
        .and_then(|_| indent.strip_prefix(base))
        .filter(|unit| !unit.is_empty())
    {
        Some(unit) => unit,
        None if indent.ends_with(b"\t") => b"\t",
        None => b"  ",
    };
    let newline = match source.iter().position(|&byte| is_line_end(byte)) {
        Some(i) if source[i..].starts_with(b"\r\n") => b"\r\n".as_slice(),
        _ => b"\n",
    };
    Style {
        base,
        unit,
        close_indent_only: close.is_some(),
        newline,
    }
}

fn extend(out: &mut Vec<u8>, bytes: &[u8]) -> Result<()> {
    if out.len() + bytes.len() > MAX_BYTES {
        return Err(Invalid);
    }
    out.extend_from_slice(bytes);
    Ok(())
}

fn indent(out: &mut Vec<u8>, style: &Style<'_>, levels: usize) -> Result<()> {
    extend(out, style.base)?;
    for _ in 0..levels {
        extend(out, style.unit)?;
    }
    Ok(())
}

fn build_member(
    path: &[&[u8]],
    key: &[u8],
    value: &[u8],
    style: &Style<'_>,
) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for (index, component) in path.iter().enumerate() {
        if index != 0 {
            indent(&mut out, style, index + 1)?;
        }
        extend(&mut out, &quote(component)?)?;
        extend(&mut out, b": {")?;
        extend(&mut out, style.newline)?;
    }
    if !path.is_empty() {
        indent(&mut out, style, path.len() + 1)?;
    }
    extend(&mut out, &quote(key)?)?;
    extend(&mut out, b": ")?;
    extend(&mut out, value)?;
    for level in (1..=path.len()).rev() {
        extend(&mut out, style.newline)?;
        indent(&mut out, style, level)?;
        extend(&mut out, b"}")?;
    }
    Ok(out)
}

fn insert_member(
    source: &[u8],
    start: usize,
    object: &Scan,
    path: &[&[u8]],
    key: &[u8],
    raw: &[u8],
) -> Result<Vec<u8>> {
    let style = style(source, object);
    let member = build_member(path, key, raw, &style)?;
    insert(source, start, object, &member, &style)
}

fn insert(
    source: &[u8],
    start: usize,
    scan: &Scan,
    member: &[u8],
    style: &Style<'_>,
) -> Result<Vec<u8>> {
    let gap = if scan.count == 0 {
        start + 1
    } else {
        scan.last_comma.map_or(scan.last_end, |comma| comma + 1)
    };
    let tight = gap == scan.close;
    let mut insertion = Vec::new();
    if tight && scan.count == 0 {
        extend(&mut insertion, style.newline)?;
        indent(&mut insertion, style, 1)?;
        extend(&mut insertion, member)?;
        extend(&mut insertion, style.newline)?;
        extend(&mut insertion, style.base)?;
    } else if source[gap..scan.close]
        .iter()
        .any(|&byte| is_line_end(byte))
    {
        if !style.close_indent_only {
            extend(&mut insertion, style.newline)?;
        }
        extend(&mut insertion, style.unit)?;
        extend(&mut insertion, member)?;
        if scan.last_comma.is_some() {
            extend(&mut insertion, b",")?;
        }
        extend(&mut insertion, style.newline)?;
        extend(&mut insertion, style.base)?;
    } else {
        if tight
            || !matches!(
                source[scan.close - 1],
                b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c
            )
        {
            extend(&mut insertion, b" ")?;
        }
        extend(&mut insertion, member)?;
        if scan.last_comma.is_some() {
            extend(&mut insertion, b",")?;
        }
        if !tight {
            extend(&mut insertion, b" ")?;
        }
    }

    let close = (scan.close..scan.close, insertion.as_slice());
    if scan.count != 0 && scan.last_comma.is_none() {
        splice(source, &[(scan.last_end..scan.last_end, b","), close])
    } else {
        splice(source, &[close])
    }
}

fn leading_whitespace(source: &[u8], token: usize) -> usize {
    let mut start = token;
    while start > 0 && is_indent(source[start - 1]) {
        start -= 1;
    }
    let mut newline = start;
    if source[..newline].ends_with(b"\r\n") {
        newline -= 2;
    } else if newline > 0 && is_line_end(source[newline - 1]) {
        newline -= 1;
    }
    if newline < start
        && newline > 0
        && matches!(source[newline - 1], b'{' | b'[' | b',')
    {
        return newline;
    }
    start
}

fn remove(source: &[u8], member: &Member) -> Result<Vec<u8>> {
    let owned = (
        leading_whitespace(source, member.start())..member.value.end,
        b"".as_slice(),
    );
    if let Some(comma) = member.comma {
        splice(source, &[owned, (comma..comma + 1, b"")])
    } else if let Some(comma) = member.previous_comma {
        splice(source, &[(comma..comma + 1, b""), owned])
    } else {
        splice(source, &[owned])
    }
}

fn replace(source: &[u8], range: Range<usize>, raw: &[u8]) -> Result<Vec<u8>> {
    splice(source, &[(range, raw)])
}

fn splice(source: &[u8], edits: &[(Range<usize>, &[u8])]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut cursor = 0;
    for (range, replacement) in edits {
        extend(&mut out, &source[cursor..range.start])?;
        extend(&mut out, replacement)?;
        cursor = range.end;
    }
    extend(&mut out, &source[cursor..])?;
    Ok(out)
}
