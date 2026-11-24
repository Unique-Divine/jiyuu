use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr;

use jiyuu_mori as _; // Link the crate's native archive into the binary test.
use serde_json::Value;

#[repr(C)]
struct Server {
    _opaque: [u8; 0],
}

type MutationBegin = unsafe extern "C" fn(*mut c_void, *const c_char) -> bool;
type MutationEnd = unsafe extern "C" fn(*mut c_void, *const c_char);

unsafe extern "C" {
    fn cbm_mcp_server_new(store_path: *const c_char) -> *mut Server;
    fn cbm_mcp_server_free(server: *mut Server);
    fn cbm_mcp_server_store(server: *mut Server) -> *mut c_void;
    fn cbm_mcp_server_set_project(server: *mut Server, project: *const c_char);
    fn cbm_store_upsert_project(
        store: *mut c_void,
        name: *const c_char,
        root_path: *const c_char,
    ) -> c_int;
    fn cbm_mcp_server_set_project_mutation_guard(
        server: *mut Server,
        begin: Option<MutationBegin>,
        end: Option<MutationEnd>,
        context: *mut c_void,
    );
    fn cbm_mcp_server_handle(
        server: *mut Server,
        line: *const c_char,
    ) -> *mut c_char;
    fn cbm_mcp_server_cancel_active(server: *mut Server) -> bool;
    fn free(allocation: *mut c_void);
}

struct Fixture(*mut Server);

impl Drop for Fixture {
    fn drop(&mut self) {
        // SAFETY: The fixture owns the native server.
        unsafe { cbm_mcp_server_free(self.0) };
    }
}

struct NestedRequest {
    server: *mut Server,
    line: &'static CStr,
    reply: *mut c_char,
    calls: usize,
    releases: usize,
}

unsafe extern "C" fn at_mutation_begin(
    context: *mut c_void,
    _: *const c_char,
) -> bool {
    // SAFETY: The fixture lends the request for this synchronous tool call.
    // The nested cancellation method does not acquire a project lease.
    unsafe {
        let nested = &mut *context.cast::<NestedRequest>();
        nested.calls += 1;
        nested.reply =
            cbm_mcp_server_handle(nested.server, nested.line.as_ptr());
    }
    true
}

unsafe extern "C" fn at_mutation_end(context: *mut c_void, _: *const c_char) {
    // SAFETY: The fixture lends the same request through mutation release.
    unsafe { (*context.cast::<NestedRequest>()).releases += 1 };
}

unsafe fn take_response(reply: *mut c_char) -> Option<Value> {
    if reply.is_null() {
        return None;
    }
    // SAFETY: The handler returns an owned, NUL-terminated response.
    let parsed =
        unsafe { serde_json::from_slice(CStr::from_ptr(reply).to_bytes()) };
    unsafe { free(reply.cast()) };
    Some(parsed.expect("valid JSON-RPC response"))
}

#[test]
fn native_cancellation_dispatch_respects_active_request_scope() {
    // SAFETY: This is the only binary unit test that installs MCP dispatch
    // callbacks. It owns the server and invokes all requests on this thread.
    unsafe {
        // Use the embedded server's in-memory store. Other unit tests in
        // this process can already have initialized SQLite.
        let fixture = Fixture(cbm_mcp_server_new(ptr::null()));
        let server = fixture.0;
        assert!(!server.is_null());
        let ping = c"{\"jsonrpc\":\"2.0\",\"id\":\"boot\",\"method\":\"ping\"}";
        let unavailable =
            take_response(cbm_mcp_server_handle(server, ping.as_ptr())).unwrap();
        assert_eq!(unavailable["id"], "boot");
        assert_eq!(unavailable["error"]["code"], -32603);

        super::register();
        let ready =
            take_response(cbm_mcp_server_handle(server, ping.as_ptr())).unwrap();
        assert_eq!(ready["id"], "boot");
        assert_eq!(ready["result"], serde_json::json!({}));

        let store = cbm_mcp_server_store(server);
        assert!(!store.is_null());
        assert_eq!(
            cbm_store_upsert_project(store, c"fixture".as_ptr(), c"/".as_ptr()),
            0
        );
        cbm_mcp_server_set_project(server, c"fixture".as_ptr());

        let tool = c"{\"jsonrpc\":\"2.0\",\"id\":\"17\",\"method\":\"tools/call\",\"params\":{\"name\":\"ingest_traces\",\"arguments\":{\"project\":\"fixture\",\"traces\":[{\"caller\":\"unresolved_a\",\"callee\":\"unresolved_b\"}]}}}";
        let matching = c"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":\"17\"}}";
        let wrong_type = c"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":17}}";
        let id_present = c"{\"jsonrpc\":\"2.0\",\"id\":99,\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":\"17\"}}";

        for (line, cancelled) in
            [(matching, true), (wrong_type, false), (id_present, false)]
        {
            let mut nested = NestedRequest {
                server,
                line,
                reply: ptr::null_mut(),
                calls: 0,
                releases: 0,
            };
            cbm_mcp_server_set_project_mutation_guard(
                server,
                Some(at_mutation_begin),
                Some(at_mutation_end),
                (&raw mut nested).cast(),
            );
            let outer_reply = cbm_mcp_server_handle(server, tool.as_ptr());
            cbm_mcp_server_set_project_mutation_guard(
                server,
                None,
                None,
                ptr::null_mut(),
            );
            let outer = take_response(outer_reply).expect("outer tool reply");
            let reply = take_response(nested.reply);
            assert_eq!(nested.calls, 1);
            assert_eq!(nested.releases, 1);
            if line == id_present {
                let error =
                    reply.expect("ID-bearing cancellation must get an error");
                assert_eq!(error["id"], 99);
                assert_eq!(error["error"]["code"], -32601);
            } else {
                assert!(reply.is_none(), "notification returned {reply:?}");
            }
            assert_eq!(outer["id"], "17");
            assert_eq!(outer["result"]["isError"], cancelled, "{outer}");
            if cancelled {
                assert_eq!(
                    outer["result"]["content"][0]["text"],
                    "project operation cancelled for this request"
                );
            }
            assert!(
                !cbm_mcp_server_cancel_active(server),
                "request scope leaked"
            );
        }
    }
}
