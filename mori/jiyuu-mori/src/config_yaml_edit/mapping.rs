use super::*;

#[derive(Clone, Copy)]
struct Section {
    header: Line,
    colon: usize,
    end: usize,
    inline_empty: bool,
    count: usize,
    entry: Option<(Line, usize)>,
}

fn analyze(
    doc: &Document<'_>,
    section_key: &[u8],
    entry_key: &[u8],
) -> Result<Option<Section>> {
    let Some((index, colon)) =
        doc.find_key(0, doc.lines.len(), 0, section_key)?
    else {
        return Ok(None);
    };
    let header = doc.lines[index];
    let end_line = doc.section_end(index, doc.lines.len());
    let mut section = Section {
        header,
        colon,
        end: doc.offset(end_line),
        inline_empty: false,
        count: 0,
        entry: None,
    };
    let data = doc.data;
    if inline_value(data, colon, header.text_end)?.0 == b"{}" {
        require(
            doc.lines[index + 1..end_line]
                .iter()
                .all(|l| l.blank || l.comment),
        )?;
        section.inline_empty = true;
        return Ok(Some(section));
    }
    check_indicators(data, colon + 1, header.text_end, false, false)?;
    require(tail_empty(data, colon, header.text_end))?;

    let mut entry_index = None;
    for i in index + 1..end_line {
        let line = doc.lines[i];
        if line.ignored() {
            continue;
        }
        require(line.indent >= 2 && line.indent.is_multiple_of(2))?;
        check_body_line(data, line, false)?;
        let colon = mapping_colon(data, line.content(), line.text_end);
        if line.indent == 2 {
            require(!multiline(data, colon?, line.text_end))?;
            section.count += 1;
            if let Some(colon) = doc.match_key(line, 2, entry_key)? {
                require(
                    entry_index.is_none()
                        && tail_empty(data, colon, line.text_end),
                )?;
                entry_index = Some(i);
            }
        } else {
            require(
                section.count > 0
                    && !colon.is_ok_and(|c| multiline(data, c, line.text_end)),
            )?;
        }
    }
    if let Some(i) = entry_index {
        let mut boundary = None;
        let mut end = section.end;
        for line in &doc.lines[i + 1..end_line] {
            if line.blank || (line.comment && line.indent <= 2) {
                boundary.get_or_insert(line.start);
                continue;
            }
            if line.indent <= 2 {
                end = line.start;
                break;
            }
            boundary = None;
        }
        section.entry = Some((doc.lines[i], boundary.unwrap_or(end)));
    }
    Ok(Some(section))
}

fn validate_block(block: &[u8]) -> Result<()> {
    validate_bytes(block, false)?;
    for raw in block.split_inclusive(|&b| b == b'\n') {
        let text = line_text(raw);
        let indent = skip_spaces(text, 0, text.len());
        if indent == text.len() {
            continue;
        }
        require(indent >= 4 && indent.is_multiple_of(2))?;
        if text[indent] == b'#' {
            continue;
        }
        require(find_comment(text, indent, text.len())? == text.len())?;
        check_indicators(text, indent, text.len(), false, false)?;
        require(
            !mapping_colon(text, indent, text.len())
                .is_ok_and(|c| multiline(text, c, text.len())),
        )?;
    }
    Ok(())
}

fn render_entry(
    doc: &Document<'_>,
    header: Option<Line>,
    key: &[u8],
    block: &[u8],
) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    if let Some(line) = header {
        append(&mut out, &doc.data[line.start..line.end])?;
        if line.end == line.text_end {
            append(&mut out, doc.eol)?;
        }
    } else {
        append(&mut out, b"  ")?;
        append(&mut out, key)?;
        append(&mut out, b":")?;
        append(&mut out, doc.eol)?;
    }
    for raw in block.split_inclusive(|&b| b == b'\n') {
        append(&mut out, line_text(raw))?;
        append(&mut out, doc.eol)?;
    }
    Ok(out)
}

fn our_binary(value: &[u8]) -> bool {
    let value = value
        .strip_prefix(b"\"")
        .and_then(|v| v.strip_suffix(b"\""))
        .unwrap_or(value);
    matches!(
        value.rsplit(|b| matches!(b, b'/' | b'\\')).next(),
        Some(b"codebase-memory-mcp" | b"codebase-memory-mcp.exe")
    )
}

fn prior_shape(data: &[u8], key: &[u8]) -> bool {
    let mut lines = data
        .split(|&b| b == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
        .filter(|line| !line.is_empty());
    if lines
        .next()
        .and_then(|line| line.strip_prefix(b"  "))
        .and_then(|line| line.strip_suffix(b":"))
        != Some(key)
    {
        return false;
    }
    // Recognize the old unquoted stdio command and the original Goose fields.
    let mut body = lines.next();
    if let Some(value) =
        body.and_then(|line| line.strip_prefix(b"    command: "))
    {
        return lines.next().is_none() && our_binary(value);
    }
    if body == Some(b"    name: codebase-memory-mcp") {
        body = lines.next();
    }
    body == Some(b"    type: stdio")
        && lines
            .next()
            .and_then(|line| line.strip_prefix(b"    cmd: "))
            .is_some_and(our_binary)
        && lines.next() == Some(b"    args: []")
        && lines.next() == Some(b"    enabled: true")
        && lines.next().is_none()
}

fn transform(
    data: &[u8],
    section_key: &[u8],
    key: &[u8],
    block: Option<&[u8]>,
    owned: bool,
    remove: bool,
) -> Edit {
    let doc = Document::parse(data)?;
    let section = analyze(&doc, section_key, key)?;
    let existing = section.as_ref().and_then(|s| s.entry);
    let rendered = if let Some(block) = block {
        let header = if owned {
            None
        } else {
            existing.map(|(line, _)| line)
        };
        render_entry(&doc, header, key, block)?
    } else {
        Vec::new()
    };

    if let Some((line, end)) = existing {
        if owned {
            let old = data.get(line.start..end).ok_or(())?;
            if old == rendered {
                if !remove {
                    return Ok(Change::Keep);
                }
            } else if !prior_shape(old, key) {
                return Ok(Change::Foreign);
            }
        }
        if !remove {
            return doc.splice(line.start, end, &rendered);
        }
        let section = section.ok_or(())?;
        if section.count != 1 {
            return doc.splice(line.start, end, b"");
        }
        require(line.start >= section.header.end && end >= line.end)?;
        let mut out = doc.rewrite_empty_mapping_header(
            section.header,
            section.colon,
            true,
        )?;
        append(&mut out, &data[section.header.end..line.start])?;
        append(&mut out, &data[end..])?;
        return Ok(Change::Replace(out));
    }
    if remove {
        return Ok(Change::Keep);
    }

    let mut out = match section {
        Some(section) if section.inline_empty => {
            let mut out = doc.rewrite_empty_mapping_header(
                section.header,
                section.colon,
                false,
            )?;
            append(&mut out, &data[section.header.end..section.end])?;
            doc.separator(&mut out)?;
            out
        }
        Some(section) => doc.prefix(section.end)?,
        None => {
            let mut out = doc.prefix(data.len())?;
            append(&mut out, section_key)?;
            append(&mut out, b":")?;
            append(&mut out, doc.eol)?;
            out
        }
    };
    append(&mut out, &rendered)?;
    if let Some(section) = section {
        append(&mut out, &data[section.end..])?;
    }
    Ok(Change::Replace(out))
}

fn edit_entry(
    path: &CStr,
    section_key: &[u8],
    key: &[u8],
    block: Option<&[u8]>,
    owned: bool,
    remove: bool,
) -> Result<c_int> {
    validate_key(section_key)?;
    validate_key(key)?;
    if let Some(block) = block {
        validate_block(block)?;
    }

    apply(path, |data| {
        transform(data, section_key, key, block, owned, remove)
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_yaml_upsert_mapping_entry(
    path: *const c_char,
    section: *const c_char,
    key: *const c_char,
    block: *const c_char,
) -> c_int {
    edit(path, false, |path| unsafe {
        edit_entry(
            path,
            argument(section, KEY_MAX)?,
            argument(key, KEY_MAX)?,
            Some(argument(block, BLOCK_MAX)?),
            false,
            false,
        )
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_yaml_remove_mapping_entry(
    path: *const c_char,
    section: *const c_char,
    key: *const c_char,
) -> c_int {
    edit(path, true, |path| unsafe {
        edit_entry(
            path,
            argument(section, KEY_MAX)?,
            argument(key, KEY_MAX)?,
            None,
            false,
            true,
        )
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_yaml_upsert_owned_mapping_entry(
    path: *const c_char,
    section: *const c_char,
    key: *const c_char,
    block: *const c_char,
) -> c_int {
    edit(path, false, |path| unsafe {
        edit_entry(
            path,
            argument(section, KEY_MAX)?,
            argument(key, KEY_MAX)?,
            Some(argument(block, BLOCK_MAX)?),
            true,
            false,
        )
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_yaml_remove_owned_mapping_entry(
    path: *const c_char,
    section: *const c_char,
    key: *const c_char,
    block: *const c_char,
) -> c_int {
    edit(path, true, |path| unsafe {
        edit_entry(
            path,
            argument(section, KEY_MAX)?,
            argument(key, KEY_MAX)?,
            Some(argument(block, BLOCK_MAX)?),
            true,
            true,
        )
    })
}
