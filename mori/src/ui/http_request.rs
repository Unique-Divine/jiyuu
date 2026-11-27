//! HTTP/1 request framing for the loopback Graph UI.
//!
//! Keep the raw target and selected headers as bytes for the existing route
//! handlers. The socket loop owns the body and applies one deadline to the
//! head and body together.

pub(super) const MAX_HEAD: usize = 16 * 1024;
pub(super) const MAX_BODY: usize = 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum RequestError {
    BadRequest,
    LengthRequired,
    ContentTooLarge,
    HeaderTooLarge,
}

impl RequestError {
    pub(super) fn status(&self) -> u16 {
        match self {
            Self::BadRequest => 400,
            Self::LengthRequired => 411,
            Self::ContentTooLarge => 413,
            Self::HeaderTooLarge => 431,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct RequestHead<'a> {
    pub(super) method: &'a [u8],
    pub(super) path: &'a [u8],
    pub(super) query: &'a [u8],
    pub(super) http_minor: u8,
    pub(super) origin: &'a [u8],
    pub(super) host: &'a [u8],
    pub(super) content_type: &'a [u8],
    pub(super) accept_language: &'a [u8],
    pub(super) content_length: usize,
    pub(super) body_offset: usize,
}

/// On a partial read, retain the cursor and pass the growing buffer back.
/// A complete head can share its final read with any number of body bytes.
#[derive(Default)]
pub(super) struct HeadScanner {
    scanned: usize,
}

impl HeadScanner {
    pub(super) fn scan(
        &mut self,
        bytes: &[u8],
    ) -> Result<Option<usize>, RequestError> {
        let limit = bytes.len().min(MAX_HEAD);
        for index in self.scanned..limit {
            match bytes[index] {
                0 => return Err(RequestError::BadRequest),
                b'\n' if index == 0 || bytes[index - 1] != b'\r' => {
                    return Err(RequestError::BadRequest);
                }
                b'\n'
                    if index >= 3
                        && &bytes[index - 3..=index] == b"\r\n\r\n" =>
                {
                    return Ok(Some(index + 1));
                }
                _ => {}
            }
        }
        self.scanned = limit;
        if limit == MAX_HEAD {
            return Err(RequestError::HeaderTooLarge);
        }
        Ok(None)
    }
}

fn split_at_byte(bytes: &[u8], separator: u8) -> Option<(&[u8], &[u8])> {
    let position = bytes.iter().position(|byte| *byte == separator)?;
    Some((&bytes[..position], &bytes[position + 1..]))
}

fn strip_http_whitespace(mut value: &[u8]) -> &[u8] {
    while let [b' ' | b'\t', rest @ ..] = value {
        value = rest;
    }
    while let [rest @ .., b' ' | b'\t'] = value {
        value = rest;
    }
    value
}

fn save_header<'a>(
    field: &mut Option<&'a [u8]>,
    value: &'a [u8],
    cap: usize,
) -> Result<(), RequestError> {
    if field.is_some() {
        return Err(RequestError::BadRequest);
    }
    if value.len() >= cap {
        return Err(RequestError::HeaderTooLarge);
    }
    *field = Some(value);
    Ok(())
}

fn parse_content_length(value: &[u8]) -> Result<usize, RequestError> {
    if value.len() >= 32 {
        return Err(RequestError::HeaderTooLarge);
    }
    if value.is_empty() {
        return Err(RequestError::BadRequest);
    }
    let mut length = 0;
    for &byte in value {
        if !byte.is_ascii_digit() {
            return Err(RequestError::BadRequest);
        }
        length = length * 10 + usize::from(byte - b'0');
        if length > MAX_BODY {
            return Err(RequestError::ContentTooLarge);
        }
    }
    Ok(length)
}

pub(super) fn parse_head(head: &[u8]) -> Result<RequestHead<'_>, RequestError> {
    let body_offset = head.len();
    let mut lines = head[..body_offset - 4].split(|byte| *byte == b'\n');

    let request_line = lines.next().ok_or(RequestError::BadRequest)?;
    let request_line = request_line.strip_suffix(b"\r").unwrap_or(request_line);
    let (method, target_and_version) =
        split_at_byte(request_line, b' ').ok_or(RequestError::BadRequest)?;
    let (target, version) = split_at_byte(target_and_version, b' ')
        .ok_or(RequestError::BadRequest)?;
    if request_line.contains(&b'\r')
        || method.is_empty()
        || method.len() >= 16
        || !method.iter().all(u8::is_ascii_uppercase)
        || !target.starts_with(b"/")
        || target.windows(3).any(|bytes| bytes == b"%00")
    {
        return Err(RequestError::BadRequest);
    }
    let http_minor = match version {
        b"HTTP/1.0" => 0,
        b"HTTP/1.1" => 1,
        _ => return Err(RequestError::BadRequest),
    };

    let (path, query) = split_at_byte(target, b'?').unwrap_or((target, b""));
    if path.len() >= 2048 || query.len() >= 2048 {
        return Err(RequestError::BadRequest);
    }

    let mut origin = None;
    let mut host = None;
    let mut content_type = None;
    let mut accept_language = None;
    let mut content_length = None;

    for line in lines {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let (name, value) =
            split_at_byte(line, b':').ok_or(RequestError::BadRequest)?;
        let value = strip_http_whitespace(value);
        if name.is_empty() || line.contains(&b'\r') {
            return Err(RequestError::BadRequest);
        }
        if name.eq_ignore_ascii_case(b"origin") {
            save_header(&mut origin, value, 256)?;
        } else if name.eq_ignore_ascii_case(b"host") {
            save_header(&mut host, value, 256)?;
        } else if name.eq_ignore_ascii_case(b"content-type") {
            save_header(&mut content_type, value, 128)?;
        } else if name.eq_ignore_ascii_case(b"accept-language") {
            save_header(&mut accept_language, value, 256)?;
        } else if name.eq_ignore_ascii_case(b"transfer-encoding") {
            return Err(RequestError::LengthRequired);
        } else if name.eq_ignore_ascii_case(b"content-length") {
            let length = parse_content_length(value)?;
            if content_length.is_some_and(|previous| previous != length) {
                return Err(RequestError::BadRequest);
            }
            content_length = Some(length);
        }
    }

    Ok(RequestHead {
        method,
        path,
        query,
        http_minor,
        origin: origin.unwrap_or_default(),
        host: host.unwrap_or_default(),
        content_type: content_type.unwrap_or_default(),
        accept_language: accept_language.unwrap_or_default(),
        content_length: content_length.unwrap_or_default(),
        body_offset,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragments_preserve_raw_bytes_and_leave_the_body_unparsed() {
        let wire = b"POST /api%2Fadr?a=one+two?three HTTP/1.0\r\n\
                     HosT: \tlocalhost:9749 \t\r\n\
                     Content-Type: application/json\r\n\
                     Origin: http://localhost:9749\r\n\
                     Accept-Language: zh-CN\r\n\
                     Content-Length: 3\r\nContent-Length: 003\r\n\r\n\xff\0\n";
        let end = wire.len() - 3;
        let mut scanner = HeadScanner::default();
        for length in 0..end {
            assert_eq!(scanner.scan(&wire[..length]), Ok(None));
        }
        let offset = scanner.scan(wire).unwrap().unwrap();
        let head = parse_head(&wire[..offset]).unwrap();
        assert_eq!(head.method, b"POST");
        assert_eq!(head.path, b"/api%2Fadr");
        assert_eq!(head.query, b"a=one+two?three");
        assert_eq!(head.host, b"localhost:9749");
        assert_eq!(head.content_type, b"application/json");
        assert_eq!(head.origin, b"http://localhost:9749");
        assert_eq!(head.accept_language, b"zh-CN");
        assert_eq!(head.http_minor, 0);
        assert_eq!(head.content_length, 3);
        assert_eq!(&wire[head.body_offset..], b"\xff\0\n");
    }

    #[test]
    fn every_selected_header_rejects_even_identical_duplicates() {
        for header in ["Host", "Origin", "Content-Type", "Accept-Language"] {
            let wire =
                format!("GET / HTTP/1.1\r\n{header}: x\r\n{header}: x\r\n\r\n");
            assert_eq!(failure(wire.as_bytes()), 400, "{header}");
        }
    }

    #[test]
    fn rejects_ambiguous_or_unsupported_framing() {
        for (wire, expected) in [
            ("GET / HTTP/1.1\nHost: localhost\r\n\r\n", 400),
            ("GET / HTTP/1.1\r\nHost: local\0host\r\n\r\n", 400),
            ("GET /?value=%00 HTTP/1.1\r\n\r\n", 400),
            ("GET http://localhost/ HTTP/1.1\r\n\r\n", 400),
            ("GET / HTTP/2.0\r\n\r\n", 400),
            ("get / HTTP/1.1\r\n\r\n", 400),
            (
                "POST / HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n",
                400,
            ),
            ("POST / HTTP/1.1\r\nContent-Length: +1\r\n\r\n", 400),
            ("POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n", 411),
            ("POST / HTTP/1.1\r\nContent-Length: 1048577\r\n\r\n", 413),
        ] {
            assert_eq!(failure(wire.as_bytes()), expected, "{wire:?}");
        }
    }

    #[test]
    fn caps_apply_at_the_boundary() {
        let prefix = b"POST / HTTP/1.1\r\nContent-Length: 1048576\r\nX-Pad: ";
        let mut wire = prefix.to_vec();
        wire.resize(MAX_HEAD - 4, b'a');
        wire.extend_from_slice(b"\r\n\r\n");
        let head = frame(&wire).unwrap().unwrap();
        assert_eq!(head.body_offset, MAX_HEAD);
        assert_eq!(head.content_length, MAX_BODY);

        wire.insert(prefix.len(), b'a');
        assert_eq!(failure(&wire), 431);

        for (header, max) in
            [("Host", 255), ("Origin", 255), ("Content-Type", 127)]
        {
            let accepted = format!(
                "GET / HTTP/1.1\r\n{header}: {}\r\n\r\n",
                "a".repeat(max)
            );
            assert!(frame(accepted.as_bytes()).unwrap().is_some());
            let denied = format!(
                "GET / HTTP/1.1\r\n{header}: {}\r\n\r\n",
                "a".repeat(max + 1)
            );
            assert_eq!(failure(denied.as_bytes()), 431);
        }
    }

    fn frame(wire: &[u8]) -> Result<Option<RequestHead<'_>>, RequestError> {
        HeadScanner::default()
            .scan(wire)?
            .map(|end| parse_head(&wire[..end]))
            .transpose()
    }

    fn failure(wire: &[u8]) -> u16 {
        frame(wire).unwrap_err().status()
    }
}
