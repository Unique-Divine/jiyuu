//! Request policy for MCP search_code. Native code gathers a complete scan
//! and lends the result rows, source reads, and wire-format helpers.

mod native;
mod response;

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_void};
use std::{panic, ptr};

use serde::de::{Error, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use native::{Native, NativeOps};
use response::Response;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Mode {
    #[default]
    Compact,
    Full,
    Files,
}

#[derive(Clone, Copy)]
struct PageRequest {
    offset: usize,
    limit: usize,
}

struct Request {
    pattern: Option<CString>,
    project: Option<CString>,
    file_pattern: Option<CString>,
    path_filter: Option<CString>,
    mode: Mode,
    results: PageRequest,
    raw: PageRequest,
    directories: PageRequest,
    raw_content_offset: Option<usize>,
    match_limit: usize,
    context: i32,
    source_max_lines: i32,
    byte_budget: usize,
    use_regex: bool,
    debug: bool,
    json: bool,
}

// Match yyjson's first-key lookup, including when the first value has the
// wrong type. Keep the original number token for integer-only presence checks.
#[derive(Default)]
struct Args<'a>(HashMap<String, Arg<'a>>);

struct Arg<'a> {
    raw: &'a str,
    value: Value,
}

impl<'de> Deserialize<'de> for Args<'de> {
    fn deserialize<D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        struct Object;

        impl<'de> Visitor<'de> for Object {
            type Value = Args<'de>;

            fn expecting(
                &self,
                formatter: &mut std::fmt::Formatter<'_>,
            ) -> std::fmt::Result {
                formatter.write_str("a search argument object")
            }

            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut fields = HashMap::new();
                while let Some((key, value)) =
                    map.next_entry::<String, &'de serde_json::value::RawValue>()?
                {
                    let raw = value.get();
                    let value =
                        serde_json::from_str(raw).map_err(A::Error::custom)?;
                    fields.entry(key).or_insert(Arg { raw, value });
                }
                Ok(Args(fields))
            }
        }

        deserializer.deserialize_map(Object)
    }
}

impl Args<'_> {
    fn value(&self, key: &str) -> Option<&Value> {
        self.0.get(key).map(|arg| &arg.value)
    }

    fn string(&self, key: &str) -> Option<CString> {
        let Value::String(value) = self.value(key)? else {
            return None;
        };
        // The native argument getter copies through the first NUL.
        Some(CString::new(value.split('\0').next().unwrap_or_default()).unwrap())
    }

    fn int(&self, key: &str, default: i32) -> i32 {
        self.0
            .get(key)
            .and_then(|arg| arg.raw.parse::<i32>().ok())
            .unwrap_or(default)
    }

    fn offset(&self, key: &str) -> usize {
        self.int(key, 0).max(0) as usize
    }

    fn flag(&self, key: &str) -> bool {
        self.value(key) == Some(&Value::Bool(true))
    }

    fn has_integer(&self, key: &str) -> bool {
        self.0.get(key).is_some_and(|arg| {
            arg.raw.parse::<i64>().is_ok() || arg.raw.parse::<u64>().is_ok()
        })
    }
}

impl Request {
    fn parse(json: &[u8]) -> Self {
        let args: Args<'_> = serde_json::from_slice(json).unwrap_or_default();
        let mode = match args.string("mode").as_deref() {
            Some(value) if value == c"full" => Mode::Full,
            Some(value) if value == c"files" => Mode::Files,
            _ => Mode::Compact,
        };
        let result_limit =
            args.int("result_limit", positive_or_default(args.int("limit", 10)));
        let tokens = args
            .int(
                "max_output_tokens",
                if mode == Mode::Full { 12000 } else { 3200 },
            )
            .clamp(128, 1_000_000);
        Self {
            pattern: args.string("pattern"),
            project: ["project", "project_name", "project_id", "projectName"]
                .into_iter()
                .find_map(|key| args.string(key)),
            file_pattern: args.string("file_pattern"),
            path_filter: args
                .string("path_filter")
                .filter(|filter| !filter.is_empty()),
            mode,
            results: PageRequest {
                offset: args.offset("result_offset"),
                limit: positive_or_default(result_limit).min(500) as usize,
            },
            raw: PageRequest {
                offset: args.offset("raw_offset"),
                limit: args.int("raw_limit", 5).clamp(0, 100) as usize,
            },
            directories: PageRequest {
                offset: args.offset("directory_offset"),
                limit: args.int("directory_limit", 20).clamp(0, 64) as usize,
            },
            raw_content_offset: args
                .has_integer("raw_content_offset")
                .then(|| args.offset("raw_content_offset")),
            match_limit: args.int("match_limit", 8).clamp(1, 500) as usize,
            context: args.int("context", 0).clamp(0, 200),
            source_max_lines: args.int("source_max_lines", 20).clamp(1, 200),
            byte_budget: tokens as usize * 4,
            use_regex: args.flag("regex"),
            debug: args.flag("debug"),
            json: args.string("format").as_deref() == Some(c"json"),
        }
    }
}

fn positive_or_default(limit: i32) -> i32 {
    if limit < 1 { 10 } else { limit }
}

fn valid_search_path(path: &CStr) -> bool {
    path.to_bytes().iter().all(|&byte| {
        !b"'\";|$`<>\n\r".contains(&byte) && (cfg!(windows) || byte != b'\\')
    })
}

fn ordered_words(pattern: &CStr) -> Option<CString> {
    let bytes = pattern.to_bytes();
    if !bytes.contains(&b' ') {
        return None;
    }
    let mut regex = Vec::new();
    let mut in_space = false;
    for &byte in bytes {
        if matches!(byte, b' ' | b'\t') {
            if !in_space {
                regex.extend_from_slice(b".*");
                in_space = true;
            }
        } else {
            if b"\\^$.|?*+()[]{}".contains(&byte) {
                regex.push(b'\\');
            }
            regex.push(byte);
            in_space = false;
        }
    }
    Some(CString::new(regex).expect("a C string cannot contain an interior NUL"))
}

const TIMEOUT_REPLY: &CStr = c"{\"content\":[{\"type\":\"text\",\"text\":\"search_code scan exceeded its execution deadline\"}],\"structuredContent\":{\"code\":\"request_timeout\",\"message\":\"search_code scan exceeded its execution deadline\"},\"isError\":true}";
const REGEX_TOO_BIG: i32 = 100; // CBM_REG_ETOOBIG from compat_regex.h.

fn search(native: Native<'_>, args: &CStr) -> *mut c_char {
    let req = Request::parse(args.to_bytes());
    let Some(pattern) = req.pattern.as_deref() else {
        return native.text_result(c"pattern is required", true);
    };
    let Some(project) = req
        .project
        .as_deref()
        .and_then(|project| native.project(project))
    else {
        return native.project_error(c"project is required");
    };
    let search_started = native.now_ms();
    let Some(root) = native.root(&project) else {
        return native.project_error(c"project not found or not indexed");
    };
    if !valid_search_path(&root)
        || req
            .file_pattern
            .as_deref()
            .is_some_and(|path| !valid_search_path(path))
    {
        return native.text_result(
            c"path or file_pattern contains invalid characters",
            true,
        );
    }
    let path_regex = match req.path_filter.as_deref().map(|filter| native.regex(filter)).transpose() {
        Ok(filter) => filter,
        Err(REGEX_TOO_BIG) => return native.text_result(c"path_filter: regex pattern too large to compile: reduce nested {m,n} repetition or alternation", true),
        Err(_) => return native.text_result(c"invalid path_filter regex: check for unbalanced (), [], or {}", true),
    };
    if req.use_regex {
        match native.regex(pattern) {
            Ok(_) => {}
            Err(REGEX_TOO_BIG) => return native.text_result(c"regex pattern too large to compile: reduce nested {m,n} repetition or alternation (regex=true)", true),
            Err(_) => return native.text_result(c"invalid regex pattern (regex=true): check for unbalanced (), [], or {}", true),
        }
    }
    let words = if req.use_regex {
        None
    } else {
        ordered_words(pattern)
    };
    let use_regex = req.use_regex || words.is_some();
    let gathered = native.gather(native::GatherArgs {
        pattern: native::c_str(words.as_deref().unwrap_or(pattern)),
        project: native::c_str(&project),
        root: native::c_str(&root),
        file_pattern: req
            .file_pattern
            .as_deref()
            .map_or(ptr::null(), native::c_str),
        path_regex: path_regex
            .as_ref()
            .map_or(ptr::null_mut(), |filter| filter.handle),
        raw_limit: req.raw.limit as i32,
        raw_offset: req.raw.offset as i32,
        raw_content_offset: req.raw_content_offset.unwrap_or_default(),
        raw_content_offset_set: req.raw_content_offset.is_some(),
        use_regex,
        debug: req.debug,
    });
    if gathered.data.timed_out {
        return native.copy(TIMEOUT_REPLY);
    }
    let error = native::array_str(&gathered.data.error);
    if !error.is_empty() {
        return native.text_result(error, true);
    }

    let ranking_started = req.debug.then(|| native.now_ms());
    let mut response = Response::new(&req, &gathered.data, native, &root);
    response.rank();
    let ranked_at = native.now_ms();
    response.elapsed_ms = ranked_at.saturating_sub(search_started);
    if let Some(started) = ranking_started {
        response.enrich_ms += ranked_at.saturating_sub(started);
    }
    response.warn_literal_pipe =
        !use_regex && pattern.to_bytes().contains(&b'|');
    response.hint_path_filter = gathered.data.grep_count == 0
        && req
            .file_pattern
            .as_deref()
            .is_some_and(|path| path.to_bytes().contains(&b'/'));
    match response.with_budget() {
        Some(payload) => native.text_result(&payload, false),
        None => native.text_result(c"out of memory", true),
    }
}

/// The native MCP dispatcher lends its request and synchronous callback table.
#[unsafe(no_mangle)]
unsafe extern "C" fn mori_search_code(
    context: *mut c_void,
    args: *const c_char,
    ops: *const NativeOps,
) -> *mut c_char {
    // SAFETY: The dispatcher keeps the table, context and request alive for this call.
    let native = Native {
        context,
        ops: unsafe { &*ops },
    };
    let args = if args.is_null() {
        c"{}"
    } else {
        unsafe { CStr::from_ptr(args) }
    };
    panic::catch_unwind(panic::AssertUnwindSafe(|| search(native, args)))
        .unwrap_or_else(|_| {
            native.text_result(
                c"search failed while assembling the response",
                true,
            )
        })
}

#[cfg(feature = "native-engine-test-seams")]
#[unsafe(no_mangle)]
unsafe extern "C" fn mori_search_code_render_fixture(
    data: *const native::GatheredData,
    json: bool,
    ops: *const NativeOps,
) -> *mut c_char {
    // SAFETY: The native test driver keeps the synthetic rows and table alive.
    let data = unsafe { &*data };
    let native = Native {
        context: ptr::null_mut(),
        ops: unsafe { &*ops },
    };
    panic::catch_unwind(panic::AssertUnwindSafe(|| {
        let mut request = Request::parse(b"{}");
        request.json = json;
        request.results.limit = data.result_count as usize;
        request.raw.limit = data.raw_stored_count as usize;
        Response::new(&request, data, native, c"")
            .render_fixture()
            .map_or(ptr::null_mut(), |payload| native.copy(&payload))
    }))
    .unwrap_or(ptr::null_mut())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_preserves_aliases_first_keys_and_integer_bounds() {
        let request = Request::parse(br#"{
            "pattern":"first\u0000ignored","pattern":"second","project":42,"projectName":"alias",
            "mode":"full","format":"json","limit":-5,"result_offset":-3,"raw_limit":999,
            "raw_content_offset":4294967297,"directory_limit":-3,"context":300,"match_limit":0,
            "source_max_lines":500,"regex":1,"debug":true
        }"#);
        assert_eq!(request.pattern.as_deref(), Some(c"first"));
        assert_eq!(request.project.as_deref(), Some(c"alias"));
        assert_eq!(request.mode, Mode::Full);
        assert_eq!(request.results.limit, 10);
        assert_eq!(request.results.offset, 0);
        assert_eq!(request.raw.limit, 100);
        assert_eq!(request.raw_content_offset, Some(0));
        assert_eq!(request.directories.limit, 0);
        assert_eq!(request.context, 200);
        assert_eq!(request.match_limit, 1);
        assert_eq!(request.source_max_lines, 200);
        assert_eq!(request.byte_budget, 48_000);
        assert!(request.debug && request.json && !request.use_regex);
        assert_eq!(
            Request::parse(br#"{"raw_content_offset":1.0}"#).raw_content_offset,
            None
        );
    }

    #[test]
    fn ordered_words_preserve_the_literal_regex_metacharacters() {
        assert_eq!(
            ordered_words(c"first.|  \tsecond[1]").as_deref(),
            Some(c"first\\.\\|.*second\\[1\\]")
        );
        assert!(ordered_words(c"first\tsecond").is_none());
        assert!(valid_search_path(c"R&D/source.rs"));
        assert!(!valid_search_path(c"unsafe'path"));
    }
}
