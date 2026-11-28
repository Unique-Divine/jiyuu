//! Lifecycle and method routing for MCP JSON-RPC requests.

use std::ffi::{CStr, c_char, c_int};
use std::ptr;

use crate::ffi_output;

const PARSE_ERROR: c_int = -32700;
const INTERNAL_ERROR: c_int = -32603;

#[repr(C)]
pub(super) struct RequestContext {
    _opaque: [u8; 0],
}

#[repr(C)]
struct Server {
    _opaque: [u8; 0],
}

/// Mirrors cbm_jsonrpc_request_t. C owns the fields until `free_parsed`.
#[repr(C)]
struct JsonRpcRequest {
    jsonrpc: *const c_char,
    method: *const c_char,
    id: i64,
    id_str: *const c_char,
    has_id: bool,
    params_raw: *const c_char,
}

type RequestOp = unsafe extern "C" fn(*mut RequestContext);

/// Mirrors mcp_request_ops_t in mcp.c. Operations borrow the context
/// synchronously. C owns allocations, the codec, and the native tool calls.
#[repr(C)]
pub(super) struct RequestOps {
    parse: unsafe extern "C" fn(*const c_char, *mut JsonRpcRequest) -> c_int,
    begin_scope: unsafe extern "C" fn(*mut Server) -> bool,
    start_timer: RequestOp,
    trace_dispatch: RequestOp,
    finish: unsafe extern "C" fn(
        *mut RequestContext,
        *const c_char,
        bool,
    ) -> *mut c_char,
    end_scope: unsafe extern "C" fn(*mut Server),
    free_parsed: unsafe extern "C" fn(*mut JsonRpcRequest),
    format_error: unsafe extern "C" fn(i64, c_int, *const c_char) -> *mut c_char,
    initialize: RequestOp,
    ping: RequestOp,
    resources_list: RequestOp,
    resource_templates_list: RequestOp,
    prompts_list: RequestOp,
    prompts_get: RequestOp,
    tools_list: RequestOp,
    tools_call: RequestOp,
    cancel_notification: RequestOp,
}

enum RouteOutcome {
    Handled,
    MethodNotFound { expected: bool },
}

/// # Safety
/// The context and operations must be live and satisfy the request ABI.
unsafe fn dispatch_method(
    method: &CStr,
    has_id: bool,
    ctx: *mut RequestContext,
    ops: &RequestOps,
) -> RouteOutcome {
    let method = method.to_bytes();
    let operation = if has_id {
        match method {
            b"initialize" => ops.initialize,
            b"ping" => ops.ping,
            b"resources/list" => ops.resources_list,
            b"resources/templates/list" => ops.resource_templates_list,
            b"prompts/list" => ops.prompts_list,
            b"prompts/get" => ops.prompts_get,
            b"tools/list" => ops.tools_list,
            b"tools/call" => ops.tools_call,
            _ => {
                // The discovery probe expects a method error and then uses
                // initialize, so leave its request diagnostics successful.
                return RouteOutcome::MethodNotFound {
                    expected: method == b"server/discover",
                };
            }
        }
    } else if method == b"notifications/cancelled" {
        ops.cancel_notification
    } else {
        return RouteOutcome::Handled;
    };
    // SAFETY: The caller provides the live context for the operation table.
    unsafe { operation(ctx) };
    RouteOutcome::Handled
}

/// # Safety
/// C supplies live context and request storage, NUL-terminated input, and the
/// matching table. The server pointer must satisfy the native scope callbacks.
unsafe fn handle_request(
    line: *const c_char,
    server: *mut Server,
    request: *mut JsonRpcRequest,
    ctx: *mut RequestContext,
    ops: &RequestOps,
) -> *mut c_char {
    // SAFETY: The parser initializes the request in C-owned storage. Copy the
    // header fields rather than holding a reference across mutable callbacks.
    unsafe {
        if (ops.parse)(line, request) < 0 {
            return (ops.format_error)(
                0,
                PARSE_ERROR,
                ffi_output::c_str(c"Parse error"),
            );
        }

        let method = (*request).method;
        let method = if method.is_null() {
            c""
        } else {
            CStr::from_ptr(method)
        };
        if !(*request).has_id {
            dispatch_method(method, false, ctx, ops);
            (ops.trace_dispatch)(ctx);
            (ops.free_parsed)(request);
            return ptr::null_mut();
        }
        if !(ops.begin_scope)(server) {
            let id = (*request).id;
            (ops.free_parsed)(request);
            return (ops.format_error)(
                id,
                INTERNAL_ERROR,
                ffi_output::c_str(c"Request cancellation scope unavailable"),
            );
        }

        (ops.start_timer)(ctx);
        let outcome = dispatch_method(method, true, ctx, ops);
        (ops.trace_dispatch)(ctx);
        let (protocol_error, unexpected_method) = match outcome {
            RouteOutcome::Handled => (ptr::null(), false),
            RouteOutcome::MethodNotFound { expected } => (
                ffi_output::c_str(
                    c"{\"code\":-32601,\"message\":\"Method not found\"}",
                ),
                !expected,
            ),
        };
        let response = (ops.finish)(ctx, protocol_error, unexpected_method);
        (ops.end_scope)(server);
        (ops.free_parsed)(request);
        response
    }
}

/// Write the C-owned response pointer back to the caller. The caller frees it
/// with the native allocator.
///
/// # Safety
/// In addition to the request ABI, `out` must point to writable pointer storage.
#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_mcp_handle_request(
    line: *const c_char,
    server: *mut Server,
    request: *mut JsonRpcRequest,
    ctx: *mut RequestContext,
    ops: *const RequestOps,
    out: *mut *mut c_char,
) {
    // SAFETY: C supplies the matching table and a writable pointer on every call.
    unsafe {
        ffi_output::write(out, handle_request(line, server, request, ctx, &*ops))
    };
}

#[cfg(test)]
#[path = "methods_tests.rs"]
mod tests;
