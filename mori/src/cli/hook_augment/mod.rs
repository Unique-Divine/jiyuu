//! Hook policy for both the native client and the daemon.
//!
//! C owns the watchdog, standard streams, platform path calls, the MCP server,
//! and the yyjson documents. Returned C strings use the engine allocator.

mod context;
mod engine;
mod json;
mod matching;

use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr::{self, NonNull};
use std::slice;

use json::Doc;
use matching::{Dialect, ToolCall, tool_event};

const STDIN_CAP: usize = 256 * 1024;

#[repr(C)]
struct HookServer {
    _opaque: [u8; 0],
}

unsafe extern "C" {
    fn mori_hook_alloc(size: usize) -> *mut c_char;
    fn mori_hook_free(ptr: *mut c_void);
    fn mori_hook_write(output: *const c_char);
    fn cbm_hook_augment_arm_deadline();
    fn cbm_hook_augment_read_stdin() -> *mut c_char;
}

struct CBytes(NonNull<c_char>);

impl CBytes {
    /// The pointer must be null or an owned, NUL-terminated engine allocation.
    unsafe fn new(ptr: *mut c_char) -> Option<Self> {
        NonNull::new(ptr).map(Self)
    }

    fn bytes(&self) -> &[u8] {
        unsafe { CStr::from_ptr(self.0.as_ptr()) }.to_bytes()
    }
}

impl Drop for CBytes {
    fn drop(&mut self) {
        unsafe { mori_hook_free(self.0.as_ptr().cast()) }
    }
}

/// The pointer must be null or a live NUL-terminated string for the borrow.
unsafe fn c_bytes<'a>(ptr: *const c_char) -> Option<&'a [u8]> {
    (!ptr.is_null()).then(|| unsafe { CStr::from_ptr(ptr) }.to_bytes())
}

fn owned_output(bytes: Option<Vec<u8>>) -> *mut c_char {
    let Some(bytes) = bytes else {
        return ptr::null_mut();
    };
    let output = unsafe { mori_hook_alloc(bytes.len() + 1) };
    if !output.is_null() {
        unsafe { copy_to_c(&bytes, output) };
    }
    output
}

/// The destination must hold bytes.len() + 1 bytes and be disjoint from bytes.
unsafe fn copy_to_c(bytes: &[u8], out: *mut c_char) {
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), out.cast::<u8>(), bytes.len());
        *out.add(bytes.len()) = 0;
    }
}

fn read_input(input: &[u8]) -> Option<Doc> {
    (input.len() <= STDIN_CAP)
        .then(|| Doc::read(input))
        .flatten()
}

fn noop(input: &[u8], forced: Option<&[u8]>, dialect: Dialect) -> bool {
    let copilot = dialect == Dialect::Copilot;
    let search_event: &[u8] = if copilot {
        b"PostToolUse"
    } else {
        b"PreToolUse"
    };
    let expected_forced = copilot.then_some(search_event);
    if forced != expected_forced {
        return false;
    }
    let Some(doc) = read_input(input) else {
        return false;
    };
    let call = ToolCall::read(doc.root(), forced, dialect);
    let supported = call
        .event
        .zip(call.tool)
        .is_some_and(|(event, tool)| tool_event(dialect, event, tool).is_some());
    (copilot && !supported)
        || (call.event == Some(search_event)
            && call.tool == Some(b"Bash")
            && call.search_token().is_none())
}

// The native callers uphold the C string, server and output-buffer contracts
// in cli/cli.h. Keep the raw pointer conversions at these entry points.

#[cfg(unix)]
#[unsafe(no_mangle)]
extern "C" fn mori_hook_deadline_ms() -> c_int {
    unsafe extern "C" {
        fn cbm_env_long(
            name: *const c_char,
            value: *mut std::ffi::c_long,
        ) -> bool;
    }
    let mut value = 0;
    if unsafe { cbm_env_long(c"CBM_HOOK_DEADLINE_MS".as_ptr(), &mut value) } {
        value.clamp(50, 10_000) as c_int
    } else {
        2000
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_hook_path_is_abs(path: *const c_char) -> bool {
    unsafe { c_bytes(path) }.is_some_and(engine::absolute)
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_hook_augment_is_graph_tool(
    tool: *const c_char,
) -> bool {
    unsafe { c_bytes(tool) }.is_some_and(matching::is_graph_tool)
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_hook_augment_invocation_supported(
    forced: *const c_char,
    dialect: *const c_char,
) -> bool {
    let (forced, dialect) = unsafe { (c_bytes(forced), c_bytes(dialect)) };
    Dialect::from_optional(dialect)
        .is_some_and(|dialect| dialect.invocation(forced))
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_hook_augment_input_is_noop_for(
    input: *const c_char,
    forced: *const c_char,
    dialect: *const c_char,
) -> bool {
    let (input, forced, dialect) =
        unsafe { (c_bytes(input), c_bytes(forced), c_bytes(dialect)) };
    input
        .zip(Dialect::from_optional(dialect))
        .is_some_and(|(input, dialect)| noop(input, forced, dialect))
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_hook_augment_process(
    server: *mut HookServer,
    input: *const c_char,
) -> *mut c_char {
    unsafe {
        cbm_hook_augment_process_for(server, input, ptr::null(), ptr::null())
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_hook_augment_process_for(
    server: *mut HookServer,
    input: *const c_char,
    forced: *const c_char,
    dialect: *const c_char,
) -> *mut c_char {
    let (input, forced, dialect) =
        unsafe { (c_bytes(input), c_bytes(forced), c_bytes(dialect)) };
    owned_output(input.zip(Dialect::from_optional(dialect)).and_then(
        |(input, dialect)| engine::process(server, input, forced, dialect),
    ))
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_hook_augment_lifecycle_json(
    input: *const c_char,
) -> *mut c_char {
    unsafe { cbm_hook_augment_lifecycle_json_for(input, ptr::null(), false) }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_hook_augment_lifecycle_json_for(
    input: *const c_char,
    forced: *const c_char,
    copilot: bool,
) -> *mut c_char {
    let (input, forced) = unsafe { (c_bytes(input), c_bytes(forced)) };
    if forced.is_some_and(|event| !Dialect::Event.lifecycle_event(event)) {
        return ptr::null_mut();
    }
    let dialect = if copilot {
        Dialect::Copilot
    } else {
        Dialect::Event
    };
    owned_output(input.and_then(read_input).and_then(|doc| {
        engine::lifecycle(ptr::null_mut(), doc.root(), forced, dialect)
    }))
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_cmd_hook_augment(
    argc: c_int,
    argv: *const *const c_char,
) -> c_int {
    unsafe { cbm_hook_augment_arm_deadline() };
    let args = if argc <= 0 || argv.is_null() {
        &[]
    } else {
        unsafe { slice::from_raw_parts(argv, argc as usize) }
    };
    let mut args = args.iter().map(|&arg| unsafe { c_bytes(arg) });
    let mut forced = None;
    let mut dialect = Dialect::Event;
    while let Some(arg) = args.next() {
        match arg {
            Some(b"--event") => match args.next().flatten() {
                Some(event) => forced = Some(event),
                None => return 0,
            },
            Some(b"--dialect") => {
                match args.next().flatten().and_then(Dialect::named) {
                    Some(value) => dialect = value,
                    None => return 0,
                }
            }
            _ => return 0,
        }
    }
    if !dialect.invocation(forced) {
        return 0;
    }
    let Some(input) = (unsafe { CBytes::new(cbm_hook_augment_read_stdin()) })
    else {
        return 0;
    };
    if !noop(input.bytes(), forced, dialect) {
        let server = engine::OwnedServer::new();
        let output = owned_output(engine::process(
            server.get(),
            input.bytes(),
            forced,
            dialect,
        ));
        if let Some(output) = unsafe { CBytes::new(output) } {
            unsafe { mori_hook_write(output.0.as_ptr()) };
        }
    }
    0
}

#[unsafe(no_mangle)]
extern "C" fn cbm_hook_admission_notice(
    reason: c_int,
    dialect: *const c_char,
) -> *const c_char {
    if !dialect.is_null() {
        return ptr::null();
    }
    // cbm_hook_admission_t in cli/cli.h.
    match reason {
        0 => c"{\"systemMessage\":\"mori: no CBM daemon is running, so graph augmentation is currently skipped. Run `mori daemon start` (or open an MCP session) to enable it.\"}".as_ptr(),
        1 => c"{\"systemMessage\":\"mori: graph augmentation is skipped: the active CBM daemon runs a different build than this binary (usually an update was installed while the old daemon kept running). Run `mori daemon stop`, then retry - the next command starts a matching daemon.\"}".as_ptr(),
        _ => ptr::null(),
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_hook_sanitize_metadata_for_testing(
    input: *const c_char,
    output: *mut c_char,
    cap: usize,
) {
    if !output.is_null() && cap != 0 {
        let input = unsafe { c_bytes(input) }.unwrap_or_default();
        let sanitized = context::sanitize(input, cap);
        unsafe { copy_to_c(&sanitized, output) };
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_hook_augment_lifecycle_json_for_dialect(
    input: *const c_char,
    forced: *const c_char,
    dialect: *const c_char,
) -> *mut c_char {
    let (input, forced, dialect) =
        unsafe { (c_bytes(input), c_bytes(forced), c_bytes(dialect)) };
    owned_output(
        input
            .and_then(read_input)
            .zip(dialect.and_then(Dialect::named))
            .and_then(|(doc, dialect)| {
                engine::lifecycle(ptr::null_mut(), doc.root(), forced, dialect)
            }),
    )
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_hook_augment_format_context_for_testing(
    envelope: *const c_char,
    token: *const c_char,
    is_error: *mut bool,
) -> *mut c_char {
    let (envelope, token) = unsafe { (c_bytes(envelope), c_bytes(token)) };
    let result = context::search_context(
        envelope.unwrap_or_default(),
        token.unwrap_or_default(),
    );
    if !is_error.is_null() {
        unsafe { *is_error = result.is_err() };
    }
    owned_output(result.ok().flatten())
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_hook_augment_tool_json_for_testing(
    input: *const c_char,
    dialect: *const c_char,
    context: *const c_char,
    path: *mut c_char,
    cap: usize,
) -> *mut c_char {
    if path.is_null() || cap == 0 {
        return ptr::null_mut();
    }
    let (input, dialect, context) =
        unsafe { (c_bytes(input), c_bytes(dialect), c_bytes(context)) };
    owned_output((|| {
        let context = context?;
        let doc = read_input(input?)?;
        let dialect = dialect.map_or(Some(Dialect::Event), Dialect::named)?;
        let root = doc.root();
        let event = matching::event_name(root)?;
        let tool = root.field(c"tool_name")?;
        if tool_event(dialect, event, tool)
            != Some(matching::ToolEvent::Coverage)
        {
            return None;
        }
        let normalized = engine::tool_path(root, root.get(c"tool_input"), cap)?;
        unsafe { copy_to_c(&normalized, path) };
        context::output(Dialect::Event, event, context)
    })())
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_hook_augment_invocation_supported_for_testing(
    dialect: *const c_char,
    forced: *const c_char,
) -> bool {
    let (dialect, forced) = unsafe { (c_bytes(dialect), c_bytes(forced)) };
    dialect
        .map_or(Some(Dialect::Event), Dialect::named)
        .is_some_and(|dialect| dialect.invocation(forced))
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_hook_path_contains_for_testing(
    root: *const c_char,
    candidate: *const c_char,
    case_insensitive: bool,
) -> bool {
    unsafe { c_bytes(root).zip(c_bytes(candidate)) }.is_some_and(
        |(root, candidate)| engine::contains(root, candidate, case_insensitive),
    )
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_hook_no_project_index_guidance_for_testing(
    event: *const c_char,
    ignored: bool,
) -> *const c_char {
    context::guidance(unsafe { c_bytes(event) }, ignored).as_ptr()
}

#[unsafe(no_mangle)]
extern "C" fn cbm_hook_set_worktree_gate_counter_for_testing(
    counter: *mut c_int,
) {
    engine::WORKTREE_COUNTER.with(|slot| slot.set(counter));
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_hook_augment_parse_bash_pattern_for_testing(
    cmd: *const c_char,
    out: *mut c_char,
    cap: usize,
) -> bool {
    if out.is_null() || cap == 0 {
        return false;
    }
    let Some(pattern) =
        (unsafe { c_bytes(cmd) }).and_then(matching::bash_pattern)
    else {
        return false;
    };
    // snprintf leaves a truncated, terminated value even when reporting failure.
    unsafe { copy_to_c(&pattern[..pattern.len().min(cap - 1)], out) };
    pattern.len() < cap
}
