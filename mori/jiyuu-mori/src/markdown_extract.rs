//! Section identities, bodies, and reference ownership from the native Markdown tree.

use std::collections::hash_map::Entry;
use std::ffi::{CStr, c_char, c_void};
use std::ptr;

use crate::native_tree::{RawNode, TreeCursor};

mod scratch;
use scratch::{Buffer, Exhausted, Map, Scratch, ScratchResult};

type EmitSection = unsafe extern "C" fn(
    *mut c_void,
    *const RawNode,
    *const c_char,
    usize,
    *const c_char,
    usize,
    *const c_char,
    usize,
);
type DiagramLabels = unsafe extern "C" fn(
    *mut c_void,
    *const RawNode,
    *mut *const c_char,
) -> bool;

#[repr(C)]
#[derive(Default)]
struct Work {
    collection_steps: u64,
    name_checks: u64,
}

struct Heading<'a> {
    segment: Buffer<'a, u8>,
    parent: Option<usize>,
}

const LEVELS: usize = 6;

#[derive(Clone, Copy)]
enum HeadingKind {
    Atx,
    Setext,
}

unsafe fn heading_kind(node: RawNode) -> Option<HeadingKind> {
    // SAFETY: The node borrows the caller's live tree.
    unsafe {
        if node.is(c"atx_heading") {
            Some(HeadingKind::Atx)
        } else if node.is(c"setext_heading") {
            Some(HeadingKind::Setext)
        } else {
            None
        }
    }
}

fn prose_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
}

fn qn_segment<'a>(
    name: &[u8],
    scratch: &'a Scratch,
) -> ScratchResult<Buffer<'a, u8>> {
    let mut segment = Buffer::exact(scratch, name.len())?;
    let mut in_space = false;
    for &byte in name {
        if prose_space(byte) {
            if !in_space && !segment.is_empty() {
                segment.push(b'-')?;
            }
            in_space = true;
        } else {
            segment.push(byte)?;
            in_space = false;
        }
    }
    Ok(segment)
}

unsafe fn setext<'a>(
    node: RawNode,
    source: &[u8],
    scratch: &'a Scratch,
) -> ScratchResult<Buffer<'a, u8>> {
    // SAFETY: The caller keeps the tree live while the cursor borrows it.
    unsafe {
        let Some(paragraph) = node.field(c"heading_content") else {
            return Ok(Buffer::new(scratch));
        };
        let content = paragraph.child_of_kind(c"inline").unwrap_or(paragraph);
        let span = content.range();
        let mut cursor = TreeCursor::new(content);
        let mut previous = span.start;
        let mut text = Buffer::exact(scratch, span.len())?;
        loop {
            let current = cursor.node();
            if current.is(c"block_continuation") {
                let marker = current.range();
                let Some(piece) = source.get(previous..marker.start) else {
                    return Ok(Buffer::new(scratch));
                };
                text.extend(piece)?;
                previous = marker.end;
            } else if cursor.first_child() {
                continue;
            }
            while !cursor.next_sibling() {
                if !cursor.parent() {
                    let Some(piece) = source.get(previous..span.end) else {
                        return Ok(Buffer::new(scratch));
                    };
                    text.extend(piece)?;
                    return Ok(text);
                }
            }
        }
    }
}

unsafe fn atx(node: RawNode, source: &[u8]) -> Option<&[u8]> {
    // SAFETY: The caller keeps the tree live. The return borrows source only.
    unsafe {
        if let Some(child) = node
            .children()
            .find(|child| child.is(c"heading_content") || child.is(c"inline"))
        {
            return source.get(child.range());
        }
        let mut full = source.get(node.range())?;
        while let Some(rest) = full.strip_prefix(b"#") {
            full = rest;
        }
        while let Some(rest) = full.strip_prefix(b" ") {
            full = rest;
        }
        Some(full)
    }
}

enum HeadingName<'source, 'scratch> {
    Borrowed(&'source [u8]),
    Owned(Buffer<'scratch, u8>),
}

impl HeadingName<'_, '_> {
    fn as_bytes(&self) -> &[u8] {
        let bytes = match self {
            Self::Borrowed(bytes) => bytes,
            Self::Owned(bytes) => &**bytes,
        };
        let end = bytes
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(bytes.len());
        let bytes = &bytes[..end];
        let end = bytes
            .iter()
            .rposition(|&byte| !matches!(byte, b' ' | b'\r' | b'\n'));
        &bytes[..end.map_or(0, |index| index + 1)]
    }
}

unsafe fn heading_name<'source, 'scratch>(
    node: RawNode,
    kind: HeadingKind,
    source: &'source [u8],
    scratch: &'scratch Scratch,
) -> ScratchResult<HeadingName<'source, 'scratch>> {
    // SAFETY: The node and source were lent by C for this call.
    unsafe {
        match kind {
            HeadingKind::Atx => {
                Ok(HeadingName::Borrowed(atx(node, source).unwrap_or_default()))
            }
            HeadingKind::Setext => {
                Ok(HeadingName::Owned(setext(node, source, scratch)?))
            }
        }
    }
}

unsafe fn heading_level(node: RawNode) -> Option<usize> {
    let markers = [
        c"atx_h1_marker",
        c"atx_h2_marker",
        c"atx_h3_marker",
        c"atx_h4_marker",
        c"atx_h5_marker",
        c"atx_h6_marker",
    ];
    // SAFETY: All children borrow the caller's live tree.
    unsafe {
        for child in node.children() {
            if child.is(c"setext_h1_underline") {
                return Some(0);
            }
            if child.is(c"setext_h2_underline") {
                return Some(1);
            }
            if let Some(level) =
                markers.iter().position(|&marker| child.is(marker))
            {
                return Some(level);
            }
        }
    }
    None
}

// Both passes use the same cursor traversal and heading parser. The emission
// pass can stream bare headings even if building the file-wide table failed.
unsafe fn walk(
    root: RawNode,
    steps: &mut u64,
    mut visit: impl FnMut(
        &TreeCursor,
        RawNode,
        usize,
        Option<HeadingKind>,
    ) -> ScratchResult<()>,
) -> ScratchResult<()> {
    let mut depth = 0;
    // SAFETY: The tree outlives both the cursor and the visitor.
    unsafe {
        let mut cursor = TreeCursor::new(root);
        loop {
            *steps += 1;
            let node = cursor.node();
            let kind = heading_kind(node);
            visit(&cursor, node, depth, kind)?;
            if kind.is_none() {
                *steps += 1;
                if cursor.first_child() {
                    depth += 1;
                    continue;
                }
            }
            loop {
                *steps += 1;
                if cursor.next_sibling() {
                    break;
                }
                *steps += 1;
                if !cursor.parent() {
                    return Ok(());
                }
                depth -= 1;
            }
        }
    }
}

unsafe fn collect<'a>(
    root: RawNode,
    source: &[u8],
    scratch: &'a Scratch,
    work: &mut Work,
) -> ScratchResult<Buffer<'a, Heading<'a>>> {
    let mut headings = Buffer::new(scratch);
    let mut levels = Map::<_, [Option<usize>; LEVELS]>::new(scratch);
    let mut frames = Buffer::new(scratch);
    // SAFETY: All nodes and cursors borrow the caller's live tree.
    unsafe {
        walk(root, &mut work.collection_steps, |_, node, depth, kind| {
            frames.truncate(depth);
            let container = frames.last().copied().unwrap_or_default();
            if let Some(kind) = kind {
                let name = heading_name(node, kind, source, scratch)?;
                let segment = qn_segment(name.as_bytes(), scratch)?;
                let mut parent = None;
                if let Some(level) = heading_level(node) {
                    let ranks = levels.entry(container)?.or_default();
                    parent =
                        ranks[..level].iter().rev().flatten().next().copied();
                    // Empty headings still reset the hierarchy.
                    ranks[level] = Some(headings.len());
                    ranks[level + 1..].fill(None);
                }
                headings.push(Heading { segment, parent })?;
            } else {
                let container = if node.is(c"section") {
                    container
                } else {
                    let span = node.range();
                    (span.start, span.end)
                };
                frames.push(container)?;
            }
            Ok(())
        })?;
    }
    Ok(headings)
}

fn segments<'a>(
    index: usize,
    headings: &'a [Heading<'_>],
) -> ([&'a [u8]; LEVELS], usize) {
    let mut chain = [b"".as_slice(); LEVELS];
    let mut start = LEVELS;
    let mut current = Some(index);
    while let Some(index) = current {
        let heading = &headings[index];
        if !heading.segment.is_empty() {
            start -= 1;
            chain[start] = &heading.segment;
        }
        current = heading.parent;
    }
    (chain, start)
}

fn join_chain<'a>(
    chain: &[&[u8]],
    separator: u8,
    scratch: &'a Scratch,
) -> ScratchResult<Buffer<'a, u8>> {
    let size = chain
        .iter()
        .try_fold(chain.len().saturating_sub(1), |size, segment| {
            size.checked_add(segment.len()).ok_or(Exhausted)
        })?;
    let mut text = Buffer::exact(scratch, size)?;
    for (index, segment) in chain.iter().enumerate() {
        if index != 0 {
            text.push(separator)?;
        }
        text.extend(segment)?;
    }
    Ok(text)
}

fn suffixed_name<'a>(
    base: &[u8],
    suffix: usize,
    scratch: &'a Scratch,
) -> ScratchResult<Buffer<'a, u8>> {
    // One decimal digit per bit is a portable upper bound.
    let mut digits = [0; usize::BITS as usize];
    let mut start = digits.len();
    let mut remaining = suffix;
    loop {
        start -= 1;
        digits[start] = b'0' + (remaining % 10) as u8;
        remaining /= 10;
        if remaining == 0 {
            break;
        }
    }
    let digits = &digits[start..];
    let size = base.len().checked_add(1 + digits.len()).ok_or(Exhausted)?;
    let mut name = Buffer::exact(scratch, size)?;
    name.extend(base)?;
    name.push(b'~')?;
    name.extend(digits)?;
    Ok(name)
}

#[derive(Default)]
struct QualifiedName<'a> {
    dotted: Option<Buffer<'a, u8>>,
    identity: Option<Buffer<'a, u8>>,
}

fn settle_names<'a>(
    headings: &[Heading<'_>],
    scratch: &'a Scratch,
    work: &mut Work,
) -> ScratchResult<Buffer<'a, QualifiedName<'a>>> {
    let mut names = Buffer::exact(scratch, headings.len())?;
    let mut by_segment = Map::<&[u8], Buffer<'_, usize>>::new(scratch);
    for (index, heading) in headings.iter().enumerate() {
        names.push(QualifiedName::default())?;
        if !heading.segment.is_empty() {
            by_segment
                .entry(&heading.segment)?
                .or_insert_with(|| Buffer::new(scratch))
                .push(index)?;
        }
    }

    for repeated in by_segment.values().filter(|indices| indices.len() > 1) {
        let (first, start) = segments(repeated[0], headings);
        work.name_checks += repeated.len() as u64;
        let distinct = repeated[1..].iter().any(|&index| {
            let (chain, offset) = segments(index, headings);
            chain[offset..] != first[start..]
        });
        if distinct {
            for &index in repeated.iter() {
                let (chain, start) = segments(index, headings);
                if start < LEVELS - 1 {
                    names[index] = QualifiedName {
                        dotted: Some(join_chain(
                            &chain[start..],
                            b'.',
                            scratch,
                        )?),
                        // Newlines distinguish chains that spell the same dotted QN.
                        identity: Some(join_chain(
                            &chain[start..],
                            b'\n',
                            scratch,
                        )?),
                    };
                }
            }
        }
    }
    drop(by_segment);
    if names.iter().all(|name| name.dotted.is_none()) {
        return Ok(names);
    }

    // Reserve every initial name before choosing any suffix. These groups
    // are in document order and a bare heading takes priority over a chain.
    let mut reserved = Map::<&[u8], Buffer<'_, usize>>::new(scratch);
    for (index, name) in names.iter().enumerate() {
        let base = name.dotted.as_deref().unwrap_or(&headings[index].segment);
        if !base.is_empty() {
            reserved
                .entry(base)?
                .or_insert_with(|| Buffer::new(scratch))
                .push(index)?;
        }
    }
    let mut overrides: Buffer<'_, (Buffer<'_, usize>, Buffer<'_, u8>)> =
        Buffer::new(scratch);
    for (&base, group) in reserved.iter().filter(|(_, group)| group.len() > 1) {
        work.name_checks += group.len() as u64;
        let bare_wins =
            group.iter().any(|&index| names[index].identity.is_none());
        let mut by_identity = Map::<&[u8], Option<usize>>::new(scratch);
        let mut suffix = 2usize;
        for &index in group.iter() {
            let Some(identity) = names[index].identity.as_deref() else {
                continue;
            };
            let needs_suffix = bare_wins || !by_identity.is_empty();
            match by_identity.entry(identity)? {
                Entry::Occupied(entry) => {
                    if let Some(owner) = *entry.get() {
                        overrides[owner].0.push(index)?;
                    }
                }
                Entry::Vacant(entry) => {
                    if needs_suffix {
                        let name = loop {
                            let candidate =
                                suffixed_name(base, suffix, scratch)?;
                            suffix = suffix.checked_add(1).ok_or(Exhausted)?;
                            work.name_checks += 1;
                            if !reserved.contains_key(&*candidate) {
                                break candidate;
                            }
                        };
                        let mut owners = Buffer::new(scratch);
                        owners.push(index)?;
                        let owner = overrides.len();
                        overrides.push((owners, name))?;
                        entry.insert(Some(owner));
                    } else {
                        entry.insert(None);
                    }
                }
            }
        }
    }
    drop(reserved);
    while let Some((mut owners, name)) = overrides.pop() {
        let first = owners.pop().expect("a suffix has an owner");
        for &index in owners.iter() {
            let mut copied = Buffer::exact(scratch, name.len())?;
            copied.extend(&name)?;
            names[index].dotted = Some(copied);
        }
        names[first].dotted = Some(name);
    }
    for name in names.iter_mut() {
        name.identity = None;
    }
    Ok(names)
}

fn collected_segment<'a>(
    headings: Option<&'a [Heading<'_>]>,
    names: Option<&'a [QualifiedName<'_>]>,
    index: usize,
) -> Option<&'a [u8]> {
    names
        .and_then(|names| names[index].dotted.as_deref())
        .or_else(|| headings.map(|headings| &*headings[index].segment))
}

struct Prose<'a> {
    text: Buffer<'a, u8>,
    cap: usize,
    pending_space: bool,
}

impl<'a> Prose<'a> {
    fn append(&mut self, input: &[u8]) -> ScratchResult<()> {
        for &byte in input {
            if self.text.len() == self.cap {
                break;
            }
            if prose_space(byte) {
                self.pending_space = true;
                continue;
            }
            if self.text.is_empty() {
                self.text.reserve_exact(self.cap)?;
            }
            if self.pending_space && !self.text.is_empty() {
                self.text.push(b' ')?;
                if self.text.len() == self.cap {
                    break;
                }
            }
            self.text.push(byte)?;
            self.pending_space = false;
        }
        Ok(())
    }

    fn finish(mut self) -> Buffer<'a, u8> {
        // Match the native C-string and partial UTF-8 tail handling.
        if let Some(nul) = self.text.iter().position(|&byte| byte == 0) {
            self.text.truncate(nul);
        }
        let Some(start) =
            self.text.iter().rposition(|&byte| byte & 0xc0 != 0x80)
        else {
            self.text.truncate(0);
            return self.text;
        };
        let lead = self.text[start];
        let len = match lead {
            0xc0..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf7 => 4,
            _ => 1,
        };
        if start + len > self.text.len() {
            self.text.truncate(start);
        }
        self.text
    }
}

unsafe fn section_body<'a>(
    heading: &TreeCursor,
    lookahead: &mut TreeCursor,
    source: &[u8],
    cap: usize,
    scratch: &'a Scratch,
    ctx: *mut c_void,
    diagram: DiagramLabels,
) -> ScratchResult<Buffer<'a, u8>> {
    // SAFETY: Both cursors and all yielded nodes borrow the same live tree.
    unsafe {
        lookahead.reset_to(heading);
        let mut start = heading.node().range().end;
        let mut end = start;
        let mut prose = Prose {
            text: Buffer::new(scratch),
            cap,
            pending_space: false,
        };
        while lookahead.next_sibling() {
            let block = lookahead.node();
            if block.is(c"section") || heading_kind(block).is_some() {
                break;
            }
            let span = block.range();
            end = span.end;
            if block.is(c"fenced_code_block") {
                prose
                    .append(source.get(start..span.start).unwrap_or_default())?;
                if prose.text.len() == cap {
                    return Ok(prose.finish());
                }
                let mut labels = ptr::null();
                // C's diagram labels belong to the result arena.
                if diagram(ctx, &block, &mut labels) {
                    if !labels.is_null() {
                        prose.append(CStr::from_ptr(labels).to_bytes())?;
                        prose.append(b" ")?;
                    }
                } else {
                    prose
                        .append(source.get(span.clone()).unwrap_or_default())?;
                }
                start = span.end;
            }
        }
        prose.append(source.get(start..end).unwrap_or_default())?;
        Ok(prose.finish())
    }
}

// SAFETY: C lends a live Tree-sitter root and source bytes, and supplies
// synchronous callbacks that copy the emitted buffers before returning.
#[unsafe(no_mangle)]
unsafe extern "C" fn mori_markdown_extract_sections(
    root: RawNode,
    source: *const c_char,
    source_len: usize,
    body_cap: usize,
    ctx: *mut c_void,
    emit: EmitSection,
    diagram: DiagramLabels,
) -> Work {
    let scratch = Scratch::default();
    // SAFETY: C lends a live tree and source, and the callbacks run synchronously.
    unsafe {
        let source = std::slice::from_raw_parts(source.cast(), source_len);
        extract_sections(root, source, body_cap, ctx, emit, diagram, &scratch)
    }
}

unsafe fn extract_sections(
    root: RawNode,
    source: &[u8],
    body_cap: usize,
    ctx: *mut c_void,
    emit: EmitSection,
    diagram: DiagramLabels,
    scratch: &Scratch,
) -> Work {
    let mut work = Work::default();
    // SAFETY: All borrowed native inputs outlive the extraction and callbacks.
    unsafe {
        let headings = collect(root, source, scratch, &mut work).ok();
        let names = headings.as_ref().and_then(|headings| {
            settle_names(headings, scratch, &mut work).ok()
        });
        let mut lookahead = None;
        let mut index = 0;
        let mut emission_steps = 0;
        let _ = walk(root, &mut emission_steps, |cursor, node, _, kind| {
            let Some(kind) = kind else {
                return Ok(());
            };
            let current = index;
            index += 1;
            // A failed title allocation omits only this heading.
            let Ok(name) = heading_name(node, kind, source, scratch) else {
                return Ok(());
            };
            let name = name.as_bytes();
            if name.is_empty() {
                return Ok(());
            }
            let mut normalized = None;
            let segment = collected_segment(
                headings.as_deref(),
                names.as_deref(),
                current,
            )
            .unwrap_or_else(|| {
                if name.iter().copied().any(prose_space) {
                    normalized = qn_segment(name, scratch).ok();
                }
                // The former C slugifier kept the raw name if it could not
                // allocate a normalized segment.
                normalized.as_deref().unwrap_or(name)
            });
            // A failed body allocation omits prose and preserves the Section.
            let lookahead =
                lookahead.get_or_insert_with(|| TreeCursor::new(root));
            let body = section_body(
                cursor, lookahead, source, body_cap, scratch, ctx, diagram,
            )
            .ok();
            let body = body.as_deref().unwrap_or_default();
            emit(
                ctx,
                &node,
                name.as_ptr().cast(),
                name.len(),
                segment.as_ptr().cast(),
                segment.len(),
                body.as_ptr().cast(),
                body.len(),
            );
            Ok(())
        });
    }
    work
}

#[repr(C)]
struct SectionOwner {
    qn: *const c_char,
    line: u32,
    shared_qn: bool,
}

impl SectionOwner {
    fn qn(&self) -> &CStr {
        // SAFETY: C supplies NUL-terminated arena strings for every row and
        // keeps them live while Rust borrows the rows.
        unsafe { CStr::from_ptr(self.qn) }
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_markdown_sort_section_owners(
    sections: *mut SectionOwner,
    count: usize,
) {
    if count == 0 {
        return;
    }
    // SAFETY: C supplies count initialized and exclusively borrowed rows.
    let sections = unsafe { std::slice::from_raw_parts_mut(sections, count) };
    sections.sort_unstable_by(|a, b| a.qn().cmp(b.qn()));
    for index in 1..sections.len() {
        if sections[index - 1].qn() == sections[index].qn() {
            sections[index - 1].shared_qn = true;
            sections[index].shared_qn = true;
        }
    }
    sections.sort_unstable_by(|a, b| {
        a.line.cmp(&b.line).then_with(|| a.qn().cmp(b.qn()))
    });
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_markdown_reference_owner(
    sections: *const SectionOwner,
    count: usize,
    line: u32,
) -> *const SectionOwner {
    if count == 0 {
        return ptr::null();
    }
    // SAFETY: C supplies count initialized rows and keeps them live until it
    // has finished using the returned pointer into the array.
    let sections = unsafe { std::slice::from_raw_parts(sections, count) };
    let past_owner = sections.partition_point(|section| section.line <= line);
    past_owner
        .checked_sub(1)
        .map(|index| &sections[index])
        // A shared QN collapses several headings into one graph node.
        // Their references belong to the file.
        .filter(|section| !section.shared_qn)
        .map_or(ptr::null(), ptr::from_ref)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "c-backend")]
    #[test]
    fn collection_reservation_failure_streams_bare_sections_with_prose() {
        unsafe extern "C" {
            fn tree_sitter_markdown() -> *const c_void;
            fn ts_parser_new() -> *mut c_void;
            fn ts_parser_set_language(
                parser: *mut c_void,
                language: *const c_void,
            ) -> bool;
            fn ts_parser_parse_string(
                parser: *mut c_void,
                old: *const c_void,
                source: *const u8,
                len: u32,
            ) -> *mut c_void;
            fn ts_parser_delete(parser: *mut c_void);
            fn ts_tree_root_node(tree: *const c_void) -> RawNode;
            fn ts_tree_delete(tree: *mut c_void);
        }

        type Sections = Vec<(String, String)>;
        unsafe extern "C" fn emit(
            ctx: *mut c_void,
            _: *const RawNode,
            _: *const c_char,
            _: usize,
            segment: *const c_char,
            segment_len: usize,
            body: *const c_char,
            body_len: usize,
        ) {
            // SAFETY: The test owns the sink. The extractor lends the text for this call.
            unsafe {
                let copy = |data: *const c_char, len| {
                    String::from_utf8_lossy(std::slice::from_raw_parts(
                        data.cast(),
                        len,
                    ))
                    .into_owned()
                };
                (*ctx.cast::<Sections>())
                    .push((copy(segment, segment_len), copy(body, body_len)));
            }
        }

        unsafe extern "C" fn diagram(
            _: *mut c_void,
            _: *const RawNode,
            _: *mut *const c_char,
        ) -> bool {
            false
        }

        let source =
            b"# A\n\n## Shared title\n\nBody text.\n\n# B\n\n## Shared title\n";
        let scratch = Scratch::default();
        scratch.fail_after(0);
        let mut sections = Sections::new();
        // SAFETY: The tree, source and sink live through the synchronous extraction.
        unsafe {
            let parser = ts_parser_new();
            assert!(ts_parser_set_language(parser, tree_sitter_markdown()));
            let tree = ts_parser_parse_string(
                parser,
                ptr::null(),
                source.as_ptr(),
                source.len() as u32,
            );
            ts_parser_delete(parser);
            assert!(!tree.is_null());
            extract_sections(
                ts_tree_root_node(tree),
                source,
                500,
                (&mut sections as *mut Sections).cast(),
                emit,
                diagram,
                &scratch,
            );
            ts_tree_delete(tree);
        }
        let segments: Vec<_> = sections
            .iter()
            .map(|(segment, _)| segment.as_str())
            .collect();
        assert_eq!(segments, ["A", "Shared-title", "B", "Shared-title"]);
        assert_eq!(sections[1].1, "Body text.");
        assert_eq!(scratch.totals().0, 0);
    }

    #[test]
    fn every_settlement_reservation_failure_leaves_bare_names() {
        let scratch = Scratch::default();
        let mut headings = Buffer::new(&scratch);
        for (segment, parent) in [
            (b"A.B".as_slice(), None),
            (b"C", Some(0)),
            (b"A", None),
            (b"B", Some(2)),
            (b"C", Some(3)),
            (b"C", Some(3)),
            (b"A.B.C", None),
            (b"A.B.C~2", None),
        ] {
            headings
                .push(Heading {
                    segment: qn_segment(segment, &scratch).unwrap(),
                    parent,
                })
                .unwrap();
        }

        let staging = Scratch::default();
        for available in 0..256 {
            staging.fail_after(available);
            let mut work = Work::default();
            let names = settle_names(&headings, &staging, &mut work).ok();
            let name = |index| {
                collected_segment(Some(&headings), names.as_deref(), index)
            };
            if names.is_some() {
                assert!(available > 1);
                assert_eq!(name(1), Some(b"A.B.C~3".as_slice()));
                assert_eq!(name(4), Some(b"A.B.C~4".as_slice()));
                assert_eq!(name(5), name(4));
                assert_eq!(name(6), Some(b"A.B.C".as_slice()));
                assert_eq!(name(7), Some(b"A.B.C~2".as_slice()));
                drop(names);
                assert_eq!(staging.totals().0, 0);
                return;
            }
            for index in 0..headings.len() {
                assert_eq!(name(index), Some(&*headings[index].segment));
            }
            assert_eq!(staging.totals().0, 0, "after {available} reservations");
        }
        panic!("fixture never reached successful settlement");
    }

    #[test]
    fn scratch_capacity_publishes_peak_before_releasing_buffers() {
        let scratch = Scratch::default();
        let headings = Buffer::<u64>::exact(&scratch, 8).unwrap();
        assert_eq!(scratch.totals(), (64, 0));
        {
            let mut name = Buffer::<u8>::exact(&scratch, 16).unwrap();
            assert_eq!(scratch.totals(), (80, 0));
            assert!(name.reserve_exact(usize::MAX).is_err());
            assert_eq!(scratch.totals(), (80, 0));
        }
        assert_eq!(scratch.totals(), (64, 80));
        drop(headings);
        assert_eq!(scratch.totals(), (0, 80));
    }

    #[test]
    fn failed_prose_reservation_keeps_title_and_releases_scratch() {
        let scratch = Scratch::default();
        let title = qn_segment(b"Some title", &scratch).unwrap();
        let baseline = scratch.totals().0;
        scratch.fail_after(0);
        {
            let mut prose = Prose {
                text: Buffer::new(&scratch),
                cap: 500,
                pending_space: false,
            };
            assert!(prose.append(b"Contents").is_err());
            assert!(prose.text.is_empty());
        }
        assert_eq!(&*title, b"Some-title");
        assert_eq!(scratch.totals().0, baseline);
    }
}
