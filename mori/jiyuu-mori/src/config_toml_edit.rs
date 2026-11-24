//! Conservative byte-preserving edits for agent TOML configuration.

mod codex;
mod config_file;
mod ffi;
mod legacy;
mod managed;
mod named;
mod syntax;

use std::ops::{Deref, Range};

use syntax::{lines, payload_start};

const MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
struct Error;

type Result<T> = std::result::Result<T, Error>;

#[derive(Default)]
struct Output(Vec<u8>);

impl Deref for Output {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl Output {
    fn append(&mut self, bytes: &[u8]) -> Result<()> {
        if bytes.len() > MAX_BYTES - self.len() {
            return Err(Error);
        }
        self.0.try_reserve(bytes.len()).map_err(|_| Error)?;
        self.0.extend_from_slice(bytes);
        Ok(())
    }

    fn normalized(&mut self, bytes: &[u8], newline: &[u8]) -> Result<()> {
        for line in lines(bytes, 0..bytes.len()) {
            self.append(line.content(bytes))?;
            if line.end > line.content_end {
                self.append(newline)?;
            }
        }
        Ok(())
    }

    fn finish_line(&mut self, newline: &[u8], prefix: usize) -> Result<()> {
        if self.len() > prefix && !self.ends_with(b"\n") {
            self.append(newline)?;
        }
        Ok(())
    }

    fn copy_without(data: &[u8], range: Range<usize>) -> Result<Self> {
        let mut output = Self::default();
        output.append(&data[..range.start])?;
        output.append(&data[range.end..])?;
        Ok(output)
    }

    fn start_append(data: &[u8], newline: &[u8]) -> Result<Self> {
        let mut output = Self::default();
        output.append(data)?;
        output.finish_line(newline, payload_start(data))?;
        Ok(output)
    }
}

enum Edit {
    Unchanged,
    Write(Output),
    Foreign,
}

fn escape_basic(input: &[u8], out: &mut [u8]) -> Result<usize> {
    escape_basic_with(input, out.len(), |offset, bytes| {
        out[offset..offset + bytes.len()].copy_from_slice(bytes);
    })
}

fn escape_basic_with(
    input: &[u8],
    capacity: usize,
    mut copy: impl FnMut(usize, &[u8]),
) -> Result<usize> {
    if input.len() > MAX_BYTES || std::str::from_utf8(input).is_err() {
        copy(0, b"\0");
        return Err(Error);
    }
    const HEX: &[u8] = b"0123456789ABCDEF";
    let mut used = 0;
    for &byte in input {
        let unicode;
        let escaped: &[u8] = match byte {
            8 => b"\\b",
            b'\t' => b"\\t",
            b'\n' => b"\\n",
            12 => b"\\f",
            b'\r' => b"\\r",
            b'"' => b"\\\"",
            b'\\' => b"\\\\",
            0..=31 | 127 => {
                unicode = [
                    b'\\',
                    b'u',
                    b'0',
                    b'0',
                    HEX[(byte >> 4) as usize],
                    HEX[(byte & 15) as usize],
                ];
                &unicode
            }
            _ => std::slice::from_ref(&byte),
        };
        if escaped.len() >= capacity - used {
            copy(0, b"\0");
            return Err(Error);
        }
        copy(used, escaped);
        used += escaped.len();
    }
    copy(used, b"\0");
    Ok(used)
}
