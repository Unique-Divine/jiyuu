//! Assemble project read responses from the request data gathered by C.

use std::borrow::Cow;
use std::ffi::{CStr, CString, c_char, c_int, c_longlong, c_void};
use std::fmt;
use std::{panic, ptr, slice};

use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;

use crate::project_page::{ProjectPage, ProjectRecord};

const EMPTY_HINT: &CStr =
    c"No projects indexed. Call index_repository(repo_path=...) first.";

type ReleasePayload = unsafe extern "C" fn(*mut c_char);
type ListRenderer = unsafe extern "C" fn(
    *const ProjectRecord,
    c_int,
    *const ProjectPage,
    bool,
    bool,
    bool,
) -> *mut c_char;

type StatusRenderer =
    unsafe extern "C" fn(*const IndexStatusReport) -> *mut c_char;

// Mirrors cbm_mcp_index_status_report_t.
#[repr(C)]
struct IndexStatusReport {
    project: *const c_char,
    root_path: *const c_char,
    indexed_at: *const c_char,
    diagnostics: *const c_char,
    nodes: c_int,
    edges: c_int,
    has_metadata: bool,
}

// Mirrors cbm_sb_t. C owns the buffer and manages its capacity.
#[repr(C)]
#[derive(Default)]
struct TreeBuffer {
    buf: *mut c_char,
    len: usize,
    cap: usize,
    oom: bool,
}

impl Drop for TreeBuffer {
    fn drop(&mut self) {
        // SAFETY: C allocated this buffer through the compact tree API.
        unsafe { cbm_sb_free(self) }
    }
}

unsafe extern "C" {
    fn cbm_mcp_set_project_list_renderer(
        render: ListRenderer,
        release: ReleasePayload,
    );
    fn cbm_mcp_set_index_status_renderer(
        render: StatusRenderer,
        release: ReleasePayload,
    );
    fn cbm_mcp_render_project_table(
        sb: *mut TreeBuffer,
        records: *const ProjectRecord,
        count: c_int,
        metadata_only: bool,
        include_stats: bool,
    );
    fn cbm_sb_free(sb: *mut TreeBuffer);
    fn cbm_tree_scalar_int(
        sb: *mut TreeBuffer,
        key: *const c_char,
        value: c_longlong,
    );
    fn cbm_tree_scalar_bool(
        sb: *mut TreeBuffer,
        key: *const c_char,
        value: bool,
    );
    fn cbm_tree_scalar_str(
        sb: *mut TreeBuffer,
        key: *const c_char,
        value: *const c_char,
    );
    fn cbm_output_encode_text(
        data: *const c_char,
        len: usize,
        encoded: *mut *mut c_char,
        encoded_len: *mut usize,
    ) -> bool;
    fn free(allocation: *mut c_void);
}

/// Apply the native output encoding for non-UTF-8 paths and reserved prefixes.
fn encode_text(text: &CStr) -> Option<Cow<'_, str>> {
    let mut encoded = ptr::null_mut();
    let mut encoded_len = 0;
    // SAFETY: C reads text's live bytes and writes both output variables.
    if !unsafe {
        cbm_output_encode_text(
            text.as_ptr(),
            text.count_bytes(),
            &mut encoded,
            &mut encoded_len,
        )
    } {
        return None;
    }
    if encoded.is_null() {
        return text.to_str().ok().map(Cow::Borrowed);
    }
    // SAFETY: the encoder returned a C allocation of encoded_len bytes.
    let result = unsafe { slice::from_raw_parts(encoded.cast(), encoded_len) };
    let result = str::from_utf8(result)
        .ok()
        .map(|text| Cow::Owned(text.to_owned()));
    // SAFETY: C allocated the encoded string with malloc.
    unsafe { free(encoded.cast()) };
    result
}

#[derive(Serialize)]
struct ProjectRow<'a> {
    name: Cow<'a, str>,
    root_path: Cow<'a, str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch: Option<Cow<'a, str>>,
    #[serde(flatten)]
    stats: Option<ProjectStats>,
}

#[derive(Serialize)]
struct ProjectStats {
    nodes: c_int,
    edges: c_int,
    size_bytes: i64,
}

#[derive(Serialize)]
struct ProjectList<'a> {
    projects: Vec<ProjectRow<'a>>,
    total: c_int,
    offset: c_int,
    limit: c_int,
    returned: c_int,
    has_more: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_offset: Option<c_int>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<&'a str>,
}

fn project_list_json(
    records: &[ProjectRecord],
    total: c_int,
    page: &ProjectPage,
    include_stats: bool,
) -> Option<CString> {
    let projects = records
        .iter()
        .map(|row| {
            // SAFETY: the renderer's C caller lends initialized page records
            // and their strings until the synchronous callback returns.
            let name = unsafe { CStr::from_ptr(row.name) };
            let root_path = unsafe { CStr::from_ptr(row.root_path) };
            let branch = if row.branch.is_null() {
                None
            } else {
                Some(encode_text(unsafe { CStr::from_ptr(row.branch) })?)
            };
            Some(ProjectRow {
                name: encode_text(name)?,
                root_path: encode_text(root_path)?,
                branch,
                stats: include_stats.then_some(ProjectStats {
                    nodes: row.nodes,
                    edges: row.edges,
                    size_bytes: row.size_bytes,
                }),
            })
        })
        .collect::<Option<Vec<_>>>()?;
    let has_more = page.end < total;
    let response = ProjectList {
        projects,
        total,
        offset: page.offset,
        limit: page.limit,
        returned: page.end - page.start,
        has_more,
        next_offset: has_more.then_some(page.end),
        hint: (total == 0).then(|| EMPTY_HINT.to_str().expect("ASCII hint")),
    };
    CString::new(serde_json::to_vec(&response).ok()?).ok()
}

fn project_list_tree(
    records: *const ProjectRecord,
    total: c_int,
    page: &ProjectPage,
    metadata_only: bool,
    include_stats: bool,
) -> Option<CString> {
    let mut sb = TreeBuffer::default();
    let returned = page.end - page.start;
    let has_more = page.end < total;
    // SAFETY: the callback borrows exactly the selected records for this
    // rendering. C permits a null array for an empty page.
    unsafe {
        cbm_mcp_render_project_table(
            &mut sb,
            records,
            returned,
            metadata_only,
            include_stats,
        );
        cbm_tree_scalar_int(&mut sb, c"total".as_ptr(), total.into());
        cbm_tree_scalar_int(&mut sb, c"returned".as_ptr(), returned.into());
        cbm_tree_scalar_bool(&mut sb, c"has_more".as_ptr(), has_more);
        if has_more {
            cbm_tree_scalar_int(
                &mut sb,
                c"next_offset".as_ptr(),
                page.end.into(),
            );
        }
        if total == 0 {
            cbm_tree_scalar_str(&mut sb, c"hint".as_ptr(), EMPTY_HINT.as_ptr());
        }
        if sb.oom || sb.buf.is_null() {
            return None;
        }
        Some(CStr::from_ptr(sb.buf).to_owned())
    }
}

unsafe extern "C" fn render_project_list(
    records: *const ProjectRecord,
    total: c_int,
    page: *const ProjectPage,
    metadata_only: bool,
    include_stats: bool,
    wants_json: bool,
) -> *mut c_char {
    let result = panic::catch_unwind(|| {
        // SAFETY: C passes a live page with computed bounds and lends the
        // selected rows for the duration of this callback.
        let page = unsafe { &*page };
        if wants_json {
            let count = (page.end - page.start) as usize;
            let records = if count == 0 {
                &[]
            } else {
                unsafe { slice::from_raw_parts(records, count) }
            };
            project_list_json(records, total, page, include_stats)
        } else {
            project_list_tree(records, total, page, metadata_only, include_stats)
        }
    });
    result
        .ok()
        .flatten()
        .map_or(ptr::null_mut(), CString::into_raw)
}

// Keep native section values as raw JSON. Re-serializing them as Value would
// reorder nested keys before the compact tree formatter sees them.
struct NativeDiagnostics<'a>(Vec<(&'a str, &'a RawValue)>);

impl<'de> Deserialize<'de> for NativeDiagnostics<'de> {
    fn deserialize<D: Deserializer<'de>>(input: D) -> Result<Self, D::Error> {
        struct OrderedFields;

        impl<'de> Visitor<'de> for OrderedFields {
            type Value = NativeDiagnostics<'de>;

            fn expecting(
                &self,
                formatter: &mut fmt::Formatter<'_>,
            ) -> fmt::Result {
                formatter.write_str("the native index diagnostic sections")
            }

            fn visit_map<M: MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Self::Value, M::Error> {
                let mut fields = Vec::new();
                while let Some(field) = map.next_entry()? {
                    fields.push(field);
                }
                Ok(NativeDiagnostics(fields))
            }
        }

        input.deserialize_map(OrderedFields)
    }
}

struct IndexStatus<'a> {
    project: Option<Cow<'a, str>>,
    metadata: Option<(Cow<'a, str>, Cow<'a, str>)>,
    diagnostics: NativeDiagnostics<'a>,
    nodes: c_int,
    edges: c_int,
}

impl IndexStatus<'_> {
    fn status_and_hint(&self) -> (&'static str, Option<&'static str>) {
        match (self.nodes, self.edges) {
            (nodes, edges) if nodes < 0 && edges < 0 => (
                "error",
                Some(concat!(
                    "The nodes and edges tables could not be read; the database may be ",
                    "corrupt. Re-run index_repository(repo_path=...) or remove the project ",
                    "cache and re-index."
                )),
            ),
            (nodes, _) if nodes < 0 => (
                "error",
                Some(concat!(
                    "The nodes table could not be read; the database may be corrupt. Re-run ",
                    "index_repository(repo_path=...) or remove the project cache and re-index."
                )),
            ),
            (_, edges) if edges < 0 => (
                "error",
                Some(concat!(
                    "The edges table could not be read; the database may be corrupt. Re-run ",
                    "index_repository(repo_path=...) or remove the project cache and re-index."
                )),
            ),
            (0, _) => (
                "empty",
                Some(
                    "Project is empty. Re-run index_repository(repo_path=...) to populate.",
                ),
            ),
            _ => ("ready", None),
        }
    }
}

impl Serialize for IndexStatus<'_> {
    fn serialize<S: Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        let Some(project) = &self.project else {
            map.serialize_entry("status", "no_project")?;
            return map.end();
        };
        let (status, hint) = self.status_and_hint();
        let unreadable = self.nodes < 0 || self.edges < 0;
        map.serialize_entry("project", project)?;
        map.serialize_entry("nodes", &if unreadable { 0 } else { self.nodes })?;
        map.serialize_entry("edges", &if unreadable { 0 } else { self.edges })?;
        map.serialize_entry("status", status)?;
        if let Some((root_path, indexed_at)) = &self.metadata {
            map.serialize_entry("root_path", root_path)?;
            map.serialize_entry("indexed_at", indexed_at)?;
        }
        let mut watch = None;
        for (key, value) in &self.diagnostics.0 {
            if *key == "watch" {
                watch = Some(value);
            } else {
                map.serialize_entry(key, value)?;
            }
        }
        if let Some(hint) = hint {
            map.serialize_entry("hint", hint)?;
        }
        if let Some(watch) = watch {
            map.serialize_entry("watch", watch)?;
        }
        map.end()
    }
}

unsafe extern "C" fn render_index_status(
    report: *const IndexStatusReport,
) -> *mut c_char {
    let result = panic::catch_unwind(|| {
        // SAFETY: the caller lends the report and all of its NUL-terminated
        // strings for the duration of this synchronous callback.
        let report = unsafe { &*report };
        let text = |value: *const c_char| {
            if value.is_null() {
                Some(Cow::Borrowed(""))
            } else {
                encode_text(unsafe { CStr::from_ptr(value) })
            }
        };
        let project = if report.project.is_null() {
            None
        } else {
            Some(text(report.project)?)
        };
        let metadata = if report.has_metadata {
            Some((text(report.root_path)?, text(report.indexed_at)?))
        } else {
            None
        };
        let diagnostics = unsafe { CStr::from_ptr(report.diagnostics) };
        let response = IndexStatus {
            project,
            metadata,
            diagnostics: serde_json::from_slice(diagnostics.to_bytes()).ok()?,
            nodes: report.nodes,
            edges: report.edges,
        };
        CString::new(serde_json::to_vec(&response).ok()?).ok()
    });
    result
        .ok()
        .flatten()
        .map_or(ptr::null_mut(), CString::into_raw)
}

/// Register before C starts accepting requests.
pub(crate) unsafe fn register_renderers(release: ReleasePayload) {
    unsafe {
        cbm_mcp_set_project_list_renderer(render_project_list, release);
        cbm_mcp_set_index_status_renderer(render_index_status, release);
    }
}
