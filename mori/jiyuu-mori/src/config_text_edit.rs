//! Byte-preserving edits for managed instruction blocks and owned documents.

use std::io;
use std::path::Path;

#[path = "config_text_edit/ffi.rs"]
mod ffi;
#[path = "config_text_edit/file.rs"]
mod file;
#[cfg(test)]
#[path = "config_text_edit/tests.rs"]
mod tests;

use file::{Snapshot, Target};

const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_MARKER_BYTES: usize = 4096;
const BOM: &str = "\u{feff}";

#[derive(Debug, PartialEq, Eq)]
enum Ownership {
    Owned,
    Foreign,
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn text(bytes: &[u8], allow_bom: bool) -> io::Result<&str> {
    let text = std::str::from_utf8(bytes).map_err(io::Error::other)?;
    if bytes.len() > MAX_BYTES {
        return Err(invalid("text exceeds the document limit"));
    }
    let body = if allow_bom {
        text.strip_prefix(BOM).unwrap_or(text)
    } else {
        text
    };
    for (offset, ch) in body.char_indices() {
        match ch {
            '\r' if body.as_bytes().get(offset + 1) == Some(&b'\n') => {}
            '\t' | '\n' => {}
            '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}' | '\u{feff}' => {
                return Err(invalid("disallowed character in text"));
            }
            _ => {}
        }
    }
    Ok(text)
}

fn single_line(bytes: &[u8], limit: usize) -> io::Result<&str> {
    if bytes.is_empty() || bytes.len() > limit || bytes.contains(&b'\n') {
        return Err(invalid("invalid path or marker"));
    }
    text(bytes, false)
}

struct Markers<'a> {
    begin: &'a str,
    end: &'a str,
}

struct MarkerLine {
    start: usize,
    text_end: usize,
    full_end: usize,
}

impl<'a> Markers<'a> {
    fn new(begin: &'a [u8], end: &'a [u8]) -> io::Result<Self> {
        let begin = single_line(begin, MAX_MARKER_BYTES)?;
        let end = single_line(end, MAX_MARKER_BYTES)?;
        if begin.contains(end) || end.contains(begin) {
            return Err(invalid("managed markers contain one another"));
        }
        Ok(Self { begin, end })
    }

    fn region(
        &self,
        document: &str,
    ) -> io::Result<Option<(MarkerLine, MarkerLine)>> {
        let body = document.strip_prefix(BOM).unwrap_or(document);
        let mut offset = document.len() - body.len();
        let mut begin = None;
        let mut end = None;
        for line in body.split_inclusive('\n') {
            let bare = line
                .strip_suffix("\r\n")
                .or_else(|| line.strip_suffix('\n'))
                .unwrap_or(line);
            let marker = MarkerLine {
                start: offset,
                text_end: offset + bare.len(),
                full_end: offset + line.len(),
            };
            offset = marker.full_end;
            if bare == self.begin && begin.is_none() {
                begin = Some(marker);
            } else if bare == self.end && end.is_none() {
                end = Some(marker);
            }
        }
        // A marker anywhere outside its standalone line is ambiguous.
        if body.matches(self.begin).take(2).count()
            != usize::from(begin.is_some())
            || body.matches(self.end).take(2).count()
                != usize::from(end.is_some())
        {
            return Err(invalid("duplicate or inline managed marker"));
        }
        match (begin, end) {
            (None, None) => Ok(None),
            (Some(begin), Some(end)) if begin.start < end.start => {
                Ok(Some((begin, end)))
            }
            _ => Err(invalid("unpaired or reversed managed markers")),
        }
    }

    fn upsert(
        &self,
        document: &str,
        owned: &str,
        limit: usize,
    ) -> io::Result<String> {
        if owned.contains(self.begin) || owned.contains(self.end) {
            return Err(invalid("owned text contains a managed marker"));
        }
        let (prefix, separator, eol, trailing, suffix) =
            match self.region(document)? {
                Some((begin, end)) => (
                    &document[..begin.start],
                    "",
                    &document[begin.text_end..begin.full_end],
                    &document[end.text_end..end.full_end],
                    &document[end.full_end..],
                ),
                None => {
                    let eol = match document.find('\n') {
                        Some(at) if document[..at].ends_with('\r') => "\r\n",
                        _ => "\n",
                    };
                    let append_line = document.is_empty()
                        || document == BOM
                        || document.ends_with('\n');
                    (
                        document,
                        if append_line { "" } else { eol },
                        eol,
                        if append_line { eol } else { "" },
                        "",
                    )
                }
            };
        let mut updated = String::from(prefix);
        updated.push_str(separator);
        updated.push_str(self.begin);
        updated.push_str(eol);
        for line in owned.lines() {
            updated.push_str(line);
            updated.push_str(eol);
        }
        updated.push_str(self.end);
        updated.push_str(trailing);
        updated.push_str(suffix);
        if updated.len() > limit {
            return Err(invalid("managed document exceeds the requested limit"));
        }
        Ok(updated)
    }

    fn remove(&self, document: &str) -> io::Result<Option<String>> {
        let Some((begin, end)) = self.region(document)? else {
            return Ok(None);
        };
        let mut prefix = &document[..begin.start];
        if end.text_end == document.len() {
            // Upsert adds this separator to an unterminated document.
            prefix = prefix
                .strip_suffix("\r\n")
                .or_else(|| prefix.strip_suffix('\n'))
                .unwrap_or(prefix);
        }
        Ok(Some([prefix, &document[end.full_end..]].concat()))
    }
}

fn upsert(
    path: &Path,
    markers: Markers<'_>,
    owned: &[u8],
    limit: usize,
) -> io::Result<()> {
    if !(1..=MAX_BYTES).contains(&limit) {
        return Err(invalid("invalid document limit"));
    }
    let owned = text(owned, false)?;
    let target = Target::open(path)?;
    let old = target.read()?;
    let updated = markers.upsert(old.text()?, owned, limit)?;
    target.write(&old, updated.as_bytes(), None)
}

fn block_status(path: &Path, markers: Markers<'_>) -> io::Result<Ownership> {
    let old = Target::open(path)?.read()?;
    let document = old.text()?;
    if markers.region(document)?.is_some()
        || document
            .trim_start_matches(BOM)
            .bytes()
            .all(|b| matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
    {
        Ok(Ownership::Owned)
    } else {
        Ok(Ownership::Foreign)
    }
}

fn remove_block(path: &Path, markers: Markers<'_>) -> io::Result<()> {
    let target = Target::open(path)?;
    let old = target.read()?;
    if let Some(updated) = markers.remove(old.text()?)? {
        target.write(&old, updated.as_bytes(), None)?;
    }
    Ok(())
}

enum Write<'a> {
    Overwrite,
    IfUnchanged(Option<&'a [u8]>),
    Create,
    Ensure,
}

fn write_document(
    path: &Path,
    current: &[u8],
    rule: Write<'_>,
) -> io::Result<()> {
    text(current, true)?;
    let target = Target::open(path)?;
    let old = target.read()?;
    let allowed = match rule {
        Write::Overwrite => {
            old.text()?;
            true
        }
        Write::IfUnchanged(expected) => {
            old.text()?;
            if let Some(expected) = expected {
                text(expected, true)?;
            }
            old.content() == expected
        }
        Write::Create => old.content().is_none(),
        Write::Ensure => old.content().is_none_or(|bytes| bytes == current),
    };
    if !allowed {
        return Err(invalid("document does not match the write precondition"));
    }
    target.write(&old, current, None)
}

fn document_owner(
    old: &Snapshot,
    current: &[u8],
    released: &[&[u8]],
) -> io::Result<Ownership> {
    old.text()?;
    if old.content().is_none_or(|bytes| bytes == current) {
        return Ok(Ownership::Owned);
    }
    for candidate in released {
        text(candidate, true)?;
        if old.content() == Some(candidate) {
            return Ok(Ownership::Owned);
        }
    }
    Ok(Ownership::Foreign)
}

fn document_status(
    path: &Path,
    current: &[u8],
    released: &[&[u8]],
) -> io::Result<Ownership> {
    text(current, true)?;
    document_owner(&Target::open(path)?.read()?, current, released)
}

fn migrate(
    path: &Path,
    current: &[u8],
    released: &[&[u8]],
    mode: Option<u32>,
) -> io::Result<Ownership> {
    if mode.is_some_and(|mode| mode & !0o777 != 0 || mode & 0o400 == 0) {
        return Err(invalid("invalid owned document mode"));
    }
    validate_releases(current, released)?;
    let target = Target::open(path)?;
    let old = target.read()?;
    old.check_mode_owner(mode)?;
    let ownership = document_owner(&old, current, released)?;
    if ownership == Ownership::Owned {
        target.write(&old, current, mode)?;
    }
    Ok(ownership)
}

fn remove_document(
    path: &Path,
    current: &[u8],
    released: &[&[u8]],
) -> io::Result<Ownership> {
    validate_releases(current, released)?;
    let target = Target::open(path)?;
    let old = target.read()?;
    let ownership = document_owner(&old, current, released)?;
    if ownership == Ownership::Owned && old.content().is_some() {
        target.delete(&old)?;
    }
    Ok(ownership)
}

fn validate_releases(current: &[u8], released: &[&[u8]]) -> io::Result<()> {
    for candidate in std::iter::once(current).chain(released.iter().copied()) {
        text(candidate, true)?;
    }
    Ok(())
}
