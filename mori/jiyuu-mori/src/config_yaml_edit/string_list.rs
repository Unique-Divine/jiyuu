use super::*;
use std::ops::Range;

struct Item {
    raw: Range<usize>,
    line_start: usize,
    value: Vec<u8>,
}

#[derive(PartialEq, Eq)]
enum Kind {
    Scalar,
    Flow,
    Block,
}

struct List {
    kind: Kind,
    header: Line,
    section_end: usize,
    inline: Range<usize>,
    items: Vec<Item>,
}

fn push_item(
    items: &mut Vec<Item>,
    doc: &Document<'_>,
    start: usize,
    end: usize,
    line: Line,
) -> Result<()> {
    let start = skip_spaces(doc.data, start, end);
    let end = trim_spaces(doc.data, start, end);
    // The native 64-bit item has four size_t fields and a pointer.
    if items.len() == items.capacity() {
        let capacity = (items.len() * 2).max(8);
        require(capacity <= INPUT_MAX / (5 * size_of::<usize>()))?;
        items
            .try_reserve_exact(capacity - items.len())
            .map_err(|_| ())?;
    }
    items.push(Item {
        raw: start..end,
        line_start: line.start,
        value: decode(&doc.data[start..end])?,
    });
    Ok(())
}

fn parse_flow(list: &mut List, doc: &Document<'_>) -> Result<()> {
    let mut segment = list.inline.start + 1;
    let end = list.inline.end - 1;
    let mut scan = Scanner {
        pos: segment,
        quote: 0,
    };
    while scan.pos < end {
        let pos = scan.pos;
        match scan.next(doc.data, end, None)? {
            Some(b'[' | b']') => return Err(()),
            Some(b',') => {
                push_item(&mut list.items, doc, segment, pos, list.header)?;
                segment = pos + 1;
            }
            _ => {}
        }
    }
    require(scan.quote == 0)?;
    if skip_spaces(doc.data, segment, end) == end {
        require(list.items.is_empty())
    } else {
        push_item(&mut list.items, doc, segment, end, list.header)
    }
}

fn analyze(doc: &Document<'_>, key: &[u8]) -> Result<Option<List>> {
    let Some((index, colon)) = doc.find_key(0, doc.lines.len(), 0, key)? else {
        return Ok(None);
    };
    let header = doc.lines[index];
    let data = doc.data;
    check_indicators(data, colon + 1, header.text_end, false, false)?;
    let (value, start, end) = inline_value(data, colon, header.text_end)?;
    let (kind, end_line) = match value.first() {
        None => (Kind::Block, doc.section_end(index, doc.lines.len())),
        Some(b'[') => {
            require(value.len() >= 2 && value.ends_with(b"]"))?;
            (Kind::Flow, index + 1)
        }
        Some(_) => (Kind::Scalar, index + 1),
    };
    let mut list = List {
        kind,
        header,
        section_end: doc.offset(end_line),
        inline: start..end,
        items: Vec::new(),
    };
    match list.kind {
        Kind::Block => {
            for &line in &doc.lines[index + 1..end_line] {
                if line.blank || line.comment {
                    continue;
                }
                let dash = line.content();
                require(
                    line.indent == 2
                        && data[dash..line.text_end].starts_with(b"- "),
                )?;
                let start = skip_spaces(data, dash + 2, line.text_end);
                let comment = find_comment(data, start, line.text_end)?;
                check_indicators(data, start, comment, false, false)?;
                push_item(&mut list.items, doc, start, comment, line)?;
            }
        }
        Kind::Flow => parse_flow(&mut list, doc)?,
        Kind::Scalar => push_item(&mut list.items, doc, start, end, header)?,
    }
    Ok(Some(list))
}

fn replace_inline(
    doc: &Document<'_>,
    list: &List,
    item: &[u8],
    remove: bool,
) -> Edit {
    let mut replacement = Vec::new();
    append(
        &mut replacement,
        &doc.data[list.header.start..list.inline.start],
    )?;
    append(&mut replacement, b"[")?;
    let mut emitted = false;
    for old in &list.items {
        if remove && old.value == item {
            continue;
        }
        if emitted {
            append(&mut replacement, b", ")?;
        }
        append(&mut replacement, &doc.data[old.raw.clone()])?;
        emitted = true;
    }
    if !remove {
        if emitted {
            append(&mut replacement, b", ")?;
        }
        quote(&mut replacement, item)?;
    }
    append(&mut replacement, b"]")?;
    append(
        &mut replacement,
        &doc.data[list.inline.end..list.header.end],
    )?;
    doc.splice(list.header.start, list.header.end, &replacement)
}

fn item_line(out: &mut Vec<u8>, doc: &Document<'_>, item: &[u8]) -> Result<()> {
    append(out, b"  - ")?;
    quote(out, item)?;
    append(out, doc.eol)
}

fn transform(data: &[u8], key: &[u8], item: &[u8], remove: bool) -> Edit {
    let doc = Document::parse(data)?;
    let Some(list) = analyze(&doc, key)? else {
        if remove {
            return Ok(Change::Keep);
        }
        let mut out = doc.prefix(data.len())?;
        append(&mut out, key)?;
        append(&mut out, b":")?;
        append(&mut out, doc.eol)?;
        item_line(&mut out, &doc, item)?;
        return Ok(Change::Replace(out));
    };
    let count = list.items.iter().filter(|old| old.value == item).count();
    if remove {
        if count == 0 {
            return Ok(Change::Keep);
        }
        if list.kind != Kind::Block {
            return if count == list.items.len() {
                doc.splice(list.header.start, list.header.end, b"")
            } else {
                replace_inline(&doc, &list, item, true)
            };
        }
        let mut out = Vec::new();
        let mut items = list.items.iter().peekable();
        for line in &doc.lines {
            while items.peek().is_some_and(|old| old.line_start < line.start) {
                items.next();
            }
            let skip = (count == list.items.len()
                && line.start == list.header.start)
                || items.peek().is_some_and(|old| {
                    old.line_start == line.start && old.value == item
                });
            if !skip {
                append(&mut out, &data[line.start..line.end])?;
            }
        }
        return Ok(Change::Replace(out));
    }

    if count > 0 {
        return Ok(Change::Keep);
    }
    if list.kind != Kind::Block {
        return replace_inline(&doc, &list, item, false);
    }
    let mut out = doc.prefix(list.section_end)?;
    item_line(&mut out, &doc, item)?;
    append(&mut out, &data[list.section_end..])?;
    Ok(Change::Replace(out))
}

fn edit_item(
    path: &CStr,
    key: &[u8],
    item: &[u8],
    remove: bool,
) -> Result<c_int> {
    validate_key(key)?;
    validate_item(item)?;
    apply(path, |data| transform(data, key, item, remove))
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_yaml_upsert_string_list_item(
    path: *const c_char,
    key: *const c_char,
    item: *const c_char,
) -> c_int {
    edit(path, false, |path| unsafe {
        edit_item(
            path,
            argument(key, KEY_MAX)?,
            argument(item, ITEM_MAX)?,
            false,
        )
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_yaml_remove_string_list_item(
    path: *const c_char,
    key: *const c_char,
    item: *const c_char,
) -> c_int {
    edit(path, true, |path| unsafe {
        edit_item(
            path,
            argument(key, KEY_MAX)?,
            argument(item, ITEM_MAX)?,
            true,
        )
    })
}
