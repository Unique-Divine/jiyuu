//! Source-window policy for the MCP tool get_code_snippet.

use std::ffi::c_int;

const DEFAULT_LINES: c_int = 50;
const MAX_LINES: c_int = 500;
const OUTLINE_THRESHOLD: c_int = 200;

// The request and result match the C structs in mcp/mcp.h. The planner does
// not receive the path, source bytes, store, or response document.
#[repr(C)]
struct Request {
    node_start: c_int,
    node_end: c_int,
    requested_start: c_int,
    max_lines: c_int,
    max_output_tokens: c_int,
    explicit_full: bool,
    explicit_outline: bool,
    container: bool,
}

#[repr(C)]
struct Window {
    original_start: c_int,
    original_end: c_int,
    start: c_int,
    end: c_int,
    max_output_tokens: c_int,
    outline: bool,
    clipped: bool,
}

unsafe extern "C" fn plan(request: *const Request) -> Window {
    // SAFETY: The C handler borrows one initialized request for the call.
    let request = unsafe { &*request };
    let original_start = request.node_start.max(1);
    let original_end = if request.node_end >= original_start {
        request.node_end
    } else {
        original_start.saturating_add(DEFAULT_LINES)
    };
    let outline = request.explicit_outline
        || (!request.explicit_full
            && request.container
            && original_end - original_start >= OUTLINE_THRESHOLD);
    let valid_start = original_start..=original_end;
    let start = if !outline && valid_start.contains(&request.requested_start) {
        request.requested_start
    } else {
        original_start
    };
    let max_lines = match request.max_lines {
        1..=MAX_LINES => request.max_lines,
        _ => MAX_LINES,
    };
    let end = if !outline && original_end - start >= max_lines {
        start + (max_lines - 1)
    } else {
        original_end
    };
    let max_output_tokens = match request.max_output_tokens {
        ..=0 => 0,
        value => value.clamp(128, 1_000_000),
    };
    Window {
        original_start,
        original_end,
        start,
        end,
        max_output_tokens,
        outline,
        clipped: end < original_end,
    }
}

unsafe extern "C" {
    fn cbm_mcp_set_snippet_window_planner(
        planner: unsafe extern "C" fn(*const Request) -> Window,
    );
}

/// Register before C starts request threads.
pub(crate) unsafe fn register() {
    // SAFETY: The registered function lives for the process lifetime.
    unsafe { cbm_mcp_set_snippet_window_planner(plan) };
}
