//! Dispatch MCP methods to C operations and decode tool call parameters.

use std::fmt;

use serde::Deserialize;
use serde::de::{MapAccess, Visitor};
use serde_json::value::RawValue;

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

mod methods;

#[cfg(feature = "c-backend")]
mod linked {
    #[cfg(test)]
    mod native_tests;

    use std::ffi::{CStr, CString, c_char};
    use std::{panic, ptr};

    use super::methods::{RequestContext, RequestOps, dispatch};

    type Dispatch = unsafe extern "C" fn(
        method: *const c_char,
        has_id: bool,
        ctx: *mut RequestContext,
        ops: *const RequestOps,
    );

    type ToolCallDecoder = unsafe extern "C" fn(
        params: *const c_char,
        name_out: *mut *mut c_char,
        args_out: *mut *mut c_char,
    ) -> bool;

    unsafe extern "C" {
        fn cbm_mcp_set_dispatch(dispatch: Dispatch);
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
            cbm_mcp_set_dispatch(dispatch);
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
}
