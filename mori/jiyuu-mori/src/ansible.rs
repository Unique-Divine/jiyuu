//! Index Ansible composition, tasks, handlers and manifest deployments.

mod deploy;
mod yaml;

use crate::native_graph_buffer::{Node as GraphNode, cbm_gbuf_find_by_label};
use deploy::Deployments;
use std::collections::HashMap;
use std::ffi::{CStr, CString, OsStr, c_char, c_int, c_long, c_void};
use std::fs::File;
use std::io::Read;
use std::{ptr, slice};
use yaml::{Document, Node, Parser, SCANNER_PADDING, starts_sequence};

const PATH_SIZE: usize = 1024;

#[repr(C)]
struct FileInfo {
    path: *const c_char,
    relative: *const c_char,
    language: c_int,
    size: i64,
    mtime_ns: i64,
}

#[repr(C)]
#[derive(Default)]
struct Stats {
    imports: c_int,
    tasks: c_int,
    notifies: c_int,
    deploys: c_int,
}

unsafe extern "C" {
    fn cbm_max_file_bytes() -> c_long;
    fn cbm_repo_rel_join(
        dir: *const c_char,
        value: *const c_char,
        output: *mut c_char,
        size: usize,
    ) -> bool;
    fn cbm_json_escape(
        output: *mut c_char,
        size: c_int,
        input: *const c_char,
    ) -> c_int;
    fn cbm_gbuf_upsert_node(
        graph: *mut c_void,
        label: *const c_char,
        name: *const c_char,
        qualified_name: *const c_char,
        file_path: *const c_char,
        start_line: c_int,
        end_line: c_int,
        properties: *const c_char,
    ) -> i64;
    fn cbm_gbuf_insert_edge(
        graph: *mut c_void,
        source: i64,
        target: i64,
        kind: *const c_char,
        properties: *const c_char,
    ) -> i64;
}

struct Run {
    ctx: *mut c_void,
    buffer: *mut c_void,
    check_cancel: unsafe extern "C" fn(*mut c_void) -> bool,
}

impl Run {
    fn cancelled(&self) -> bool {
        unsafe { (self.check_cancel)(self.ctx) }
    }

    fn edge(&self, source: i64, target: i64, kind: &CStr, props: &CStr) {
        // The graph copies the property string during the call.
        unsafe {
            cbm_gbuf_insert_edge(
                self.buffer,
                source,
                target,
                kind.as_ptr(),
                props.as_ptr(),
            )
        };
    }

    fn index<T>(
        &self,
        label: &CStr,
        value: impl Fn(&GraphNode) -> T,
    ) -> HashMap<Vec<u8>, T> {
        let mut nodes = ptr::null();
        let mut count = 0;
        let status = unsafe {
            cbm_gbuf_find_by_label(
                self.buffer,
                label.as_ptr(),
                &mut nodes,
                &mut count,
            )
        };
        if status != 0 || count <= 0 || nodes.is_null() {
            return HashMap::new();
        }
        // Copy the data before mutations can invalidate the query's pointers.
        unsafe { slice::from_raw_parts(nodes, count as usize) }
            .iter()
            .map(|&node| unsafe { &*node })
            .filter(|node| !node.file_path.is_null())
            .map(|node| {
                (
                    unsafe { CStr::from_ptr(node.file_path) }
                        .to_bytes()
                        .to_vec(),
                    value(node),
                )
            })
            .collect()
    }
}

struct FileNode {
    id: i64,
    qualified_name: Vec<u8>,
}

struct Source<'run> {
    absolute: &'run CStr,
    relative: &'run CStr,
}

fn read(path: &CStr) -> Option<Vec<u8>> {
    #[cfg(unix)]
    let path = {
        use std::os::unix::ffi::OsStrExt;
        OsStr::from_bytes(path.to_bytes())
    };
    #[cfg(not(unix))]
    let path = OsStr::new(path.to_str().ok()?);

    let file = File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    let cap = (unsafe { cbm_max_file_bytes() } as u64).min(u32::MAX.into());
    if size == 0 || size > cap {
        return None;
    }
    read_source(file, size)
}

fn read_source(file: impl Read, size: u64) -> Option<Vec<u8>> {
    let capacity = usize::try_from(size).ok()?.checked_add(SCANNER_PADDING)?;
    let mut source = Vec::new();
    source.try_reserve_exact(capacity).ok()?;
    file.take(size).read_to_end(&mut source).ok()?;
    Some(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_read_reserves_scanner_padding_without_changing_the_input() {
        let input = b"- name: fixture\ntrailing bytes";
        let size = b"- name: fixture\n".len();
        let mut source = read_source(input.as_slice(), size as u64).unwrap();
        assert_eq!(source, input[..size]);
        assert!(source.capacity() - source.len() >= SCANNER_PADDING);

        let before_padding = source.as_ptr();
        source.extend_from_slice(&[0; SCANNER_PADDING]);
        assert_eq!(source.as_ptr(), before_padding);
        assert_eq!(&source[size..], &[0; SCANNER_PADDING]);
        assert!(read_source(input.as_slice(), u64::MAX).is_none());
    }
}

fn parse_candidate(
    parser: &mut Parser,
    source: &Source<'_>,
) -> Option<Document> {
    let path = source.relative.to_bytes();
    if !path.ends_with(b".yml") && !path.ends_with(b".yaml") {
        return None;
    }
    let text = read(source.absolute)?;
    starts_sequence(&text).then(|| parser.parse(text)).flatten()
}

fn join(dir: &[u8], value: &[u8]) -> Option<Vec<u8>> {
    let dir = CString::new(dir).ok()?;
    let value = CString::new(value).ok()?;
    let mut output = [0; PATH_SIZE];
    let found = unsafe {
        cbm_repo_rel_join(
            dir.as_ptr(),
            value.as_ptr(),
            output.as_mut_ptr(),
            output.len(),
        )
    };
    found.then(|| {
        unsafe { CStr::from_ptr(output.as_ptr()) }
            .to_bytes()
            .to_vec()
    })
}

fn escape(value: &[u8], output: &mut [c_char]) -> Vec<u8> {
    let Ok(value) = CString::new(value) else {
        return Vec::new();
    };
    unsafe {
        cbm_json_escape(
            output.as_mut_ptr(),
            output.len() as c_int,
            value.as_ptr(),
        )
    };
    unsafe { CStr::from_ptr(output.as_ptr()) }
        .to_bytes()
        .to_vec()
}

fn truncated(mut bytes: Vec<u8>, size: usize) -> Vec<u8> {
    bytes.truncate(size - 1);
    bytes
}

fn contains(text: &[u8], needle: &[u8]) -> bool {
    text.windows(needle.len()).any(|part| part == needle)
}

fn literal(text: &[u8]) -> bool {
    !contains(text, b"{{")
}

fn parent(path: &[u8]) -> &[u8] {
    path.iter()
        .rposition(|&byte| byte == b'/')
        .map_or(b"", |offset| &path[..offset])
}

struct Location<'a> {
    path: &'a CStr,
    dir: &'a [u8],
    role: &'a [u8],
    task_role: &'a [u8],
}

impl<'a> Location<'a> {
    fn new(path: &'a CStr) -> Self {
        let relative = path.to_bytes();
        let (mut role, mut task_role) = (b"".as_slice(), b"".as_slice());
        for (offset, component) in relative
            .split_inclusive(|&byte| byte == b'/')
            .scan(0, |offset, component| {
                let start = *offset;
                *offset += component.len();
                Some((start, component))
            })
        {
            if component != b"roles/" {
                continue;
            }
            let body = offset + b"roles/".len();
            let rest = &relative[body..];
            if let Some(slash) = rest.iter().position(|&byte| byte == b'/')
                && rest[slash + 1..].contains(&b'/')
            {
                role = &relative[..body + slash];
            }
            // Start after at least one byte of the role name.
            if let Some(rest) = rest.get(1..)
                && let Some(end) = rest
                    .windows(b"/tasks/".len())
                    .position(|w| w == b"/tasks/")
                    .into_iter()
                    .chain(
                        rest.windows(b"/handlers/".len())
                            .position(|w| w == b"/handlers/"),
                    )
                    .min()
            {
                task_role = &relative[..body + 1 + end];
            }
        }
        Self {
            path,
            dir: &parent(relative)[..parent(relative).len().min(PATH_SIZE - 1)],
            role: &role[..role.len().min(PATH_SIZE - 1)],
            task_role: &task_role[..task_role.len().min(PATH_SIZE - 1)],
        }
    }
}

type NodeIds = HashMap<Vec<u8>, i64>;

struct Composition {
    modules: NodeIds,
    folders: NodeIds,
}

impl Composition {
    fn role(&self, directory: &[u8], name: &[u8]) -> Option<i64> {
        if matches!(name.first(), Some(b'.' | b'/')) {
            return self.folders.get(&join(directory, name)?).copied();
        }

        let mut dir = directory;
        loop {
            let prefix = if dir.is_empty() { b"".as_slice() } else { b"/" };
            let path =
                truncated([dir, prefix, b"roles/", name].concat(), PATH_SIZE);
            if let Some(&id) = self.folders.get(&path) {
                return Some(id);
            }
            if dir.is_empty() {
                return None;
            }
            dir = parent(dir);
        }
    }

    fn task_module(
        &self,
        location: &Location<'_>,
        value: &[u8],
    ) -> Option<(Vec<u8>, i64)> {
        let module = |dir: &[u8]| {
            let path = join(dir, value)?;
            let id = *self.modules.get(&path)?;
            Some((path, id))
        };
        module(location.dir).or_else(|| {
            if location.role.is_empty() {
                return None;
            }
            let tasks =
                truncated([location.role, b"/tasks"].concat(), PATH_SIZE);
            module(&tasks)
        })
    }

    fn walk(
        &self,
        run: &Run,
        doc: &Document,
        location: &Location<'_>,
        from: i64,
        imports: &mut c_int,
        includes: &mut Vec<Vec<u8>>,
    ) {
        let mut ancestors = Vec::new();
        let mut next = Some(doc.root());
        let mut edge = |target, directive: &str, line| {
            if let Some(id) = target
                && from > 0
                && id != from
            {
                let props = CString::new(format!(
                    "{{\"local_name\":\"{directive}\",\"via\":\"ansible\",\"line\":{line}}}"
                )).unwrap();
                run.edge(from, id, c"IMPORTS", &props);
                *imports += 1;
            }
        };
        while let Some(node) = next.take() {
            if run.cancelled() {
                return;
            }
            if node.kind() == b"block_mapping"
                && node
                    .parent()
                    .and_then(Node::parent)
                    .is_some_and(|p| p.kind() == b"block_sequence_item")
            {
                for pair in node
                    .children()
                    .filter(|n| n.kind() == b"block_mapping_pair")
                {
                    let Some((key, value)) = doc.key(pair, 64).zip(pair.value())
                    else {
                        continue;
                    };
                    match key.as_slice() {
                        b"import_playbook" => {
                            let target = doc
                                .scalar(value, PATH_SIZE)
                                .filter(|value| literal(value))
                                .and_then(|value| join(location.dir, &value))
                                .and_then(|path| {
                                    self.modules.get(&path).copied()
                                });
                            edge(target, "import_playbook", pair.line());
                        }
                        b"include_tasks" | b"import_tasks" | b"include" => {
                            if let Some((path, id)) = doc
                                .scalar(value, PATH_SIZE)
                                .or_else(|| {
                                    doc.string(value, b"file", PATH_SIZE)
                                })
                                .filter(|value| literal(value))
                                .and_then(|value| {
                                    self.task_module(location, &value)
                                })
                            {
                                edge(
                                    Some(id),
                                    std::str::from_utf8(&key).unwrap(),
                                    pair.line(),
                                );
                                includes.push(path);
                            }
                        }
                        b"include_role" | b"import_role" => {
                            let target = doc
                                .string(value, b"name", PATH_SIZE)
                                .filter(|value| literal(value))
                                .and_then(|value| {
                                    self.role(location.dir, &value)
                                });
                            edge(
                                target,
                                std::str::from_utf8(&key).unwrap(),
                                pair.line(),
                            );
                        }
                        b"roles" => {
                            for item in value
                                .sequence()
                                .into_iter()
                                .flat_map(Node::children)
                            {
                                let target = item
                                    .at(0)
                                    .and_then(|node| {
                                        doc.scalar(node, PATH_SIZE)
                                            .or_else(|| {
                                                doc.string(
                                                    node, b"role", PATH_SIZE,
                                                )
                                            })
                                            .or_else(|| {
                                                doc.string(
                                                    node, b"name", PATH_SIZE,
                                                )
                                            })
                                    })
                                    .filter(|value| literal(value))
                                    .and_then(|value| {
                                        self.role(location.dir, &value)
                                    });
                                edge(target, "roles", item.line());
                            }
                        }
                        _ => {}
                    }
                }
            }
            // Keep one iterator per ancestor, even for a wide YAML sequence.
            let mut children = node.children();
            if let Some(child) = children.next() {
                if ancestors.try_reserve(1).is_err() {
                    return;
                }
                ancestors.push(children);
                next = Some(child);
            } else {
                while let Some(siblings) = ancestors.last_mut() {
                    if let Some(sibling) = siblings.next() {
                        next = Some(sibling);
                        break;
                    }
                    ancestors.pop();
                }
            }
        }
    }
}

struct Candidate {
    file: usize,
    playbook: bool,
    ansible: bool,
    includes: Vec<Vec<u8>>,
}

fn mark_ansible(
    run: &Run,
    candidates: &mut [Candidate],
    sources: &[Source<'_>],
) {
    let by_path: HashMap<_, _> = candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            (sources[candidate.file].relative.to_bytes(), index)
        })
        .collect();
    let mut work: Vec<_> = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| candidate.ansible.then_some(index))
        .collect();

    while let Some(index) = work.pop() {
        if run.cancelled() {
            return;
        }
        // Take the list because following even a self-include can address this candidate.
        for target in std::mem::take(&mut candidates[index].includes) {
            if let Some(&next) = by_path.get(target.as_slice())
                && !candidates[next].ansible
            {
                candidates[next].ansible = true;
                work.push(next);
            }
        }
    }
}

fn playbook(doc: &Document) -> bool {
    doc.top()
        .and_then(Node::sequence)
        .into_iter()
        .flat_map(Node::children)
        .filter_map(Node::item_mapping)
        .any(|play| {
            play.children().any(|pair| {
                matches!(
                    doc.key(pair, 64).as_deref(),
                    Some(b"hosts" | b"import_playbook")
                )
            })
        })
}

const TASK_KEYWORDS: &[&[u8]] = &[
    b"name",
    b"when",
    b"tags",
    b"become",
    b"become_user",
    b"become_method",
    b"register",
    b"loop",
    b"with_items",
    b"with_dict",
    b"with_list",
    b"with_fileglob",
    b"with_nested",
    b"with_together",
    b"with_sequence",
    b"with_subelements",
    b"with_first_found",
    b"loop_control",
    b"notify",
    b"listen",
    b"ignore_errors",
    b"changed_when",
    b"failed_when",
    b"delegate_to",
    b"run_once",
    b"vars",
    b"environment",
    b"until",
    b"retries",
    b"delay",
    b"no_log",
    b"check_mode",
    b"diff",
    b"any_errors_fatal",
    b"throttle",
    b"timeout",
    b"args",
    b"action",
    b"local_action",
    b"async",
    b"poll",
    b"debugger",
    b"collections",
    b"module_defaults",
    b"remote_user",
    b"connection",
    b"port",
    b"ignore_unreachable",
    b"block",
    b"rescue",
    b"always",
    b"delegate_facts",
];

fn names(doc: &Document, value: Node<'_>) -> Vec<Vec<u8>> {
    if let Some(name) = doc.scalar(value, 256) {
        return vec![name];
    }
    let node = value.bare();
    match node.kind() {
        b"flow_sequence" => node
            .children()
            .filter_map(|item| doc.scalar(item, 256))
            .collect(),
        b"block_sequence" => node
            .children()
            .filter_map(|item| doc.scalar(item.at(0)?, 256))
            .collect(),
        _ => Vec::new(),
    }
}

struct Notify<'a> {
    source: i64,
    path: &'a [u8],
    role: &'a [u8],
    name: Vec<u8>,
    line: c_int,
}

#[derive(Default)]
struct Handlers<'a> {
    by_role: HashMap<Vec<u8>, Vec<i64>>,
    by_file: HashMap<Vec<u8>, Vec<i64>>,
    pending: Vec<Notify<'a>>,
}

fn handler_key(scope: &[u8], name: &[u8]) -> Vec<u8> {
    truncated([scope, b"\x1f", name].concat(), 2048)
}

impl Handlers<'_> {
    fn add(&mut self, location: &Location<'_>, name: &[u8], id: i64) {
        let add = |map: &mut HashMap<_, Vec<_>>, key| {
            let ids = map.entry(key).or_default();
            if ids.last() != Some(&id) {
                ids.push(id);
            }
        };
        if !location.task_role.is_empty() {
            add(&mut self.by_role, handler_key(location.task_role, name));
        }
        let file_scope = [b"F:", location.path.to_bytes()].concat();
        add(&mut self.by_file, handler_key(&file_scope, name));
    }

    fn link(&self, run: &Run, count: &mut c_int) {
        for note in &self.pending {
            if run.cancelled() {
                return;
            }
            let handlers = self
                .by_role
                .get(&handler_key(note.role, &note.name))
                .or_else(|| {
                    let scope = [b"F:", note.path].concat();
                    self.by_file.get(&handler_key(&scope, &note.name))
                });
            let props = CString::new(format!(
                "{{\"via\":\"notify\",\"line\":{}}}",
                note.line
            ))
            .unwrap();
            for &handler in handlers
                .into_iter()
                .flatten()
                .filter(|&&id| id != note.source)
            {
                run.edge(note.source, handler, c"CALLS", &props);
                *count += 1;
            }
        }
    }
}

struct Tasks<'a, 'source> {
    run: &'a Run,
    location: &'a Location<'source>,
    file: &'a FileNode,
    handlers: &'a mut Handlers<'source>,
    deployments: Deployments,
    ordinal: [usize; 2],
    count: &'a mut c_int,
}

impl Tasks<'_, '_> {
    fn task(&mut self, doc: &Document, mapping: Node<'_>, handler: bool) {
        let module = mapping
            .children()
            .filter_map(|pair| doc.key(pair, 128))
            .find(|key| {
                !key.is_empty() && !TASK_KEYWORDS.contains(&key.as_slice())
            })
            .or_else(|| {
                let action = doc
                    .string(mapping, b"action", 256)
                    .or_else(|| doc.string(mapping, b"local_action", 256))?;
                let name =
                    action.split(|byte| matches!(byte, b' ' | b'\t')).next()?;
                Some(truncated(name.to_vec(), 128))
            })
            .unwrap_or_default();
        let named = doc.string(mapping, b"name", 512);
        let name = named.as_deref().unwrap_or_else(|| {
            if module.is_empty() { b"task" } else { &module }
        });
        let kind = if handler { "handler" } else { "task" };
        let ordinal = &mut self.ordinal[usize::from(handler)];
        *ordinal += 1;
        let qn = &self.file.qualified_name;
        let base = qn
            .strip_suffix(b".__file__")
            .filter(|s| !s.is_empty())
            .unwrap_or(qn);
        let suffix = format!("::{kind}/{ordinal}");
        let qn = [base.get(..PATH_SIZE - 1).unwrap_or(base), suffix.as_bytes()]
            .concat();
        let module_json = escape(&module, &mut [0; 512]);
        let source = format!(
            "{{\"source\":\"ansible\",\"kind\":\"{kind}\",\"module\":\""
        );
        let qn = CString::new(qn).unwrap();
        let name = CString::new(name).unwrap();
        let props =
            CString::new([source.as_bytes(), &module_json, b"\"}"].concat())
                .unwrap();
        let id = unsafe {
            cbm_gbuf_upsert_node(
                self.run.buffer,
                c"Function".as_ptr(),
                name.as_ptr(),
                qn.as_ptr(),
                self.location.path.as_ptr(),
                mapping.line(),
                mapping.last_line(),
                props.as_ptr(),
            )
        };
        if id <= 0 {
            return;
        }
        self.run.edge(self.file.id, id, c"DEFINES", c"{}");
        *self.count += 1;
        self.deployments.collect(doc, mapping, id);
        if handler {
            if let Some(name) = named {
                self.handlers.add(self.location, &name, id);
            }
            if let Some(listen) = doc.get(mapping, b"listen") {
                for name in names(doc, listen) {
                    self.handlers.add(self.location, &name, id);
                }
            }
        }
        for pair in mapping
            .children()
            .filter(|&pair| doc.key(pair, 64).as_deref() == Some(b"notify"))
        {
            for name in pair
                .value()
                .into_iter()
                .flat_map(|value| names(doc, value))
                .filter(|name| literal(name))
            {
                self.handlers.pending.push(Notify {
                    source: id,
                    path: self.location.path.to_bytes(),
                    role: self.location.task_role,
                    name,
                    line: pair.line(),
                });
            }
        }
    }

    fn list(&mut self, doc: &Document, sequence: Node<'_>, handler: bool) {
        let mut stack = vec![sequence.children()];
        while let Some(items) = stack.last_mut() {
            if self.run.cancelled() {
                return;
            }
            let Some(item) = items.next() else {
                stack.pop();
                continue;
            };
            let Some(mapping) = item.item_mapping() else {
                continue;
            };
            if let Some(block) =
                doc.get(mapping, b"block").and_then(Node::sequence)
            {
                for part in [b"always".as_slice(), b"rescue"] {
                    if let Some(seq) =
                        doc.get(mapping, part).and_then(Node::sequence)
                    {
                        stack.push(seq.children());
                    }
                }
                stack.push(block.children());
            } else {
                self.task(doc, mapping, handler);
            }
        }
    }

    fn file(&mut self, doc: &Document, is_playbook: bool) {
        let Some(sequence) = doc.top().and_then(Node::sequence) else {
            return;
        };
        if !is_playbook {
            let path = self.location.path.to_bytes();
            self.list(
                doc,
                sequence,
                path.starts_with(b"handlers/") || contains(path, b"/handlers/"),
            );
            return;
        }
        for play in sequence.children().filter_map(Node::item_mapping) {
            if self.run.cancelled() {
                return;
            }
            for section in [
                b"pre_tasks".as_slice(),
                b"tasks",
                b"post_tasks",
                b"handlers",
            ] {
                if let Some(seq) =
                    doc.get(play, section).and_then(Node::sequence)
                {
                    self.list(doc, seq, section == b"handlers");
                }
            }
        }
    }
}

fn index(
    run: &Run,
    parser: &mut Parser,
    sources: &[Source<'_>],
    stats: &mut Stats,
) {
    let mut candidates = Vec::new();
    let mut graph = None;
    for (index, source) in sources.iter().enumerate() {
        if run.cancelled() {
            return;
        }
        let Some(doc) = parse_candidate(parser, source) else {
            continue;
        };
        let (files, composition) = graph.get_or_insert_with(|| {
            (
                run.index(c"File", |node| FileNode {
                    id: node.id,
                    qualified_name: if node.qualified_name.is_null() {
                        Vec::new()
                    } else {
                        unsafe { CStr::from_ptr(node.qualified_name) }
                            .to_bytes()
                            .to_vec()
                    },
                }),
                Composition {
                    modules: run.index(c"Module", |node| node.id),
                    folders: run.index(c"Folder", |node| node.id),
                },
            )
        });
        let Some(file) = files.get(source.relative.to_bytes()) else {
            continue;
        };
        let location = Location::new(source.relative);
        let mut includes = Vec::new();
        composition.walk(
            run,
            &doc,
            &location,
            file.id,
            &mut stats.imports,
            &mut includes,
        );
        let playbook = playbook(&doc);
        candidates.push(Candidate {
            file: index,
            playbook,
            ansible: playbook || !location.task_role.is_empty(),
            includes,
        });
    }

    let Some((files, _)) = graph else {
        return;
    };
    mark_ansible(run, &mut candidates, sources);
    if run.cancelled() || !candidates.iter().any(|candidate| candidate.ansible) {
        return;
    }
    let by_path: HashMap<_, _> = sources
        .iter()
        .map(|source| (source.relative.to_bytes(), source.absolute))
        .collect();
    let mut handlers = Handlers::default();
    for candidate in candidates.iter().filter(|candidate| candidate.ansible) {
        if run.cancelled() {
            return;
        }
        let source = &sources[candidate.file];
        let Some(doc) = parse_candidate(parser, source) else {
            continue;
        };
        let location = Location::new(source.relative);
        let mut tasks = Tasks {
            run,
            location: &location,
            file: &files[source.relative.to_bytes()],
            handlers: &mut handlers,
            deployments: Deployments::default(),
            ordinal: [0; 2],
            count: &mut stats.tasks,
        };
        tasks.file(&doc, candidate.playbook);
        tasks.deployments.link(
            run,
            &location,
            &files,
            &by_path,
            parser,
            &mut stats.deploys,
        );
    }
    handlers.link(run, &mut stats.notifies);
}

/// Run against a live C context, graph, file array and pinned YAML grammar.
/// The callback and output pointer must remain valid for the duration of the call.
#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_ansible_run(
    ctx: *mut c_void,
    buffer: *mut c_void,
    files: *const FileInfo,
    count: c_int,
    language: *const c_void,
    check_cancel: unsafe extern "C" fn(*mut c_void) -> bool,
    stats: *mut Stats,
) -> bool {
    let run = Run {
        ctx,
        buffer,
        check_cancel,
    };
    if count <= 0 || run.cancelled() {
        return false;
    }
    let Some(mut parser) = (unsafe { Parser::new(language.cast()) }) else {
        return false;
    };
    let mut sources = Vec::new();
    for file in unsafe { slice::from_raw_parts(files, count as usize) } {
        if run.cancelled() {
            return false;
        }
        sources.push(Source {
            absolute: unsafe { CStr::from_ptr(file.path) },
            relative: unsafe { CStr::from_ptr(file.relative) },
        });
    }
    index(&run, &mut parser, &sources, unsafe { &mut *stats });
    !run.cancelled()
}

/// Classify the same YAML prefix for the pass and the closure decline rule.
#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_ansible_is_sequence(
    source: *const c_char,
) -> bool {
    !source.is_null()
        && starts_sequence(unsafe { CStr::from_ptr(source) }.to_bytes())
}
