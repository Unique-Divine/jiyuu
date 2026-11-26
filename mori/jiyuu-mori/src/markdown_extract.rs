//! Section identities, bodies, and reference ownership from the native Markdown tree.

use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_void};
use std::ops::Range;
use std::ptr;

use crate::native_tree::{RawNode, TreeCursor};

type EmitSection = unsafe extern "C" fn(
    *mut c_void,
    RawNode,
    *const c_char,
    usize,
    *const c_char,
    usize,
    *const c_char,
    usize,
);
type DiagramLabels =
    unsafe extern "C" fn(*mut c_void, RawNode, *mut *const c_char) -> bool;

#[repr(C)]
#[derive(Default)]
struct Work {
    collection_steps: u64,
    name_checks: u64,
}

struct Heading {
    node: RawNode,
    name: Vec<u8>,
    segment: Vec<u8>,
    parent: Option<usize>,
    body: Range<usize>,
    fences: Vec<RawNode>,
}

const LEVELS: usize = 6;

#[derive(Default)]
struct Frame {
    // Skip parser section wrappers when determining the heading hierarchy.
    container: (usize, usize),
    body_owner: Option<usize>,
}

fn prose_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
}

fn qn_segment(name: &[u8]) -> Vec<u8> {
    let mut segment = Vec::with_capacity(name.len());
    let mut in_space = false;
    for &byte in name {
        if prose_space(byte) {
            if !in_space && !segment.is_empty() {
                segment.push(b'-');
            }
            in_space = true;
        } else {
            segment.push(byte);
            in_space = false;
        }
    }
    segment
}

unsafe fn setext(node: RawNode, source: &[u8]) -> Option<Vec<u8>> {
    // SAFETY: The caller keeps the tree live while the cursor borrows it.
    unsafe {
        let paragraph = node.field(c"heading_content")?;
        let content = paragraph.child_of_kind(c"inline").unwrap_or(paragraph);
        let span = content.range();
        let mut cursor = TreeCursor::new(content);
        let mut previous = span.start;
        let mut text = Vec::with_capacity(span.len());
        loop {
            let current = cursor.node();
            if current.is(c"block_continuation") {
                let marker = current.range();
                text.extend_from_slice(source.get(previous..marker.start)?);
                previous = marker.end;
            } else if cursor.first_child() {
                continue;
            }
            while !cursor.next_sibling() {
                if !cursor.parent() {
                    text.extend_from_slice(source.get(previous..span.end)?);
                    return Some(text);
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

unsafe fn collect(
    root: RawNode,
    source: &[u8],
    work: &mut Work,
) -> Vec<Heading> {
    let mut headings: Vec<Heading> = Vec::new();
    let mut levels: HashMap<_, [Option<usize>; LEVELS]> = HashMap::new();
    let mut frames = vec![Frame::default()];
    // SAFETY: All nodes and cursors borrow the caller's live tree.
    unsafe {
        let mut cursor = TreeCursor::new(root);
        loop {
            work.collection_steps += 1;
            let frame = frames.last_mut().expect("cursor has a frame");
            let node = cursor.node();
            let span = node.range();
            let atx_heading = node.is(c"atx_heading");
            let is_heading = atx_heading || node.is(c"setext_heading");
            let is_section = node.is(c"section");
            if is_heading || is_section {
                // The previous heading owns only its direct siblings before
                // the next heading or parser section.
                frame.body_owner = None;
            } else if let Some(index) = frame.body_owner {
                let body = &mut headings[index];
                body.body.end = span.end;
                if node.is(c"fenced_code_block") {
                    body.fences.push(node);
                }
            }
            if is_heading {
                let mut name = if atx_heading {
                    atx(node, source).map(<[u8]>::to_vec)
                } else {
                    setext(node, source)
                }
                .unwrap_or_default();
                if let Some(nul) = name.iter().position(|&byte| byte == 0) {
                    name.truncate(nul);
                }
                while name
                    .last()
                    .is_some_and(|&byte| matches!(byte, b' ' | b'\r' | b'\n'))
                {
                    name.pop();
                }
                let index = headings.len();
                let parent = heading_level(node).and_then(|level| {
                    let ranks = levels.entry(frame.container).or_default();
                    let parent =
                        ranks[..level].iter().rev().flatten().next().copied();
                    // Empty headings still reset the hierarchy.
                    ranks[level] = Some(index);
                    ranks[level + 1..].fill(None);
                    parent
                });
                let segment = qn_segment(&name);
                frame.body_owner = (!name.is_empty()).then_some(index);
                headings.push(Heading {
                    node,
                    name,
                    segment,
                    parent,
                    body: span.end..span.end,
                    fences: Vec::new(),
                });
            } else {
                work.collection_steps += 1;
                if cursor.first_child() {
                    let container = if is_section {
                        frame.container
                    } else {
                        (span.start, span.end)
                    };
                    frames.push(Frame {
                        container,
                        body_owner: None,
                    });
                    continue;
                }
            }
            loop {
                work.collection_steps += 1;
                if cursor.next_sibling() {
                    break;
                }
                work.collection_steps += 1;
                if !cursor.parent() {
                    return headings;
                }
                frames.pop();
            }
        }
    }
}

fn segments(index: usize, headings: &[Heading]) -> Vec<&[u8]> {
    let mut chain = Vec::with_capacity(LEVELS);
    let mut current = Some(index);
    while let Some(index) = current {
        let heading = &headings[index];
        if !heading.segment.is_empty() {
            chain.push(heading.segment.as_slice());
        }
        current = heading.parent;
    }
    chain.reverse();
    chain
}

fn settle_names(headings: &[Heading], work: &mut Work) -> Vec<Vec<u8>> {
    let mut names: Vec<_> = headings.iter().map(|h| h.segment.clone()).collect();
    let mut by_segment = HashMap::<&[u8], Vec<usize>>::new();
    for (index, heading) in headings.iter().enumerate() {
        if !heading.segment.is_empty() {
            by_segment.entry(&heading.segment).or_default().push(index);
        }
    }

    let mut identities = vec![Vec::new(); headings.len()];
    for repeated in by_segment.values().filter(|indices| indices.len() > 1) {
        let chains: Vec<_> = repeated
            .iter()
            .map(|&index| segments(index, headings))
            .collect();
        work.name_checks += chains.len() as u64;
        if chains[1..].iter().all(|chain| *chain == chains[0]) {
            continue;
        }
        for (&index, chain) in repeated.iter().zip(chains) {
            names[index] = chain.join(&b'.');
            // The separator preserves distinct chains that spell the same
            // dotted name. QN segments never contain newlines.
            identities[index] = chain.join(&b'\n');
        }
    }

    // Reserve every initial name before choosing any suffix. Each group is
    // in document order. A bare heading takes priority over a parent chain.
    let mut reserved = HashMap::<Vec<u8>, Vec<usize>>::new();
    for (index, name) in names
        .iter()
        .enumerate()
        .filter(|(_, name)| !name.is_empty())
    {
        reserved.entry(name.clone()).or_default().push(index);
    }
    for (base, group) in reserved.iter().filter(|(_, group)| group.len() > 1) {
        work.name_checks += group.len() as u64;
        let bare_wins =
            group.iter().any(|&index| headings[index].segment == *base);
        let mut by_identity = HashMap::<&[u8], Vec<u8>>::new();
        let mut suffix = 2;
        for &index in group {
            if headings[index].segment == *base {
                continue;
            }
            if let Some(name) = by_identity.get(identities[index].as_slice()) {
                names[index].clone_from(name);
                continue;
            }
            if bare_wins || !by_identity.is_empty() {
                loop {
                    let mut candidate = base.clone();
                    candidate.extend_from_slice(format!("~{suffix}").as_bytes());
                    suffix += 1;
                    work.name_checks += 1;
                    if !reserved.contains_key(&candidate) {
                        names[index] = candidate;
                        break;
                    }
                }
            }
            by_identity.insert(&identities[index], names[index].clone());
        }
    }
    names
}

struct Prose {
    text: Vec<u8>,
    cap: usize,
    pending_space: bool,
}

impl Prose {
    fn append(&mut self, input: &[u8]) {
        for &byte in input {
            if self.text.len() == self.cap {
                break;
            }
            if prose_space(byte) {
                self.pending_space = true;
                continue;
            }
            if self.pending_space && !self.text.is_empty() {
                self.text.push(b' ');
                if self.text.len() == self.cap {
                    break;
                }
            }
            self.text.push(byte);
            self.pending_space = false;
        }
    }

    fn finish(mut self) -> Vec<u8> {
        // Match the native C-string and partial UTF-8 tail handling.
        if let Some(nul) = self.text.iter().position(|&byte| byte == 0) {
            self.text.truncate(nul);
        }
        let Some(start) =
            self.text.iter().rposition(|&byte| byte & 0xc0 != 0x80)
        else {
            return Vec::new();
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

unsafe fn section_body(
    heading: &Heading,
    source: &[u8],
    cap: usize,
    ctx: *mut c_void,
    diagram: DiagramLabels,
) -> Vec<u8> {
    let mut prose = Prose {
        text: Vec::with_capacity(heading.body.len().min(cap)),
        cap,
        pending_space: false,
    };
    let mut start = heading.body.start;
    for &fence in &heading.fences {
        // SAFETY: C lends the node and context, and any returned label string
        // belongs to the live result arena.
        unsafe {
            let span = fence.range();
            prose.append(source.get(start..span.start).unwrap_or_default());
            if prose.text.len() == cap {
                return prose.finish();
            }
            let mut labels = ptr::null();
            if diagram(ctx, fence, &mut labels) {
                if !labels.is_null() {
                    prose.append(CStr::from_ptr(labels).to_bytes());
                    prose.append(b" ");
                }
            } else {
                prose.append(source.get(span.clone()).unwrap_or_default());
            }
            start = span.end;
        }
    }
    prose.append(source.get(start..heading.body.end).unwrap_or_default());
    prose.finish()
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
    let mut work = Work::default();
    // SAFETY: All borrowed native inputs outlive the extraction and callbacks.
    unsafe {
        let source = std::slice::from_raw_parts(source.cast(), source_len);
        let headings = collect(root, source, &mut work);
        let names = settle_names(&headings, &mut work);
        for (heading, name) in headings.iter().zip(&names) {
            if !heading.name.is_empty() {
                let body = section_body(heading, source, body_cap, ctx, diagram);
                emit(
                    ctx,
                    heading.node,
                    heading.name.as_ptr().cast(),
                    heading.name.len(),
                    name.as_ptr().cast(),
                    name.len(),
                    body.as_ptr().cast(),
                    body.len(),
                );
            }
        }
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
