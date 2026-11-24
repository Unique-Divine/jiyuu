//! Marker-delimited edits. Ownership is assigned by decoded table and key
//! paths while the source ranges preserve the user's spelling and whitespace.

use std::ops::Range;

use super::syntax::{
    Header, Key, Line, Strings, assignment, header, lines, newline, overlaps,
    validate_strings,
};
use super::{Error, Output, Result};

#[derive(Clone, Copy)]
pub(super) struct Markers<'a> {
    pub begin: &'a [u8],
    pub end: &'a [u8],
}

pub(super) enum Span {
    Absent,
    Orphan(Line),
    Pair(Line, Line),
}

impl Markers<'_> {
    fn matches(data: &[u8], line: Line, marker: &[u8]) -> bool {
        &data[line.edit_start(data)..line.content_end] == marker
    }

    pub fn find(self, data: &[u8]) -> Result<Span> {
        let mut begin = None;
        let mut end = None;
        let mut strings = Strings::default();
        for line in lines(data, 0..data.len()) {
            if !strings.in_multiline() {
                if Self::matches(data, line, self.begin)
                    && begin.replace(line).is_some()
                {
                    return Err(Error);
                }
                if Self::matches(data, line, self.end)
                    && end.replace(line).is_some()
                {
                    return Err(Error);
                }
            }
            strings.scan(data, line)?;
        }
        strings.finish()?;
        match (begin, end) {
            (None, None) => Ok(Span::Absent),
            (Some(line), None) | (None, Some(line)) => Ok(Span::Orphan(line)),
            (Some(begin), Some(end)) if begin.start < end.start => {
                Ok(Span::Pair(begin, end))
            }
            _ => Err(Error),
        }
    }

    pub fn validate_body(self, body: &[u8]) -> Result<()> {
        if lines(body, 0..body.len()).any(|line| {
            Self::matches(body, line, self.begin)
                || Self::matches(body, line, self.end)
        }) {
            return Err(Error);
        }
        validate_strings(body)
    }

    pub fn append(
        self,
        out: &mut Output,
        body: &[u8],
        newline: &[u8],
    ) -> Result<()> {
        out.append(self.begin)?;
        out.append(newline)?;
        out.normalized(body, newline)?;
        if !body.is_empty() && !body.ends_with(b"\n") {
            out.append(newline)?;
        }
        out.append(self.end)?;
        out.append(newline)
    }
}

pub(super) fn upsert(
    data: &[u8],
    markers: Markers<'_>,
    block: &[u8],
) -> Result<Output> {
    match markers.find(data)? {
        Span::Absent => {
            check_conflicts(data, 0..0, block)?;
            let newline = newline(data);
            let mut out = Output::start_append(data, newline)?;
            markers.append(&mut out, block, newline)?;
            Ok(out)
        }
        Span::Pair(begin, end) => {
            check_conflicts(data, begin.start..end.end, block)?;
            rewrite(data, begin, end, markers, block, true)
        }
        Span::Orphan(_) => Err(Error),
    }
}

pub(super) fn remove(
    data: &[u8],
    markers: Markers<'_>,
    owned: Option<&[u8]>,
) -> Result<Option<Output>> {
    match markers.find(data)? {
        Span::Absent => Ok(None),
        Span::Orphan(stray) => {
            Output::copy_without(data, stray.start..stray.end).map(Some)
        }
        Span::Pair(begin, end) => match owned {
            Some(owned) => {
                rewrite(data, begin, end, markers, owned, false).map(Some)
            }
            None => Output::copy_without(data, begin.edit_start(data)..end.end)
                .map(Some),
        },
    }
}

fn check_conflicts(
    data: &[u8],
    exclude: Range<usize>,
    block: &[u8],
) -> Result<()> {
    let mut strings = Strings::default();
    for line in lines(block, 0..block.len()) {
        if !strings.in_multiline()
            && let Some(table) = header(block, line)?
            && !table.array
        {
            check_table(block, 0..line.start, 0..0, &table.key, false)?;
            check_table(data, 0..data.len(), exclude.clone(), &table.key, true)?;
        }
        strings.scan(block, line)?;
    }
    strings.finish()
}

fn check_table(
    data: &[u8],
    range: Range<usize>,
    exclude: Range<usize>,
    desired: &Key,
    check_arrays: bool,
) -> Result<()> {
    let mut strings = Strings::default();
    let mut scope = Key::new();
    for line in lines(data, range) {
        if !strings.in_multiline() {
            let excluded = exclude.contains(&line.start);
            if let Some(table) = header(data, line)? {
                if !excluded
                    && (check_arrays || !table.array)
                    && table.key.starts_with(desired)
                {
                    return Err(Error);
                }
                scope = table.key;
            } else if !excluded
                && let Some(entry) = assignment(data, line)?
                && scope
                    .iter()
                    .chain(&entry.key)
                    .zip(desired)
                    .all(|(existing, wanted)| existing == wanted)
            {
                return Err(Error);
            }
        }
        strings.scan(data, line)?;
    }
    strings.finish()
}

struct Section {
    start: usize,
    body: usize,
    end: usize,
    header: Header,
}

fn section_end(data: &[u8], range: Range<usize>) -> Result<usize> {
    let mut strings = Strings::default();
    let end = range.end;
    for line in lines(data, range) {
        if !strings.in_multiline() && header(data, line)?.is_some() {
            return Ok(line.start);
        }
        strings.scan(data, line)?;
    }
    strings.finish()?;
    Ok(end)
}

fn section_at(data: &[u8], start: usize, end: usize) -> Result<Section> {
    let line = lines(data, start..end).next().ok_or(Error)?;
    Ok(Section {
        start,
        body: line.end,
        end: section_end(data, line.end..end)?,
        header: header(data, line)?.ok_or(Error)?,
    })
}

const MAX_TABLES: usize = 16;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ownership {
    Owned(usize),
    Attached(usize),
    Foreign,
}

struct ManagedSpan<'a> {
    data: &'a [u8],
    range: Range<usize>,
    body: &'a [u8],
    declared: Vec<Section>,
}

impl ManagedSpan<'_> {
    fn classify(
        &self,
        header: &Header,
        seen: &mut [bool; MAX_TABLES],
    ) -> Result<Ownership> {
        let mut exact = None;
        let mut exact_taken = false;
        let mut parent = None;
        let mut depth = 0;
        for (i, section) in self.declared.iter().enumerate() {
            let declared = &section.header;
            if header.key == declared.key {
                if header.array != declared.array {
                    return Err(Error);
                }
                if !seen[i] && exact.is_none() {
                    exact = Some(i);
                } else {
                    exact_taken = true;
                }
            } else if header.key.starts_with(&declared.key) {
                if declared.array {
                    return Err(Error);
                }
                if declared.key.len() > depth {
                    parent = Some(i);
                    depth = declared.key.len();
                }
            }
        }
        match (exact, exact_taken, parent) {
            (Some(i), _, _) => {
                seen[i] = true;
                Ok(Ownership::Owned(i))
            }
            (None, true, _) => Err(Error),
            (None, false, Some(i)) => Ok(Ownership::Attached(i)),
            (None, false, None) => Ok(Ownership::Foreign),
        }
    }

    fn append_sections(
        &self,
        want: Ownership,
        out: &mut Output,
    ) -> Result<Option<Ownership>> {
        let mut seen = [false; MAX_TABLES];
        let mut pos = self.range.start;
        let mut last = None;
        while pos < self.range.end {
            let section = section_at(self.data, pos, self.range.end)?;
            let ownership = self.classify(&section.header, &mut seen)?;
            if ownership == want {
                match ownership {
                    Ownership::Owned(i) => {
                        self.append_user_lines(&section, &self.declared[i], out)?
                    }
                    _ => out.append(&self.data[section.start..section.end])?,
                }
            }
            last = Some(ownership);
            pos = section.end;
        }
        Ok(last)
    }

    fn append_user_lines(
        &self,
        section: &Section,
        declared: &Section,
        out: &mut Output,
    ) -> Result<()> {
        let mut strings = Strings::default();
        let mut keep = false;
        let mut blank_start = None;
        for line in lines(self.data, section.body..section.end) {
            if !strings.in_multiline() {
                let trimmed = line.trimmed(self.data);
                if trimmed.is_empty() {
                    blank_start.get_or_insert(line.start);
                    continue;
                }
                keep = if trimmed.starts_with(b"#") {
                    true
                } else {
                    let entry = assignment(self.data, line)?.ok_or(Error)?;
                    !self.writes(declared, &entry.key)?
                };
                if !keep {
                    blank_start = None;
                }
            }
            if keep {
                out.append(
                    &self.data
                        [blank_start.take().unwrap_or(line.start)..line.end],
                )?;
            }
            strings.scan(self.data, line)?;
        }
        strings.finish()
    }

    fn writes(&self, section: &Section, key: &Key) -> Result<bool> {
        let mut strings = Strings::default();
        let mut found = false;
        for line in lines(self.body, section.body..section.end) {
            if !strings.in_multiline()
                && let Some(entry) = assignment(self.body, line)?
            {
                if &entry.key == key {
                    found = true;
                } else if overlaps(&entry.key, key) {
                    return Err(Error);
                }
            }
            strings.scan(self.body, line)?;
        }
        Ok(found)
    }
}

fn rewrite(
    data: &[u8],
    begin: Line,
    end: Line,
    markers: Markers<'_>,
    body: &[u8],
    install: bool,
) -> Result<Output> {
    let first_declared = section_end(body, 0..body.len())?;
    let mut declared = Vec::new();
    let mut pos = first_declared;
    while pos < body.len() {
        if declared.len() == MAX_TABLES {
            return Err(Error);
        }
        let section = section_at(body, pos, body.len())?;
        pos = section.end;
        declared.push(section);
    }
    let lead_end = section_end(data, begin.end..end.start)?;
    let tail_lead_end = section_end(data, end.end..data.len())?;
    let span = ManagedSpan {
        data,
        range: lead_end..end.start,
        body,
        declared,
    };
    let mut moved = Output::default();
    let last = span.append_sections(Ownership::Foreign, &mut moved)?;
    if !moved.is_empty()
        && last != Some(Ownership::Foreign)
        && lines(data, end.end..tail_lead_end).any(|line| line.significant(data))
    {
        return Err(Error);
    }

    let mut out = Output::default();
    out.append(&data[..begin.edit_start(data)])?;
    if !lines(body, 0..first_declared).any(|line| line.significant(body)) {
        out.append(&data[begin.end..lead_end])?;
    }
    let block_start = out.len();
    if install {
        let newline = newline(data);
        out.append(markers.begin)?;
        out.append(newline)?;
        out.normalized(&body[..first_declared], newline)?;
        for (i, section) in span.declared.iter().enumerate() {
            out.normalized(&body[section.start..section.end], newline)?;
            out.finish_line(newline, 0)?;
            span.append_sections(Ownership::Owned(i), &mut out)?;
            span.append_sections(Ownership::Attached(i), &mut out)?;
        }
        out.finish_line(newline, 0)?;
        out.append(markers.end)?;
        out.append(newline)?;
    }
    let block_end = out.len();
    out.append(&moved)?;
    out.append(&data[end.end..])?;
    if install {
        check_conflicts(&out, block_start..block_end, body)?;
    }
    Ok(out)
}
