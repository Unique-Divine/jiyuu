//! Encode the page selected by the MCP tool registry.

use serde::Serialize;
use serde_json::value::RawValue;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Annotations {
    read_only_hint: bool,
    destructive_hint: bool,
    idempotent_hint: bool,
    open_world_hint: bool,
}

#[derive(Serialize)]
struct ToolMeta {
    #[serde(rename = "anthropic/alwaysLoad")]
    always_load: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Tool<'a> {
    name: &'a str,
    description: &'a str,
    input_schema: &'a RawValue,
    annotations: Annotations,
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    meta: Option<ToolMeta>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolList<'a> {
    tools: Vec<Tool<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_cursor: Option<String>,
}

fn build_tool_list(
    tools: Vec<Tool<'_>>,
    next_cursor: Option<usize>,
) -> Option<Vec<u8>> {
    serde_json::to_vec(&ToolList {
        tools,
        next_cursor: next_cursor.map(|offset| offset.to_string()),
    })
    .ok()
}

#[cfg(all(feature = "c-backend", not(test)))]
pub(crate) mod c_tool_list {
    use std::ffi::{CStr, CString, c_char, c_int};
    use std::{ptr, slice};

    use super::{Annotations, Tool, ToolMeta, build_tool_list};

    #[repr(C)]
    struct ToolPageEntry {
        name: *const c_char,
        description: *const c_char,
        input_schema: *const c_char,
        read_only: bool,
        destructive: bool,
        idempotent: bool,
        open_world: bool,
        load_upfront: bool,
    }

    type ToolListBuilder =
        unsafe extern "C" fn(*const ToolPageEntry, usize, c_int) -> *mut c_char;

    unsafe extern "C" {
        fn cbm_mcp_set_tool_list_builder(builder: ToolListBuilder);
        fn strdup(s: *const c_char) -> *mut c_char;
    }

    impl ToolPageEntry {
        unsafe fn tool(&self) -> Option<Tool<'_>> {
            // SAFETY: C provides the live registry strings for the entire call.
            let (name, description, schema) = unsafe {
                (
                    CStr::from_ptr(self.name),
                    CStr::from_ptr(self.description),
                    CStr::from_ptr(self.input_schema),
                )
            };
            Some(Tool {
                name: name.to_str().ok()?,
                description: description.to_str().ok()?,
                input_schema: serde_json::from_str(schema.to_str().ok()?)
                    .ok()?,
                annotations: Annotations {
                    read_only_hint: self.read_only,
                    destructive_hint: self.destructive,
                    idempotent_hint: self.idempotent,
                    open_world_hint: self.open_world,
                },
                meta: self
                    .load_upfront
                    .then_some(ToolMeta { always_load: true }),
            })
        }
    }

    unsafe extern "C" fn build(
        entries: *const ToolPageEntry,
        count: usize,
        next_cursor: c_int,
    ) -> *mut c_char {
        // SAFETY: the C stack owns count initialized entries for the call.
        let entries = unsafe { slice::from_raw_parts(entries, count) };
        let tools = entries
            .iter()
            .map(|entry| unsafe { entry.tool() })
            .collect::<Option<Vec<_>>>();
        let Some(response) = tools.and_then(|tools| {
            build_tool_list(tools, usize::try_from(next_cursor).ok())
                .and_then(|json| CString::new(json).ok())
        }) else {
            return ptr::null_mut();
        };
        // SAFETY: the pointer is live during the call and C frees the copy.
        unsafe { strdup(response.as_ptr()) }
    }

    pub(crate) unsafe fn register() {
        // SAFETY: caller registers once before starting the C daemon.
        unsafe { cbm_mcp_set_tool_list_builder(build) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_schema_and_annotations_and_omits_absent_cursor() {
        let schema = r#"{"type":"object","properties":{"z":{"type":"string"},"a":{"type":"integer"}}}"#;
        let tool = Tool {
            name: "search_graph",
            description: "Search the graph",
            input_schema: serde_json::from_str(schema).unwrap(),
            annotations: Annotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
                open_world_hint: false,
            },
            meta: Some(ToolMeta { always_load: true }),
        };
        let response = build_tool_list(vec![tool], Some(8)).unwrap();
        assert_eq!(
            String::from_utf8(response).unwrap(),
            format!(
                r#"{{"tools":[{{"name":"search_graph","description":"Search the graph","inputSchema":{schema},"annotations":{{"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}},"_meta":{{"anthropic/alwaysLoad":true}}}}],"nextCursor":"8"}}"#
            ),
        );
        assert_eq!(build_tool_list(vec![], None).unwrap(), br#"{"tools":[]}"#);
    }
}
