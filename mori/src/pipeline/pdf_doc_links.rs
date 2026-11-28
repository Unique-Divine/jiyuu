//! Resolve PDF mentions against the complete graph view for the generation.
//! The graph owns node identities. The resolver owns its immutable path
//! index and each document's join, suppression, and memoization state.

use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::{ptr, slice};

const KEY_CAPACITY: usize = 1024;
const MAX_COMPONENTS: usize = 64;

type NodeRef = *const c_void;

#[repr(C)]
struct NodeView {
    id: i64,
    start_line: c_int,
    label: *const c_char,
    name: *const c_char,
    qualified_name: *const c_char,
    path: *const c_char,
    properties: *const c_char,
}

#[repr(C)]
struct NodeList {
    nodes: *const NodeRef,
    count: c_int,
}

#[repr(C)]
pub(crate) struct PdfGraphOps {
    by_name: extern "C" fn(*const c_void, *const c_char) -> NodeList,
    by_label: extern "C" fn(*const c_void, *const c_char) -> NodeList,
    visit: extern "C" fn(
        *const c_void,
        extern "C" fn(NodeRef, *mut c_void),
        *mut c_void,
    ),
    describe: extern "C" fn(NodeRef) -> NodeView,
}

struct Graph<'a> {
    data: *const c_void,
    ops: &'a PdfGraphOps,
}

struct Node<'a> {
    id: i64,
    start_line: c_int,
    label: &'a [u8],
    name: Option<&'a [u8]>,
    qn: &'a [u8],
    path: &'a [u8],
    properties: &'a [u8],
}

/// The native graph and token views lend C strings until the caller returns.
unsafe fn bytes<'a>(raw: *const c_char) -> Option<&'a [u8]> {
    if raw.is_null() {
        return None;
    }
    // SAFETY: The caller guarantees a live, NUL-terminated string.
    Some(unsafe { CStr::from_ptr(raw) }.to_bytes())
}

impl Graph<'_> {
    fn by_name(&self, name: &[u8]) -> &[NodeRef] {
        let nodes = if name.len() < KEY_CAPACITY {
            let mut key = [0; KEY_CAPACITY];
            key[..name.len()].copy_from_slice(name);
            (self.ops.by_name)(self.data, key.as_ptr().cast())
        } else {
            let key =
                CString::new(name).expect("lookup key comes from a C string");
            (self.ops.by_name)(self.data, key.as_ptr())
        };
        self.nodes(nodes)
    }

    fn by_label(&self, label: &CStr) -> &[NodeRef] {
        self.nodes((self.ops.by_label)(self.data, label.as_ptr()))
    }

    fn nodes(&self, list: NodeList) -> &[NodeRef] {
        if list.nodes.is_null() || list.count <= 0 {
            return &[];
        }
        // SAFETY: The graph lends the unchanged secondary-index slice.
        unsafe { slice::from_raw_parts(list.nodes, list.count as usize) }
    }

    fn describe(&self, node: NodeRef) -> Node<'_> {
        let view = (self.ops.describe)(node);
        // SAFETY: The graph owns the strings. The resulting node borrows self.
        unsafe {
            Node {
                id: view.id,
                start_line: view.start_line,
                label: bytes(view.label).unwrap_or_default(),
                name: bytes(view.name),
                qn: bytes(view.qualified_name).unwrap_or_default(),
                path: bytes(view.path).unwrap_or_default(),
                properties: bytes(view.properties).unwrap_or_default(),
            }
        }
    }
}

#[derive(Default)]
struct Index {
    project: Vec<u8>,
    paths: HashMap<Vec<u8>, Vec<NodeRef>>,
    basenames: HashMap<Vec<u8>, Vec<NodeRef>>,
    components: HashSet<Vec<u8>>,
    irregular: HashMap<Vec<u8>, Vec<NodeRef>>,
}

fn trim_path(path: &[u8]) -> &[u8] {
    let Some(first) = path.iter().position(|&b| b != b'/') else {
        return &[];
    };
    let last = path.iter().rposition(|&b| b != b'/').unwrap_or(first);
    &path[first..=last]
}

fn basename(path: &[u8]) -> &[u8] {
    path.rsplit(|&b| b == b'/').next().unwrap_or_default()
}

fn component_suffix(value: &[u8], suffix: &[u8], delimiter: u8) -> bool {
    value.strip_suffix(suffix).is_some_and(|prefix| {
        prefix.is_empty() || prefix.last() == Some(&delimiter)
    })
}

impl Index {
    fn relative_qn<'a>(&self, qn: &'a [u8]) -> &'a [u8] {
        if !self.project.is_empty()
            && let Some(relative) = qn.strip_prefix(self.project.as_slice())
            && let Some(relative) = relative.strip_prefix(b".")
        {
            return relative;
        }
        qn
    }

    fn code_node(&self, node: &Node<'_>) -> bool {
        !node.label.is_empty()
            && !matches!(
                node.label,
                b"Section"
                    | b"Project"
                    | b"Branch"
                    | b"Route"
                    | b"Channel"
                    | b"Decorator"
                    | b"Macro"
                    | b"Comment"
                    | b"Commit"
                    | b"Tag"
                    | b"File"
                    | b"Folder"
            )
            && !node.qn.is_empty()
            && !node.path.starts_with(b"<")
            && !self.relative_qn(node.qn).starts_with(b"__")
    }

    fn add_path(&mut self, graph: &Graph<'_>, node: NodeRef) {
        let path = graph.describe(node).path;
        if path.is_empty() || path.starts_with(b"<") {
            return;
        }
        let path = trim_path(path);
        self.paths.entry(path.to_vec()).or_default().push(node);
        self.basenames
            .entry(basename(path).to_vec())
            .or_default()
            .push(node);
        for component in path.split(|&b| b == b'/').filter(|s| !s.is_empty()) {
            if !self.components.contains(component) {
                self.components.insert(component.to_vec());
            }
        }
    }

    fn add_irregular(&mut self, graph: &Graph<'_>, reference: NodeRef) {
        let node = graph.describe(reference);
        if !self.code_node(&node) {
            return;
        }
        let last = self.relative_qn(node.qn).rsplit(|&b| b == b'.').next();
        if let Some(last) = last.filter(|last| Some(*last) != node.name) {
            self.irregular
                .entry(last.to_vec())
                .or_default()
                .push(reference);
        }
    }

    fn owner_name(&self, node: &Node<'_>) -> Option<Vec<u8>> {
        const KEY: &[u8] = b"\"parent_class\":\"";
        let start = node
            .properties
            .windows(KEY.len())
            .position(|part| part == KEY)?;
        let mut remaining = &node.properties[start + KEY.len()..];
        let mut parent = Vec::new();
        while let Some((&byte, rest)) = remaining.split_first() {
            if byte == b'"' {
                let name = node.name.unwrap_or_default();
                let mut owner = self.relative_qn(&parent).to_vec();
                if owner.len() + 1 + name.len() >= KEY_CAPACITY {
                    return None;
                }
                owner.push(b'.');
                owner.extend_from_slice(name);
                return Some(owner);
            }
            if parent.len() == KEY_CAPACITY - 1 {
                return None;
            }
            if byte == b'\\'
                && let Some((&escaped, tail)) = rest.split_first()
            {
                parent.push(escaped);
                remaining = tail;
            } else {
                parent.push(byte);
                remaining = rest;
            }
        }
        None
    }

    fn qn_match(&self, node: &Node<'_>, form: &[u8]) -> Option<bool> {
        if !self.code_node(node) {
            return None;
        }
        let relative = self.relative_qn(node.qn);
        let owner = self.owner_name(node);
        let ownerless = owner
            .as_deref()
            .is_some_and(|owner| !component_suffix(relative, owner, b'.'));
        if !ownerless && component_suffix(relative, form, b'.') {
            Some(relative == form)
        } else if owner.as_deref().is_some_and(|owner| {
            owner != relative && component_suffix(owner, form, b'.')
        }) {
            Some(false)
        } else {
            None
        }
    }

    fn collapse(&self, graph: &Graph<'_>, nodes: &[NodeRef]) -> Option<NodeRef> {
        if nodes.len() == 1 {
            return Some(nodes[0]);
        }

        let mut containers = nodes.iter().copied().filter(|&node| {
            matches!(
                graph.describe(node).label,
                b"Class"
                    | b"Struct"
                    | b"Interface"
                    | b"Enum"
                    | b"Trait"
                    | b"Type"
                    | b"Record"
                    | b"Object"
                    | b"Protocol"
                    | b"Union"
            )
        });
        if let Some(container) = containers.next()
            && containers.next().is_none()
        {
            let owner = graph.describe(container);
            let owner_qn = self.relative_qn(owner.qn);
            if nodes.iter().copied().all(|reference| {
                if reference == container {
                    return true;
                }
                let node = graph.describe(reference);
                owner.name.is_some()
                    && owner.name == node.name
                    && self
                        .relative_qn(node.qn)
                        .strip_prefix(owner_qn)
                        .is_some_and(|tail| tail.starts_with(b"."))
            }) {
                return Some(container);
            }
        }

        // Duplicate nodes representing one definition use the lowest ID.
        let first = graph.describe(*nodes.first()?);
        let mut lowest = (first.id, nodes[0]);
        for &reference in &nodes[1..] {
            let node = graph.describe(reference);
            if node.start_line != first.start_line
                || node.path != first.path
                || node.name.unwrap_or_default()
                    != first.name.unwrap_or_default()
            {
                return None;
            }
            if node.id < lowest.0 {
                lowest = (node.id, reference);
            }
        }
        Some(lowest.1)
    }

    fn outcome(
        &self,
        graph: &Graph<'_>,
        nodes: &[NodeRef],
        exact: bool,
    ) -> Resolution {
        if nodes.is_empty() {
            return Resolution::None;
        }
        self.collapse(graph, nodes)
            .map_or(Resolution::Ambiguous, |node| Resolution::Hit {
                node,
                exact,
            })
    }

    fn resolve_qn(&self, graph: &Graph<'_>, form: &[u8]) -> Resolution {
        let mut starts = [0; MAX_COMPONENTS];
        let mut count = 1;
        for (i, &b) in form.iter().enumerate() {
            if count == starts.len() {
                break;
            }
            if b == b'.' {
                starts[count] = i + 1;
                count += 1;
            }
        }
        if count == 1 {
            return Resolution::None;
        }

        // A graph name can itself contain dots. Look up each suffix as an
        // entire name, then add nodes whose last QN component differs from
        // their graph name (for example, some Module nodes).
        let suffixes = starts[..count].iter().rev().map(|&start| &form[start..]);
        let irregular = self.irregular.get(&form[starts[count - 1]..]);
        let nodes = suffixes
            .flat_map(|suffix| graph.by_name(suffix))
            .chain(irregular.into_iter().flatten());
        let mut seen = HashSet::new();
        let mut matches = Vec::new();
        let mut exact = None;
        let mut exact_count = 0;
        for &node in nodes {
            if seen.insert(node)
                && let Some(is_exact) =
                    self.qn_match(&graph.describe(node), form)
            {
                matches.push(node);
                if is_exact {
                    exact = Some(node);
                    exact_count += 1;
                }
            }
        }
        if exact_count == 1 {
            return Resolution::Hit {
                node: exact.expect("a unique exact candidate was counted"),
                exact: true,
            };
        }
        self.outcome(graph, &matches, false)
    }

    fn resolve_form(
        &self,
        graph: &Graph<'_>,
        syntax: c_int,
        form: &[u8],
    ) -> Resolution {
        if matches!(syntax, PATH | FILE) {
            let path = trim_path(form);
            if path.is_empty() || path.len() >= KEY_CAPACITY {
                return Resolution::None;
            }
            if let Some(nodes) = self.paths.get(path) {
                return self.outcome(graph, nodes, true);
            }
            let suffix_nodes: Vec<_> = self
                .basenames
                .get(basename(path))
                .into_iter()
                .flatten()
                .copied()
                .filter(|&node| {
                    component_suffix(
                        trim_path(graph.describe(node).path),
                        path,
                        b'/',
                    )
                })
                .collect();
            if !suffix_nodes.is_empty() {
                return self.outcome(graph, &suffix_nodes, false);
            }
        }
        match syntax {
            QN | FILE => self.resolve_qn(graph, form),
            NAME => {
                let nodes: Vec<_> = graph
                    .by_name(form)
                    .iter()
                    .copied()
                    .filter(|&node| self.code_node(&graph.describe(node)))
                    .collect();
                self.outcome(graph, &nodes, false)
            }
            _ => Resolution::None,
        }
    }

    fn first_known(&self, graph: &Graph<'_>, syntax: c_int, raw: &[u8]) -> bool {
        let sep = if syntax == PATH { b'/' } else { b'.' };
        let start = raw.iter().position(|&b| b != b'/').unwrap_or(raw.len());
        let raw = &raw[start..];
        let len = raw
            .iter()
            .take(KEY_CAPACITY - 1)
            .take_while(|&&b| b != sep)
            .count();
        let first = &raw[..len];
        !first.is_empty()
            && (self.components.contains(first)
                || self.basenames.contains_key(first)
                || !graph.by_name(first).is_empty())
    }
}

// These values describe this private ABI. The C adapter translates the
// public doc-link syntax and reason enums.
const PATH: c_int = 1;
const FILE: c_int = 2;
const QN: c_int = 3;
const NAME: c_int = 4;

#[derive(Clone, Copy, Default)]
enum Resolution {
    #[default]
    None,
    Ambiguous,
    Hit {
        node: NodeRef,
        exact: bool,
    },
}

impl Index {
    fn resolve_join(
        &self,
        graph: &Graph<'_>,
        syntax: c_int,
        forms: &[u8],
    ) -> Resolution {
        if forms.len() >= KEY_CAPACITY {
            return Resolution::None;
        }
        let mut forms = forms.splitn(2, |&b| b == 0x1f);
        let first =
            self.resolve_form(graph, syntax, forms.next().unwrap_or_default());
        let Some(alternate) = forms.next() else {
            return first;
        };
        let second = self.resolve_form(graph, syntax, alternate);
        match (first, second) {
            (
                Resolution::Hit { node: a, .. },
                Resolution::Hit { node: b, .. },
            ) if a != b => Resolution::Ambiguous,
            (Resolution::Hit { .. }, _) => first,
            (_, Resolution::Hit { .. }) => second,
            _ => first,
        }
    }
}

fn data_target(path: &[u8]) -> bool {
    const EXTS: &[&[u8]] = &[
        b".json", b".yaml", b".yml", b".toml", b".csv", b".tsv", b".lock",
    ];
    path.len() < KEY_CAPACITY
        && EXTS.iter().any(|ext| {
            path.len() >= ext.len()
                && path[path.len() - ext.len()..].eq_ignore_ascii_case(ext)
        })
}

fn test_target(path: &[u8]) -> bool {
    const SUFFIXES: &[&[u8]] = &[
        b"_test.py",
        b"_test.go",
        b"Test.cs",
        b"Tests.cs",
        b"Test.java",
        b"Tests.java",
        b".t.sol",
    ];
    const JS_SUFFIXES: &[&[u8]] = &[
        b".test.js",
        b".test.jsx",
        b".test.ts",
        b".test.tsx",
        b".spec.js",
        b".spec.jsx",
        b".spec.ts",
        b".spec.tsx",
    ];
    let mut components = path.rsplit(|&b| b == b'/');
    let base = components.next().unwrap_or_default();
    (base.starts_with(b"test_") && base.ends_with(b".py"))
        || base == b"conftest.py"
        || SUFFIXES.iter().any(|ext| base.ends_with(ext))
        || JS_SUFFIXES
            .iter()
            .any(|ext| base.len() > ext.len() && base.ends_with(ext))
        || components.any(|part| {
            matches!(
                part,
                b"tests"
                    | b"__tests__"
                    | b"test"
                    | b"mocks"
                    | b"testutil"
                    | b"testing"
                    | b"testdata"
            )
        })
}

#[repr(C)]
pub(crate) struct PdfLink {
    raw: *const c_char,
    syntax: c_int,
    join: bool,
}

type LinkAt = extern "C" fn(*const c_void, c_int) -> PdfLink;

struct FileState {
    suppressed: Vec<bool>,
    joins: Vec<Resolution>,
    memo: HashMap<Vec<u8>, (c_int, Resolution)>,
}

// A successful join replaces the line-local token indexes encoded after RS.
// Consume the same signed decimal fields as strtol, stopping at malformed
// separators. Overflow cannot name a token but does not end the list.
fn suppress_fragments(mut list: &[u8], suppressed: &mut [bool]) {
    loop {
        let start = list
            .iter()
            .position(|&byte| !(byte.is_ascii_whitespace() || byte == b'\x0b'))
            .unwrap_or(list.len());
        list = &list[start..];
        let negative = list.starts_with(b"-");
        if list.starts_with(b"+") || negative {
            list = &list[1..];
        }
        let digits = list.iter().take_while(|b| b.is_ascii_digit()).count();
        if digits == 0 {
            return;
        }
        let mut index = 0usize;
        for &digit in &list[..digits] {
            index = index
                .saturating_mul(10)
                .saturating_add(usize::from(digit - b'0'));
        }
        if (!negative || index == 0)
            && let Some(slot) = suppressed.get_mut(index)
        {
            *slot = true;
        }

        let Some(rest) = list[digits..].strip_prefix(b",") else {
            return;
        };
        list = rest;
    }
}

#[repr(C)]
pub(crate) struct PdfOutcome {
    target: NodeRef,
    exact: bool,
    reason: c_int,
}

impl PdfOutcome {
    const LOCAL: Self = Self::unresolved(0);
    const MISSING: Self = Self::unresolved(1);
    const AMBIGUOUS: Self = Self::unresolved(2);
    const TEST_ONLY: Self = Self::unresolved(3);

    const fn unresolved(reason: c_int) -> Self {
        Self {
            target: ptr::null(),
            exact: false,
            reason,
        }
    }

    fn hit(graph: &Graph<'_>, node: NodeRef, exact: bool) -> Self {
        let path = graph.describe(node).path;
        if data_target(path) {
            Self::LOCAL
        } else if test_target(path) {
            Self::TEST_ONLY
        } else {
            Self {
                target: node,
                exact,
                reason: 0,
            }
        }
    }
}

// Safety for the following exports: C owns a stable graph and its callback
// table. The graph that built the index must outlive the index. Every token
// string is read only within its call. A file state is used by one worker
// and its index can be read concurrently until all workers are done.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mori_pdf_links_build(
    project: *const c_char,
    data: *const c_void,
    ops: &PdfGraphOps,
) -> *mut c_void {
    // SAFETY: The native build callback passes its project C string.
    let Some(project) = (unsafe { bytes(project) }) else {
        return ptr::null_mut();
    };
    let graph = Graph { data, ops };
    let mut index = Index {
        project: project.to_vec(),
        ..Index::default()
    };
    for label in [c"File", c"Folder"] {
        for &node in graph.by_label(label) {
            index.add_path(&graph, node);
        }
    }

    struct Build<'a> {
        graph: &'a Graph<'a>,
        index: &'a mut Index,
    }
    extern "C" fn visit(node: NodeRef, context: *mut c_void) {
        // SAFETY: The C visitor calls synchronously with this stack context.
        let build = unsafe { &mut *context.cast::<Build<'_>>() };
        build.index.add_irregular(build.graph, node);
    }

    let mut build = Build {
        graph: &graph,
        index: &mut index,
    };
    (ops.visit)(data, visit, (&raw mut build).cast());

    Box::into_raw(Box::new(index)).cast()
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mori_pdf_links_destroy(index: *mut c_void) {
    if !index.is_null() {
        // SAFETY: The adapter returns this Box exactly once.
        drop(unsafe { Box::from_raw(index.cast::<Index>()) });
    }
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mori_pdf_links_prepare(
    index: *const c_void,
    data: *const c_void,
    ops: &PdfGraphOps,
    links: *const c_void,
    len: c_int,
    link_at: LinkAt,
) -> *mut c_void {
    // SAFETY: The resolver index outlives this file's state.
    let Some(index) = (unsafe { index.cast::<Index>().as_ref() }) else {
        return ptr::null_mut();
    };

    let len = len.max(0) as usize;
    let mut state = FileState {
        suppressed: Vec::new(),
        joins: Vec::new(),
        memo: HashMap::new(),
    };
    if state.suppressed.try_reserve_exact(len).is_err()
        || state.joins.try_reserve_exact(len).is_err()
    {
        return ptr::null_mut();
    }
    state.suppressed.resize(len, false);
    state.joins.resize(len, Resolution::None);
    let graph = Graph { data, ops };
    for i in 0..len {
        let link = link_at(links, i as c_int);
        if !link.join {
            continue;
        }
        // SAFETY: C owns the link strings throughout the prepare call.
        let Some(raw) = (unsafe { bytes(link.raw) }) else {
            continue;
        };
        let mut parts = raw.splitn(2, |&b| b == 0x1e);
        let result = index.resolve_join(
            &graph,
            link.syntax,
            parts.next().unwrap_or_default(),
        );
        state.joins[i] = result;
        if matches!(result, Resolution::Hit { .. })
            && let Some(fragments) = parts.next()
        {
            suppress_fragments(fragments, &mut state.suppressed);
        }
    }
    Box::into_raw(Box::new(state)).cast()
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mori_pdf_links_end(state: *mut c_void) {
    if !state.is_null() {
        // SAFETY: The adapter returns this Box exactly once.
        drop(unsafe { Box::from_raw(state.cast::<FileState>()) });
    }
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mori_pdf_links_resolve(
    index: *const c_void,
    state: *mut c_void,
    data: *const c_void,
    ops: &PdfGraphOps,
    token: PdfLink,
    position: c_int,
) -> PdfOutcome {
    // SAFETY: The adapter lends both the index and a native token string.
    let (Some(index), Some(raw)) =
        (unsafe { (index.cast::<Index>().as_ref(), bytes(token.raw)) })
    else {
        return PdfOutcome::LOCAL;
    };
    // SAFETY: C lends the mutable file state exclusively for this call.
    let state = unsafe { state.cast::<FileState>().as_mut() }.filter(|state| {
        position >= 0 && (position as usize) < state.joins.len()
    });
    let i = position as usize;
    let graph = Graph { data, ops };
    if token.join {
        return match state.map(|state| state.joins[i]) {
            Some(Resolution::Hit { node, exact }) if token.syntax != NAME => {
                PdfOutcome::hit(&graph, node, exact)
            }
            _ => PdfOutcome::LOCAL,
        };
    }
    if token.syntax == NAME
        || state.as_ref().is_some_and(|state| state.suppressed[i])
    {
        return PdfOutcome::LOCAL;
    }

    let result = if let Some(state) = state {
        match state.memo.get(raw) {
            Some(&(syntax, result)) if syntax == token.syntax => result,
            Some(_) => index.resolve_form(&graph, token.syntax, raw),
            None => {
                let result = index.resolve_form(&graph, token.syntax, raw);
                state.memo.insert(raw.to_vec(), (token.syntax, result));
                result
            }
        }
    } else {
        index.resolve_form(&graph, token.syntax, raw)
    };

    match result {
        Resolution::Hit { node, exact } => PdfOutcome::hit(&graph, node, exact),
        Resolution::Ambiguous => PdfOutcome::AMBIGUOUS,
        Resolution::None if index.first_known(&graph, token.syntax, raw) => {
            PdfOutcome::MISSING
        }
        Resolution::None => PdfOutcome::LOCAL,
    }
}
