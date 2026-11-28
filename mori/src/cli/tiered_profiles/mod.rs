//! Install and remove the Scout, Verify, and Audit documents for each client.

use std::ffi::CStr;

#[cfg(feature = "native-engine-abi")]
#[path = "native.rs"]
mod native;

const PATH_CAPACITY: usize = 1024; // CLI_BUF_1K

// Keep the vendor suffix and directory bytes. Both slash styles are accepted
// even on POSIX, as in the installer.
fn sibling_path<'a>(
    verify: &[u8],
    slug: &[u8],
    buffer: &'a mut [u8; PATH_CAPACITY],
) -> Option<&'a CStr> {
    let start = verify
        .iter()
        .rposition(|&byte| matches!(byte, b'/' | b'\\'))
        .map_or(0, |separator| separator + 1);
    let suffix = verify[start..].strip_prefix(b"codebase-memory")?;
    if !suffix.starts_with(b".") {
        return None;
    }
    let end = start + slug.len() + suffix.len();
    let path = buffer.get_mut(..=end)?;
    path[..start].copy_from_slice(&verify[..start]);
    path[start..start + slug.len()].copy_from_slice(slug);
    path[start + slug.len()..end].copy_from_slice(suffix);
    path[end] = 0;
    CStr::from_bytes_with_nul(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn siblings_preserve_path_bytes_and_vendor_suffixes() {
        let mut buffer = [0; PATH_CAPACITY];
        let slug = b"codebase-memory-scout";
        for (input, expected) in [
            (
                b"/home/\xff/agents/codebase-memory.agent.md".as_slice(),
                b"/home/\xff/agents/codebase-memory-scout.agent.md".as_slice(),
            ),
            (
                b"C:\\users\\codebase-memory.toml",
                b"C:\\users\\codebase-memory-scout.toml",
            ),
            (b"codebase-memory.json", b"codebase-memory-scout.json"),
        ] {
            assert_eq!(
                sibling_path(input, slug, &mut buffer).unwrap().to_bytes(),
                expected
            );
        }
        for input in [
            b"".as_slice(),
            b"codebase-memory",
            b"codebase-memory-scout.toml",
            b"codebase-memory.toml/other",
        ] {
            assert!(sibling_path(input, slug, &mut buffer).is_none());
        }
    }

    #[test]
    fn sibling_capacity_includes_terminator() {
        let mut buffer = [0; PATH_CAPACITY];
        let mut path = b"codebase-memory.".to_vec();
        path.resize(PATH_CAPACITY - 1, b'x');
        assert!(sibling_path(&path, b"codebase-memory", &mut buffer).is_some());
        path.push(b'x');
        assert!(sibling_path(&path, b"codebase-memory", &mut buffer).is_none());
    }
}
