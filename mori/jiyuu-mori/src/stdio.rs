//! Stream framing and input routing for the linked Mori MCP frontend.

use std::io::{self, Read, Write};

// Match the standalone C protocol bounds in src/mcp/mcp.c.
const MAX_MESSAGE: usize = 10 * 1024 * 1024;
const MAX_HEADER: usize = 8 * 1024;
const LENGTH_PREFIX: &[u8] = b"Content-Length:";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Framing {
    Line,
    ContentLength,
}

fn read_line(
    input: &mut impl Read,
    line: &mut Vec<u8>,
    limit: usize,
) -> io::Result<usize> {
    line.clear();
    let mut byte = [0];
    while input.read(&mut byte)? != 0 {
        if line.len() == limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "MCP line exceeds limit",
            ));
        }
        if line.len() == line.capacity() {
            let additional = (limit - line.len()).min(line.capacity().max(256));
            line.try_reserve_exact(additional)
                .map_err(io::Error::other)?;
        }
        line.push(byte[0]);
        if byte[0] == b'\n' {
            break;
        }
    }
    if line.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "NUL in MCP line",
        ));
    }
    let bytes_read = line.len();
    while matches!(line.last(), Some(b'\n' | b'\r')) {
        line.pop();
    }
    Ok(bytes_read)
}

fn horizontal_space(byte: &u8) -> bool {
    matches!(byte, b' ' | b'\t')
}

fn content_length(value: &[u8]) -> Option<usize> {
    let start = value.iter().position(|byte| !horizontal_space(byte))?;
    let value = &value[start..];
    let digits = value
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if !value[digits..].iter().all(horizontal_space) {
        return None;
    }
    let mut length = 0usize;
    for digit in &value[..digits] {
        length = length
            .checked_mul(10)?
            .checked_add(usize::from(*digit - b'0'))?;
    }
    (1..=MAX_MESSAGE).contains(&length).then_some(length)
}

fn read_message(
    input: &mut impl Read,
) -> io::Result<Option<(Vec<u8>, Framing)>> {
    let mut message = Vec::new();
    let first_line_bytes = loop {
        let bytes_read = read_line(input, &mut message, MAX_MESSAGE)?;
        if bytes_read == 0 {
            return Ok(None);
        }
        if !message.is_empty() {
            break bytes_read;
        }
    };
    let Some(value) = message.strip_prefix(LENGTH_PREFIX) else {
        return Ok(Some((message, Framing::Line)));
    };
    let Some(length) =
        content_length(value).filter(|_| first_line_bytes <= MAX_HEADER)
    else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid MCP Content-Length",
        ));
    };
    let mut header_bytes = first_line_bytes;
    loop {
        if header_bytes == MAX_HEADER {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "MCP header exceeds limit",
            ));
        }
        let bytes_read =
            read_line(input, &mut message, MAX_HEADER - header_bytes)?;
        if bytes_read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "incomplete MCP header",
            ));
        }
        if message.is_empty() {
            break;
        }
        header_bytes += bytes_read;
    }

    message
        .try_reserve_exact(length)
        .map_err(io::Error::other)?;
    message.resize(length, 0);
    input.read_exact(&mut message)?;
    if message.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "NUL in MCP body",
        ));
    }
    Ok(Some((message, Framing::ContentLength)))
}

fn write_response(
    output: &mut impl Write,
    response: &[u8],
    framing: Framing,
) -> io::Result<()> {
    let written = if framing == Framing::ContentLength {
        write!(output, "Content-Length: {}\r\n\r\n", response.len())
            .and_then(|()| output.write_all(response))
    } else {
        output
            .write_all(response)
            .and_then(|()| output.write_all(b"\n"))
    };
    let flushed = output.flush();
    written.and(flushed)
}

pub(crate) mod frontend_receive {
    use std::ffi::{c_char, c_int, c_void};
    use std::ptr;

    use super::{Framing, Read, read_message};

    #[cfg(all(feature = "native-engine-abi", not(test)))]
    unsafe extern "C" {
        // C's MemorySanitizer cannot see storage initialized by Rust.
        fn cbm_ffi_mark_written(address: *const c_void, size: usize);
    }

    #[repr(C)]
    pub(crate) struct FrontendState {
        _opaque: [u8; 0],
    }

    /// Admission callbacks borrow the session. Enqueue takes the buffer on success.
    #[repr(C)]
    pub(crate) struct ReceiveOps {
        pub(crate) alloc_message: unsafe extern "C" fn(usize) -> *mut c_void,
        pub(crate) free_message: unsafe extern "C" fn(*mut c_void),
        pub(crate) route_cancellation: unsafe extern "C" fn(
            *mut FrontendState,
            *const c_char,
            *mut bool,
        ) -> bool,
        pub(crate) cancel_active:
            unsafe extern "C" fn(*mut FrontendState) -> bool,
        pub(crate) enqueue:
            unsafe extern "C" fn(*mut FrontendState, *mut c_char, bool) -> bool,
        pub(crate) stop_begin: unsafe extern "C" fn(*mut FrontendState) -> bool,
    }

    pub(crate) unsafe fn receive_loop(
        input: &mut impl Read,
        state: *mut FrontendState,
        ops: &ReceiveOps,
        stop_begun: &mut bool,
    ) -> c_int {
        // SAFETY: The caller keeps the opaque state live for the operations.
        unsafe {
            loop {
                let (bytes, framing) = match read_message(input) {
                    Ok(None) => return 0,
                    Ok(Some(frame)) => frame,
                    Err(_) => return -1,
                };

                // The decoder bounds the length and rejects embedded NULs.
                // Use the native allocator because the queue frees the copy.
                let message = (ops.alloc_message)(bytes.len() + 1).cast::<u8>();
                if message.is_null() {
                    return -1;
                }
                ptr::copy_nonoverlapping(bytes.as_ptr(), message, bytes.len());
                *message.add(bytes.len()) = 0;
                #[cfg(all(feature = "native-engine-abi", not(test)))]
                cbm_ffi_mark_written(message.cast(), bytes.len() + 1);
                drop(bytes);

                // Route cancellation before enqueue can block on a full queue.
                let mut active = false;
                if (ops.route_cancellation)(state, message.cast(), &mut active) {
                    (ops.free_message)(message.cast());
                    if active && !(ops.cancel_active)(state) {
                        *stop_begun = (ops.stop_begin)(state);
                        return -1;
                    }
                    continue;
                }

                if !(ops.enqueue)(
                    state,
                    message.cast(),
                    framing == Framing::ContentLength,
                ) {
                    (ops.free_message)(message.cast());
                    return -1;
                }
            }
        }
    }

    #[cfg(test)]
    mod tests;
}

#[cfg(feature = "native-engine-abi")]
pub(crate) mod c_transport {
    use std::ffi::c_void;

    use super::*;

    unsafe extern "C" {
        fn mori_frontend_read_bytes(
            stream: *mut c_void,
            bytes: *mut u8,
            size: usize,
        ) -> isize;
        fn mori_frontend_write_bytes(
            stream: *mut c_void,
            bytes: *const u8,
            size: usize,
        ) -> bool;
        fn mori_frontend_flush(stream: *mut c_void) -> bool;
    }

    // C lends the FILE for the call and performs I/O with its own CRT.
    pub(crate) struct FileStream(pub(crate) *mut c_void);

    impl Read for FileStream {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }
            let count = unsafe {
                mori_frontend_read_bytes(self.0, buf.as_mut_ptr(), buf.len())
            };
            usize::try_from(count)
                .map_err(|_| io::Error::other("reading MCP stdio failed"))
        }
    }

    impl Write for FileStream {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if unsafe {
                mori_frontend_write_bytes(self.0, bytes.as_ptr(), bytes.len())
            } {
                Ok(bytes.len())
            } else {
                Err(io::Error::other("writing MCP stdio failed"))
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            if unsafe { mori_frontend_flush(self.0) } {
                Ok(())
            } else {
                Err(io::Error::other("flushing MCP stdio failed"))
            }
        }
    }

    pub(crate) fn write_frame(
        stream: *mut c_void,
        response: &[u8],
        framed: bool,
    ) -> bool {
        let framing = if framed {
            Framing::ContentLength
        } else {
            Framing::Line
        };
        write_response(&mut FileStream(stream), response, framing).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_mixed_frames_and_drains_to_eof() {
        let mut input =
            &b"\r\n{}\r\nContent-Length:\t5 \r\nX-Header: value\r\n\r\n{\n  }{}"
                [..];
        assert_eq!(
            read_message(&mut input).unwrap(),
            Some((b"{}".to_vec(), Framing::Line))
        );
        assert_eq!(
            read_message(&mut input).unwrap(),
            Some((b"{\n  }".to_vec(), Framing::ContentLength)),
        );
        assert_eq!(
            read_message(&mut input).unwrap(),
            Some((b"{}".to_vec(), Framing::Line))
        );
        assert_eq!(read_message(&mut input).unwrap(), None);
    }

    #[test]
    fn rejects_incomplete_invalid_and_oversized_frames() {
        for invalid in [
            &b"Content-Length: 2\n"[..],
            b"Content-Length: 2\n\n{",
            b"Content-Length: +2\n\n{}",
            b"Content-Length: 2x\n\n{}",
            b"Content-Length: 0\n\n",
            b"Content-Length: 10485761\n\n",
            b"Content-Length: 9999999999999999999999999\n\n",
            b"Content-Length: 2\n\n\0}",
            b"{\0}\n",
        ] {
            assert!(read_message(&mut &invalid[..]).is_err(), "{invalid:?}");
        }
        let too_large = vec![b'a'; MAX_MESSAGE + 1];
        assert!(read_message(&mut too_large.as_slice()).is_err());
        let mut header = b"Content-Length: 2\n".to_vec();
        header.resize(MAX_HEADER, b' ');
        header.extend_from_slice(b"\n\n{}");
        assert!(read_message(&mut header.as_slice()).is_err());
    }

    #[test]
    fn response_preserves_request_framing_and_counts_bytes() {
        let response = "{\"é\":true}".as_bytes();
        let mut output = Vec::new();
        write_response(&mut output, response, Framing::Line).unwrap();
        assert_eq!(output, [response, b"\n"].concat());
        output.clear();
        write_response(&mut output, response, Framing::ContentLength).unwrap();
        assert_eq!(output, [b"Content-Length: 11\r\n\r\n", response].concat());
    }
}
