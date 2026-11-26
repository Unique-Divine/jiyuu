//! Source-window policy for the MCP tool get_code_snippet.

use std::ffi::c_int;

mod source;

const DEFAULT_LINES: c_int = 50;
const MAX_LINES: c_int = 500;
const OUTLINE_THRESHOLD: c_int = 200;

// The request and result match the C structs in mcp/mcp.h. The planner does
// not receive the path, source bytes, store, or response document.
#[repr(C)]
pub(crate) struct Request {
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
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Window {
    original_start: c_int,
    original_end: c_int,
    start: c_int,
    end: c_int,
    max_output_tokens: c_int,
    outline: bool,
    clipped: bool,
}

fn plan(request: &Request) -> Window {
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

/// # Safety
/// The caller must provide a readable, initialized request for this call.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mori_snippet_plan(
    request: *const Request,
) -> Window {
    // SAFETY: The caller borrows one initialized request for the call.
    plan(unsafe { &*request })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> Request {
        Request {
            node_start: 1,
            node_end: 603,
            requested_start: 0,
            max_lines: 0,
            max_output_tokens: 2_500,
            explicit_full: false,
            explicit_outline: false,
            container: true,
        }
    }

    #[test]
    fn full_source_pages_keep_indexed_bounds() {
        let mut request = request();
        request.explicit_full = true;
        request.max_output_tokens = 1;
        let first = plan(&request);
        assert_eq!((first.start, first.end, first.clipped), (1, 500, true));
        assert_eq!((first.original_start, first.original_end), (1, 603));
        assert_eq!(first.max_output_tokens, 128);

        request.requested_start = first.end + 1;
        request.max_lines = 80;
        let second = plan(&request);
        assert_eq!((second.start, second.end, second.clipped), (501, 580, true));
        request.requested_start = second.end + 1;
        let last = plan(&request);
        assert_eq!((last.start, last.end, last.clipped), (581, 603, false));
    }

    #[test]
    fn outline_ignores_source_pagination() {
        let mut request = request();
        request.requested_start = 200;
        request.max_lines = 3;
        let auto = plan(&request);
        assert_eq!((auto.start, auto.end), (1, 603));
        assert!(auto.outline);
        assert!(!auto.clipped);
        request.explicit_outline = true;
        assert_eq!(plan(&request), auto);

        request.explicit_outline = false;
        request.requested_start = 0;
        request.node_end = 200;
        assert!(!plan(&request).outline);
        request.node_end = 201;
        assert!(plan(&request).outline);
    }

    #[test]
    fn one_line_and_missing_end_handle_out_of_range_starts() {
        let mut request = request();
        request.node_start = 10;
        request.node_end = 10;
        request.requested_start = 999;
        let single = plan(&request);
        assert_eq!((single.start, single.end), (10, 10));
        request.node_end = 0;
        assert_eq!(plan(&request).end, 60);
        request.node_start = c_int::MAX - 1;
        assert_eq!(plan(&request).end, c_int::MAX);
    }
}
