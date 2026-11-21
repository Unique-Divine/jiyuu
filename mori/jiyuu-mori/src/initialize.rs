//! The MCP initialize result for the linked Mori daemon.

use serde::Serialize;

#[derive(Serialize)]
struct ServerInfo<'a> {
    name: &'a str,
    version: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ListCapability {
    list_changed: bool,
}

#[derive(Serialize)]
struct Capabilities {
    tools: ListCapability,
    prompts: ListCapability,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Initialize<'a> {
    protocol_version: &'a str,
    server_info: ServerInfo<'a>,
    capabilities: Capabilities,
    instructions: &'a str,
}

fn build_initialize(
    requested: Option<&str>,
    supported: &[&str],
    name: &str,
    version: &str,
    instructions: &str,
) -> Option<Vec<u8>> {
    let latest = *supported.first()?;
    let protocol_version = requested
        .filter(|version| supported.contains(version))
        .unwrap_or(latest);
    let response = Initialize {
        protocol_version,
        server_info: ServerInfo { name, version },
        capabilities: Capabilities {
            tools: ListCapability {
                list_changed: false,
            },
            prompts: ListCapability {
                list_changed: false,
            },
        },
        instructions,
    };
    serde_json::to_vec(&response).ok()
}

#[cfg(all(feature = "c-backend", not(test)))]
pub(crate) mod c_initialize {
    use std::ffi::{CStr, CString, c_char};
    use std::{ptr, slice};

    type InitializeBuilder = unsafe extern "C" fn(
        *const c_char,
        *const *const c_char,
        usize,
        *const c_char,
        *const c_char,
        *const c_char,
    ) -> *mut c_char;

    unsafe extern "C" {
        fn cbm_mcp_set_initialize_builder(builder: InitializeBuilder);
        fn strdup(s: *const c_char) -> *mut c_char;
    }

    fn result(
        requested: Option<&CStr>,
        supported: &[&CStr],
        name: &CStr,
        version: &CStr,
        instructions: &CStr,
    ) -> Option<CString> {
        let requested = requested.map(CStr::to_str).transpose().ok()?;
        let supported = supported
            .iter()
            .map(|version| version.to_str())
            .collect::<Result<Vec<_>, _>>()
            .ok()?;
        let json = super::build_initialize(
            requested,
            &supported,
            name.to_str().ok()?,
            version.to_str().ok()?,
            instructions.to_str().ok()?,
        )?;
        CString::new(json).ok()
    }

    unsafe extern "C" fn build(
        requested: *const c_char,
        supported: *const *const c_char,
        count: usize,
        name: *const c_char,
        version: *const c_char,
        instructions: *const c_char,
    ) -> *mut c_char {
        // SAFETY: C borrows its version table and identity for the call.
        // Only the requested protocol can be absent.
        let supported = unsafe { slice::from_raw_parts(supported, count) }
            .iter()
            .map(|&version| unsafe { CStr::from_ptr(version) })
            .collect::<Vec<_>>();
        let response = unsafe {
            let requested =
                (!requested.is_null()).then(|| CStr::from_ptr(requested));
            result(
                requested,
                &supported,
                CStr::from_ptr(name),
                CStr::from_ptr(version),
                CStr::from_ptr(instructions),
            )
        };
        let Some(response) = response else {
            return ptr::null_mut();
        };
        // SAFETY: C can free this copy after framing the JSON-RPC response.
        unsafe { strdup(response.as_ptr()) }
    }

    pub(crate) unsafe fn register() {
        // SAFETY: caller registers once before starting the C daemon.
        unsafe { cbm_mcp_set_initialize_builder(build) };
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    #[test]
    fn negotiates_from_the_servers_ordered_versions() {
        let versions = ["2025-11-25", "2025-06-18", "2024-11-05"];
        for (requested, expected) in [
            (Some("2024-11-05"), "2024-11-05"),
            (Some("9999-01-01"), "2025-11-25"),
            (None, "2025-11-25"),
        ] {
            let response = build_initialize(
                requested,
                &versions,
                "mori-graph",
                "0.13.1",
                "Scout instructions with a \"quote\".",
            )
            .unwrap();
            assert_eq!(
                serde_json::from_slice::<Value>(&response).unwrap(),
                json!({
                    "protocolVersion": expected,
                    "serverInfo": {"name": "mori-graph", "version": "0.13.1"},
                    "capabilities": {
                        "tools": {"listChanged": false},
                        "prompts": {"listChanged": false},
                    },
                    "instructions": "Scout instructions with a \"quote\".",
                }),
            );
        }
        assert!(build_initialize(None, &[], "mori", "test", "").is_none());
    }
}
