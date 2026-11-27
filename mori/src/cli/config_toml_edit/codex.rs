//! Exact recognition of hook configurations emitted by released installers.

use std::ffi::CStr;
use std::ops::Range;

use super::legacy::owned_basename;
use super::managed::{Markers, Span};
use super::syntax::{
    Key, Strings, assignment, header, lines, newline, parse_string,
    payload_start, single, trim_start,
};
use super::{Error, Output, Result, escape_basic};

#[derive(Clone, Copy)]
#[repr(i32)]
pub(super) enum Failure {
    InvalidArgument = 1,
    CommandRender,
    ConfigRead,
    UnsafeContent,
    MalformedConfig,
    AmbiguousOwnership,
    ConflictingHooks,
    EditBuild,
    ConfigWrite,
}

pub(super) fn failure_name(code: i32) -> &'static CStr {
    match code {
        0 => c"none",
        1 => c"invalid_argument",
        2 => c"command_render",
        3 => c"config_read",
        4 => c"unsafe_content",
        5 => c"malformed_config",
        6 => c"ambiguous_hook_ownership",
        7 => c"conflicting_hook_representations",
        8 => c"edit_build",
        9 => c"config_write",
        _ => c"unknown",
    }
}

const EVENTS: [&[u8]; 3] = [b"SessionStart", b"SubagentStart", b"PreToolUse"];
const MATCHERS: [&[u8]; 3] = [b"startup|resume|clear|compact", b"*", b"Bash"];
const LEGACY_SHORT: &[u8] =
    b"echo \"Code discovery: prefer codebase-memory-mcp\"";
const LEGACY_RELEASED: &[u8] = b"echo \"Code discovery: prefer codebase-memory-mcp (search_graph, trace_path, get_code_snippet, query_graph, search_code) over grep/file-read; run index_repository first if the project is not indexed.\"";

fn executable_word(encoded: &[u8]) -> Option<&[u8]> {
    let quoted = encoded.starts_with(b"'");
    let (start, end) = if quoted {
        if encoded.len() < 2 || !encoded.ends_with(b"'") {
            return None;
        }
        (1, encoded.len() - 1)
    } else {
        (0, encoded.len())
    };
    let lowest = if quoted { b' ' } else { b'!' };
    let mut pos = start;
    while pos < end {
        let byte = encoded[pos];
        pos += 1;
        if byte < lowest
            || byte == 127
            || (!quoted && b"\"'`$;&|<>(){}[]*?!".contains(&byte))
        {
            return None;
        }
        if quoted && byte == b'\'' {
            if pos < end && encoded[pos] == b'\'' {
                pos += 1;
            } else if encoded.get(pos..pos + 3) == Some(b"\\''") {
                pos += 3;
            } else {
                return None;
            }
        }
    }
    (start < end).then(|| &encoded[start..end])
}

fn owned_command(command: &[u8]) -> bool {
    let command = command.strip_prefix(b"& ").unwrap_or(command);
    command
        .strip_suffix(b" hook-augment")
        .and_then(executable_word)
        .is_some_and(owned_basename)
}

struct Inline<'a>(&'a [u8]);

impl Inline<'_> {
    fn take(&mut self, tokens: &[&[u8]]) -> Result<()> {
        for &token in tokens {
            self.0 = trim_start(self.0).strip_prefix(token).ok_or(Error)?;
        }
        Ok(())
    }

    fn string(&mut self) -> Result<Vec<u8>> {
        let rest = trim_start(self.0);
        let (decoded, used) = parse_string(rest)?;
        self.0 = &rest[used..];
        Ok(decoded)
    }
}

fn owned_inline(value: &[u8], event: usize) -> Result<()> {
    let mut value = Inline(value);
    value.take(&[b"[", b"{", b"matcher", b"="])?;
    if value.string()? != MATCHERS[event] {
        return Err(Error);
    }
    value.take(&[b",", b"hooks", b"=", b"[", b"{", b"type", b"="])?;
    if value.string()? != b"command" {
        return Err(Error);
    }
    value.take(&[b",", b"command", b"="])?;
    let command = value.string()?;
    if owned_command(&command) {
        value.take(&[b",", b"command_windows", b"="])?;
        if !owned_command(&value.string()?) {
            return Err(Error);
        }
        value.take(&[b",", b"timeout", b"=", b"5"])?;
    } else if event != 0
        || !matches!(command.as_slice(), LEGACY_SHORT | LEGACY_RELEASED)
    {
        return Err(Error);
    }
    value.take(&[b"}", b"]", b"}", b"]"])?;
    if trim_start(value.0).is_empty() {
        Ok(())
    } else {
        Err(Error)
    }
}

pub(super) fn build_block(
    command: &[u8],
    windows: &[u8],
    pretool: bool,
) -> Result<Output> {
    let mut escaped = [0; 8192];
    let mut escaped_windows = [0; 8192];
    let size = escape_basic(command, &mut escaped)?;
    let windows_size = escape_basic(windows, &mut escaped_windows)?;
    if !owned_command(command) || !owned_command(windows) {
        return Err(Error);
    }
    let mut block = Output::default();
    for (i, (&event, &matcher)) in EVENTS.iter().zip(&MATCHERS).enumerate() {
        if i == 2 && !pretool {
            break;
        }
        if i > 0 {
            block.append(b"\n")?;
        }
        for part in [
            b"[[hooks.",
            event,
            b"]]\nmatcher = \"",
            matcher,
            b"\"\n\n[[hooks.",
            event,
            b".hooks]]\ntype = \"command\"\ncommand = \"",
            &escaped[..size],
            b"\"\ncommand_windows = \"",
            &escaped_windows[..windows_size],
            b"\"\ntimeout = 5\n",
        ] {
            block.append(part)?;
        }
    }
    if block.len() < 32768 {
        Ok(block)
    } else {
        Err(Error)
    }
}

fn owned_span(
    data: &[u8],
    markers: Option<Markers<'_>>,
    newline: &[u8],
) -> Option<usize> {
    let mut command = None;
    let mut windows = None;
    for line in lines(data, 0..data.len()) {
        if let Some(entry) = assignment(data, line).ok()? {
            if command.is_none() && single(&entry.key, b"command") {
                command = Some(entry.string_value().ok()?);
            } else if windows.is_none() && single(&entry.key, b"command_windows")
            {
                windows = Some(entry.string_value().ok()?);
            }
        }
        if command.is_some() && windows.is_some() {
            break;
        }
    }
    let (command, windows) = (command?, windows?);
    for pretool in [true, false] {
        if let Ok(block) = build_block(&command, &windows, pretool) {
            let mut expected = Output::default();
            if match markers {
                Some(markers) => markers.append(&mut expected, &block, newline),
                None => expected.normalized(&block, newline),
            }
            .is_ok()
                && data.starts_with(&expected)
            {
                return Some(expected.len());
            }
        }
    }
    None
}

type CodexResult<T> = std::result::Result<T, Failure>;

#[derive(Default)]
struct Edits(Vec<Range<usize>>);

impl Edits {
    fn push(&mut self, range: Range<usize>) -> CodexResult<()> {
        if self.0.len() == 4 || range.start > range.end {
            return Err(Failure::EditBuild);
        }
        self.0.push(range);
        Ok(())
    }

    fn apply(mut self, data: &[u8]) -> CodexResult<Output> {
        self.0.sort_unstable_by_key(|edit| edit.start);
        if self.0.windows(2).any(|pair| pair[0].end > pair[1].start) {
            return Err(Failure::ConflictingHooks);
        }
        let build = |_| Failure::EditBuild;
        let mut out = Output::default();
        let mut cursor = 0;
        for edit in self.0 {
            out.append(&data[cursor..edit.start]).map_err(build)?;
            cursor = edit.end;
        }
        out.append(&data[cursor..]).map_err(build)?;
        Ok(out)
    }
}

fn inline_edits(
    data: &[u8],
    edits: &mut Edits,
) -> CodexResult<([bool; 3], bool)> {
    let mut found = [false; 3];
    let mut foreign_pretool = false;
    let mut scope = Key::new();
    let mut strings = Strings::default();
    let malformed = |_| Failure::MalformedConfig;
    for line in lines(data, 0..data.len()) {
        if !strings.in_multiline() {
            if let Some(table) = header(data, line).map_err(malformed)? {
                scope = table.key;
            } else if let Some(entry) =
                assignment(data, line).map_err(malformed)?
            {
                let mut key = scope.iter().chain(entry.key.iter());
                let event = match (key.next(), key.next(), key.next()) {
                    (Some(root), Some(name), None) if root == b"hooks" => {
                        EVENTS.iter().position(|&event| name == event)
                    }
                    _ => None,
                };
                if let Some(event) = event {
                    let owned = !entry.multiline
                        && !entry.trailing_comment
                        && owned_inline(entry.value, event).is_ok();
                    if event == 2 && !owned {
                        foreign_pretool = true;
                    } else if found[event] {
                        return Err(Failure::ConflictingHooks);
                    } else if !owned {
                        return Err(Failure::AmbiguousOwnership);
                    } else {
                        found[event] = true;
                        edits.push(line.edit_start(data)..line.end)?;
                    }
                }
            }
        }
        strings.scan(data, line).map_err(malformed)?;
    }
    strings.finish().map_err(malformed)?;
    Ok((found, foreign_pretool))
}

pub(super) fn reconcile(
    data: &[u8],
    markers: Markers<'_>,
    command: &[u8],
    windows: &[u8],
    mut block: Output,
    upsert: bool,
) -> CodexResult<(Output, bool)> {
    let newline = newline(data);
    let mut edits = Edits::default();
    let pair = match markers.find(data).map_err(|_| Failure::ConflictingHooks)? {
        Span::Pair(begin, end) => {
            let range = begin.edit_start(data)..end.end;
            if owned_span(&data[range.clone()], Some(markers), newline)
                != Some(range.len())
            {
                return Err(Failure::AmbiguousOwnership);
            }
            edits.push(range.clone())?;
            range
        }
        Span::Absent => 0..0,
        Span::Orphan(_) => return Err(Failure::ConflictingHooks),
    };
    let mut markerless = false;
    let mut skip = 0;
    for line in lines(data, payload_start(data)..data.len()) {
        if line.start < skip || pair.contains(&line.start) {
            continue;
        }
        let tail = &data[line.start..];
        if tail.starts_with(b"[[hooks.SessionStart]]")
            && let Some(length) = owned_span(tail, None, newline)
        {
            if markerless {
                return Err(Failure::ConflictingHooks);
            }
            markerless = true;
            skip = line.start + length;
            edits.push(line.start..skip)?;
        }
    }
    let (found, foreign_pretool) = inline_edits(data, &mut edits)?;
    if markerless && (!pair.is_empty() || found.into_iter().any(|event| event)) {
        return Err(Failure::ConflictingHooks);
    }
    if foreign_pretool {
        block = build_block(command, windows, false)
            .map_err(|_| Failure::CommandRender)?;
    }
    let mut out = edits.apply(data)?;
    if upsert {
        let prefix = payload_start(&out);
        out.finish_line(newline, prefix)
            .map_err(|_| Failure::EditBuild)?;
        markers
            .append(&mut out, &block, newline)
            .map_err(|_| Failure::EditBuild)?;
    }
    Ok((out, upsert && !foreign_pretool))
}
