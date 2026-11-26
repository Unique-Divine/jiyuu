use super::context::{
    coverage_context, envelope_payload, lifecycle_context, output,
    search_context,
};
use super::json::{Doc, Value, quote};
use super::matching::{
    Dialect, ToolCall, ToolEvent, event_name, is_graph_tool, tool_event,
};
use super::{CBytes, HookServer, c_bytes};
use std::cell::Cell;
use std::ffi::{CStr, CString, c_char, c_int};
use std::ptr;
use std::sync::Mutex;

const PATH_CAP: usize = 4096;
const MAX_WALKUP: usize = 8;

unsafe extern "C" {
    fn mori_hook_getcwd(buffer: *mut c_char, cap: usize) -> bool;
    fn mori_hook_fs_resolve(
        input: *const c_char,
        buffer: *mut c_char,
        cap: usize,
    ) -> bool;
    fn cbm_project_name_from_path(path: *const c_char) -> *mut c_char;
    fn cbm_mcp_server_new(store: *const c_char) -> *mut HookServer;
    fn cbm_mcp_server_free(server: *mut HookServer);
    fn cbm_mcp_server_session_root(server: *const HookServer) -> *const c_char;
    fn cbm_mcp_handle_tool(
        server: *mut HookServer,
        tool: *const c_char,
        args: *const c_char,
    ) -> *mut c_char;
    fn cbm_mcp_ignore_worktrees_runtime_enabled(
        server: *const HookServer,
    ) -> bool;
    fn cbm_git_is_linked_worktree(path: *const c_char) -> bool;
    fn cbm_git_same_linked_worktree(
        left: *const c_char,
        right: *const c_char,
    ) -> bool;
}

pub(super) struct OwnedServer(*mut HookServer);

impl OwnedServer {
    pub(super) fn new() -> Self {
        Self(unsafe { cbm_mcp_server_new(ptr::null()) })
    }

    pub(super) fn get(&self) -> *mut HookServer {
        self.0
    }
}

impl Drop for OwnedServer {
    fn drop(&mut self) {
        unsafe { cbm_mcp_server_free(self.0) }
    }
}

pub(super) fn absolute(path: &[u8]) -> bool {
    path.starts_with(b"/")
        || (matches!(path, [_, b':'] | [_, b':', b'/', ..])
            && path[0].is_ascii_alphabetic())
}

pub(super) fn contains(
    root: &[u8],
    candidate: &[u8],
    case_insensitive: bool,
) -> bool {
    if root.is_empty() {
        return false;
    }
    let Some(prefix) = candidate.get(..root.len()) else {
        return false;
    };
    (if case_insensitive {
        root.eq_ignore_ascii_case(prefix)
    } else {
        root == prefix
    }) && (candidate.len() == root.len()
        || root.ends_with(b"/")
        || candidate[root.len()] == b'/')
}

fn contains_path(root: &[u8], path: &[u8]) -> bool {
    contains(root, path, cfg!(windows))
}

fn slash_normalized(path: &[u8]) -> Vec<u8> {
    path.iter()
        .map(|&byte| if byte == b'\\' { b'/' } else { byte })
        .collect()
}

fn parent(path: &[u8]) -> Option<&[u8]> {
    let slash = path.iter().rposition(|&byte| byte == b'/')?;
    if slash == 0 || (slash == 2 && path[1] == b':') {
        return None;
    }
    Some(&path[..slash])
}

fn canonical(path: &[u8]) -> Option<Vec<u8>> {
    if !absolute(path) {
        return None;
    }
    let input = CString::new(path).ok()?;
    let mut resolved = [0; PATH_CAP];
    let path = if unsafe {
        mori_hook_fs_resolve(input.as_ptr(), resolved.as_mut_ptr(), PATH_CAP)
    } {
        unsafe { CStr::from_ptr(resolved.as_ptr()) }.to_bytes()
    } else {
        path
    };
    if path.len() >= PATH_CAP {
        return None;
    }
    let mut path = slash_normalized(path);
    while path.len() > 1
        && path.ends_with(b"/")
        && !(path.len() == 3 && path[1] == b':')
    {
        path.pop();
    }
    absolute(&path).then_some(path)
}

fn cwd(root: Value<'_>, server: *mut HookServer) -> Option<Vec<u8>> {
    let supplied = root.field(c"cwd").or_else(|| {
        let roots = root.get(c"workspace_roots");
        let roots = if roots.missing() {
            root.get(c"workspaceRoots")
        } else {
            roots
        };
        roots.at(0).string()
    });
    if let Some(path) = supplied {
        let normalized = slash_normalized(&path[..path.len().min(PATH_CAP - 1)]);
        if absolute(&normalized) {
            return Some(normalized);
        }
    }
    if !server.is_null() {
        let session = unsafe { c_bytes(cbm_mcp_server_session_root(server)) };
        if let Some(path) = session.filter(|path| path.len() < PATH_CAP) {
            let path = slash_normalized(path);
            if absolute(&path) {
                return Some(path);
            }
        }
    }
    let mut buffer = [0; PATH_CAP];
    if unsafe { mori_hook_getcwd(buffer.as_mut_ptr(), PATH_CAP) } {
        let path = unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_bytes();
        return absolute(path).then(|| path.to_vec());
    }
    None
}

pub(super) fn tool_path(
    root: Value<'_>,
    input: Value<'_>,
    cap: usize,
) -> Option<Vec<u8>> {
    let source = input.field(c"file_path").or_else(|| input.field(c"path"))?;
    if source.is_empty() || source.len() >= cap {
        return None;
    }
    let path = slash_normalized(source);
    if absolute(&path) {
        return Some(path);
    }
    if root.field(c"cwd").is_some_and(|cwd| {
        cwd.len() >= PATH_CAP || !absolute(&slash_normalized(cwd))
    }) {
        return None;
    }
    let mut directory = cwd(root, ptr::null_mut())?;
    directory.push(b'/');
    directory.extend(path);
    (directory.len() < cap && absolute(&directory)).then_some(directory)
}

fn query(server: *mut HookServer, tool: &CStr, args: String) -> Option<CBytes> {
    let args = CString::new(args).ok()?;
    let reply =
        unsafe { cbm_mcp_handle_tool(server, tool.as_ptr(), args.as_ptr()) };
    unsafe { CBytes::new(reply) }
}

struct Project {
    name: Vec<u8>,
    root: Option<Vec<u8>>,
}

fn registry_project(server: *mut HookServer, cwd: &[u8]) -> Option<Project> {
    let cwd = canonical(cwd)?;
    let mut best: Option<Project> = None;
    let mut best_length = 0;
    let mut offset = 0;
    loop {
        let args = format!(
            r#"{{"metadata_only":true,"format":"json","limit":500,"offset":{offset}}}"#
        );
        let envelope = query(server, c"list_projects", args)?;
        let doc = envelope_payload(envelope.bytes()).ok()??;
        let projects = doc.root().get(c"projects");
        if !projects.is_array() {
            return None;
        }
        for project in projects.elements() {
            let Some(name) =
                project.field(c"name").filter(|name| !name.is_empty())
            else {
                continue;
            };
            let Some(root) = project.field(c"root_path").and_then(canonical)
            else {
                continue;
            };
            if root.len() > best_length && contains_path(&root, &cwd) {
                best_length = root.len();
                best = Some(Project {
                    name: name.to_vec(),
                    root: Some(root),
                });
            }
        }
        if !doc.root().get(c"has_more").boolean()? {
            return best;
        }
        // The reference reads next_offset through yyjson_get_int (32 bits).
        let next = doc.root().get(c"next_offset").integer()? as i32;
        if next <= offset {
            return None;
        }
        offset = next;
    }
}

fn resolve_project(
    server: *mut HookServer,
    cwd: &[u8],
    need_root: bool,
) -> Option<Project> {
    if server.is_null() || !absolute(cwd) {
        return None;
    }
    let mut dir = cwd;
    for _ in 0..MAX_WALKUP {
        if !absolute(dir) {
            break;
        }
        let path = CString::new(dir).ok()?;
        let raw_name = unsafe { cbm_project_name_from_path(path.as_ptr()) };
        if let Some(name) = unsafe { CBytes::new(raw_name) } {
            let result = quote(name.bytes()).and_then(|name| {
                query(
                    server,
                    c"index_status",
                    format!(r#"{{"project":{name},"format":"json"}}"#),
                )
            });
            let parsed = result
                .as_ref()
                .and_then(|envelope| Doc::read(envelope.bytes()));
            if parsed.is_some_and(|doc| {
                doc.root().get(c"isError").boolean() != Some(true)
            }) {
                let root = if need_root {
                    Some(canonical(dir)?)
                } else {
                    None
                };
                return Some(Project {
                    name: name.bytes().to_vec(),
                    root,
                });
            }
        }
        let Some(next) = parent(dir) else {
            break;
        };
        dir = next;
    }
    registry_project(server, cwd)
}

fn search(server: *mut HookServer, cwd: &[u8], token: &[u8]) -> Option<Vec<u8>> {
    let project = resolve_project(server, cwd, false)?;
    let name = quote(&project.name)?;
    let token_text = std::str::from_utf8(token).ok()?;
    let args = format!(
        r#"{{"project":{name},"name_pattern":".*{token_text}.*","limit":5,"format":"json"}}"#
    );
    let envelope = query(server, c"search_graph", args)?;
    search_context(envelope.bytes(), token).ok()?
}

fn coverage(server: *mut HookServer, path: &[u8]) -> Option<Vec<u8>> {
    let project = resolve_project(server, parent(path)?, true)?;
    let root = project.root?;
    let file = canonical(path)?;
    if !contains_path(&root, &file) {
        return None;
    }
    let relative = &file[root.len()..];
    let relative = relative.strip_prefix(b"/").unwrap_or(relative);
    if relative.is_empty() {
        return None;
    }
    let name = quote(&project.name)?;
    let relative_json = quote(relative)?;
    let args = format!(
        r#"{{"project":{name},"paths":[{relative_json}],"format":"json"}}"#
    );
    let envelope = query(server, c"check_index_coverage", args)?;
    coverage_context(envelope.bytes(), relative)
}

thread_local! {
    pub(super) static WORKTREE_COUNTER: Cell<*mut c_int> = const { Cell::new(ptr::null_mut()) };
}

fn lifecycle_project(
    server: *mut HookServer,
    cwd: &[u8],
) -> (Option<Project>, bool) {
    if !unsafe { cbm_mcp_ignore_worktrees_runtime_enabled(server) } {
        return (resolve_project(server, cwd, false), false);
    }
    WORKTREE_COUNTER.with(|counter| {
        if let Some(counter) = unsafe { counter.get().as_mut() } {
            *counter = counter.wrapping_add(1);
        }
    });
    let canonical = canonical(cwd);
    let cwd = canonical.as_deref().unwrap_or(cwd);
    let project = resolve_project(server, cwd, true);
    let Ok(path) = CString::new(cwd) else {
        return (project, false);
    };
    if !unsafe { cbm_git_is_linked_worktree(path.as_ptr()) } {
        return (project, false);
    }
    let shared_worktree = project
        .as_ref()
        .and_then(|project| project.root.as_ref())
        .and_then(|root| CString::new(root.as_slice()).ok())
        .is_some_and(|root| unsafe {
            cbm_git_same_linked_worktree(path.as_ptr(), root.as_ptr())
        });
    if shared_worktree {
        (project, false)
    } else {
        (None, true)
    }
}

pub(super) fn lifecycle(
    server: *mut HookServer,
    root: Value<'_>,
    forced: Option<&[u8]>,
    dialect: Dialect,
) -> Option<Vec<u8>> {
    if !root.is_object() {
        return None;
    }
    let event = forced.or_else(|| event_name(root))?;
    if !dialect.lifecycle_event(event) {
        return None;
    }
    let owned = server.is_null().then(OwnedServer::new);
    let server = owned.as_ref().map_or(server, OwnedServer::get);
    let (project, ignored) = if !server.is_null() {
        cwd(root, server)
            .map_or((None, false), |cwd| lifecycle_project(server, &cwd))
    } else {
        (None, false)
    };
    drop(owned);
    let name = project.as_ref().map(|project| project.name.as_slice());
    let text = lifecycle_context(root, event, name, ignored);
    output(dialect, event, text.as_bytes())
}

struct DriftEntry {
    session: Vec<u8>,
    searches: u32,
    threshold: u32,
}

// Entries are ordered least-recently used first. The table belongs to the
// process serving hook requests and is shared by concurrent daemon sessions.
static DRIFT: Mutex<Vec<DriftEntry>> = Mutex::new(Vec::new());

fn drift_session(root: Value<'_>, dialect: Dialect) -> Option<&[u8]> {
    if dialect != Dialect::Event || !root.get(c"turn_id").missing() {
        return None;
    }
    root.field(c"session_id").filter(|session| {
        (1..=128).contains(&session.len())
            && session.iter().all(|&byte| {
                byte.is_ascii_alphanumeric() || b"-_.".contains(&byte)
            })
    })
}

fn with_drift<T>(f: impl FnOnce(&mut Vec<DriftEntry>) -> T) -> T {
    let mut entries = DRIFT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    f(&mut entries)
}

fn drift_reset(session: &[u8]) {
    with_drift(|entries| entries.retain(|entry| entry.session != session));
}

fn drift_search(session: &[u8], augmented: bool) -> Option<u32> {
    with_drift(|entries| {
        let mut entry = if let Some(index) =
            entries.iter().position(|entry| entry.session == session)
        {
            entries.remove(index)
        } else {
            if entries.len() == 64 {
                entries.remove(0);
            }
            DriftEntry {
                session: session.to_vec(),
                searches: 0,
                threshold: 3,
            }
        };
        entry.searches = entry.searches.saturating_add(1);
        let due = augmented && entry.searches >= entry.threshold;
        if due {
            while entry.threshold <= entry.searches
                && entry.threshold <= u32::MAX / 2
            {
                entry.threshold *= 2;
            }
        }
        let searches = entry.searches;
        entries.push(entry);
        due.then_some(searches)
    })
}

pub(super) fn process(
    server: *mut HookServer,
    input: &[u8],
    forced: Option<&[u8]>,
    dialect: Dialect,
) -> Option<Vec<u8>> {
    if server.is_null() || !dialect.invocation(forced) {
        return None;
    }
    let doc = super::read_input(input)?;
    let root = doc.root();
    if let Some(output) = lifecycle(server, root, forced, dialect) {
        return Some(output);
    }
    let call = ToolCall::read(root, forced, dialect);
    let event = call.event?;
    let tool = call.tool?;
    let session = drift_session(root, dialect);
    if let Some(session) =
        session.filter(|_| event == b"PostToolUse" && is_graph_tool(tool))
    {
        drift_reset(session);
        return None;
    }
    match tool_event(dialect, event, tool)? {
        ToolEvent::Coverage => {
            let path = tool_path(root, call.input, PATH_CAP)?;
            output(Dialect::Event, event, &coverage(server, &path)?)
        }
        ToolEvent::Search => {
            let token = call.search_token()?;
            let cwd = cwd(root, server)?;
            let context = search(server, &cwd, &token);
            let reminder = session
                .and_then(|session| drift_search(session, context.is_some()));
            let mut context = context?;
            if let Some(searches) = reminder {
                let line = format!(
                    "\n[mori] This session has run {searches} searches since its last mori-graph tool call. Questions about callers, callees, call paths and definitions are one graph call each."
                );
                if context.len() + line.len() < 5120 {
                    context.extend_from_slice(line.as_bytes());
                }
            }
            output(dialect, event, &context)
        }
    }
}
