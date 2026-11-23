//! Select the MCP method handler and decode tool call parameters.

use std::ffi::{CStr, c_char, c_int};
use std::fmt;

use serde::Deserialize;
use serde::de::{MapAccess, Visitor};
use serde_json::value::RawValue;

/// Handler IDs shared by `cbm_mcp_server_handle` in `mcp.c`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
enum Method {
    Unknown,
    Initialize,
    Ping,
    ResourcesList,
    ResourceTemplatesList,
    PromptsList,
    PromptsGet,
    ToolsList,
    ToolsCall,
    Cancelled,
    Discover,
}

fn select(method: &[u8]) -> Method {
    match method {
        b"initialize" => Method::Initialize,
        b"ping" => Method::Ping,
        b"resources/list" => Method::ResourcesList,
        b"resources/templates/list" => Method::ResourceTemplatesList,
        b"prompts/list" => Method::PromptsList,
        b"prompts/get" => Method::PromptsGet,
        b"tools/list" => Method::ToolsList,
        b"tools/call" => Method::ToolsCall,
        b"notifications/cancelled" => Method::Cancelled,
        b"server/discover" => Method::Discover,
        _ => Method::Unknown,
    }
}

unsafe extern "C" fn select_c(method: *const c_char) -> c_int {
    if method.is_null() {
        return Method::Unknown as c_int;
    }
    // SAFETY: C retains the parsed NUL-terminated method until we return.
    select(unsafe { CStr::from_ptr(method) }.to_bytes()) as c_int
}

#[derive(Default)]
struct ToolCall<'a> {
    name: Option<String>,
    arguments: Option<&'a RawValue>,
}

impl<'de> Deserialize<'de> for ToolCall<'de> {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        struct ToolCallVisitor;

        impl<'de> Visitor<'de> for ToolCallVisitor {
            type Value = ToolCall<'de>;

            fn expecting(
                &self,
                formatter: &mut fmt::Formatter<'_>,
            ) -> fmt::Result {
                formatter.write_str("MCP tool call parameters")
            }

            fn visit_map<M: MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Self::Value, M::Error> {
                let mut name: Option<&RawValue> = None;
                let mut arguments = None;
                while let Some((key, value)) =
                    map.next_entry::<String, &RawValue>()?
                {
                    // yyjson_obj_get returns the first key, even when
                    // its value is null or has the wrong type.
                    match key.as_str() {
                        "name" if name.is_none() => name = Some(value),
                        "arguments" if arguments.is_none() => {
                            arguments = Some(value)
                        }
                        _ => {}
                    }
                }
                let name = name
                    .filter(|value| value.get().starts_with('"'))
                    .map(|value| serde_json::from_str(value.get()))
                    .transpose()
                    .map_err(serde::de::Error::custom)?;
                Ok(ToolCall { name, arguments })
            }
        }

        deserializer.deserialize_map(ToolCallVisitor)
    }
}

fn decode_tool_call(params: &str) -> serde_json::Result<ToolCall<'_>> {
    // The C JSON-RPC parser has already validated the value. As in the C
    // build, looking up a key on a non-object gives a missing name.
    if params.trim_start().starts_with('{') {
        serde_json::from_str(params)
    } else {
        Ok(ToolCall::default())
    }
}

#[cfg(all(feature = "c-backend", not(test)))]
mod linked {
    use std::ffi::{CStr, CString, c_char, c_int};
    use std::{panic, ptr};

    type ToolCallDecoder = unsafe extern "C" fn(
        params: *const c_char,
        name_out: *mut *mut c_char,
        args_out: *mut *mut c_char,
    ) -> bool;

    unsafe extern "C" {
        fn cbm_mcp_set_method_selector(
            select: unsafe extern "C" fn(*const c_char) -> c_int,
        );
        fn cbm_mcp_set_tool_call_decoder(decode: ToolCallDecoder);
        fn strdup(s: *const c_char) -> *mut c_char;
    }

    unsafe extern "C" fn decode(
        params: *const c_char,
        name_out: *mut *mut c_char,
        args_out: *mut *mut c_char,
    ) -> bool {
        let Ok(Some((name, args))) = panic::catch_unwind(|| {
            // SAFETY: C borrows the validated JSON until the callback returns.
            let params = if params.is_null() {
                ""
            } else {
                unsafe { CStr::from_ptr(params) }.to_str().ok()?
            };
            let call = super::decode_tool_call(params).ok()?;
            let name = call.name.map(|name| {
                // The C build passes a NUL-terminated tool name to dispatch.
                CString::new(name.split('\0').next().unwrap_or_default())
                    .expect("tool name prefix contains no NUL")
            });
            let raw_args = call.arguments.map_or("{}", |raw| raw.get());
            // Retain the original arguments object, including duplicate keys.
            let args = CString::new(raw_args).ok()?;
            Some((name, args))
        }) else {
            return false;
        };

        // SAFETY: C supplies writable output pointers and frees both copies
        // after dispatch using the same allocator that strdup uses.
        unsafe {
            *name_out =
                name.map_or(ptr::null_mut(), |name| strdup(name.as_ptr()));
            *args_out = strdup(args.as_ptr());
        }
        true
    }

    pub(crate) unsafe fn register() {
        // SAFETY: the caller registers once, before starting request threads.
        unsafe {
            cbm_mcp_set_method_selector(super::select_c);
            cbm_mcp_set_tool_call_decoder(decode);
        }
    }
}

#[cfg(all(feature = "c-backend", not(test)))]
pub(crate) use linked::register;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_first_keys_without_rewriting_arguments() {
        let input = r#"{"name":"list_\u0070rojects","name":"ignored","arguments":{"format":"json","format":"tree"},"arguments":null}"#;
        let call = decode_tool_call(input).unwrap();
        assert_eq!(call.name.as_deref(), Some("list_projects"));
        assert_eq!(
            call.arguments.unwrap().get(),
            r#"{"format":"json","format":"tree"}"#,
        );

        let call = decode_tool_call(
            r#"{"name":null,"name":"ignored","arguments":null}"#,
        )
        .unwrap();
        assert!(call.name.is_none());
        assert_eq!(call.arguments.unwrap().get(), "null");
    }

    #[test]
    fn leaves_missing_fields_and_non_object_parameters_empty() {
        for input in ["", "null", "[]", "5", "{}"] {
            let call = decode_tool_call(input).unwrap();
            assert!(call.name.is_none(), "{input}");
            assert!(call.arguments.is_none(), "{input}");
        }
    }

    #[test]
    fn matches_exact_method_names_and_preserves_handler_ids() {
        for (method, handler_id) in [
            (c"initialize", 1),
            (c"ping", 2),
            (c"resources/list", 3),
            (c"resources/templates/list", 4),
            (c"prompts/list", 5),
            (c"prompts/get", 6),
            (c"tools/list", 7),
            (c"tools/call", 8),
            (c"notifications/cancelled", 9),
            (c"server/discover", 10),
            (c"tools/CALL", 0),
            (c"ping/extra", 0),
            (c"\xff", 0),
        ] {
            // SAFETY: The pointer comes from a static C string.
            assert_eq!(unsafe { select_c(method.as_ptr()) }, handler_id);
        }
        // SAFETY: A null method is supported and classified as unknown.
        assert_eq!(unsafe { select_c(std::ptr::null()) }, 0);
    }
}
