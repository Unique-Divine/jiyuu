use super::*;
use std::cell::RefCell;
use std::mem::MaybeUninit;

#[derive(Default)]
struct Probe {
    events: Vec<&'static str>,
    id: Option<i64>,
    deny_scope: bool,
    protocol_error: Option<String>,
    unexpected_method: bool,
    formatted_error: Option<(i64, c_int, String)>,
}

thread_local! {
    static PROBE: RefCell<Probe> = RefCell::new(Probe::default());
}

fn event(name: &'static str) {
    PROBE.with_borrow_mut(|probe| probe.events.push(name));
}

unsafe extern "C" fn parse(
    line: *const c_char,
    request: *mut JsonRpcRequest,
) -> c_int {
    event("parse");
    // SAFETY: The test passes a static C string and writable request storage.
    if unsafe { CStr::from_ptr(line) } == c"invalid" {
        return -1;
    }
    let id = PROBE.with_borrow(|probe| probe.id);
    unsafe {
        request.write(JsonRpcRequest {
            jsonrpc: c"2.0".as_ptr(),
            method: line,
            id: id.unwrap_or(-1),
            id_str: ptr::null(),
            has_id: id.is_some(),
            params_raw: ptr::null(),
        });
    }
    0
}

unsafe extern "C" fn begin(_: *mut Server) -> bool {
    event("begin");
    PROBE.with_borrow(|probe| !probe.deny_scope)
}

unsafe extern "C" fn start(_: *mut RequestContext) {
    event("timer");
}

unsafe extern "C" fn trace(_: *mut RequestContext) {
    event("trace");
}

unsafe extern "C" fn finish(
    _: *mut RequestContext,
    protocol_error: *const c_char,
    unexpected_method: bool,
) -> *mut c_char {
    event("finish");
    PROBE.with_borrow_mut(|probe| {
        probe.protocol_error = (!protocol_error.is_null()).then(|| {
            // SAFETY: Routing passes a NUL-terminated error or null.
            unsafe { CStr::from_ptr(protocol_error) }
                .to_string_lossy()
                .into_owned()
        });
        probe.unexpected_method = unexpected_method;
    });
    c"response".as_ptr().cast_mut()
}

unsafe extern "C" fn end(_: *mut Server) {
    event("end");
}

unsafe extern "C" fn free(_: *mut JsonRpcRequest) {
    event("free");
}

unsafe extern "C" fn format_error(
    id: i64,
    code: c_int,
    message: *const c_char,
) -> *mut c_char {
    event("error");
    // SAFETY: The request protocol supplies a static error message.
    let message = unsafe { CStr::from_ptr(message) }
        .to_string_lossy()
        .into_owned();
    PROBE.with_borrow_mut(|probe| {
        probe.formatted_error = Some((id, code, message));
    });
    c"error".as_ptr().cast_mut()
}

unsafe extern "C" fn request(_: *mut RequestContext) {
    event("method");
}

unsafe extern "C" fn cancel(_: *mut RequestContext) {
    event("cancel");
}

fn run(method: &CStr, id: Option<i64>, deny_scope: bool) -> (bool, Probe) {
    PROBE.with_borrow_mut(|probe| {
        *probe = Probe {
            id,
            deny_scope,
            ..Probe::default()
        };
    });
    let ops = RequestOps {
        parse,
        begin_scope: begin,
        start_timer: start,
        trace_dispatch: trace,
        finish,
        end_scope: end,
        free_parsed: free,
        format_error,
        initialize: request,
        ping: request,
        resources_list: request,
        resource_templates_list: request,
        prompts_list: request,
        prompts_get: request,
        tools_list: request,
        tools_call: request,
        cancel_notification: cancel,
    };
    let mut parsed = MaybeUninit::uninit();
    let mut response = ptr::null_mut();
    // SAFETY: All fixture operations ignore the mock server and context.
    // The parser initializes the supplied request storage on success.
    unsafe {
        cbm_rust_mcp_handle_request(
            method.as_ptr(),
            ptr::null_mut(),
            parsed.as_mut_ptr(),
            ptr::null_mut(),
            &ops,
            &raw mut response,
        );
    }
    (!response.is_null(), PROBE.with_borrow_mut(std::mem::take))
}

#[test]
fn rejects_parse_errors_and_unavailable_scope_without_dispatch() {
    let (has_response, invalid) = run(c"invalid", None, false);
    assert!(has_response);
    assert_eq!(invalid.events, ["parse", "error"]);
    assert_eq!(
        invalid.formatted_error,
        Some((0, -32700, "Parse error".into())),
    );

    let (has_response, unavailable) = run(c"ping", Some(42), true);
    assert!(has_response);
    assert_eq!(unavailable.events, ["parse", "begin", "free", "error"]);
    assert_eq!(
        unavailable.formatted_error,
        Some((42, -32603, "Request cancellation scope unavailable".into())),
    );
}

#[test]
fn cancellation_is_dispatched_only_as_a_notification() {
    let (has_response, notification) =
        run(c"notifications/cancelled", None, false);
    assert!(!has_response);
    assert_eq!(notification.events, ["parse", "cancel", "trace", "free"]);

    let (has_response, notification) = run(c"tools/call", None, false);
    assert!(!has_response);
    assert_eq!(notification.events, ["parse", "trace", "free"]);

    let (has_response, request) =
        run(c"notifications/cancelled", Some(1), false);
    assert!(has_response);
    assert_eq!(
        request.events,
        ["parse", "begin", "timer", "trace", "finish", "end", "free"],
    );
    assert!(request.unexpected_method);
}

#[test]
fn serializes_before_closing_scope_and_preserves_discover_diagnostics() {
    let (has_response, ping) = run(c"ping", Some(1), false);
    assert!(has_response);
    assert_eq!(
        ping.events,
        [
            "parse", "begin", "timer", "method", "trace", "finish", "end",
            "free"
        ],
    );
    assert!(ping.protocol_error.is_none());

    for (method, unexpected_method) in
        [(c"server/discover", false), (c"bogus", true)]
    {
        let (has_response, probe) = run(method, Some(1), false);
        assert!(has_response);
        assert_eq!(
            probe.events,
            ["parse", "begin", "timer", "trace", "finish", "end", "free"],
        );
        assert_eq!(probe.unexpected_method, unexpected_method);
        assert_eq!(
            probe.protocol_error.as_deref(),
            Some(r#"{"code":-32601,"message":"Method not found"}"#),
        );
    }
}
