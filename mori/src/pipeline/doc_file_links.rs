//! Markdown file-to-file references for the pre-dump graph pass.
//!
//! The MENTIONS resolvers have a different contract. This pass counts textual
//! occurrences and links only to existing File nodes.

use std::collections::{BTreeMap, HashMap};
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::slice;

const PATH_CAPACITY: usize = 512;
const MAX_SEGMENTS: usize = 64;
const NO_DOC_FILES: c_int = -1;
const NO_REPO_PATH: c_int = -2;

#[repr(C)]
struct File {
    id: i64,
    path: *const c_char,
}

#[repr(C)]
struct Source {
    bytes: *mut u8,
    len: usize,
}

#[repr(C)]
pub(crate) struct DocFileLinkOps {
    file_at: extern "C" fn(*mut c_void, c_int) -> File,
    cancelled: extern "C" fn(*mut c_void) -> bool,
    read: extern "C" fn(*mut c_void, *const c_char) -> Source,
    release: extern "C" fn(*mut u8),
    insert_edge: extern "C" fn(*mut c_void, i64, i64, *const c_char) -> bool,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Strategy {
    Bare,
    Backtick,
    Inline,
}

impl Strategy {
    fn properties(self, count: usize) -> CString {
        let (name, confidence) = match self {
            Self::Bare => ("md_bare_mention", "0.70"),
            Self::Backtick => ("md_backtick_path", "0.85"),
            Self::Inline => ("md_inline_link", "0.95"),
        };
        CString::new(format!(
            r#"{{"strategy":"{name}","confidence":{confidence},"count":{count}}}"#
        ))
        .expect("edge properties contain only ASCII literals and digits")
    }
}

fn is_markdown(path: &[u8]) -> bool {
    path.ends_with(b".md") || path.ends_with(b".mdx")
}

fn normalize(path: &[u8], output: &mut [u8; PATH_CAPACITY]) -> Option<usize> {
    let mut starts = [0; MAX_SEGMENTS];
    let mut depth: usize = 0;
    let mut len = 0;
    for segment in path.split(|&byte| byte == b'/') {
        match segment {
            b"" | b"." => {}
            b".." => {
                depth = depth.checked_sub(1)?;
                len = starts[depth];
            }
            _ => {
                if depth >= MAX_SEGMENTS
                    || len + segment.len() + 2 >= output.len()
                {
                    return None;
                }
                starts[depth] = len;
                depth += 1;
                if len > 0 {
                    output[len] = b'/';
                    len += 1;
                }
                output[len..len + segment.len()].copy_from_slice(segment);
                len += segment.len();
            }
        }
    }
    (len > 0).then_some(len)
}

fn resolve(
    files: &HashMap<&[u8], i64>,
    dir: &[u8],
    mut target: &[u8],
) -> Option<i64> {
    let relative = target.starts_with(b"./");
    let rooted = target.starts_with(b"/");
    if rooted {
        target = &target[1..];
    } else {
        while let Some(rest) = target.strip_prefix(b"./") {
            target = rest;
        }
    }
    if target.is_empty() {
        return None;
    }

    let mut normalized = [0; PATH_CAPACITY];
    let joined_len = dir.len() + 1 + target.len();
    if !rooted && !dir.is_empty() && joined_len < PATH_CAPACITY {
        let mut joined = [0; PATH_CAPACITY];
        joined[..dir.len()].copy_from_slice(dir);
        joined[dir.len()] = b'/';
        joined[dir.len() + 1..joined_len].copy_from_slice(target);
        if let Some(len) = normalize(&joined[..joined_len], &mut normalized)
            && let Some(&id) = files.get(&normalized[..len])
        {
            return Some(id);
        }
    }
    if relative {
        return None;
    }
    let len = normalize(target, &mut normalized)?;
    files.get(&normalized[..len]).copied()
}

fn pathlike(token: &[u8]) -> bool {
    let mut basename_has_dot = false;
    for &byte in token {
        match byte {
            b'/' => basename_has_dot = false,
            b'.' => basename_has_dot = true,
            b'_' | b'-' | b'+' | b'@' | b'~' => {}
            _ if byte.is_ascii_alphanumeric() => {}
            _ => return false,
        }
    }
    basename_has_dot
}

fn bare_delimiter(byte: u8) -> bool {
    byte.is_ascii_whitespace()
        || byte == b'\x0b'
        || b"()[]{}<>\"',;:`*|".contains(&byte)
}

fn scan_line(line: &mut [u8], mut record: impl FnMut(&[u8], Strategy)) {
    let mut start = 0;
    while let Some(offset) =
        line[start..].windows(2).position(|pair| pair == b"](")
    {
        let link = start + offset;
        let target_start = link + 2;
        let Some(offset) =
            line[target_start..].iter().position(|&byte| byte == b')')
        else {
            break;
        };
        let end = target_start + offset;
        let target = &line[target_start..end];
        if target.len() < PATH_CAPACITY {
            let destination =
                target.split(|&byte| matches!(byte, b' ' | b'#')).next();
            if let Some(target) = destination
                && !target.is_empty()
                && !target.starts_with(b"mailto:")
                && !target.windows(3).any(|part| part == b"://")
            {
                record(target, Strategy::Inline);
            }
        }
        let open = line[..link]
            .iter()
            .rposition(|&byte| byte == b'[')
            .unwrap_or(link);
        line[open..=end].fill(b' ');
        start = end + 1;
    }

    start = 0;
    while let Some(offset) = line[start..].iter().position(|&byte| byte == b'`')
    {
        let open = start + offset;
        let Some(offset) =
            line[open + 1..].iter().position(|&byte| byte == b'`')
        else {
            break;
        };
        let end = open + 1 + offset;
        let token = &line[open + 1..end];
        if token.len() < PATH_CAPACITY && pathlike(token) {
            record(token, Strategy::Backtick);
        }
        line[open..=end].fill(b' ');
        start = end + 1;
    }

    for token in line.split(|&byte| bare_delimiter(byte)) {
        if token.len() >= PATH_CAPACITY {
            continue;
        }
        let Some(end) = token.iter().rposition(|&byte| byte != b'.') else {
            continue;
        };
        let token = &token[..=end];
        if token.contains(&b'/') && pathlike(token) {
            record(token, Strategy::Bare);
        }
    }
}

fn scan_file(
    files: &HashMap<&[u8], i64>,
    path: &[u8],
    source_id: i64,
    source: &mut [u8],
) -> BTreeMap<i64, (usize, Strategy)> {
    let dir = path
        .iter()
        .rposition(|&byte| byte == b'/')
        .map_or(&[][..], |end| &path[..end]);
    let mut targets = BTreeMap::new();
    if dir.len() >= PATH_CAPACITY {
        return targets;
    }

    // The native reader historically stopped at the first NUL, even in a
    // Markdown file containing binary data.
    let end = source
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(source.len());
    for line in source[..end].split_mut(|&byte| byte == b'\n') {
        let len = line.len() - usize::from(line.last() == Some(&b'\r'));
        let line = &mut line[..len];
        scan_line(line, |target, strategy| {
            if let Some(id) = resolve(files, dir, target)
                && id != source_id
            {
                let entry = targets.entry(id).or_insert((0, strategy));
                entry.0 += 1;
                entry.1 = entry.1.max(strategy);
            }
        });
    }
    targets
}

/// Run the pre-dump pass against a borrowed, ordered graph File view.
///
/// # Safety
/// The callbacks must keep every non-null path valid as a NUL-terminated
/// string until return. Each read must return null or an exclusive buffer
/// valid for `len` bytes, which is returned to `release` exactly once. Edge
/// properties are borrowed only for the duration of `insert_edge`.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mori_doc_file_links_run(
    ctx: *mut c_void,
    ops: &DocFileLinkOps,
    file_count: c_int,
    have_repo_path: bool,
) -> c_int {
    let mut has_doc = false;
    let mut ordered = Vec::new();
    if ordered.try_reserve(file_count.max(0) as usize).is_err() {
        return 0;
    }
    for i in 0..file_count {
        let file = (ops.file_at)(ctx, i);
        let path = if file.path.is_null() {
            None
        } else {
            // SAFETY: File paths are borrowed from nodes owned by the C graph.
            let path = unsafe { CStr::from_ptr(file.path) };
            has_doc |= is_markdown(path.to_bytes());
            Some(path)
        };
        ordered.push((file.id, path));
    }
    if !has_doc {
        return NO_DOC_FILES;
    }
    if !have_repo_path {
        return NO_REPO_PATH;
    }

    let mut files = HashMap::new();
    if files.try_reserve(ordered.len()).is_err() {
        return 0;
    }
    for &(id, path) in &ordered {
        if let Some(path) = path {
            files.insert(path.to_bytes(), id);
        }
    }

    let mut emitted = 0;
    for (source_id, path) in ordered {
        // Poll before every File node, including nodes the pass skips.
        if (ops.cancelled)(ctx) {
            break;
        }
        let Some(path) = path.filter(|path| is_markdown(path.to_bytes())) else {
            continue;
        };
        let source = (ops.read)(ctx, path.as_ptr());
        if source.bytes.is_null() {
            continue;
        }
        // SAFETY: The read callback exclusively lends this buffer until release.
        let bytes =
            unsafe { slice::from_raw_parts_mut(source.bytes, source.len) };
        let targets = scan_file(&files, path.to_bytes(), source_id, bytes);
        (ops.release)(source.bytes);
        for (target_id, (count, strategy)) in targets {
            let props = strategy.properties(count);
            if (ops.insert_edge)(ctx, source_id, target_id, props.as_ptr()) {
                emitted += 1;
            }
        }
    }
    emitted
}
