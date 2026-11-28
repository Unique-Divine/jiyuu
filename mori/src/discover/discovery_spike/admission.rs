//! Count indexable files without collecting discovery results.

use super::{
    Directory, FileVerdict, IgnoreAt, IgnoreStack, Mode, Suffixes, Walk,
};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::{fs, io};

const C_PATH_CAPACITY: usize = 4096;

unsafe extern "C" {
    fn cbm_now_ms() -> u64;
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum Count {
    Exact(usize),
    LimitExceeded(usize),
}

struct Frame {
    relative: PathBuf,
    under_gitignore: bool,
    ignore_stack: Rc<IgnoreStack>,
    suffixes: Rc<Suffixes>,
}

/// Use the existing discovery filters with the C walk's admission order.
/// The deadline is absolute in the C monotonic clock domain. Zero disables it.
pub(super) fn count_bounded(
    root: &Path,
    mode: Mode,
    max_files: usize,
    deadline_ms: u64,
) -> io::Result<Count> {
    check_deadline(deadline_ms)?;
    let (walk, root, ignore_stack) = Walk::prepare(root, mode, 0)?;
    let mut pending = vec![Frame {
        relative: PathBuf::new(),
        under_gitignore: false,
        ignore_stack: Rc::new(ignore_stack),
        suffixes: Rc::new(Suffixes::default()),
    }];
    let mut count = 0;
    while let Some(mut frame) = pending.pop() {
        check_deadline(deadline_ms)?;
        let directory = root.join(&frame.relative);
        check_path(&directory)?;
        if !frame.relative.as_os_str().is_empty()
            && let Some(rule) =
                IgnoreAt::load(&directory, &directory.join(".gitignore"))
        {
            Rc::make_mut(&mut frame.ignore_stack).push(rule);
        }
        if let Some(suffixes) = Suffixes::read(&directory) {
            frame.suffixes = Rc::new(suffixes);
        }

        // C scans all entries in a directory before visiting any of its children,
        // then pops children in reverse discovery order. The order matters if the
        // count can reach the ceiling before an unreadable directory is opened.
        let mut entries = fs::read_dir(directory)?;
        loop {
            check_deadline(deadline_ms)?;
            let Some(entry) = entries.next() else {
                break;
            };
            let entry = entry?;
            let name = entry.file_name();
            let relative = frame.relative.join(&name);
            let path = entry.path();
            check_path(&path)?;
            check_path(&relative)?;
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.is_dir() {
                let under_gitignore = match walk.classify_dir(
                    &name,
                    &path,
                    &relative,
                    frame.under_gitignore,
                    &frame.ignore_stack,
                ) {
                    Directory::Keep => false,
                    Directory::GitIgnored
                        if walk.tracked.dirs.contains(&relative) =>
                    {
                        true
                    }
                    Directory::GitIgnored | Directory::Skip => continue,
                };
                pending.push(Frame {
                    relative,
                    under_gitignore,
                    ignore_stack: Rc::clone(&frame.ignore_stack),
                    suffixes: Rc::clone(&frame.suffixes),
                });
            } else if metadata.is_file()
                && matches!(
                    walk.classify_file(
                        &path,
                        &relative,
                        metadata.len(),
                        frame.under_gitignore,
                        &frame.ignore_stack,
                        &frame.suffixes,
                    ),
                    FileVerdict::Include(_)
                )
            {
                if count == max_files {
                    check_deadline(deadline_ms)?;
                    return Ok(Count::LimitExceeded(count));
                }
                count += 1;
            }
        }
    }
    Ok(Count::Exact(count))
}

fn check_path(path: &Path) -> io::Result<()> {
    if path.as_os_str().len() < C_PATH_CAPACITY {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "discovery path exceeds the native path capacity",
        ))
    }
}

fn check_deadline(deadline_ms: u64) -> io::Result<()> {
    // SAFETY: the native clock takes no arguments and has no caller state.
    if deadline_ms != 0 && unsafe { cbm_now_ms() } >= deadline_ms {
        Err(io::ErrorKind::TimedOut.into())
    } else {
        Ok(())
    }
}
