//! Converts the process arguments into the C `argv` strings.

use std::ffi::{CString, OsString};

/// Returns one C string per argument, in order.
///
/// On Unix an argument is the exact bytes the kernel passed. Those bytes
/// cannot hold a NUL, but if one appears the string stops there, which is
/// what C would have read. Elsewhere arguments go through UTF-8 (lossy); on
/// Windows the C program rebuilds argv from the wide command line anyway.
pub fn c_args<I>(args: I) -> Vec<CString>
where
    I: IntoIterator<Item = OsString>,
{
    args.into_iter().map(c_arg).collect()
}

fn c_arg(arg: OsString) -> CString {
    let mut bytes = os_bytes(arg);
    if let Some(nul) = bytes.iter().position(|b| *b == 0) {
        bytes.truncate(nul);
    }
    CString::new(bytes).unwrap_or_default()
}

#[cfg(unix)]
fn os_bytes(arg: OsString) -> Vec<u8> {
    use std::os::unix::ffi::OsStringExt;
    arg.into_vec()
}

#[cfg(not(unix))]
fn os_bytes(arg: OsString) -> Vec<u8> {
    arg.to_string_lossy().into_owned().into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[CString]) -> Vec<&[u8]> {
        args.iter().map(|a| a.as_bytes()).collect()
    }

    #[test]
    fn keeps_order_and_empty_arguments() {
        let args =
            c_args(["mori", "cli", "", "list_projects"].map(OsString::from));
        assert_eq!(
            strings(&args),
            [&b"mori"[..], b"cli", b"", b"list_projects"]
        );
    }

    #[test]
    fn stops_at_an_interior_nul() {
        let args = c_args([OsString::from("a\0b")]);
        assert_eq!(strings(&args), [&b"a"[..]]);
    }

    #[cfg(unix)]
    #[test]
    fn passes_non_utf8_bytes_through() {
        use std::os::unix::ffi::OsStringExt;
        let raw = vec![b'p', 0xff, b'q'];
        let args = c_args([OsString::from_vec(raw.clone())]);
        assert_eq!(args[0].as_bytes(), raw.as_slice());
    }
}
