use std::borrow::Cow;
use std::ffi::{CStr, CString, c_char, c_int};
use std::{fmt, panic, ptr};

use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;

use super::encode_text;

type ReleasePayload = unsafe extern "C" fn(*mut c_char);
type StatusRenderer =
    unsafe extern "C" fn(*const IndexStatusReport) -> *mut c_char;

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

unsafe extern "C" {
    fn cbm_mcp_set_index_status_renderer(
        render: StatusRenderer,
        release: ReleasePayload,
    );
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

/// Register the index-status renderer before C starts accepting requests.
///
/// # Safety
/// Supply a release callback that frees the Rust allocation exactly once.
pub unsafe fn register_index_status_renderer(release: ReleasePayload) {
    // SAFETY: the caller supplies the Rust payload deallocator before requests.
    unsafe { cbm_mcp_set_index_status_renderer(render_index_status, release) };
}
