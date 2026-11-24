//! Graph linking for the shell, Make and CI facts collected by the native parser.

use super::native_graph_buffer::{
    Node, cbm_gbuf_find_by_label, cbm_gbuf_find_by_name,
};
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::{ptr, slice};

type NodeId = i64;

// Mirrors sh_hit_t in pass_shell.c. Strings belong to the collector.
#[repr(C)]
struct Hit {
    file: *const c_char,
    line: c_int,
    via: *const c_char,
    target: NodeId,
    external: *const c_char,
    path: *const c_char,
    ci: *const c_char,
    job: *const c_char,
    chart: bool,
    spawn: bool,
    make: bool,
    build: bool,
    http: bool,
    build_args: *const c_char,
    default_of: *const c_char,
}

type Publish = unsafe extern "C" fn(
    *mut c_void,
    *mut c_void,
    *mut c_void,
    *const Hit,
    c_int,
    *const c_void,
) -> c_int;
type MakeFiles = unsafe extern "C" fn(*const *const c_char, c_int, *mut c_void);
type FindNodes = unsafe extern "C" fn(
    *const c_void,
    *const c_char,
    *mut *const *const Node,
    *mut c_int,
) -> c_int;

unsafe extern "C" {
    fn cbm_shell_collect(
        ctx: *mut c_void,
        files: *const c_void,
        count: c_int,
        publish: Publish,
    );
    fn cbm_shell_make_files(
        collector: *mut c_void,
        file: *const c_char,
        emit: MakeFiles,
        data: *mut c_void,
    );
    fn cbm_shell_count_make_candidates(count: u64);
    fn cbm_shell_file_id(ctx: *mut c_void, file: *const c_char) -> NodeId;
    fn cbm_pipeline_image_node(ctx: *mut c_void, image: *const c_char)
    -> NodeId;
    fn cbm_pipeline_external_chart(
        ctx: *mut c_void,
        name: *const c_char,
        url: *const c_char,
        file: *const c_char,
    ) -> NodeId;
    fn cbm_gbuf_upsert_node(
        graph: *mut c_void,
        label: *const c_char,
        name: *const c_char,
        qn: *const c_char,
        file: *const c_char,
        start: c_int,
        end: c_int,
        props: *const c_char,
    ) -> NodeId;
    fn cbm_gbuf_insert_edge(
        graph: *mut c_void,
        source: NodeId,
        target: NodeId,
        kind: *const c_char,
        props: *const c_char,
    ) -> i64;
    fn cbm_gbuf_node_set_properties_json(
        node: *mut Node,
        props: *const c_char,
    ) -> c_int;
    fn cbm_ht_count(table: *const c_void) -> u32;
    fn cbm_ht_get(table: *const c_void, key: *const c_char) -> *mut c_void;
    fn cbm_json_escape(
        out: *mut c_char,
        size: c_int,
        text: *const c_char,
    ) -> c_int;
    fn cbm_route_canon_path(
        input: *const c_char,
        out: *mut c_char,
        size: usize,
    ) -> *const c_char;
}

// SAFETY: the caller keeps the native owner of ptr alive for the returned borrow.
unsafe fn text<'a>(ptr: *const c_char) -> &'a CStr {
    if ptr.is_null() {
        c""
    } else {
        unsafe { CStr::from_ptr(ptr) }
    }
}

fn c_string(bytes: Vec<u8>) -> CString {
    CString::new(bytes)
        .expect("shell strings come from NUL-terminated native inputs")
}

fn bounded(mut bytes: Vec<u8>, capacity: usize) -> CString {
    // Match snprintf, including its byte limit, for stored identities and JSON.
    bytes.truncate(capacity - 1);
    c_string(bytes)
}

fn escape<const N: usize>(value: &CStr) -> Vec<u8> {
    let mut buf = [0u8; N];
    let len = unsafe {
        cbm_json_escape(buf.as_mut_ptr().cast(), N as c_int, value.as_ptr())
    };
    buf[..len as usize].to_vec()
}

struct Graph {
    ctx: *mut c_void,
    buffer: *mut c_void,
}

impl Graph {
    // Consume the native array before any call that writes to the graph.
    fn nodes(&self, find: FindNodes, key: &CStr) -> &[*const Node] {
        let mut nodes = ptr::null();
        let mut count = 0;
        let rc =
            unsafe { find(self.buffer, key.as_ptr(), &mut nodes, &mut count) };
        if rc == 0 && count > 0 {
            unsafe { slice::from_raw_parts(nodes, count as usize) }
        } else {
            &[]
        }
    }

    fn label(&self, label: &CStr) -> &[*const Node] {
        self.nodes(cbm_gbuf_find_by_label, label)
    }

    fn file_id(&self, file: &CStr) -> NodeId {
        unsafe { cbm_shell_file_id(self.ctx, file.as_ptr()) }
    }

    fn node(
        &mut self,
        label: &CStr,
        name: &CStr,
        qn: &CStr,
        props: &CStr,
    ) -> NodeId {
        unsafe {
            cbm_gbuf_upsert_node(
                self.buffer,
                label.as_ptr(),
                name.as_ptr(),
                qn.as_ptr(),
                c"".as_ptr(),
                0,
                0,
                props.as_ptr(),
            )
        }
    }

    fn edge(
        &mut self,
        src: NodeId,
        target: NodeId,
        kind: &CStr,
        props: &CStr,
    ) -> c_int {
        // The pass counter records attempted inserts, including graph deduplication.
        unsafe {
            cbm_gbuf_insert_edge(
                self.buffer,
                src,
                target,
                kind.as_ptr(),
                props.as_ptr(),
            )
        };
        1
    }
}

impl Hit {
    fn file(&self) -> &CStr {
        unsafe { text(self.file) }
    }

    fn path(&self) -> &CStr {
        unsafe { text(self.path) }
    }

    fn external(&self) -> &CStr {
        unsafe { text(self.external) }
    }

    fn via(&self) -> &[u8] {
        unsafe { text(self.via) }.to_bytes()
    }

    fn ci_props(&self) -> Vec<u8> {
        if self.ci.is_null() {
            return Vec::new();
        }
        let ci = unsafe { text(self.ci) }.to_bytes();
        let mut props = [b",\"ci\":\"" as &[u8], ci, b"\""].concat();
        if !self.job.is_null() {
            let job = escape::<512>(unsafe { text(self.job) });
            props.extend_from_slice(
                &[b",\"job\":\"" as &[u8], &job, b"\""].concat(),
            );
        }
        props.truncate(1023);
        props
    }
}

struct Span {
    id: NodeId,
    first: c_int,
    last: c_int,
    qn: Vec<u8>,
}

#[derive(Default)]
struct Sources {
    modules: HashMap<Vec<u8>, NodeId>,
    spans: HashMap<Vec<u8>, Vec<Span>>,
}

impl Sources {
    fn collect(graph: &Graph, hits: &[Hit], targets: &[NodeId]) -> Self {
        let mut files = HashSet::new();
        for (hit, target) in hits.iter().zip(targets) {
            files.insert(hit.file().to_bytes());
            if (hit.spawn && *target > 0) || hit.build {
                files.insert(hit.path().to_bytes());
            }
        }
        let mut sources = Self::default();
        for &ptr in graph.label(c"Function") {
            let node = unsafe { &*ptr };
            if node.file_path.is_null() {
                continue;
            }
            let file = unsafe { text(node.file_path) }.to_bytes();
            if files.contains(file) {
                sources.spans.entry(file.to_vec()).or_default().push(Span {
                    id: node.id,
                    first: node.start_line,
                    last: node.end_line,
                    qn: unsafe { text(node.qualified_name) }.to_bytes().to_vec(),
                });
            }
        }
        // Make targets that share a recipe span emit in qualified-name order.
        for spans in sources.spans.values_mut() {
            spans.sort_by(|a, b| a.qn.cmp(&b.qn));
        }
        for &ptr in graph.label(c"Module") {
            let node = unsafe { &*ptr };
            if node.file_path.is_null() {
                continue;
            }
            let file = unsafe { text(node.file_path) }.to_bytes();
            if files.contains(file) {
                sources.modules.entry(file.to_vec()).or_insert(node.id);
            }
        }
        sources
    }

    fn module_id(&self, file: &CStr) -> Option<NodeId> {
        self.modules
            .get(file.to_bytes())
            .copied()
            .filter(|id| *id > 0)
    }

    fn at(
        &self,
        graph: &Graph,
        file: &CStr,
        line: c_int,
        out: &mut Vec<NodeId>,
    ) {
        out.clear();
        if let Some(spans) = self.spans.get(file.to_bytes()) {
            let best = spans
                .iter()
                .filter(|span| span.first <= line && line <= span.last)
                .min_by_key(|span| span.last - span.first);
            if let Some(best) = best {
                out.extend(
                    spans
                        .iter()
                        .filter(|span| {
                            (span.first, span.last) == (best.first, best.last)
                        })
                        .map(|span| span.id),
                );
                return;
            }
        }
        out.push(self.module_id(file).unwrap_or_else(|| graph.file_id(file)));
    }
}

unsafe extern "C" fn copy_make_files(
    paths: *const *const c_char,
    count: c_int,
    data: *mut c_void,
) {
    if count > 0 {
        let files = unsafe { &mut *data.cast::<Vec<Vec<u8>>>() };
        for &path in unsafe { slice::from_raw_parts(paths, count as usize) } {
            files.push(unsafe { text(path) }.to_bytes().to_vec());
        }
    }
}

fn make_targets(
    graph: &Graph,
    collector: *mut c_void,
    hits: &[Hit],
) -> Vec<NodeId> {
    let mut closures = HashMap::<&[u8], Vec<Vec<u8>>>::new();
    for hit in hits.iter().filter(|hit| hit.make) {
        let path = hit.path();
        closures.entry(path.to_bytes()).or_insert_with(|| {
            let mut files = Vec::<Vec<u8>>::new();
            unsafe {
                cbm_shell_make_files(
                    collector,
                    path.as_ptr(),
                    copy_make_files,
                    (&mut files as *mut Vec<Vec<u8>>).cast(),
                );
            }
            files
        });
    }
    let files: HashSet<&[u8]> =
        closures.values().flatten().map(Vec::as_slice).collect();
    let mut rules = HashMap::new();
    if !files.is_empty() {
        for &ptr in graph.label(c"Function") {
            let node = unsafe { &*ptr };
            if node.file_path.is_null() || node.name.is_null() {
                continue;
            }
            let file = unsafe { text(node.file_path) }.to_bytes();
            if files.contains(file) {
                let name = unsafe { text(node.name) }.to_bytes();
                let key = [file, b"\x1f", name].concat();
                let rule =
                    rules.entry(key).or_insert((node.start_line, node.id));
                if node.start_line < rule.0 {
                    *rule = (node.start_line, node.id);
                }
            }
        }
    }
    let mut candidates = 0;
    let targets = hits
        .iter()
        .map(|hit| {
            if !hit.make || hit.target > 0 {
                return hit.target;
            }
            let name = hit.external().to_bytes();
            for file in &closures[hit.path().to_bytes()] {
                candidates += 1;
                if let Some(&(_, target)) =
                    rules.get(&[file.as_slice(), b"\x1f", name].concat())
                {
                    if target > 0 {
                        return target;
                    }
                }
            }
            0
        })
        .collect();
    if candidates > 0 {
        unsafe { cbm_shell_count_make_candidates(candidates) };
    }
    targets
}

fn spawn(
    graph: &mut Graph,
    sources: &Sources,
    hit: &Hit,
    src: NodeId,
    target: NodeId,
) -> c_int {
    if src <= 0 {
        return 0;
    }
    let program = escape::<128>(if hit.make { c"make" } else { hit.external() });
    let (target, path_props) = if hit.make {
        let path = escape::<1024>(hit.path());
        let name = escape::<512>(hit.external());
        (
            target,
            [
                b",\"path\":\"" as &[u8],
                &path,
                b"\",\"target\":\"",
                &name,
                b"\"",
            ]
            .concat(),
        )
    } else if target > 0 {
        let path = escape::<1024>(hit.path());
        let target = sources.module_id(hit.path()).unwrap_or(target);
        (target, [b",\"path\":\"" as &[u8], &path, b"\""].concat())
    } else {
        let qn = bounded(
            [b"__process__" as &[u8], hit.external().to_bytes()].concat(),
            256,
        );
        let props =
            c_string([b"{\"program\":\"" as &[u8], &program, b"\"}"].concat());
        (
            graph.node(c"Process", hit.external(), &qn, &props),
            Vec::new(),
        )
    };
    if target <= 0 || target == src {
        return 0;
    }
    let props = bounded(
        [
            b"{\"api\":\"" as &[u8],
            hit.via(),
            b"\",\"program\":\"",
            &program,
            b"\"",
            &path_props,
            &hit.ci_props(),
            b",\"line\":",
            hit.line.to_string().as_bytes(),
            b",\"confidence\":1.00}",
        ]
        .concat(),
        4096,
    );
    graph.edge(src, target, c"SPAWNS", &props)
}

fn build(graph: &mut Graph, sources: &Sources, hit: &Hit) -> c_int {
    let Some(module) = sources.module_id(hit.path()) else {
        return 0;
    };
    let image =
        unsafe { cbm_pipeline_image_node(graph.ctx, hit.external().as_ptr()) };
    if image <= 0 {
        return 0;
    }
    let mut props = [
        b"{\"via\":\"build\",\"tool\":\"docker-build\",\"file\":\"" as &[u8],
        &escape::<1024>(hit.file()),
        b"\",\"image\":\"",
        &escape::<1024>(hit.external()),
        b"\"",
        &hit.ci_props(),
        b",\"line\":",
        hit.line.to_string().as_bytes(),
    ]
    .concat();
    for (key, value) in [
        (b",\"default_of\":" as &[u8], hit.default_of),
        (b",\"build_args\":", hit.build_args),
    ] {
        if !value.is_null() {
            props.extend_from_slice(key);
            props.extend_from_slice(unsafe { text(value) }.to_bytes());
        }
    }
    props.push(b'}');
    graph.edge(image, module, c"DEPENDS_ON", &c_string(props))
}

fn http_service(graph: &Graph, host: &[u8]) -> bool {
    if host == b"localhost" {
        return false;
    }
    let mut labels = host.split(|&c| c == b'.');
    let name = labels.next().unwrap_or_default();
    if labels.next().is_some() && labels.next() != Some(b"svc") {
        return false;
    }
    let name = bounded([b"Service/" as &[u8], name].concat(), 256);
    graph
        .nodes(cbm_gbuf_find_by_name, &name)
        .iter()
        .any(|&ptr| {
            let node = unsafe { &*ptr };
            (unsafe { text(node.label) }) == c"Resource"
                && !node.qualified_name.is_null()
                && !unsafe { text(node.qualified_name) }
                    .to_bytes()
                    .windows(b"::compose/".len())
                    .any(|part| part == b"::compose/")
        })
}

fn http(graph: &mut Graph, hit: &Hit, src: NodeId) -> c_int {
    let url = hit.external().to_bytes();
    let Some(prefix) = url.windows(3).position(|part| part == b"://") else {
        return 0;
    };
    if src <= 0 {
        return 0;
    }
    let auth_and_path = &url[prefix + 3..];
    let auth_len = auth_and_path
        .iter()
        .position(|&c| c == b'/')
        .unwrap_or(auth_and_path.len());
    let authority = &auth_and_path[..auth_len];
    let host_len = if authority.starts_with(b"[") {
        authority
            .iter()
            .position(|&c| c == b']')
            .map_or(auth_len + 1, |at| at + 1)
    } else {
        authority
            .iter()
            .position(|&c| c == b':')
            .unwrap_or(auth_len)
    };
    if host_len == 0 || host_len > auth_len || auth_len >= 256 {
        return 0;
    }
    let host = authority[..host_len].to_ascii_lowercase();
    let local = http_service(graph, &host);
    let path = c_string(auth_and_path[auth_len..].to_vec());
    let mut buf = [0u8; 2048];
    unsafe {
        cbm_route_canon_path(path.as_ptr(), buf.as_mut_ptr().cast(), buf.len())
    };
    let canonical = unsafe { text(buf.as_ptr().cast()) };
    let method = hit.path().to_bytes();
    let scope = if local {
        &[]
    } else {
        &url[..prefix + 3 + auth_len]
    };
    let qn = bounded(
        [
            b"__route__" as &[u8],
            method,
            b"__",
            scope,
            canonical.to_bytes(),
        ]
        .concat(),
        768,
    );
    let authority = c_string(authority.to_ascii_lowercase());
    let host_prop = escape::<512>(&authority);
    let mut node_props = [b"{\"method\":\"" as &[u8], method, b"\""].concat();
    if !local {
        node_props.extend_from_slice(
            &[
                b",\"host\":\"" as &[u8],
                &host_prop,
                b"\",\"external\":true",
            ]
            .concat(),
        );
    }
    node_props.push(b'}');
    let route = graph.node(
        c"Route",
        if local { &path } else { hit.external() },
        &qn,
        &bounded(node_props, 1024),
    );
    if route <= 0 || route == src {
        return 0;
    }
    let props = bounded(
        [
            b"{\"via\":\"" as &[u8],
            hit.via(),
            b"\",\"method\":\"",
            method,
            b"\",\"url_path\":\"",
            &escape::<4096>(&path),
            b"\",\"host\":\"",
            &host_prop,
            b"\"",
            &hit.ci_props(),
            b",\"line\":",
            hit.line.to_string().as_bytes(),
            b"}",
        ]
        .concat(),
        5120,
    );
    graph.edge(src, route, c"HTTP_CALLS", &props)
}

fn deploy(
    graph: &mut Graph,
    hit: &Hit,
    src: NodeId,
    mut target: NodeId,
) -> c_int {
    let external = target <= 0;
    if external {
        target = unsafe {
            cbm_pipeline_external_chart(
                graph.ctx,
                hit.external,
                ptr::null(),
                hit.file,
            )
        };
    }
    if src <= 0 || target <= 0 || src == target {
        return 0;
    }
    let props = bounded(
        [
            b"{\"via\":\"" as &[u8],
            hit.via(),
            if hit.chart {
                b"\",\"chart\":\""
            } else {
                b"\",\"path\":\""
            },
            &escape::<1024>(hit.path()),
            b"\"",
            if external { b",\"external\":true" } else { b"" },
            &hit.ci_props(),
            b",\"line\":",
            hit.line.to_string().as_bytes(),
            b"}",
        ]
        .concat(),
        4096,
    );
    graph.edge(src, target, c"DEPLOYS", &props)
}

fn mark_declarers(graph: &mut Graph, declarers: *const c_void) {
    if declarers.is_null() || unsafe { cbm_ht_count(declarers) } == 0 {
        return;
    }
    const MARK: &[u8] = b"\"declares_build\":true";
    for &ptr in graph.label(c"File") {
        let node = unsafe { &*ptr };
        if node.file_path.is_null()
            || unsafe { cbm_ht_get(declarers, node.file_path) }.is_null()
        {
            continue;
        }
        let props = if node.properties_json.is_null() {
            c"{}"
        } else {
            unsafe { text(node.properties_json) }
        }
        .to_bytes();
        if !props.ends_with(b"}")
            || props.len() + MARK.len() + 3 > 4096
            || props.windows(MARK.len()).any(|part| part == MARK)
        {
            continue;
        }
        let props = if props == b"{}" || props.len() < 2 {
            [b"{" as &[u8], MARK, b"}"].concat()
        } else {
            [&props[..props.len() - 1], b",", MARK, b"}"].concat()
        };
        // The setter replaces only this node's property string.
        unsafe {
            cbm_gbuf_node_set_properties_json(
                ptr.cast_mut(),
                c_string(props).as_ptr(),
            )
        };
    }
}

unsafe extern "C" fn publish(
    ctx: *mut c_void,
    buffer: *mut c_void,
    collector: *mut c_void,
    hits: *const Hit,
    count: c_int,
    declarers: *const c_void,
) -> c_int {
    let mut graph = Graph { ctx, buffer };
    let mut edges = 0;
    if count > 0 {
        // All hit strings and the Make parser live until this callback returns.
        let hits = unsafe { slice::from_raw_parts(hits, count as usize) };
        let targets = make_targets(&graph, collector, hits);
        let sources = Sources::collect(&graph, hits, &targets);
        let mut source_ids = Vec::new();
        for (hit, target) in hits.iter().zip(targets) {
            if hit.build {
                edges += build(&mut graph, &sources, hit);
                continue;
            }
            sources.at(&graph, hit.file(), hit.line, &mut source_ids);
            for &src in &source_ids {
                edges += if hit.spawn {
                    spawn(&mut graph, &sources, hit, src, target)
                } else if hit.http {
                    http(&mut graph, hit, src)
                } else {
                    deploy(&mut graph, hit, src, target)
                };
            }
        }
    }
    mark_declarers(&mut graph, declarers);
    edges
}

/// Run the normal shell indexing pass against the caller's native pipeline context.
///
/// # Safety
/// The C pipeline supplies its live context and an array of `count` file records.
#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_pipeline_pass_shell(
    ctx: *mut c_void,
    files: *const c_void,
    count: c_int,
) {
    unsafe { cbm_shell_collect(ctx, files, count, publish) };
}
