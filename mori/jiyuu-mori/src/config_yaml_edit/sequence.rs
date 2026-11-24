use super::*;
use std::{collections::HashSet, ops::Range};

struct Identity<'a> {
    key: &'a [u8],
    value: &'a [u8],
}

struct Target {
    missing_depth: usize,
    insert: usize,
    items: usize,
    identity: Option<Range<usize>>,
    empty_parent: Option<(Line, usize)>,
}

#[derive(Default)]
struct FieldKeys {
    keys: HashSet<Vec<u8>>,
}

impl FieldKeys {
    fn add(&mut self, key: Vec<u8>) -> Result<()> {
        // The reference doubles an array of pointers and caps it at INPUT_MAX.
        require(self.keys.len() < INPUT_MAX / size_of::<usize>())?;
        self.keys.try_reserve(1).map_err(|_| ())?;
        require(self.keys.insert(key))
    }
}

fn field_key(data: &[u8], start: usize, end: usize) -> Result<(Vec<u8>, usize)> {
    let colon = mapping_colon(data, start, end)?;
    let key = decode(&data[start..colon])?;
    require(key != b"<<")?;
    Ok((key, colon))
}

fn validate_mapping(
    doc: &Document<'_>,
    begin: usize,
    end: usize,
    indent: usize,
) -> Result<()> {
    let mut have_key = false;
    let mut key_open = false;
    let mut keys = FieldKeys::default();
    for &line in &doc.lines[begin..end] {
        if line.ignored() {
            continue;
        }
        require(line.indent >= indent)?;
        if line.indent > indent {
            require(have_key)?;
            continue;
        }
        let text = &doc.data[line.content()..line.text_end];
        require(!text.is_empty())?;
        if text[0] == b'-' {
            require(sequence_dash(text) && key_open)?;
            continue;
        }
        let (key, colon) = field_key(doc.data, line.content(), line.text_end)?;
        keys.add(key)?;
        have_key = true;
        key_open = tail_empty(doc.data, colon, line.text_end);
    }
    Ok(())
}

fn parse_item(
    doc: &Document<'_>,
    begin: usize,
    end: usize,
    indent: usize,
    identity: &Identity<'_>,
) -> Result<bool> {
    let mut keys = FieldKeys::default();
    let mut field_found = false;
    let mut identity_match = false;
    for (offset, &line) in doc.lines[begin..end].iter().enumerate() {
        if line.ignored() {
            continue;
        }
        let start = if offset == 0 {
            require(
                line.indent == indent
                    && doc.data[line.content()..line.text_end]
                        .starts_with(b"- "),
            )?;
            line.content() + 2
        } else if line.indent == indent + 2 {
            line.content()
        } else {
            require(line.indent > indent + 2 && field_found)?;
            continue;
        };
        let (key, colon) = field_key(doc.data, start, line.text_end)?;
        let is_identity = key == identity.key;
        keys.add(key)?;
        let (raw, _, _) = inline_value(doc.data, colon, line.text_end)?;
        if raw.is_empty() {
            require(!is_identity)?;
        } else {
            require(!multiline(doc.data, colon, line.text_end))?;
            let value = decode(raw)?;
            if is_identity {
                identity_match = value == identity.value;
            }
        }
        field_found = true;
    }
    require(field_found)?;
    Ok(identity_match)
}

fn parse_block(
    doc: &Document<'_>,
    begin: usize,
    end: usize,
    indent: usize,
    identity: &Identity<'_>,
) -> Result<(usize, Option<Range<usize>>)> {
    let mut starts = Vec::new();
    starts.try_reserve_exact(end - begin).map_err(|_| ())?;
    for i in begin..end {
        let line = doc.lines[i];
        if line.ignored() {
            continue;
        }
        if line.indent == indent {
            require(doc.data[line.content()..line.text_end].starts_with(b"- "))?;
            starts.push(i);
        } else {
            require(line.indent > indent && !starts.is_empty())?;
        }
    }
    let mut found = None;
    for (index, &start) in starts.iter().enumerate() {
        let next = starts.get(index + 1).copied().unwrap_or(end);
        if parse_item(doc, start, next, indent, identity)? {
            require(found.is_none())?;
            found = Some(doc.offset(start)..doc.offset(next));
        }
    }
    Ok((starts.len(), found))
}

fn analyze(
    doc: &Document<'_>,
    path: &[&[u8]],
    identity: &Identity<'_>,
) -> Result<Target> {
    let line_count = doc.lines.len();
    // Other top-level values are opaque, but every path mapping must have
    // unique decoded keys. Validate the target section's indicators first.
    if let Some((root, _)) = doc.find_key(0, line_count, 0, path[0])? {
        for &line in &doc.lines[root..doc.section_end(root, line_count)] {
            if !line.ignored() {
                require(line.indent.is_multiple_of(2))?;
                check_body_line(doc.data, line, true)?;
            }
        }
    }
    let mut begin = 0;
    let mut end = line_count;
    for (depth, key) in path.iter().enumerate() {
        let indent = depth * 2;
        validate_mapping(doc, begin, end, indent)?;
        let Some((index, colon)) = doc.find_key(begin, end, indent, key)? else {
            return Ok(Target {
                missing_depth: depth,
                insert: doc.offset(end),
                items: 0,
                identity: None,
                empty_parent: None,
            });
        };
        let header = doc.lines[index];
        let child_end = doc.section_end(index, end);
        let insert = doc.offset(child_end);
        let last = depth + 1 == path.len();
        if !tail_empty(doc.data, colon, header.text_end) {
            require(
                !last
                    && inline_value(doc.data, colon, header.text_end)?.0
                        == b"{}",
            )?;
            require(
                doc.lines[index + 1..child_end]
                    .iter()
                    .all(|l| l.blank || l.comment),
            )?;
            return Ok(Target {
                missing_depth: depth + 1,
                insert,
                items: 0,
                identity: None,
                empty_parent: Some((header, colon)),
            });
        }
        if last {
            let (items, identity) =
                parse_block(doc, index + 1, child_end, indent + 2, identity)?;
            return Ok(Target {
                missing_depth: path.len(),
                insert,
                items,
                identity,
                empty_parent: None,
            });
        }
        begin = index + 1;
        end = child_end;
    }
    Err(())
}

const SPACES: &[u8] = b"                                ";

fn render(canonical: &[u8], indent: usize, eol: &[u8]) -> Result<Vec<u8>> {
    require(!canonical.is_empty())?;
    validate_bytes(canonical, false)?;
    let mut rendered = Vec::new();
    for (i, raw) in canonical.split_inclusive(|&b| b == b'\n').enumerate() {
        let text = line_text(raw);
        let width = skip_spaces(text, 0, text.len());
        require(if i == 0 {
            text.starts_with(b"- ")
        } else {
            width == 2 && width < text.len()
        })?;
        append(&mut rendered, &SPACES[..indent])?;
        append(&mut rendered, text)?;
        append(&mut rendered, eol)?;
    }
    Ok(rendered)
}

fn validate_canonical(canonical: &[u8], identity: &Identity<'_>) -> Result<()> {
    let rendered = render(canonical, 4, b"\n")?;
    let mut wrapped = Vec::new();
    append(&mut wrapped, b"root:\n  sequence:\n")?;
    let item_start = wrapped.len();
    append(&mut wrapped, &rendered)?;
    let doc = Document::parse(&wrapped)?;
    let target = analyze(&doc, &[b"root", b"sequence"], identity)?;
    require(
        target.missing_depth == 2
            && target.items == 1
            && target.identity == Some(item_start..wrapped.len()),
    )
}

fn transform(
    data: &[u8],
    path: &[&[u8]],
    identity: &Identity<'_>,
    canonical: &[u8],
    remove: bool,
) -> Edit {
    let doc = Document::parse(data)?;
    let target = analyze(&doc, path, identity)?;
    let rendered = render(canonical, path.len() * 2, doc.eol)?;
    if let Some(range) = target.identity {
        if data[range.clone()] != rendered {
            return Ok(Change::Foreign);
        }
        return if remove {
            doc.splice(range.start, range.end, b"")
        } else {
            Ok(Change::Keep)
        };
    }
    if remove {
        return Ok(Change::Keep);
    }
    let mut out = if let Some((parent, colon)) = target.empty_parent {
        let mut out = doc.rewrite_empty_mapping_header(parent, colon, false)?;
        append(&mut out, &data[parent.end..target.insert])?;
        doc.separator(&mut out)?;
        out
    } else {
        doc.prefix(target.insert)?
    };
    for (depth, part) in path.iter().enumerate().skip(target.missing_depth) {
        append(&mut out, &SPACES[..depth * 2])?;
        append(&mut out, part)?;
        append(&mut out, b":")?;
        append(&mut out, doc.eol)?;
    }
    append(&mut out, &rendered)?;
    append(&mut out, &data[target.insert..])?;
    Ok(Change::Replace(out))
}

fn edit_item(
    file: &CStr,
    path: &[&[u8]],
    key: &[u8],
    scalar: &[u8],
    canonical: &[u8],
    remove: bool,
) -> Result<c_int> {
    validate_key(key)?;
    validate_bytes(scalar, false)?;
    for part in path {
        validate_key(part)?;
    }
    let value = decode(scalar)?;
    let identity = Identity { key, value: &value };
    validate_canonical(canonical, &identity)?;

    apply(file, |data| {
        transform(data, path, &identity, canonical, remove)
    })
}

unsafe fn sequence_args<'a>(
    path: *const *const c_char,
    len: usize,
) -> Result<[&'a [u8]; PATH_DEPTH_MAX]> {
    require(!path.is_null() && (1..=PATH_DEPTH_MAX).contains(&len))?;
    let mut parts: [&[u8]; PATH_DEPTH_MAX] = [&[]; PATH_DEPTH_MAX];
    for (part, &ptr) in parts
        .iter_mut()
        .zip(unsafe { slice::from_raw_parts(path, len) })
    {
        *part = unsafe { argument(ptr, KEY_MAX) }?;
    }
    Ok(parts)
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_yaml_upsert_mapping_sequence_item(
    path: *const c_char,
    sequence: *const *const c_char,
    len: usize,
    key: *const c_char,
    scalar: *const c_char,
    canonical: *const c_char,
) -> c_int {
    edit(path, false, |path| unsafe {
        let parts = sequence_args(sequence, len)?;
        edit_item(
            path,
            &parts[..len],
            argument(key, KEY_MAX)?,
            argument(scalar, ITEM_MAX)?,
            argument(canonical, BLOCK_MAX)?,
            false,
        )
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_yaml_remove_mapping_sequence_item(
    path: *const c_char,
    sequence: *const *const c_char,
    len: usize,
    key: *const c_char,
    scalar: *const c_char,
    canonical: *const c_char,
) -> c_int {
    edit(path, true, |path| unsafe {
        let parts = sequence_args(sequence, len)?;
        edit_item(
            path,
            &parts[..len],
            argument(key, KEY_MAX)?,
            argument(scalar, ITEM_MAX)?,
            argument(canonical, BLOCK_MAX)?,
            true,
        )
    })
}
