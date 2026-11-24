//! Kubernetes pass sequencing and graph resolution over native extraction facts.

use super::native_graph_buffer::{
    Node, cbm_gbuf_find_by_id, cbm_gbuf_find_by_label,
};
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr;
use std::slice;

const LABEL_LEN: usize = 128;
const REF_PROPS_LEN: usize = 1024;
const SERVICE: c_int = 1;
const WORKLOAD: c_int = 2;
const POD: c_int = 3;
const PDB: c_int = 4;
const NETPOL: c_int = 5;

#[repr(i32)]
#[derive(Clone, Copy)]
enum Scope {
    Build,
    Deploy,
    ExplicitBuild,
}

type Phase = unsafe extern "C" fn(*mut c_void);

#[repr(C)]
struct Record {
    id: i64,
    name: [c_char; LABEL_LEN],
    namespace: [c_char; LABEL_LEN],
    kind: c_int,
    pair_offset: c_int,
    pair_count: c_int,
    has_expressions: bool,
}

#[repr(C)]
struct Reference {
    source: i64,
    kind: *const c_char,
    namespace: *const c_char,
    name: *const c_char,
    via: *const c_char,
    edge: *const c_char,
    route: *const c_char,
    key: *const c_char,
    container: *const c_char,
}

#[repr(C)]
struct Crd {
    id: i64,
    group: *const c_char,
    kind: *const c_char,
}

#[repr(C)]
#[derive(Default)]
struct Facts {
    records: *const Record,
    record_count: c_int,
    pairs: *const *const c_char,
    pair_count: c_int,
    refs: *const Reference,
    ref_count: c_int,
    crds: *const Crd,
    crd_count: c_int,
}

#[repr(C)]
#[derive(Default)]
struct Member {
    bundle: *const c_char,
    namespace: *const c_char,
}

// cbm_gbuf_node_t. Nodes retain their addresses for the duration of the
// resolution phases; Route upserts touch a separate label index.
#[repr(C)]
struct Ops {
    cancelled: unsafe extern "C" fn(*mut c_void) -> bool,
    prepare: unsafe extern "C" fn(*mut c_void, c_int, c_int),
    consume: unsafe extern "C" fn(*mut c_void, c_int, c_int, bool),
    build_scopes: Phase,
    deploys: Phase,
    images_and_transforms: Phase,
    chart_defaults: Phase,
    shell_and_terraform: Phase,
    chart_releases: Phase,
    image_hubs_and_env: Phase,
    resolution: unsafe extern "C" fn(*const c_void, *mut Facts),
    members: unsafe extern "C" fn(
        *mut c_void,
        *const c_char,
        Scope,
        *mut Member,
        *mut c_int,
    ) -> *const Member,
    scope_key_limit: unsafe extern "C" fn(*const c_char) -> usize,
    cluster_scoped: unsafe extern "C" fn(*const c_char) -> bool,
    set_namespace: unsafe extern "C" fn(*const Node, *const c_char),
    route:
        unsafe extern "C" fn(*mut c_void, *const Node, *const Reference) -> i64,
    ref_properties: unsafe extern "C" fn(*const Reference, *mut c_char),
    mark_crd: unsafe extern "C" fn(*const Node, *const Crd),
    report: unsafe extern "C" fn(u64, c_int, c_int, c_int, c_int, c_int),
}

unsafe extern "C" {
    fn cbm_gbuf_insert_edge(
        graph: *mut c_void,
        source: i64,
        target: i64,
        edge: *const c_char,
        properties: *const c_char,
    ) -> i64;
}

struct Pass<'a> {
    state: *mut c_void,
    context: *mut c_void,
    graph: *mut c_void,
    ops: &'a Ops,
}

#[derive(Default)]
struct Counts {
    candidates: u64,
    selectors: c_int,
    ref_edges: c_int,
    unresolved: c_int,
    instances: c_int,
}

type Index<'a, T> = HashMap<*const c_char, HashMap<&'a [u8], Vec<T>>>;

// Callers supply validated C facts and graph-owned strings. Keep bytes as
// C supplied them, since name and path comparison does not require UTF-8.
unsafe fn string<'a>(raw: *const c_char) -> Option<&'a CStr> {
    (!raw.is_null()).then(|| unsafe { CStr::from_ptr(raw) })
}

unsafe fn items<'a, T>(raw: *const T, count: c_int) -> &'a [T] {
    if count <= 0 {
        &[]
    } else {
        unsafe { slice::from_raw_parts(raw, count as usize) }
    }
}

fn namespace_after_kind<'a>(key: &'a [u8], unspecified: &[&[u8]]) -> &'a [u8] {
    let mut parts = key.splitn(4, |&b| b == b'/');
    let ns = parts.nth(2).unwrap_or_default();
    if parts.next().is_none() || unspecified.contains(&ns) {
        return b"";
    }
    &ns[..ns.len().min(LABEL_LEN - 1)]
}

fn after_marker<'a>(text: &'a [u8], marker: &[u8]) -> Option<&'a [u8]> {
    let at = text.windows(marker.len()).position(|part| part == marker)?;
    Some(&text[at + marker.len()..])
}

impl Node {
    fn key(&self) -> &[u8] {
        let qn = unsafe { string(self.qualified_name) }
            .map_or(&[][..], CStr::to_bytes);
        after_marker(qn, b"::").unwrap_or_default()
    }

    fn namespace(&self, deploy: bool) -> &[u8] {
        if deploy
            && let Some(properties) = unsafe { string(self.properties_json) }
            && let Some(key) =
                after_marker(properties.to_bytes(), b"\"obj_key\":\"")
        {
            return namespace_after_kind(key, &[b"?", b"_"]);
        }
        namespace_after_kind(self.key(), &[b"-"])
    }
}

fn namespaces_match(source: &[u8], target: &[u8]) -> bool {
    source.is_empty() || target.is_empty() || source == target
}

fn scope_namespace<'a>(member: &'a Member, declared: &'a [u8]) -> &'a [u8] {
    unsafe { string(member.namespace) }.map_or(declared, CStr::to_bytes)
}

fn share_namespace(
    source: &[Member],
    target: &[Member],
    source_ns: &[u8],
    target_ns: &[u8],
    explicit: Option<&[u8]>,
    cluster: bool,
) -> bool {
    source.iter().any(|s| {
        target.iter().any(|t| {
            s.bundle == t.bundle
                && (cluster
                    || namespaces_match(
                        explicit
                            .unwrap_or_else(|| scope_namespace(s, source_ns)),
                        scope_namespace(t, target_ns),
                    ))
        })
    })
}

fn unique<T: Ord>(values: &mut Vec<T>) {
    values.sort_unstable();
    values.dedup();
}

fn scan_files(
    files: c_int,
    chunk: c_int,
    mut cancelled: impl FnMut() -> bool,
    mut prepare: impl FnMut(c_int, c_int),
    mut consume: impl FnMut(c_int, c_int, bool),
) -> bool {
    let mut begin = 0;
    while begin < files {
        if cancelled() {
            return false;
        }
        let end = begin + chunk.min(files - begin);
        prepare(begin, end);
        let mut stop = false;
        for file in begin..end {
            // Once cancelled, release the rest of the prepared chunk.
            stop = stop || cancelled();
            consume(begin, file, !stop);
        }
        if stop {
            return false;
        }
        begin = end;
    }
    true
}

impl Pass<'_> {
    fn resources(&self) -> &[*const Node] {
        let mut nodes = ptr::null();
        let mut count = 0;
        unsafe {
            cbm_gbuf_find_by_label(
                self.graph,
                c"Resource".as_ptr(),
                &mut nodes,
                &mut count,
            );
            items(nodes, count)
        }
    }

    fn node(&self, id: i64) -> Option<&Node> {
        unsafe { cbm_gbuf_find_by_id(self.graph, id).as_ref() }
    }

    fn members<'a>(
        &'a self,
        file: *const c_char,
        scope: Scope,
        one: &'a mut Member,
    ) -> &'a [Member] {
        if file.is_null() {
            return &[];
        }
        let mut count = 0;
        unsafe {
            items(
                (self.ops.members)(self.state, file, scope, one, &mut count),
                count,
            )
        }
    }

    fn edge(&self, source: i64, target: i64, edge: &CStr, props: &CStr) {
        unsafe {
            cbm_gbuf_insert_edge(
                self.graph,
                source,
                target,
                edge.as_ptr(),
                props.as_ptr(),
            )
        };
    }

    fn apply_namespaces(&self) {
        for &raw in self.resources() {
            let node = unsafe { &*raw };
            let mut one = Member::default();
            let members =
                self.members(node.file_path, Scope::ExplicitBuild, &mut one);
            if members.iter().all(|m| m.namespace.is_null()) {
                continue;
            }
            let declared = node.namespace(false);
            let first = scope_namespace(&members[0], declared);
            let agree = !first.is_empty()
                && members
                    .iter()
                    .all(|m| scope_namespace(m, declared) == first);
            let ns = if agree { first } else { b"?" };
            if let Ok(ns) = CString::new(ns) {
                unsafe { (self.ops.set_namespace)(raw, ns.as_ptr()) };
            }
        }
    }

    fn selectors(
        &self,
        records: &[Record],
        pairs: &[*const c_char],
        counts: &mut Counts,
    ) {
        let pairs: Vec<_> = pairs
            .iter()
            .map(|&p| unsafe { CStr::from_ptr(p) }.to_bytes())
            .collect();
        let record_pairs = |r: &Record| {
            let off = r.pair_offset as usize;
            &pairs[off..off + r.pair_count as usize]
        };
        let mut index: Index<'_, usize> = HashMap::new();
        for (i, record) in records.iter().enumerate() {
            if !matches!(record.kind, WORKLOAD | POD) {
                continue;
            }
            let Some(node) = self.node(record.id) else {
                continue;
            };
            for scope in self.members(
                node.file_path,
                Scope::Build,
                &mut Member::default(),
            ) {
                let postings = index.entry(scope.bundle).or_default();
                for &pair in record_pairs(record) {
                    let ids =
                        postings.entry(self.scope_key(scope, pair)).or_default();
                    if ids.last() != Some(&i) {
                        ids.push(i);
                    }
                }
            }
        }

        let mut candidates = Vec::new();
        let mut matches = Vec::new();
        for selector in records {
            if !matches!(selector.kind, SERVICE | PDB | NETPOL)
                || selector.has_expressions
                || selector.pair_count == 0
            {
                continue;
            }
            let Some(source) = self.node(selector.id) else {
                continue;
            };
            let Some(source_file) = (unsafe { string(source.file_path) }) else {
                continue;
            };
            let mut source_one = Member::default();
            let scopes =
                self.members(source.file_path, Scope::Build, &mut source_one);
            let wanted = record_pairs(selector);
            candidates.clear();
            for scope in scopes {
                let Some(postings) = index.get(&scope.bundle) else {
                    continue;
                };
                // A missing pair rules the whole bundle out.
                let mut rarest: Option<&Vec<usize>> = None;
                for pair in wanted {
                    let Some(posting) =
                        postings.get(self.scope_key(scope, pair))
                    else {
                        rarest = None;
                        break;
                    };
                    if rarest.is_none_or(|current| posting.len() < current.len())
                    {
                        rarest = Some(posting);
                    }
                }
                if let Some(posting) = rarest {
                    candidates.extend_from_slice(posting);
                }
            }
            unique(&mut candidates);
            counts.candidates += candidates.len() as u64;
            let source_ns =
                unsafe { CStr::from_ptr(selector.namespace.as_ptr()) }
                    .to_bytes();
            matches.clear();
            for &i in &candidates {
                let workload = &records[i];
                let Some(target) = self.node(workload.id) else {
                    continue;
                };
                let (Some(name), Some(file)) =
                    (unsafe { (string(target.name), string(target.file_path)) })
                else {
                    continue;
                };
                let namespace =
                    unsafe { CStr::from_ptr(workload.namespace.as_ptr()) }
                        .to_bytes();
                let mut target_one = Member::default();
                let target_scopes = self.members(
                    target.file_path,
                    Scope::Build,
                    &mut target_one,
                );
                if share_namespace(
                    scopes,
                    target_scopes,
                    source_ns,
                    namespace,
                    None,
                    false,
                ) && wanted
                    .iter()
                    .all(|pair| record_pairs(workload).contains(pair))
                {
                    matches.push((
                        (name.to_bytes(), namespace),
                        i,
                        file == source_file,
                    ));
                }
            }
            matches.sort_unstable_by_key(|&(key, i, _)| (key, i));
            for group in matches.chunk_by(|a, b| a.0 == b.0) {
                let own_file = group.iter().any(|m| m.2);
                for &(_, i, own) in group {
                    if !own_file || own {
                        self.selector_edge(selector, &records[i]);
                        counts.selectors += 1;
                    }
                }
            }
        }
    }

    fn selector_edge(&self, selector: &Record, workload: &Record) {
        let props = match selector.kind {
            PDB => c"{\"ref\":\"selector\"}",
            NETPOL => c"{\"ref\":\"podSelector\"}",
            _ => {
                let service =
                    unsafe { CStr::from_ptr(selector.name.as_ptr()) }.to_bytes();
                let name =
                    unsafe { CStr::from_ptr(workload.name.as_ptr()) }.to_bytes();
                let mut props = [
                    b"{\"kind\":\"selector\",\"service\":\"".as_slice(),
                    service,
                    b"\",\"workload\":\"",
                    name,
                    b"\"}",
                ]
                .concat();
                // Match the native writer's 256-byte buffer, including NUL.
                props.truncate(255);
                props.push(0);
                let props = CStr::from_bytes_with_nul(&props)
                    .expect("selector names are C strings");
                self.edge(selector.id, workload.id, c"INFRA_MAPS", props);
                return;
            }
        };
        self.edge(selector.id, workload.id, c"USAGE", props);
    }

    fn scope_key<'a>(&self, scope: &Member, name: &'a [u8]) -> &'a [u8] {
        // The old buffer had ample space for the pointer prefix below this
        // length. For long inputs use the native platform's exact limit.
        if name.len() <= 1000 {
            return name;
        }
        let limit = unsafe { (self.ops.scope_key_limit)(scope.bundle) };
        &name[..name.len().min(limit)]
    }

    fn index_add<'a>(
        &self,
        index: &mut Index<'a, i64>,
        node: &Node,
        name: &'a [u8],
        deploys: bool,
    ) {
        let mut one = Member::default();
        let scopes = self.members(node.file_path, Scope::Build, &mut one);
        let mut deploy_one = Member::default();
        let deploy = if deploys {
            self.members(node.file_path, Scope::Deploy, &mut deploy_one)
        } else {
            &[]
        };
        for scope in scopes.iter().chain(deploy) {
            let ids = index
                .entry(scope.bundle)
                .or_default()
                .entry(self.scope_key(scope, name))
                .or_default();
            if ids.last() != Some(&node.id) {
                ids.push(node.id);
            }
        }
    }

    fn find(
        &self,
        index: &Index<'_, i64>,
        file: *const c_char,
        name: &[u8],
        deploys: bool,
        found: &mut Vec<i64>,
    ) {
        let mut one = Member::default();
        let scopes = self.members(file, Scope::Build, &mut one);
        let mut deploy_one = Member::default();
        let deploy = if deploys {
            self.members(file, Scope::Deploy, &mut deploy_one)
        } else {
            &[]
        };
        found.clear();
        for scope in scopes.iter().chain(deploy) {
            if let Some(ids) = index
                .get(&scope.bundle)
                .and_then(|names| names.get(self.scope_key(scope, name)))
            {
                found.extend_from_slice(ids);
            }
        }
        unique(found);
    }

    fn ref_matches(
        &self,
        reference: &Reference,
        source: &Node,
        target: &Node,
        cluster: bool,
    ) -> bool {
        if target.id == source.id
            || target.file_path.is_null()
            || unsafe { string(target.label) } != Some(c"Resource")
        {
            return false;
        }
        let explicit =
            unsafe { string(reference.namespace) }.map(CStr::to_bytes);
        [Scope::Build, Scope::Deploy].into_iter().any(|scope| {
            let deploy = matches!(scope, Scope::Deploy);
            let mut src_one = Member::default();
            let mut tgt_one = Member::default();
            share_namespace(
                self.members(source.file_path, scope, &mut src_one),
                self.members(target.file_path, scope, &mut tgt_one),
                source.namespace(deploy),
                target.namespace(deploy),
                explicit,
                cluster,
            )
        })
    }

    fn references(&self, refs: &[Reference], counts: &mut Counts) {
        if refs.is_empty() {
            return;
        }
        let mut index = Index::new();
        for &raw in self.resources() {
            let node = unsafe { &*raw };
            if let Some(name) = unsafe { string(node.name) } {
                self.index_add(&mut index, node, name.to_bytes(), true);
            }
        }
        let mut found = Vec::new();
        let mut matches = Vec::new();
        for reference in refs {
            let source = self
                .node(reference.source)
                .filter(|n| !n.file_path.is_null());
            let from = if reference.route.is_null() {
                reference.source
            } else {
                let route = source.map_or(0, |node| unsafe {
                    (self.ops.route)(self.context, node, reference)
                });
                if route <= 0
                    || reference.kind.is_null()
                    || reference.name.is_null()
                {
                    continue;
                }
                route
            };
            let Some(source) = source else {
                counts.unresolved += 1;
                continue;
            };
            let (Some(kind), Some(name), Some(edge)) = (unsafe {
                (
                    string(reference.kind),
                    string(reference.name),
                    string(reference.edge),
                )
            }) else {
                counts.unresolved += 1;
                continue;
            };
            let name = type_key(kind, name, 1023);
            self.find(&index, source.file_path, &name, true, &mut found);
            let cluster = unsafe { (self.ops.cluster_scoped)(reference.kind) };
            let source_file = unsafe { CStr::from_ptr(source.file_path) };
            matches.clear();
            for target in found.iter().filter_map(|&id| self.node(id)) {
                counts.candidates += 1;
                if self.ref_matches(reference, source, target, cluster) {
                    let own = unsafe { CStr::from_ptr(target.file_path) }
                        == source_file;
                    matches.push((target.id, own));
                }
            }
            if matches.is_empty() {
                counts.unresolved += 1;
                continue;
            }
            // Multiple declarations of the same object are alternatives.
            // Prefer a compatible declaration in the reference's own file.
            let own_file = matches.iter().any(|m| m.1);
            let mut props = [0; REF_PROPS_LEN];
            unsafe { (self.ops.ref_properties)(reference, props.as_mut_ptr()) };
            let props = unsafe { CStr::from_ptr(props.as_ptr()) };
            for &(target, own) in &matches {
                if !own_file || own {
                    self.edge(from, target, edge, props);
                    counts.ref_edges += 1;
                }
            }
        }
    }

    fn crds(&self, crds: &[Crd], counts: &mut Counts) {
        if crds.is_empty() {
            return;
        }
        let keys: Vec<_> = crds
            .iter()
            .filter_map(|crd| unsafe {
                Some((crd, type_key(string(crd.group)?, string(crd.kind)?, 511)))
            })
            .collect();
        for (crd, _) in &keys {
            let node = unsafe { cbm_gbuf_find_by_id(self.graph, crd.id) };
            unsafe { (self.ops.mark_crd)(node, *crd) };
        }
        let mut all: HashMap<&[u8], Vec<_>> = HashMap::new();
        let mut beside = Index::new();
        for (crd, key) in &keys {
            all.entry(key).or_default().push(crd.id);
            if let Some(node) = self.node(crd.id) {
                self.index_add(&mut beside, node, key, false);
            }
        }
        let mut found = Vec::new();
        for &raw in self.resources() {
            let node = unsafe { &*raw };
            let key = node.key();
            let Some(end) = key
                .iter()
                .enumerate()
                .filter(|&(_, &b)| b == b'/')
                .nth(1)
                .map(|(i, _)| i)
            else {
                continue;
            };
            let key = &key[..end.min(511)];
            let Some(every) = all.get(key) else { continue };
            if node.file_path.is_null() {
                continue;
            }
            self.find(&beside, node.file_path, key, false, &mut found);
            let candidates = if found.is_empty() { every } else { &found };
            counts.candidates += candidates.len() as u64;
            for &id in candidates {
                if id != node.id {
                    self.edge(node.id, id, c"USAGE", c"{\"ref\":\"crd\"}");
                    counts.instances += 1;
                }
            }
        }
    }

    fn resolve(&self) {
        let mut facts = Facts::default();
        let mut counts = Counts::default();
        unsafe { (self.ops.resolution)(self.state, &mut facts) };
        self.apply_namespaces();
        unsafe {
            self.selectors(
                items(facts.records, facts.record_count),
                items(facts.pairs, facts.pair_count),
                &mut counts,
            );
            self.references(items(facts.refs, facts.ref_count), &mut counts);
            self.crds(items(facts.crds, facts.crd_count), &mut counts);
            (self.ops.report)(
                counts.candidates,
                counts.selectors,
                facts.ref_count,
                counts.ref_edges,
                counts.unresolved,
                counts.instances,
            );
        }
    }
}

fn type_key(group: &CStr, kind: &CStr, limit: usize) -> Vec<u8> {
    let mut key = [group.to_bytes(), b"/", kind.to_bytes()].concat();
    key.truncate(limit);
    key
}

// SAFETY: C supplies a live synchronous pass, a positive chunk size, and
// the operation table matched above. All borrowed facts end before C frees
// them. Graph operations do not run concurrently with this driver.
#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_k8s_run(
    state: *mut c_void,
    context: *mut c_void,
    graph: *mut c_void,
    files: c_int,
    chunk: c_int,
    ops: *const Ops,
) -> bool {
    let ops = unsafe { &*ops };
    let pass = Pass {
        state,
        context,
        graph,
        ops,
    };
    if !scan_files(
        files,
        chunk,
        || unsafe { (ops.cancelled)(state) },
        |begin, end| unsafe { (ops.prepare)(state, begin, end) },
        |begin, file, publish| unsafe {
            (ops.consume)(state, begin, file, publish)
        },
    ) {
        return false;
    }
    unsafe { (ops.build_scopes)(state) };
    pass.resolve();
    unsafe {
        (ops.deploys)(state);
        (ops.images_and_transforms)(state);
        (ops.chart_defaults)(state);
        (ops.shell_and_terraform)(state);
        (ops.chart_releases)(state);
        (ops.image_hubs_and_env)(state);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_releases_the_rest_of_the_prepared_chunk() {
        let mut checks = 0;
        let mut prepared = Vec::new();
        let mut consumed = Vec::new();
        assert!(!scan_files(
            7,
            3,
            || {
                checks += 1;
                checks == 3
            },
            |begin, end| prepared.push((begin, end)),
            |begin, file, publish| consumed.push((begin, file, publish)),
        ));
        assert_eq!(checks, 3);
        assert_eq!(prepared, [(0, 3)]);
        assert_eq!(consumed, [(0, 0, true), (0, 1, false), (0, 2, false)]);
    }

    #[test]
    fn cancellation_at_a_chunk_boundary_skips_the_next_prepare() {
        let mut checks = 0;
        let mut prepared = Vec::new();
        let mut consumed = Vec::new();
        assert!(!scan_files(
            3,
            2,
            || {
                checks += 1;
                checks == 4
            },
            |begin, end| prepared.push((begin, end)),
            |begin, file, publish| consumed.push((begin, file, publish)),
        ));
        assert_eq!(prepared, [(0, 2)]);
        assert_eq!(consumed, [(0, 0, true), (0, 1, true)]);
    }

    #[test]
    fn namespace_agrees_only_within_the_same_bundle() {
        fn member(bundle: &CStr, namespace: Option<&CStr>) -> Member {
            Member {
                bundle: bundle.as_ptr(),
                namespace: namespace.map_or(ptr::null(), CStr::as_ptr),
            }
        }

        let a = c"overlay/a";
        let b = c"overlay/b";
        let source = [member(a, Some(c"blue")), member(b, Some(c"red"))];
        let target = [member(a, Some(c"red")), member(b, Some(c"blue"))];
        assert!(!share_namespace(&source, &target, b"", b"", None, false));
        assert!(share_namespace(
            &source,
            &target,
            b"",
            b"",
            Some(b"red"),
            false
        ));
        assert!(share_namespace(&source, &target, b"", b"", None, true));
        assert!(share_namespace(
            &source,
            &[member(b, None)],
            b"",
            b"",
            None,
            false
        ));
        assert!(!share_namespace(
            &source,
            &[member(c"other", None)],
            b"",
            b"",
            None,
            true
        ));
    }

    #[test]
    fn declared_and_effective_keys_have_distinct_missing_namespace_markers() {
        let node = Node {
            id: 1,
            label: c"Resource".as_ptr(),
            name: c"Service/api".as_ptr(),
            qualified_name: c"repo.api::core/Service/dev/api".as_ptr(),
            file_path: c"api.yaml".as_ptr(),
            start_line: 1,
            end_line: 5,
            properties_json: c"{\"obj_key\":\"core/Service/?/api\"}".as_ptr(),
        };
        assert_eq!(node.namespace(false), b"dev");
        assert_eq!(node.namespace(true), b"");
        assert_eq!(namespace_after_kind(b"core/Service/-/api", &[b"-"]), b"");
        assert_eq!(namespace_after_kind(b"core/Node/_/db", &[b"?", b"_"]), b"");
    }
}
