//! Byte-oriented multi-argument scalar functions shared by both Cypher executors.

use std::ffi::{CStr, CString, c_char, c_long, c_void};
use std::slice;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scalar {
    Coalesce,
    Substring,
    Left,
    Right,
    Replace,
}

pub enum Value<T> {
    Argument(T),
    Computed(usize),
}

impl Scalar {
    pub fn parse(name: &[u8]) -> Option<Self> {
        [
            Self::Coalesce,
            Self::Substring,
            Self::Left,
            Self::Right,
            Self::Replace,
        ]
        .into_iter()
        .find(|scalar| name.eq_ignore_ascii_case(scalar.name().as_bytes()))
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Coalesce => "coalesce",
            Self::Substring => "substring",
            Self::Left => "left",
            Self::Right => "right",
            Self::Replace => "replace",
        }
    }

    pub fn evaluate<T>(
        self,
        count: usize,
        mut read: impl FnMut(usize) -> T,
        bytes: impl Fn(&T) -> &[u8],
        number: impl Fn(&T) -> c_long,
        output: &mut [u8],
    ) -> Value<T> {
        let minimum = match self {
            Self::Coalesce => {
                return (0..count)
                    .map(read)
                    .find(|value| !bytes(value).is_empty())
                    .map_or(Value::Computed(0), Value::Argument);
            }
            Self::Left | Self::Right | Self::Substring => 2,
            Self::Replace => 3,
        };
        if count < minimum {
            return Value::Computed(0);
        }

        let text = read(0);
        let second = read(1);
        let text = bytes(&text);
        let value = match self {
            Self::Substring => {
                let start = usize::try_from(number(&second)).ok();
                let Some(tail) = start
                    .filter(|&start| start < text.len())
                    .map(|start| &text[start..])
                else {
                    return Value::Computed(0);
                };
                let take = if count >= 3 {
                    usize::try_from(number(&read(2))).unwrap_or(0)
                } else {
                    tail.len()
                };
                &tail[..tail.len().min(take)]
            }
            Self::Left | Self::Right => {
                let take = usize::try_from(number(&second))
                    .unwrap_or(0)
                    .min(text.len())
                    .min(output.len());
                if self == Self::Left {
                    &text[..take]
                } else {
                    &text[text.len() - take..]
                }
            }
            Self::Replace => {
                let third = read(2);
                return Value::Computed(replace(
                    text,
                    bytes(&second),
                    bytes(&third),
                    output,
                ));
            }
            Self::Coalesce => unreachable!(),
        };
        Value::Computed(copy(output, value))
    }
}

fn copy(output: &mut [u8], value: &[u8]) -> usize {
    let len = output.len().min(value.len());
    output[..len].copy_from_slice(&value[..len]);
    len
}

fn replace(mut text: &[u8], from: &[u8], to: &[u8], output: &mut [u8]) -> usize {
    if from.is_empty() {
        return copy(output, text);
    }
    let mut written = 0;
    while !text.is_empty() && written < output.len() {
        if let Some(rest) = text.strip_prefix(from) {
            written += copy(&mut output[written..], to);
            text = rest;
        } else {
            output[written] = text[0];
            written += 1;
            text = &text[1..];
        }
    }
    written
}

pub fn long_from_bytes(bytes: &[u8]) -> c_long {
    let bytes = CString::new(bytes).expect("NUL-trimmed integer");
    native_long(&bytes)
}

fn native_long(bytes: &CStr) -> c_long {
    unsafe extern "C" {
        fn strtol(
            input: *const c_char,
            end: *mut *mut c_char,
            base: std::ffi::c_int,
        ) -> c_long;
    }
    // SAFETY: The input is NUL-terminated and no end pointer is requested.
    unsafe { strtol(bytes.as_ptr(), std::ptr::null_mut(), 10) }
}

/// Evaluate a call whose arguments and binding remain owned by the C executor.
///
/// The caller supplies a writable, disjoint result buffer of at least one byte.
/// The argument reader returns non-null C strings. The computed functions read
/// at most three arguments, which fits the native property's eight scratch
/// buffers. Coalesce uses each argument before reading another and can return
/// the winning pointer directly, preserving unbounded field values.
#[unsafe(no_mangle)]
unsafe extern "C" fn mori_cypher_eval_scalar(
    name: *const c_char,
    count: usize,
    read: unsafe extern "C" fn(*mut c_void, usize) -> *const c_char,
    context: *mut c_void,
    output: *mut c_char,
    size: usize,
) -> *const c_char {
    // SAFETY: The native parser supplies a canonical, NUL-terminated name.
    let Some(scalar) = Scalar::parse(unsafe { CStr::from_ptr(name) }.to_bytes())
    else {
        return c"".as_ptr();
    };
    // SAFETY: Callers provide an exclusive scratch buffer and the callback
    // keeps the argument strings readable as described above.
    let buffer = unsafe { slice::from_raw_parts_mut(output.cast::<u8>(), size) };
    let result = scalar.evaluate(
        count,
        |index| unsafe { CStr::from_ptr(read(context, index)) },
        |value| value.to_bytes(),
        |value| native_long(value),
        &mut buffer[..size - 1],
    );
    match result {
        Value::Argument(value) => value.as_ptr(),
        Value::Computed(len) => {
            buffer[len] = 0;
            output
        }
    }
}
