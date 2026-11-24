//! Recognize the pre-marker MCP server schema before removing it.

use super::managed::{Markers, Span};
use super::syntax::{
    Assignment, Key, Strings, assignment, header, lines, parse_string, single,
    trim_start,
};
use super::{Edit, Error, Output, Result};

pub(super) fn owned_basename(path: &[u8]) -> bool {
    matches!(
        path.rsplit(|&byte| matches!(byte, b'/' | b'\\')).next(),
        Some(b"codebase-memory-mcp" | b"codebase-memory-mcp.exe")
    )
}

fn empty_args(value: &[u8]) -> bool {
    value
        .strip_prefix(b"[")
        .is_some_and(|rest| trim_start(rest) == b"]")
}

fn owned_env_vars(value: &[u8]) -> bool {
    let Some(mut rest) = value.strip_prefix(b"[") else {
        return false;
    };
    loop {
        rest = trim_start(rest);
        if let Some(tail) = rest.strip_prefix(b"]") {
            return trim_start(tail).is_empty();
        }
        let Ok((name, consumed)) = parse_string(rest) else {
            return false;
        };
        if !matches!(name.as_slice(), b"CBM_CACHE_DIR" | b"CBM_RUNTIME_DIR") {
            return false;
        }
        rest = trim_start(&rest[consumed..]);
        if let Some(tail) = rest.strip_prefix(b",") {
            rest = tail;
        } else if !rest.starts_with(b"]") {
            return false;
        }
    }
}

#[derive(Default)]
struct Shape {
    command: Option<bool>,
    args: Option<bool>,
    env_vars: Option<bool>,
}

impl Shape {
    fn classify(&mut self, assignment: &Assignment<'_>) -> Result<bool> {
        let key = &assignment.key;
        let (seen, owned) = if single(key, b"command") {
            (
                &mut self.command,
                owned_basename(&assignment.string_value()?),
            )
        } else if single(key, b"args") {
            (
                &mut self.args,
                !assignment.multiline && empty_args(assignment.value),
            )
        } else if single(key, b"env_vars") {
            (
                &mut self.env_vars,
                !assignment.multiline && owned_env_vars(assignment.value),
            )
        } else {
            return Ok(false);
        };
        if seen.replace(owned).is_some() {
            return Err(Error);
        }
        Ok(owned)
    }
}

pub(super) fn remove(
    data: &[u8],
    desired: &Key,
    markers: Markers<'_>,
) -> Result<Edit> {
    let healed = match markers.find(data)? {
        Span::Pair(..) => return Ok(Edit::Unchanged),
        Span::Orphan(line) => {
            Some(Output::copy_without(data, line.start..line.end)?)
        }
        Span::Absent => None,
    };
    let scan = healed.as_deref().unwrap_or(data);
    let mut strings = Strings::default();
    let mut current = None::<Shape>;
    let mut exact_kind = None::<bool>;
    let mut foreign = false;
    let mut start = 0;
    let mut end = scan.len();
    for line in lines(scan, 0..scan.len()) {
        let multiline = strings.in_multiline();
        strings.scan(scan, line)?;
        if multiline {
            continue;
        }
        if let Some(table) = header(scan, line)? {
            if &table.key == desired {
                if exact_kind.is_some_and(|previous_array| {
                    !previous_array || !table.array
                }) {
                    return Err(Error);
                }
                exact_kind = Some(table.array);
                foreign |= table.array;
                current = Some(Shape::default());
                start = line.edit_start(scan);
            } else if let Some(shape) = current.take() {
                foreign |= table.key.starts_with(desired)
                    || shape.command != Some(true);
                end = line.edit_start(scan);
            }
        } else if let Some(shape) = &mut current
            && line.significant(scan)
        {
            let entry = assignment(scan, line)?.ok_or(Error)?;
            foreign |= !shape.classify(&entry)?;
        }
    }
    strings.finish()?;
    if foreign || current.is_some_and(|shape| shape.command != Some(true)) {
        Ok(Edit::Foreign)
    } else if exact_kind.is_some() {
        Output::copy_without(scan, start..end).map(Edit::Write)
    } else if let Some(healed) = healed {
        Ok(Edit::Write(healed))
    } else {
        Ok(Edit::Unchanged)
    }
}
