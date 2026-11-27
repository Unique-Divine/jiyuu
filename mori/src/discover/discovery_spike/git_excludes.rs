use std::env;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

const CONFIG_LINE_BYTES: u64 = 4095;

// Follow the C walk's user-config order. An explicit path wins even
// when the file at that path is missing.
pub(super) fn user_ignore_path() -> Option<PathBuf> {
    let home =
        environment_path("HOME").or_else(|| environment_path("USERPROFILE"));
    if let Some(path) = home
        .as_ref()
        .and_then(|home| configured_ignore(&home.join(".gitconfig"), Some(home)))
    {
        return Some(path);
    }

    let config_dir = environment_path("XDG_CONFIG_HOME")
        .or_else(|| Some(home.as_ref()?.join(".config")))?;
    let git_config_dir = config_dir.join("git");
    configured_ignore(&git_config_dir.join("config"), home.as_deref())
        .or_else(|| Some(git_config_dir.join("ignore")))
}

fn environment_path(key: &str) -> Option<PathBuf> {
    let raw = env::var_os(key)?;
    (!raw.is_empty()).then(|| normalized_path(raw.as_bytes()))
}

fn normalized_path(raw: &[u8]) -> PathBuf {
    let mut bytes = raw.to_vec();
    for byte in &mut bytes {
        if *byte == b'\\' {
            *byte = b'/';
        }
    }
    if matches!(
        bytes.as_slice(),
        [b'a'..=b'z', b':'] | [b'a'..=b'z', b':', b'/', ..]
    ) {
        bytes[0].make_ascii_uppercase();
    }
    PathBuf::from(OsStr::from_bytes(&bytes))
}

fn configured_ignore(path: &Path, home: Option<&Path>) -> Option<PathBuf> {
    let mut reader = BufReader::new(File::open(path).ok()?);
    let mut line_bytes = Vec::new();
    let mut in_core = false;
    let mut ignore = None;
    loop {
        line_bytes.clear();
        // Match the C parser's fixed-size fgets buffer, including its
        // treatment of overlong lines as consecutive chunks.
        let count = reader
            .by_ref()
            .take(CONFIG_LINE_BYTES)
            .read_until(b'\n', &mut line_bytes)
            .unwrap_or(0);
        if count == 0 {
            break;
        }
        let line =
            trim(line_bytes.split(|&byte| byte == 0).next().unwrap_or(&[]));
        if matches!(line.first(), None | Some(b'#' | b';')) {
            continue;
        }
        if let Some(section) = line.strip_prefix(b"[") {
            in_core = section.iter().position(|&byte| byte == b']').is_some_and(
                |end| trim(&section[..end]).eq_ignore_ascii_case(b"core"),
            );
        } else if in_core
            && let Some(eq) = line.iter().position(|&byte| byte == b'=')
            && trim(&line[..eq]).eq_ignore_ascii_case(b"excludesfile")
            && let Some(path) =
                expand_path(value_without_inline_comment(&line[eq + 1..]), home)
        {
            ignore = Some(path);
        }
    }
    ignore
}

fn space(byte: u8) -> bool {
    byte.is_ascii_whitespace() || byte == b'\x0b'
}

fn trim(bytes: &[u8]) -> &[u8] {
    let start = bytes.iter().position(|&byte| !space(byte));
    let end = bytes.iter().rposition(|&byte| !space(byte));
    match (start, end) {
        (Some(start), Some(end)) => &bytes[start..=end],
        _ => &[],
    }
}

fn value_without_inline_comment(raw: &[u8]) -> &[u8] {
    let value = trim(raw);
    let mut quote = None;
    let mut end = value.len();
    for (index, &byte) in value.iter().enumerate() {
        let previous = index.checked_sub(1).map(|i| value[i]);
        if matches!(byte, b'\'' | b'"') && previous != Some(b'\\') {
            if quote == Some(byte) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(byte);
            }
        } else if quote.is_none()
            && matches!(byte, b'#' | b';')
            && previous.is_none_or(space)
        {
            end = index;
            break;
        }
    }
    let value = trim(&value[..end]);
    match value {
        [first @ (b'\'' | b'"'), rest @ .., last] if first == last => rest,
        _ => value,
    }
}

fn expand_path(value: &[u8], home: Option<&Path>) -> Option<PathBuf> {
    let path = normalized_path(value);
    match path.as_os_str().as_bytes() {
        b"" => None,
        b"~" => home.map(Path::to_owned),
        [b'~', b'/', tail @ ..] => {
            // C appends the tail even if it starts with an extra slash.
            let start = tail
                .iter()
                .position(|&byte| byte != b'/')
                .unwrap_or(tail.len());
            Some(home?.join(OsStr::from_bytes(&tail[start..])))
        }
        [b'~', ..] => None,
        _ => Some(path),
    }
}
