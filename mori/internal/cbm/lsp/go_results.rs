//! Encode Go result lists for the native cross-file type registry.

use std::ffi::{CStr, c_char, c_void};
use std::ptr;

const MAX_RESULTS: usize = 16;

struct ResultTypes<'a> {
    spans: [&'a [u8]; MAX_RESULTS],
    count: usize,
}

impl<'a> ResultTypes<'a> {
    fn split(raw: &'a [u8]) -> Option<Self> {
        let body = raw.strip_prefix(b"(")?.strip_suffix(b")")?;
        let mut list = Self {
            spans: [&[]; MAX_RESULTS],
            count: 0,
        };
        let mut depth: usize = 0;
        let mut start = 0;
        for (index, &byte) in body.iter().enumerate() {
            match byte {
                b',' if depth == 0 => {
                    list.push(trim(&body[start..index]))?;
                    start = index + 1;
                }
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        let last = trim(&body[start..]);
        if !last.is_empty() {
            list.push(last)?;
        }
        (list.count > 0).then_some(list)
    }

    fn push(&mut self, span: &'a [u8]) -> Option<()> {
        if span.is_empty() {
            return None;
        }
        *self.spans.get_mut(self.count)? = span;
        self.count += 1;
        Some(())
    }

    fn parse(raw: &'a [u8]) -> Option<Self> {
        let mut list = Self::split(raw)?;
        if list
            .as_slice()
            .iter()
            .any(|span| matches!(part(span), Part::Named(_)))
        {
            let mut group_start = 0;
            for index in 0..list.count {
                match part(list.spans[index]) {
                    Part::Lone => {}
                    Part::Bare => return None,
                    Part::Named(ty) => {
                        list.spans[group_start..=index].fill(ty);
                        group_start = index + 1;
                    }
                }
            }
            if group_start != list.count {
                return None;
            }
        }
        Some(list)
    }

    fn as_slice(&self) -> &[&'a [u8]] {
        &self.spans[..self.count]
    }
}

enum Part<'a> {
    Lone,
    Bare,
    Named(&'a [u8]),
}

fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
}

fn trim(mut span: &[u8]) -> &[u8] {
    while let Some((_, rest)) =
        span.split_first().filter(|(byte, _)| is_space(**byte))
    {
        span = rest;
    }
    while let Some((_, rest)) =
        span.split_last().filter(|(byte, _)| is_space(**byte))
    {
        span = rest;
    }
    span
}

fn part(span: &[u8]) -> Part<'_> {
    let end = span
        .iter()
        .take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'_')
        .count();
    if end == span.len() {
        return Part::Lone;
    }
    if end == 0
        || !is_space(span[end])
        || matches!(
            &span[..end],
            b"chan" | b"func" | b"map" | b"struct" | b"interface"
        )
    {
        return Part::Bare;
    }
    Part::Named(trim(&span[end..]))
}

type Allocate =
    unsafe extern "C" fn(context: *mut c_void, len: usize) -> *mut c_char;

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_go_encode_results(
    raw: *const c_char,
    context: *mut c_void,
    allocate: Allocate,
) -> *const c_char {
    // SAFETY: The C caller lends a non-null, NUL-terminated signature.
    let raw = unsafe { CStr::from_ptr(raw) }.to_bytes();
    let Some(list) = ResultTypes::parse(raw) else {
        return ptr::null();
    };
    let types = list.as_slice();
    // The count pays for one separator between adjacent types and one NUL.
    let Some(len) = types
        .iter()
        .try_fold(types.len(), |len, ty| len.checked_add(ty.len()))
        .filter(|&len| len <= isize::MAX as usize)
    else {
        return ptr::null();
    };

    // SAFETY: The allocator returns len bytes in the destination arena or NULL.
    let output = unsafe { allocate(context, len) }.cast::<u8>();
    if output.is_null() {
        return ptr::null();
    }
    // SAFETY: The arena lends a fresh allocation disjoint from the source.
    // The computed length covers every write.
    unsafe {
        let mut cursor = output;
        for (index, ty) in types.iter().enumerate() {
            if index != 0 {
                cursor.write(b'|');
                cursor = cursor.add(1);
            }
            ptr::copy_nonoverlapping(ty.as_ptr(), cursor, ty.len());
            cursor = cursor.add(ty.len());
        }
        cursor.write(0);
    }
    output.cast()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_native_result_types_without_rewriting_their_bytes() {
        let cases: &[(&[u8], &[u8])] = &[
            (b"(*Encoder, error)", b"*Encoder|error"),
            (
                b"(a, b *Encoder, err error, \n)",
                b"*Encoder|*Encoder|error",
            ),
            (b"(n []T, err error)", b"[]T|error"),
            (
                b"(chan T, func () U, map [K]V, struct {}, interface {})",
                b"chan T|func () U|map [K]V|struct {}|interface {}",
            ),
            (
                b"(func(A, B) C, struct{ A, B int }, [2]T)",
                b"func(A, B) C|struct{ A, B int }|[2]T",
            ),
            (b"(\xffT, \x0bT)", b"\xffT|\x0bT"),
            (b"(*T], error)", b"*T]|error"),
            (b"(*[T, error)", b"*[T, error"),
        ];
        for &(raw, expected) in cases {
            let actual = ResultTypes::parse(raw).unwrap().as_slice().join(&b'|');
            assert_eq!(actual, expected, "{}", String::from_utf8_lossy(raw));
        }
    }

    #[test]
    fn keeps_raw_text_for_non_lists_and_unresolvable_groups() {
        for input in [
            b"[]T".as_slice(),
            b"()",
            b"( )",
            b"(, T)",
            b"(T, , error)",
            b"(T, error,,)",
            b"(a int, missing)",
            b"(a int, *T)",
            b"(a int, \xff T)",
            b" (T, error)",
            b"(T, error) ",
        ] {
            assert!(ResultTypes::parse(input).is_none(), "{input:?}");
        }
        let at_bound = format!("({}T,)", "T,".repeat(MAX_RESULTS - 1));
        assert_eq!(
            ResultTypes::parse(at_bound.as_bytes()).unwrap().count,
            MAX_RESULTS
        );
        let over_bound = format!("({}T)", "T,".repeat(MAX_RESULTS));
        assert!(ResultTypes::parse(over_bound.as_bytes()).is_none());
    }

    #[test]
    fn encodes_into_the_arena_and_respects_c_string_lengths() {
        const CAPACITY: usize = 20;
        unsafe extern "C" fn allocate(
            context: *mut c_void,
            len: usize,
        ) -> *mut c_char {
            if len > CAPACITY {
                return ptr::null_mut();
            }
            // SAFETY: The test provides an exclusive, live buffer of CAPACITY bytes.
            unsafe {
                (&mut *context.cast::<[u8; CAPACITY]>()).as_mut_ptr().cast()
            }
        }

        let mut output = [0xFF; CAPACITY];
        let context = (&raw mut output).cast();
        let raw = b"(*T, error)\0ignored";
        // SAFETY: The input is live and NUL-terminated. The buffer is disjoint.
        let result = unsafe {
            mori_go_encode_results(raw.as_ptr().cast(), context, allocate)
        };
        assert_eq!(result, output.as_ptr().cast());
        // SAFETY: The encoder returned a NUL-terminated pointer to output.
        assert_eq!(unsafe { CStr::from_ptr(result) }.to_bytes(), b"*T|error");
        // SAFETY: The callback contract above also applies to this larger input.
        let too_large = unsafe {
            mori_go_encode_results(
                c"(ABCDEFGH, abcdefgh, error)".as_ptr(),
                context,
                allocate,
            )
        };
        assert!(too_large.is_null());
        assert_eq!(output[9], 0xFF);
    }
}
