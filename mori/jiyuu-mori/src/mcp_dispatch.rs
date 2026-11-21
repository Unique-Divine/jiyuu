//! Select the native request handler for a parsed MCP method.

use std::ffi::{CStr, c_char, c_int};

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

#[cfg(all(feature = "c-backend", not(test)))]
unsafe extern "C" {
    fn cbm_mcp_set_method_selector(
        select: unsafe extern "C" fn(*const c_char) -> c_int,
    );
}

#[cfg(all(feature = "c-backend", not(test)))]
pub(crate) unsafe fn register() {
    // SAFETY: C calls the static selector only during request dispatch.
    unsafe { cbm_mcp_set_method_selector(select_c) };
}

#[cfg(test)]
mod tests {
    use super::*;

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
