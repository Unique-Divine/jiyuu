//! Stream framing for the linked Mori MCP frontend. The daemon session owns
//! JSON-RPC dispatch and request cancellation.

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

#[cfg(all(feature = "c-backend", not(test)))]
pub(crate) mod c_transport {
    use std::ffi::{c_char, c_int, c_void};
    use std::ptr;
    use std::slice;

    use super::*;

    type Stream = *mut c_void;
    type ReadMessage =
        unsafe extern "C" fn(Stream, *mut *mut c_char, *mut bool) -> c_int;
    type WriteResponse =
        unsafe extern "C" fn(Stream, *const u8, u32, bool) -> bool;

    unsafe extern "C" {
        fn cbm_daemon_frontend_set_stdio(
            read: ReadMessage,
            write: WriteResponse,
        );
        fn fgetc(stream: Stream) -> c_int;
        fn fread(
            buf: *mut u8,
            size: usize,
            count: usize,
            stream: Stream,
        ) -> usize;
        fn fwrite(
            buf: *const u8,
            size: usize,
            count: usize,
            stream: Stream,
        ) -> usize;
        fn ferror(stream: Stream) -> c_int;
        fn fflush(stream: Stream) -> c_int;
        fn malloc(size: usize) -> *mut u8;
    }

    // C owns the FILE and gives each callback exclusive access for its duration.
    struct FileStream(Stream);

    impl Read for FileStream {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            // SAFETY: self.0 is the live C stream. Reads stay within buf.
            let count = unsafe {
                if let [byte] = buf {
                    let next = fgetc(self.0);
                    if next < 0 {
                        0
                    } else {
                        *byte = next as u8;
                        1
                    }
                } else {
                    fread(buf.as_mut_ptr(), 1, buf.len(), self.0)
                }
            };
            // SAFETY: the stream remains valid until the callback returns.
            if count == 0 && unsafe { ferror(self.0) } != 0 {
                Err(io::Error::other("reading MCP stdio failed"))
            } else {
                Ok(count)
            }
        }
    }

    impl Write for FileStream {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            // SAFETY: C borrowed this live stream and the byte range is valid.
            let count =
                unsafe { fwrite(bytes.as_ptr(), 1, bytes.len(), self.0) };
            if count != bytes.len() {
                Err(io::Error::other("writing MCP stdio failed"))
            } else {
                Ok(count)
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            // SAFETY: C borrowed this live stream.
            if unsafe { fflush(self.0) } != 0 {
                Err(io::Error::other("flushing MCP stdio failed"))
            } else {
                Ok(())
            }
        }
    }

    unsafe extern "C" fn read(
        stream: Stream,
        message: *mut *mut c_char,
        framed: *mut bool,
    ) -> c_int {
        // SAFETY: the C frontend supplies writable output arguments and a live
        // input FILE. Initialize both outputs even on EOF and failure.
        unsafe {
            *message = ptr::null_mut();
            *framed = false;
        }
        let (bytes, framing) = match read_message(&mut FileStream(stream)) {
            Ok(None) => return 0,
            Ok(Some(frame)) => frame,
            Err(_) => return -1,
        };
        // SAFETY: C uses free for the message, so allocate with the same
        // allocator. The parser caps bytes.len() and rejects embedded NULs.
        unsafe {
            let output = malloc(bytes.len() + 1);
            if output.is_null() {
                return -1;
            }
            ptr::copy_nonoverlapping(bytes.as_ptr(), output, bytes.len());
            *output.add(bytes.len()) = 0;
            *message = output.cast();
            *framed = framing == Framing::ContentLength;
        }
        1
    }

    unsafe extern "C" fn write(
        stream: Stream,
        response: *const u8,
        length: u32,
        framed: bool,
    ) -> bool {
        let framing = if framed {
            Framing::ContentLength
        } else {
            Framing::Line
        };
        // SAFETY: C gives its writer the full response range for this callback.
        let response =
            unsafe { slice::from_raw_parts(response, length as usize) };
        write_response(&mut FileStream(stream), response, framing).is_ok()
    }

    pub(crate) unsafe fn register() {
        // SAFETY: caller registers once before any C frontend thread exists.
        unsafe { cbm_daemon_frontend_set_stdio(read, write) };
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
