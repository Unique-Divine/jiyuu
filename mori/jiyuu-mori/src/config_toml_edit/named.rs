//! Named array elements and their contiguous descendant tables.

use super::syntax::{
    Key, Line, Strings, assignment, header, lines, newline, overlaps,
    payload_start, single, validate_strings,
};
use super::{Edit, Error, Output, Result};

pub(super) enum Operation<'a> {
    Merge(&'a [u8]),
    Remove,
    InstallExact(&'a [u8]),
    RemoveExact(&'a [u8]),
}

impl<'a> Operation<'a> {
    fn body(&self) -> Option<&'a [u8]> {
        match *self {
            Self::Merge(body)
            | Self::InstallExact(body)
            | Self::RemoveExact(body) => Some(body),
            Self::Remove => None,
        }
    }
}

pub(super) struct Entries(Vec<(Key, Line)>);

pub(super) fn validate_body(
    operation: &Operation<'_>,
    identity_key: &[u8],
    identity: &[u8],
) -> Result<Entries> {
    let Some(body) = operation.body() else {
        return Ok(Entries(Vec::new()));
    };
    validate_strings(body)?;
    let mut entries: Vec<(Key, Line)> = Vec::new();
    let mut has_identity = false;
    for line in lines(body, 0..body.len()) {
        if header(body, line)?.is_some() {
            return Err(Error);
        }
        if !line.significant(body) {
            continue;
        }
        let entry = assignment(body, line)?.ok_or(Error)?;
        if entry.multiline
            || entry.key.len() != 1
            || entries.iter().any(|(key, _)| *key == entry.key)
        {
            return Err(Error);
        }
        if single(&entry.key, identity_key) {
            if entry.string_value()? != identity {
                return Err(Error);
            }
            has_identity = true;
        }
        entries.push((entry.key, line));
    }
    if has_identity {
        Ok(Entries(entries))
    } else {
        Err(Error)
    }
}

#[derive(Default)]
struct Target {
    start: usize,
    header_end: usize,
    direct_significant_end: usize,
    last_significant_end: usize,
    descendants: bool,
    identity_matches: Option<bool>,
}

fn finish(
    current: &mut Option<Target>,
    matched: &mut Option<Target>,
) -> Result<()> {
    if let Some(current) = current.take()
        && current.identity_matches.ok_or(Error)?
        && matched.replace(current).is_some()
    {
        return Err(Error);
    }
    Ok(())
}

fn scan(
    data: &[u8],
    table: &[u8],
    identity_key: &[u8],
    identity: &[u8],
) -> Result<Option<Target>> {
    let root = vec![table.to_vec()];
    let mut matched = None;
    let mut current = None::<Target>;
    let mut strings = Strings::default();
    let mut at_root = true;
    for line in lines(data, 0..data.len()) {
        let multiline = strings.in_multiline();
        strings.scan(data, line)?;
        if !multiline && let Some(header) = header(data, line)? {
            at_root = false;
            if header.key == root {
                if !header.array {
                    return Err(Error);
                }
                finish(&mut current, &mut matched)?;
                current = Some(Target {
                    start: line.edit_start(data),
                    header_end: line.end,
                    direct_significant_end: line.end,
                    last_significant_end: line.end,
                    ..Target::default()
                });
            } else if header.key.starts_with(&root) {
                let target = current.as_mut().ok_or(Error)?;
                target.descendants = true;
                target.last_significant_end = line.end;
            } else {
                finish(&mut current, &mut matched)?;
            }
            continue;
        }
        if let Some(target) = &mut current {
            let significant = line.significant(data);
            if significant {
                target.last_significant_end = line.end;
                if !target.descendants {
                    target.direct_significant_end = line.end;
                }
            }
            if !multiline {
                let entry = assignment(data, line)?;
                if significant && entry.is_none() {
                    return Err(Error);
                }
                if !target.descendants
                    && let Some(entry) = entry
                    && single(&entry.key, identity_key)
                    && target
                        .identity_matches
                        .replace(entry.string_value()? == identity)
                        .is_some()
                {
                    return Err(Error);
                }
            }
        } else if at_root
            && !multiline
            && let Some(entry) = assignment(data, line)?
            && overlaps(&entry.key, &root)
        {
            return Err(Error);
        }
    }
    strings.finish()?;
    finish(&mut current, &mut matched)?;
    Ok(matched)
}

fn render(
    out: &mut Output,
    table: &[u8],
    body: &[u8],
    newline: &[u8],
) -> Result<()> {
    out.append(b"[[")?;
    out.append(table)?;
    out.append(b"]]")?;
    out.append(newline)?;
    out.normalized(body, newline)?;
    if !body.is_empty() && !body.ends_with(b"\n") {
        out.append(newline)?;
    }
    Ok(())
}

fn append_entry(
    out: &mut Output,
    line: Line,
    body: &[u8],
    newline: &[u8],
) -> Result<()> {
    out.append(line.content(body))?;
    out.append(newline)
}

fn merge(
    data: &[u8],
    target: Target,
    body: &[u8],
    entries: Entries,
    newline: &[u8],
) -> Result<Output> {
    let mut out = Output::default();
    out.append(&data[..target.header_end])?;
    let mut emitted = vec![false; entries.0.len()];
    let mut strings = Strings::default();
    for line in lines(data, target.header_end..target.direct_significant_end) {
        let desired = if strings.in_multiline() {
            None
        } else {
            assignment(data, line)?.and_then(|entry| {
                entries
                    .0
                    .iter()
                    .position(|(key, _)| *key == entry.key)
                    .map(|i| (i, entry.multiline))
            })
        };
        if let Some((i, multiline)) = desired {
            if emitted[i] || multiline {
                return Err(Error);
            }
            append_entry(&mut out, entries.0[i].1, body, newline)?;
            emitted[i] = true;
        } else {
            out.append(line.raw(data))?;
        }
        strings.scan(data, line)?;
    }
    strings.finish()?;
    for ((_, line), emitted) in entries.0.into_iter().zip(emitted) {
        if !emitted {
            out.finish_line(newline, 0)?;
            append_entry(&mut out, line, body, newline)?;
        }
    }
    out.append(&data[target.direct_significant_end..])?;
    Ok(out)
}

fn insert(
    data: &[u8],
    table: &[u8],
    body: &[u8],
    newline: &[u8],
) -> Result<Output> {
    let mut out = Output::start_append(data, newline)?;
    if data.len() > payload_start(data)
        && (out.len() < newline.len() * 2
            || &out[out.len() - newline.len() * 2..out.len() - newline.len()]
                != newline)
    {
        out.append(newline)?;
    }
    render(&mut out, table, body, newline)?;
    Ok(out)
}

pub(super) fn edit(
    data: &[u8],
    table: &[u8],
    identity_key: &[u8],
    identity: &[u8],
    operation: Operation<'_>,
    entries: Entries,
) -> Result<Edit> {
    let target = scan(data, table, identity_key, identity)?;
    let newline = newline(data);
    match (target, operation) {
        (None, Operation::Remove | Operation::RemoveExact(_)) => {
            Ok(Edit::Unchanged)
        }
        (None, Operation::Merge(body) | Operation::InstallExact(body)) => {
            insert(data, table, body, newline).map(Edit::Write)
        }
        (Some(target), Operation::Remove) => {
            Output::copy_without(data, target.start..target.last_significant_end)
                .map(Edit::Write)
        }
        (Some(target), Operation::Merge(body)) => {
            merge(data, target, body, entries, newline).map(Edit::Write)
        }
        (
            Some(target),
            operation @ (Operation::InstallExact(body)
            | Operation::RemoveExact(body)),
        ) => {
            if target.direct_significant_end != target.last_significant_end {
                return Ok(Edit::Foreign);
            }
            let mut expected = Output::default();
            render(&mut expected, table, body, newline)?;
            if data[target.start..target.direct_significant_end] != *expected {
                Ok(Edit::Foreign)
            } else if matches!(operation, Operation::RemoveExact(_)) {
                Output::copy_without(
                    data,
                    target.start..target.last_significant_end,
                )
                .map(Edit::Write)
            } else {
                Ok(Edit::Unchanged)
            }
        }
    }
}
