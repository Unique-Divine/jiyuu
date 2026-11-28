//! MCP request and response policy for get_file_outline.

use std::borrow::Cow;
use std::ffi::{CStr, CString, c_char, c_int, c_longlong, c_void};
use std::{panic, ptr, slice};

use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::value::RawValue;

use super::{CANCELLED, Cancel, OK, OutlineRow, SCAN_LIMIT};
use crate::ffi_output::{self, c_str};
use crate::project_response::encode_text;
use crate::snippet_window::response::{
    JsonFormatter,
    native::{CANDIDATES_NOTE, Candidate},
};

const COLUMNS: [&CStr; 4] = [c"name", c"label", c"lines", c"qn"];
const MAX_LIMIT: c_int = 200;
const MAX_LABELS: usize = 16;
const OUTPUT_CAP: usize = 2 * 1024 * 1024;

const OBJECT_REQUIRED: &CStr =
    c"get_file_outline arguments must be a JSON object";
const QUERY_FAILED: &CStr = c"get_file_outline query failed";
const OUTPUT_FAILED: &CStr = c"get_file_outline output allocation failed";
const OUTPUT_LIMIT: &CStr =
    c"get_file_outline exceeded its fixed output safety limit";

#[derive(Default)]
struct Args<'a> {
    path: Option<&'a RawValue>,
    limit: Option<&'a RawValue>,
    offset: Option<&'a RawValue>,
    format: Option<&'a RawValue>,
    labels: Option<&'a RawValue>,
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
                formatter.write_str("an outline argument object")
            }

            fn visit_map<M: MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Self::Value, M::Error> {
                let mut args = Args::default();
                while let Some((key, value)) =
                    map.next_entry::<String, &RawValue>()?
                {
                    ffi_output::bytes(key.as_bytes());
                    let field = match key.as_str() {
                        "file_path" => &mut args.path,
                        "limit" => &mut args.limit,
                        "offset" => &mut args.offset,
                        "format" => &mut args.format,
                        "labels" => &mut args.labels,
                        _ => continue,
                    };
                    // yyjson_obj_get uses the first occurrence, even if its type is wrong.
                    field.get_or_insert(value);
                }
                Ok(args)
            }
        }
        deserializer.deserialize_map(Object)
    }
}

struct Request {
    path: CString,
    limit: c_int,
    offset: c_int,
    json: bool,
    labels: Vec<CString>,
}

fn native_string(text: &str) -> CString {
    CString::new(text.split('\0').next().unwrap_or_default())
        .expect("truncated at NUL")
}

fn string(value: &RawValue) -> Option<CString> {
    serde_json::from_str::<String>(value.get())
        .ok()
        .map(|text| native_string(&text))
}

fn integer(value: &RawValue) -> Option<i64> {
    // Match yyjson's signed getter for both integer subtypes.
    value
        .get()
        .parse()
        .ok()
        .or_else(|| value.get().parse::<u64>().ok().map(|n| n as i64))
}

impl Request {
    fn decode(args: Args<'_>) -> Result<Self, &'static CStr> {
        let input = args
            .path
            .and_then(string)
            .filter(|path| !path.is_empty())
            .ok_or(c"file_path is required")?;
        let mut normalized = [0; 4096];
        unsafe { ffi_output::written(normalized.as_ptr(), normalized.len()) };
        // SAFETY: C borrows the input and writes at most capacity bytes including NUL.
        if !unsafe {
            cbm_mcp_normalize_read_path(
                c_str(&input),
                normalized.as_mut_ptr(),
                normalized.len(),
            )
        } {
            return Err(
                c"file_path must be a repository-relative path without '..'",
            );
        }
        let path = unsafe { CStr::from_ptr(normalized.as_ptr()) }.to_owned();

        let limit = match args.limit {
            None => 100,
            Some(value) => {
                let value = integer(value).ok_or(c"limit must be an integer")?;
                if !(1..=i64::from(MAX_LIMIT)).contains(&value) {
                    return Err(c"limit must be between 1 and 200");
                }
                value as c_int
            }
        };
        let offset = match args.offset {
            None => 0,
            Some(value) => integer(value)
                .and_then(|n| c_int::try_from(n).ok())
                .filter(|&n| n >= 0)
                .ok_or(c"offset must be a non-negative integer")?,
        };
        let json = match args.format {
            None => false,
            Some(value) => match string(value).as_deref().map(CStr::to_bytes) {
                Some(b"json") => true,
                Some(b"tree") => false,
                _ => return Err(c"format must be either 'tree' or 'json'"),
            },
        };
        let labels = match args.labels {
            None => Vec::new(),
            Some(value) => {
                let labels: Vec<&RawValue> = serde_json::from_str(value.get())
                    .ok()
                    .filter(|labels: &Vec<_>| labels.len() <= MAX_LABELS)
                    .ok_or(c"labels must be an array of at most 16 strings")?;
                labels.into_iter().map(|value| {
                    string(value).filter(|label| (1..128).contains(&label.count_bytes()))
                        .ok_or(c"labels must contain only non-empty bounded strings")
                }).collect::<Result<_, _>>()?
            }
        };
        Ok(Self {
            path,
            limit,
            offset,
            json,
            labels,
        })
    }
}

struct NativeProject(*mut c_char);

impl Drop for NativeProject {
    fn drop(&mut self) {
        // SAFETY: The shared project resolver returns a C-owned string or NULL.
        unsafe { cbm_result_free(self.0.cast()) };
    }
}

#[derive(Default)]
struct Rows {
    data: *mut OutlineRow,
    count: c_int,
    total: c_int,
}

impl Rows {
    fn get(&self) -> &[OutlineRow] {
        if self.count == 0 {
            return &[];
        }
        // SAFETY: The native query returned count initialized rows.
        unsafe { slice::from_raw_parts(self.data, self.count as usize) }
    }
}

impl Drop for Rows {
    fn drop(&mut self) {
        // SAFETY: The destructor accepts NULL or the page returned by the store.
        unsafe { cbm_store_free_file_outline(self.data, self.count) };
    }
}

struct Section {
    name: CString,
    path: CString,
    line: c_int,
    probability: f64,
    kind: Option<CString>,
    position: Option<CString>,
}

unsafe fn optional_text<'a>(text: *const c_char) -> Option<&'a CStr> {
    // SAFETY: Callers pass strings borrowed from a live row or callback.
    (!text.is_null()).then(|| unsafe { CStr::from_ptr(text) })
}

unsafe extern "C" fn collect_section(
    context: *mut c_void,
    candidate: *const Candidate,
) {
    // SAFETY: The visitor borrows its stack row until this callback returns.
    let (sections, candidate, node) = unsafe {
        (
            &mut *context.cast::<Vec<Section>>(),
            &*candidate,
            &*(*candidate).node,
        )
    };
    // SAFETY: The visitor keeps the node and evidence strings live here.
    sections.push(unsafe {
        Section {
            name: optional_text(node.qualified_name).unwrap_or(c"").to_owned(),
            path: optional_text(node.file_path).unwrap_or(c"").to_owned(),
            line: node.start_line,
            probability: f64::from(
                (candidate.probability * 1000.0 + 0.5) as c_int,
            ) / 1000.0,
            kind: optional_text(candidate.kind).map(CStr::to_owned),
            position: optional_text(candidate.position).map(CStr::to_owned),
        }
    });
}

fn note() -> &'static str {
    str::from_utf8(CANDIDATES_NOTE).expect("candidate note is UTF-8")
}

#[derive(Serialize)]
struct JsonSection<'a> {
    section: Cow<'a, str>,
    file_path: Cow<'a, str>,
    start_line: c_int,
    p: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    position: Option<Cow<'a, str>>,
}

#[derive(Serialize)]
struct JsonOutline<'a> {
    file_path: Cow<'a, str>,
    cols: [&'static str; 4],
    rows: Vec<[Cow<'a, str>; 4]>,
    total: c_int,
    offset: c_int,
    limit: c_int,
    returned: c_int,
    has_more: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    possibly_described_in: Vec<JsonSection<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    candidates_note: Option<&'static str>,
}

impl OutlineRow {
    fn cells<'a>(&'a self, lines: &'a CStr) -> [&'a CStr; 4] {
        // SAFETY: Each string belongs to the page for the lifetime of this row.
        unsafe {
            [
                CStr::from_ptr(self.name),
                CStr::from_ptr(self.label),
                lines,
                CStr::from_ptr(self.qualified_name),
            ]
        }
    }

    fn lines(&self) -> CString {
        if self.start_line <= 0 {
            return CString::default();
        }
        CString::new(format!(
            "{}-{}",
            self.start_line,
            self.end_line.max(self.start_line)
        ))
        .unwrap()
    }
}

// Mirrors cbm_sb_t. All growth and escaping use the shared compact emitter.
#[repr(C)]
#[derive(Default)]
struct TreeBuffer {
    data: *mut c_char,
    len: usize,
    cap: usize,
    oom: bool,
}

impl Drop for TreeBuffer {
    fn drop(&mut self) {
        // SAFETY: The compact emitter owns the allocation.
        unsafe { cbm_sb_free(self) };
    }
}

impl TreeBuffer {
    fn table<const N: usize>(
        &mut self,
        key: &CStr,
        columns: [&CStr; N],
        cells: &[&CStr],
    ) {
        let columns = columns.map(c_str);
        let cells: Vec<_> = cells.iter().map(|text| c_str(text)).collect();
        // SAFETY: C borrows the pointer arrays for this call.
        unsafe {
            ffi_output::written(columns.as_ptr(), N);
            ffi_output::written(cells.as_ptr(), cells.len());
            cbm_tree_table_rows(
                self,
                c_str(key),
                (cells.len() / N) as c_int,
                columns.as_ptr(),
                N as c_int,
                cells.as_ptr(),
            );
        }
    }
}

fn json_payload(
    request: &Request,
    rows: &Rows,
    sections: &[Section],
    has_more: bool,
) -> Option<CString> {
    let lines: Vec<_> = rows.get().iter().map(OutlineRow::lines).collect();
    let mut json_rows = Vec::with_capacity(rows.count as usize);
    for (row, lines) in rows.get().iter().zip(&lines) {
        let [name, label, lines, qn] = row.cells(lines);
        json_rows.push([
            encode_text(name)?,
            encode_text(label)?,
            encode_text(lines)?,
            encode_text(qn)?,
        ]);
    }
    let mut candidates = Vec::with_capacity(sections.len());
    for section in sections {
        candidates.push(JsonSection {
            section: encode_text(&section.name)?,
            file_path: encode_text(&section.path)?,
            start_line: section.line,
            p: section.probability,
            kind: match &section.kind {
                Some(text) => Some(encode_text(text)?),
                None => None,
            },
            position: match &section.position {
                Some(text) => Some(encode_text(text)?),
                None => None,
            },
        });
    }
    let response = JsonOutline {
        file_path: encode_text(&request.path)?,
        cols: COLUMNS.map(|column| column.to_str().expect("column is UTF-8")),
        rows: json_rows,
        total: rows.total,
        offset: request.offset,
        limit: request.limit,
        returned: rows.count,
        has_more,
        possibly_described_in: candidates,
        candidates_note: (!sections.is_empty()).then(note),
    };
    let mut bytes = Vec::new();
    response
        .serialize(&mut serde_json::Serializer::with_formatter(
            &mut bytes,
            JsonFormatter,
        ))
        .ok()?;
    CString::new(bytes).ok()
}

fn tree_result(
    request: &Request,
    rows: &Rows,
    sections: &[Section],
    has_more: bool,
) -> *mut c_char {
    let lines: Vec<_> = rows.get().iter().map(OutlineRow::lines).collect();
    let cells: Vec<_> = rows
        .get()
        .iter()
        .zip(&lines)
        .flat_map(|(row, line)| row.cells(line))
        .collect();
    let mut tree = TreeBuffer::default();
    // SAFETY: The tree and scalar inputs live throughout each synchronous call.
    unsafe {
        ffi_output::written(&tree, 1);
        cbm_tree_scalar_str(
            &mut tree,
            c_str(c"file_path"),
            c_str(&request.path),
        );
        tree.table(c"results", COLUMNS, &cells);
        for (key, value) in [
            (c"total", rows.total),
            (c"offset", request.offset),
            (c"limit", request.limit),
            (c"returned", rows.count),
        ] {
            cbm_tree_scalar_int(&mut tree, c_str(key), value.into());
        }
        cbm_tree_scalar_bool(&mut tree, c_str(c"has_more"), has_more);
    }
    if !sections.is_empty() {
        let numbers: Vec<_> = sections
            .iter()
            .map(|section| {
                [
                    CString::new(section.line.to_string()).unwrap(),
                    CString::new(format!("{:.2}", section.probability)).unwrap(),
                ]
            })
            .collect();
        let cells: Vec<_> = sections
            .iter()
            .zip(&numbers)
            .flat_map(|(section, [line, p])| {
                [
                    section.name.as_c_str(),
                    section.path.as_c_str(),
                    line,
                    p,
                    section.kind.as_deref().unwrap_or(c""),
                ]
            })
            .collect();
        tree.table(
            c"possibly_described_in",
            [c"section", c"file_path", c"line", c"p", c"kind"],
            &cells,
        );
        let note = CString::new(note()).unwrap();
        // SAFETY: C consumes the note while both it and the buffer are live.
        unsafe {
            cbm_tree_scalar_str(
                &mut tree,
                c_str(c"candidates_note"),
                c_str(&note),
            )
        };
    }
    // SAFETY: The tree owns the terminated payload until after finish copies it.
    finish(unsafe { (!tree.oom).then(|| optional_text(tree.data)).flatten() })
}

fn text_result(text: &CStr, error: bool) -> *mut c_char {
    // SAFETY: The wrapper copies the text and returns a C-owned result.
    unsafe { cbm_mcp_text_result(c_str(text), error) }
}

fn finish(payload: Option<&CStr>) -> *mut c_char {
    match payload {
        None => text_result(OUTPUT_FAILED, true),
        Some(text) if text.count_bytes() > OUTPUT_CAP => {
            text_result(OUTPUT_LIMIT, true)
        }
        Some(text) => text_result(text, false),
    }
}

fn run(
    server: *mut c_void,
    arguments: &CStr,
) -> Result<*mut c_char, &'static CStr> {
    let args = serde_json::from_slice::<Args<'_>>(arguments.to_bytes())
        .map_err(|_| OBJECT_REQUIRED)?;
    // SAFETY: The shared resolver reads a live C string and allocates the result.
    let project =
        NativeProject(unsafe { cbm_mcp_resolve_project_arg(c_str(arguments)) });
    if project.0.is_null() {
        return Err(c"project is required");
    }
    let request = Request::decode(args)?;
    let mut error = ptr::null_mut();
    let labels: Vec<_> =
        request.labels.iter().map(|label| c_str(label)).collect();
    let mut rows = Rows::default();
    // SAFETY: The dispatcher owns server and its read batch. The native store
    // query owns the progress handler while Rust's SQL steps on that same store.
    let (store, status) = unsafe {
        let store = cbm_mcp_resolve_read_store(server, project.0, &mut error);
        if store.is_null() {
            return Ok(error);
        }
        ffi_output::written(labels.as_ptr(), labels.len());
        let status = cbm_store_get_file_outline(
            store,
            project.0,
            c_str(&request.path),
            labels.as_ptr(),
            labels.len() as c_int,
            request.limit,
            request.offset,
            cbm_mcp_read_request_cancelled,
            server,
            &mut rows.data,
            &mut rows.count,
            &mut rows.total,
        );
        (store, status)
    };
    match status {
        OK => {}
        CANCELLED => return Err(c"get_file_outline cancelled for this request"),
        SCAN_LIMIT => return Err(OUTPUT_LIMIT),
        _ => return Err(QUERY_FAILED),
    }
    let mut sections: Vec<Section> = Vec::new();
    if request.offset == 0 {
        // SAFETY: The native visitor borrows the store in the same read batch and
        // synchronously copies each resolved candidate through collect_section.
        unsafe {
            cbm_mcp_visit_path_candidates(
                store,
                project.0,
                c_str(&request.path),
                collect_section,
                (&mut sections as *mut Vec<Section>).cast(),
            );
        }
    }
    let has_more = i64::from(request.offset) + i64::from(rows.count)
        < i64::from(rows.total);
    Ok(if request.json {
        finish(json_payload(&request, &rows, &sections, has_more).as_deref())
    } else {
        tree_result(&request, &rows, &sections, has_more)
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_mcp_get_file_outline(
    server: *mut c_void,
    arguments: *const c_char,
) -> *mut c_char {
    // SAFETY: The dispatcher lends the argument string for this call.
    let arguments = unsafe { optional_text(arguments) }.unwrap_or(c"{}");
    match panic::catch_unwind(|| run(server, arguments)) {
        Ok(Ok(result)) => result,
        Ok(Err(message)) => text_result(message, true),
        Err(_) => text_result(QUERY_FAILED, true),
    }
}

type CandidateSink = unsafe extern "C" fn(*mut c_void, *const Candidate);

unsafe extern "C" {
    fn cbm_mcp_resolve_project_arg(args: *const c_char) -> *mut c_char;
    fn cbm_mcp_resolve_read_store(
        server: *mut c_void,
        project: *const c_char,
        error: *mut *mut c_char,
    ) -> *mut c_void;
    fn cbm_mcp_normalize_read_path(
        path: *const c_char,
        output: *mut c_char,
        capacity: usize,
    ) -> bool;
    fn cbm_mcp_read_request_cancelled(context: *mut c_void) -> bool;
    fn cbm_mcp_text_result(text: *const c_char, error: bool) -> *mut c_char;
    fn cbm_mcp_visit_path_candidates(
        store: *mut c_void,
        project: *const c_char,
        path: *const c_char,
        sink: CandidateSink,
        context: *mut c_void,
    );
    fn cbm_result_free(block: *mut c_void);
    fn cbm_store_get_file_outline(
        store: *mut c_void,
        project: *const c_char,
        path: *const c_char,
        labels: *const *const c_char,
        label_count: c_int,
        limit: c_int,
        offset: c_int,
        cancel: Cancel,
        context: *mut c_void,
        rows: *mut *mut OutlineRow,
        count: *mut c_int,
        total: *mut c_int,
    ) -> c_int;
    fn cbm_store_free_file_outline(rows: *mut OutlineRow, count: c_int);
    fn cbm_sb_free(tree: *mut TreeBuffer);
    fn cbm_tree_scalar_str(
        tree: *mut TreeBuffer,
        key: *const c_char,
        text: *const c_char,
    );
    fn cbm_tree_scalar_int(
        tree: *mut TreeBuffer,
        key: *const c_char,
        value: c_longlong,
    );
    fn cbm_tree_scalar_bool(
        tree: *mut TreeBuffer,
        key: *const c_char,
        value: bool,
    );
    fn cbm_tree_table_rows(
        tree: *mut TreeBuffer,
        key: *const c_char,
        rows: c_int,
        cols: *const *const c_char,
        col_count: c_int,
        cells: *const *const c_char,
    );
}
