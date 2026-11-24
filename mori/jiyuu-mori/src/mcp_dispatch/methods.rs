//! Method routing for a parsed JSON-RPC request.

use std::ffi::{CStr, c_char};

/// C owns the request and its outputs. Only the C operations may
/// dereference this pointer, and only while dispatch is running.
#[repr(C)]
pub(super) struct RequestContext {
    _opaque: [u8; 0],
}

type RequestOp = unsafe extern "C" fn(*mut RequestContext);

/// Mirrors mcp_request_ops_t in mcp.c.
#[repr(C)]
pub(super) struct RequestOps {
    initialize: RequestOp,
    ping: RequestOp,
    resources_list: RequestOp,
    resource_templates_list: RequestOp,
    prompts_list: RequestOp,
    prompts_get: RequestOp,
    tools_list: RequestOp,
    tools_call: RequestOp,
    cancel_notification: RequestOp,
    unknown: unsafe extern "C" fn(*mut RequestContext, bool),
}

pub(super) unsafe extern "C" fn dispatch(
    method: *const c_char,
    has_id: bool,
    ctx: *mut RequestContext,
    ops: *const RequestOps,
) {
    // SAFETY: C supplies the parsed method, live request, and matching
    // operation table for the duration of this synchronous callback.
    unsafe {
        let method = CStr::from_ptr(method).to_bytes();
        let ops = &*ops;
        if !has_id {
            if method == b"notifications/cancelled" {
                (ops.cancel_notification)(ctx);
            }
            return;
        }

        match method {
            b"initialize" => (ops.initialize)(ctx),
            b"ping" => (ops.ping)(ctx),
            b"resources/list" => (ops.resources_list)(ctx),
            b"resources/templates/list" => (ops.resource_templates_list)(ctx),
            b"prompts/list" => (ops.prompts_list)(ctx),
            b"prompts/get" => (ops.prompts_get)(ctx),
            b"tools/list" => (ops.tools_list)(ctx),
            b"tools/call" => (ops.tools_call)(ctx),
            _ => (ops.unknown)(ctx, method == b"server/discover"),
        }
    }
}

#[cfg(test)]
#[test]
fn dispatches_cancel_only_as_a_notification() {
    #[derive(Debug, PartialEq)]
    enum Operation {
        None,
        Cancel,
        Unknown,
        Discover,
        Request,
    }

    unsafe extern "C" fn request(ctx: *mut RequestContext) {
        // SAFETY: The test passes a writable Operation as its context.
        unsafe { *ctx.cast() = Operation::Request };
    }
    unsafe extern "C" fn cancel(ctx: *mut RequestContext) {
        // SAFETY: The test passes a writable Operation as its context.
        unsafe { *ctx.cast() = Operation::Cancel };
    }
    unsafe extern "C" fn unknown(ctx: *mut RequestContext, discover: bool) {
        // SAFETY: The test passes a writable Operation as its context.
        unsafe {
            *ctx.cast() = if discover {
                Operation::Discover
            } else {
                Operation::Unknown
            };
        }
    }

    let ops = RequestOps {
        initialize: request,
        ping: request,
        resources_list: request,
        resource_templates_list: request,
        prompts_list: request,
        prompts_get: request,
        tools_list: request,
        tools_call: request,
        cancel_notification: cancel,
        unknown,
    };
    for (method, has_id, expected) in [
        (c"notifications/cancelled", false, Operation::Cancel),
        (c"notifications/cancelled", true, Operation::Unknown),
        (c"initialize", false, Operation::None),
        (c"tools/call", false, Operation::None),
        (c"tools/call", true, Operation::Request),
        (c"tools/list", true, Operation::Request),
        (c"ping", true, Operation::Request),
        (c"server/discover", false, Operation::None),
        (c"server/discover", true, Operation::Discover),
        (c"bogus", true, Operation::Unknown),
    ] {
        let mut observed = Operation::None;
        // SAFETY: The method and table are live, and the test operations
        // cast the borrowed pointer back to its original type.
        unsafe {
            dispatch(method.as_ptr(), has_id, (&raw mut observed).cast(), &ops);
        }
        assert_eq!(observed, expected, "{method:?}, has_id={has_id}");
    }
}
