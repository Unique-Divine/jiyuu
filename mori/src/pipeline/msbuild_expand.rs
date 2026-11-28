//! Expand MSBuild property references using the evaluator's property view and scratch arena.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr;

const VALUE_CAPACITY: usize = 4096; // includes the NUL terminator
const FIRST_CAPACITY: usize = 64;

type Lookup =
    unsafe extern "C" fn(*mut c_void, *const c_char, usize) -> *const c_char;
type ScratchAlloc = unsafe extern "C" fn(*mut c_void, usize) -> *mut c_char;

unsafe extern "C" {
    fn isalnum(byte: c_int) -> c_int;
}

fn property_name_byte(byte: u8) -> bool {
    if byte.is_ascii() {
        return byte.is_ascii_alphanumeric() || byte == b'_';
    }
    // Preserve native classification in single-byte C locales.
    // SAFETY: An unsigned byte is in the domain of isalnum.
    unsafe { isalnum(byte.into()) != 0 }
}

struct Expansion {
    buffer: *mut c_char,
    len: usize,
    capacity: usize,
    over_limit: bool,
    work: usize,
}

impl Expansion {
    // The allocator keeps previous buffers live until the native caller resets
    // scratch. Inputs may also live in scratch but never overlap this buffer.
    unsafe fn append(
        &mut self,
        bytes: &[u8],
        context: *mut c_void,
        alloc: ScratchAlloc,
    ) {
        if self.over_limit {
            return;
        }
        if bytes.len() >= VALUE_CAPACITY - self.len {
            self.over_limit = true;
            return;
        }
        let needed = self.len + bytes.len() + 1;
        if needed > self.capacity {
            let capacity = needed.next_power_of_two().max(FIRST_CAPACITY);
            // SAFETY: The callback gets the live evaluator context.
            let grown = unsafe { alloc(context, capacity) };
            if grown.is_null() {
                return;
            }
            if self.len > 0 {
                // SAFETY: Both allocations are live and disjoint.
                unsafe {
                    ptr::copy_nonoverlapping(self.buffer, grown, self.len)
                };
                self.work += self.len;
            }
            self.buffer = grown;
            self.capacity = capacity;
        }
        // SAFETY: The input and output do not overlap and capacity covers the NUL.
        unsafe {
            ptr::copy_nonoverlapping(
                bytes.as_ptr().cast(),
                self.buffer.add(self.len),
                bytes.len(),
            );
            self.len += bytes.len();
            self.buffer.add(self.len).write(0);
        }
        self.work += bytes.len() + 1;
    }

    unsafe fn expand(
        &mut self,
        mut text: &[u8],
        context: *mut c_void,
        lookup: Lookup,
        alloc: ScratchAlloc,
        oom: *const bool,
    ) -> *const c_char {
        while let Some((&byte, rest)) = text.split_first() {
            let (value, next) = match (byte, rest) {
                (b'$', [b'(', tail @ ..]) => {
                    let len = tail
                        .iter()
                        .take_while(|&&byte| property_name_byte(byte))
                        .count();
                    if len == 0
                        || tail.get(len) != Some(&b')')
                        || tail[0].is_ascii_digit()
                    {
                        return ptr::null();
                    }
                    // SAFETY: The callback borrows the name and returns a live C
                    // string or null for absent and explicitly unknown values.
                    let prop =
                        unsafe { lookup(context, tail.as_ptr().cast(), len) };
                    if prop.is_null() {
                        return ptr::null();
                    }
                    (
                        unsafe { CStr::from_ptr(prop) }.to_bytes(),
                        &tail[len + 1..],
                    )
                }
                (b'@' | b'%', [b'(', ..]) => return ptr::null(),
                (b'%', [a, b, ..])
                    if a.is_ascii_hexdigit() && b.is_ascii_hexdigit() =>
                {
                    return ptr::null();
                }
                _ => (&text[..1], rest),
            };
            // Keep reading properties after overflow or OOM so the host sees the
            // original sequence of dependency reads. A lookup can itself set OOM.
            if !unsafe { *oom } {
                // SAFETY: The host keeps the input and all scratch allocations live.
                unsafe { self.append(value, context, alloc) };
            }
            text = next;
        }
        if self.over_limit || unsafe { *oom } {
            ptr::null()
        } else if self.buffer.is_null() {
            c"".as_ptr()
        } else {
            self.buffer
        }
    }
}

/// The input and any non-null lookup result are live NUL-terminated strings.
/// The allocator returns disjoint scratch storage or sets OOM and returns null.
/// Both callbacks can change the live OOM flag.
/// The work output counts the bytes copied, including append terminators and growth.
#[unsafe(no_mangle)]
unsafe extern "C" fn mori_msbuild_expand(
    text: *const c_char,
    context: *mut c_void,
    lookup: Lookup,
    alloc: ScratchAlloc,
    oom: *const bool,
    work: *mut usize,
) -> *const c_char {
    let mut expansion = Expansion {
        buffer: ptr::null_mut(),
        len: 0,
        capacity: 0,
        over_limit: false,
        work: 0,
    };
    // SAFETY: The caller supplies a live input and evaluator per the ABI above.
    let text = unsafe { CStr::from_ptr(text) }.to_bytes();
    let result = unsafe { expansion.expand(text, context, lookup, alloc, oom) };
    // SAFETY: The host provides writable storage for the work count.
    unsafe { work.write(expansion.work) };
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{ffi::CString, slice};

    struct Host {
        value: CString,
        reads: Vec<Vec<u8>>,
        buffers: Vec<Vec<u8>>,
        requests: Vec<usize>,
        fail_allocation: Option<usize>,
        oom: *mut bool,
    }

    unsafe extern "C" fn lookup(
        context: *mut c_void,
        name: *const c_char,
        len: usize,
    ) -> *const c_char {
        let host = unsafe { &mut *context.cast::<Host>() };
        let name = unsafe { slice::from_raw_parts(name.cast::<u8>(), len) };
        host.reads.push(name.to_vec());
        match name {
            b"Unknown" => ptr::null(),
            b"Empty" => c"".as_ptr(),
            _ => host.value.as_ptr(),
        }
    }

    unsafe extern "C" fn allocate(
        context: *mut c_void,
        size: usize,
    ) -> *mut c_char {
        let host = unsafe { &mut *context.cast::<Host>() };
        host.requests.push(size);
        if host.fail_allocation == Some(host.requests.len()) {
            unsafe { host.oom.write(true) };
            return ptr::null_mut();
        }
        let mut buffer = vec![0; size];
        let out = buffer.as_mut_ptr().cast();
        host.buffers.push(buffer);
        out
    }

    fn run(
        text: &[u8],
        value: &[u8],
        fail_allocation: Option<usize>,
    ) -> (Option<Vec<u8>>, Host, usize) {
        let text = CString::new(text).unwrap();
        let mut oom = false;
        let mut host = Host {
            value: CString::new(value).unwrap(),
            reads: Vec::new(),
            buffers: Vec::new(),
            requests: Vec::new(),
            fail_allocation,
            oom: &raw mut oom,
        };
        let mut work = usize::MAX;
        let out = unsafe {
            mori_msbuild_expand(
                text.as_ptr(),
                (&raw mut host).cast(),
                lookup,
                allocate,
                host.oom,
                &mut work,
            )
        };
        let result = (!out.is_null())
            .then(|| unsafe { CStr::from_ptr(out) }.to_bytes().to_vec());
        host.oom = ptr::null_mut();
        (result, host, work)
    }

    #[test]
    fn preserves_path_bytes_and_counts_appends_and_growth() {
        let mut path = b"\x01dir\\\xff/".to_vec();
        path.resize(63, b'a');
        let (result, host, work) =
            run(b"$(MSBuildThisFileDirectory)\xfe.props", &path, None);
        path.extend_from_slice(b"\xfe.props");
        assert_eq!(result, Some(path));
        assert_eq!(host.reads, [b"MSBuildThisFileDirectory"]);
        assert_eq!(host.requests, [64, 128]);
        assert_eq!(work, 64 + 63 + 2 * b"\xfe.props".len());
    }

    #[test]
    fn distinguishes_empty_values_from_unknown_and_unsupported_syntax() {
        let (result, host, work) = run(b"", b"", None);
        assert_eq!(result, Some(vec![]));
        assert!(host.requests.is_empty());
        assert_eq!(work, 0);

        let (result, host, work) = run(b"$(Empty)", b"", None);
        assert_eq!(result, Some(vec![]));
        assert_eq!(host.requests, [64]);
        assert_eq!(work, 1);

        let (result, host, work) = run(b"x$(Unknown)$(Skipped)", b"", None);
        assert_eq!(result, None);
        assert_eq!(host.reads, [b"Unknown"]);
        assert_eq!(work, 2);

        for text in [
            b"$()".as_slice(),
            b"$(3abc)",
            b"$(No.Close",
            b"$([Fn])",
            b"@(Item)",
            b"%(Meta)",
            b"%2F",
        ] {
            let (result, host, work) = run(text, b"", None);
            assert_eq!(result, None);
            assert!(host.reads.is_empty());
            assert!(host.requests.is_empty());
            assert_eq!(work, 0);
        }
        assert_eq!(
            run(b"$ @ % %2 %q0", b"", None).0,
            Some(b"$ @ % %2 %q0".to_vec())
        );
    }

    #[test]
    fn keeps_reading_properties_after_the_size_limit_or_scratch_failure() {
        let value = vec![b'x'; VALUE_CAPACITY - 1];
        let (result, host, work) = run(b"$(Long)$(Empty)", &value, None);
        assert_eq!(result, Some(value.clone()));
        assert_eq!(host.requests, [VALUE_CAPACITY]);
        assert_eq!(work, VALUE_CAPACITY + 1);

        let (result, host, work) = run(b"$(Long)X$(Later)", &value, None);
        assert_eq!(result, None);
        assert_eq!(host.reads, [b"Long".as_slice(), b"Later"]);
        assert_eq!(host.requests, [VALUE_CAPACITY]);
        assert_eq!(work, VALUE_CAPACITY);

        let (result, host, work) =
            run(b"x$(Large)$(Later)", &[b'a'; 63], Some(2));
        assert_eq!(result, None);
        assert_eq!(host.reads, [b"Large", b"Later"]);
        assert_eq!(host.requests, [64, 128]);
        assert_eq!(work, 2);
    }

    struct Properties {
        values: Vec<(&'static [u8], CString)>,
        buffers: Vec<Vec<u8>>,
    }

    unsafe extern "C" fn property(
        context: *mut c_void,
        name: *const c_char,
        len: usize,
    ) -> *const c_char {
        let host = unsafe { &*context.cast::<Properties>() };
        let name = unsafe { slice::from_raw_parts(name.cast::<u8>(), len) };
        host.values
            .iter()
            .find(|(key, _)| *key == name)
            .map_or(ptr::null(), |(_, value)| value.as_ptr())
    }

    unsafe extern "C" fn keep(context: *mut c_void, size: usize) -> *mut c_char {
        let host = unsafe { &mut *context.cast::<Properties>() };
        let mut buffer = vec![0; size];
        let out = buffer.as_mut_ptr().cast();
        host.buffers.push(buffer);
        out
    }

    // The cases the C and Rust expanders were compared on through the
    // evaluator before the C implementation was deleted.
    #[test]
    fn matches_the_retired_c_expansion_cases() {
        let repeat = |byte: u8, len: usize| vec![byte; len];
        let mut host = Properties {
            values: vec![
                (b"Empty".as_slice(), c"".into()),
                (b"Short", c"Abc".into()),
                (b"Later", c"L".into()),
                (b"_Under", c"U".into()),
                (b"Utf", CString::new("\u{e9}t\u{e9}").unwrap()),
                (b"P63", CString::new(repeat(b'a', 63)).unwrap()),
                (b"Long4094", CString::new(repeat(b'l', 4094)).unwrap()),
                (b"Long4095", CString::new(repeat(b'l', 4095)).unwrap()),
            ],
            buffers: Vec::new(),
        };
        let concat = |parts: &[&[u8]]| Some(parts.concat());
        let long_name = [b"$(".as_slice(), &repeat(b'N', 300), b")"].concat();
        let cases: Vec<(&[u8], Option<Vec<u8>>)> = vec![
            (b"", Some(vec![])),
            (b"Plain.Namespace", Some(b"Plain.Namespace".to_vec())),
            (b"$(Empty)$(Empty)", Some(vec![])),
            (b"Pre.$(Short).$(Later)", Some(b"Pre.Abc.L".to_vec())),
            (b"$(_Under)", Some(b"U".to_vec())),
            (b"x$(Unknown)$(Short)", None),
            (b"$()", None),
            (b"$(", None),
            (b"$(3abc)", None),
            (b"$(No.Close)", None),
            (b"$(Open", None),
            (b"$([System.String]::Copy('a'))", None),
            (b"@(Item)", None),
            (b"%(Meta)", None),
            (b"%2F", None),
            (b"%2f.Lower", None),
            (b"$", Some(b"$".to_vec())),
            (b"$ @ % %2 %q0", Some(b"$ @ % %2 %q0".to_vec())),
            (b"a$b@c%d", Some(b"a$b@c%d".to_vec())),
            (b"$(P63)x", concat(&[&repeat(b'a', 63), b"x"])),
            (b"$(Long4094)x", concat(&[&repeat(b'l', 4094), b"x"])),
            (b"$(Long4094)xy", None),
            (b"$(Long4095)", Some(repeat(b'l', 4095))),
            (b"$(Long4095)$(Empty)", Some(repeat(b'l', 4095))),
            (b"$(Long4095)x", None),
            (b"$(Long4095)x$(Later)", None),
            (
                b"$(Utf)\xc3\xa9",
                Some("\u{e9}t\u{e9}\u{e9}".as_bytes().to_vec()),
            ),
            (b"$(Pr\xc3\xa9)", None),
            (&long_name, None),
        ];
        for (text, expected) in cases {
            let input = CString::new(text).unwrap();
            let oom = false;
            let mut work = 0;
            let out = unsafe {
                mori_msbuild_expand(
                    input.as_ptr(),
                    (&raw mut host).cast(),
                    property,
                    keep,
                    &oom,
                    &mut work,
                )
            };
            let actual = (!out.is_null())
                .then(|| unsafe { CStr::from_ptr(out) }.to_bytes().to_vec());
            assert_eq!(actual, expected, "{}", String::from_utf8_lossy(text));
        }
    }
}
