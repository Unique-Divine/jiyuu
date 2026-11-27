//! Stable architecture grouping and result selection for the native projection.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, c_char, c_int, c_void};
use std::mem::size_of;
use std::ops::{Deref, DerefMut};
use std::ptr::null_mut;
use std::slice;

use rusqlite::ffi::{
    SQLITE_OK, sqlite3, sqlite3_exec, sqlite3_get_autocommit,
    sqlite3_progress_handler,
};

const ERROR: c_int = -1;
const TEXT_LIMIT: c_int = -4;
const MAX_GROUPS: usize = 64;
const OVERVIEW_COMPONENTS: usize = 4096;
const GROUP_ENTRIES: usize = 16;
const CLUSTER_PASSES: usize = 6;
const GROUP_ATOMS: usize = 32;

// Edge type numbers are positions in the native projection vocabulary.
const CALLS: c_int = 1;
const IMPORTS: c_int = 5;
const USAGE: c_int = 11;
const DEFINES: c_int = 14;
const DEFINES_METHOD: c_int = 15;
const CONTAINS: c_int = 16;
const HANDLES: c_int = 19;

// These layouts mirror store/architecture_projection_ffi.h.
#[repr(C)]
struct Node {
    id: i64,
    name: *const c_char,
    qn: *const c_char,
    file: *const c_char,
    label: *const c_char,
    line: c_int,
    end: c_int,
    component: c_int,
    degree: c_int,
    entry: bool,
    handler: bool,
    route: bool,
    callable: bool,
    structural: bool,
    declared: bool,
    test: bool,
    test_property: bool,
}

#[repr(C)]
struct Edge {
    id: i64,
    source: c_int,
    target: c_int,
    kind: c_int,
}

impl Edge {
    fn endpoints(&self) -> (usize, usize) {
        (self.source as usize, self.target as usize)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Component {
    root: c_int,
    members: c_int,
    files: c_int,
    atoms: c_int,
    representative: c_int,
    degree: c_int,
    displayed: c_int,
    overview_group: c_int,
    listed: bool,
}

#[repr(C)]
struct GroupRow {
    id: *const c_char,
    label: *const c_char,
    test: bool,
    components: c_int,
    members: c_int,
    files: c_int,
}

#[repr(C)]
struct Partition {
    nodes: *mut Node,
    edges: *const Edge,
    components: *mut Component,
    node_count: c_int,
    edge_count: c_int,
    component_count: c_int,
    accounted: c_int,
    grouping_depth: c_int,
    relationship_edges: c_int,
    text_bytes: usize,
    text_budget: usize,
}

#[repr(C)]
struct Dependency {
    source: c_int,
    target: c_int,
    kind: c_int,
    edge: c_int,
}

#[repr(C)]
struct PathCandidate {
    node: c_int,
    first_hop: c_int,
    depth: c_int,
    crossings: c_int,
}

type StopFn = unsafe extern "C" fn(*mut c_void) -> bool;
type StoreGroupFn =
    unsafe extern "C" fn(*mut c_void, *const GroupRow, usize, usize) -> bool;
type RunFn = unsafe extern "C" fn(*mut c_void) -> c_int;
type ProjectionResult<T> = Result<T, c_int>;

struct Stop {
    context: *mut c_void,
    poll: StopFn,
}

impl Stop {
    fn stopped(&self) -> bool {
        // SAFETY: The request owns the callback context until the ABI call returns.
        unsafe { (self.poll)(self.context) }
    }

    fn checkpoint(&self, index: usize) -> ProjectionResult<()> {
        if index.is_multiple_of(256) && self.stopped() {
            return Err(ERROR);
        }
        Ok(())
    }
}

unsafe extern "C" fn sqlite_progress(state: *mut c_void) -> c_int {
    // SAFETY: The progress handler is installed only while this Stop is live.
    c_int::from(unsafe { &*state.cast::<Stop>() }.stopped())
}

struct Snapshot {
    db: *mut sqlite3,
    owned: bool,
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        // SAFETY: The caller lends the connection through the entire request.
        unsafe {
            sqlite3_progress_handler(self.db, 0, None, null_mut());
            if self.owned {
                sqlite3_exec(
                    self.db,
                    c"ROLLBACK".as_ptr(),
                    None,
                    null_mut(),
                    null_mut(),
                );
            }
        }
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_architecture_snapshot(
    db: *mut sqlite3,
    context: *mut c_void,
    stop: StopFn,
    run: RunFn,
) -> c_int {
    // SAFETY: C lends its live, exclusively used SQLite connection for both
    // callbacks. The run callback finalizes all statements before returning.
    unsafe {
        let progress = Stop {
            context,
            poll: stop,
        };
        let autocommit = sqlite3_get_autocommit(db) != 0;
        sqlite3_progress_handler(
            db,
            1000,
            Some(sqlite_progress),
            std::ptr::from_ref(&progress).cast_mut().cast(),
        );
        let mut snapshot = Snapshot { db, owned: false };
        if autocommit {
            if sqlite3_exec(db, c"BEGIN".as_ptr(), None, null_mut(), null_mut())
                != SQLITE_OK
            {
                return ERROR;
            }
            snapshot.owned = true;
        }
        run(context)
    }
}

unsafe extern "C" {
    fn mori_architecture_scratch_add(bytes: usize);
    fn mori_architecture_scratch_remove(bytes: usize);
    fn mori_architecture_directory_written(directory: *const u8, length: usize);
}

#[derive(Default)]
struct ScratchBytes(usize);

impl ScratchBytes {
    fn add(&mut self, bytes: usize) {
        if bytes != 0 {
            self.0 += bytes;
            // SAFETY: The reservation succeeded and stays on this request thread.
            unsafe { mori_architecture_scratch_add(bytes) };
        }
    }

    // HashMap and HashSet expose usable capacity, so this charge covers the
    // entry storage at that capacity. Spare buckets and control bytes are private.
    fn add_table<T>(&mut self, capacity: usize) {
        self.add(capacity * size_of::<T>());
    }
}

impl Drop for ScratchBytes {
    fn drop(&mut self) {
        if self.0 != 0 {
            // SAFETY: Each byte was charged once on the current request thread.
            unsafe { mori_architecture_scratch_remove(self.0) };
        }
    }
}

struct ScratchVec<T> {
    values: Vec<T>,
    _charge: ScratchBytes,
}

impl<T> Deref for ScratchVec<T> {
    type Target = Vec<T>;

    fn deref(&self) -> &Self::Target {
        &self.values
    }
}

impl<T> DerefMut for ScratchVec<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.values
    }
}

fn reserved<T>(capacity: usize) -> ProjectionResult<ScratchVec<T>> {
    let mut values = Vec::new();
    values.try_reserve_exact(capacity).map_err(|_| ERROR)?;
    let mut charge = ScratchBytes::default();
    charge.add(values.capacity() * size_of::<T>());
    // All writers fit in the initial reservation.
    Ok(ScratchVec {
        values,
        _charge: charge,
    })
}

fn filled<T: Clone>(length: usize, value: T) -> ProjectionResult<ScratchVec<T>> {
    let mut values = reserved(length)?;
    values.resize(length, value);
    Ok(values)
}

#[derive(Clone, Copy)]
struct NodeText<'a> {
    label: &'a [u8],
    file: &'a [u8],
    qn: &'a [u8],
}

fn separator(byte: u8) -> bool {
    matches!(byte, b'/' | b'\\')
}

fn normalized_separator(byte: u8) -> u8 {
    if byte == b'\\' { b'/' } else { byte }
}

fn test_path(path: &[u8], indexed: bool) -> bool {
    let mut nested = false;
    let mut base = path;
    for (index, segment) in path.split(|&byte| separator(byte)).enumerate() {
        let singular = segment == b"test";
        if (singular && index == 0) || matches!(segment, b"tests" | b"__tests__")
        {
            return true;
        }
        nested |= singular;
        base = segment;
    }
    // A nested Python test/ can contain a shipped package such as django/test.
    // Its directory alone must not supply the test role, including via is_test.
    let python = base.len() > 3 && base.ends_with(b".py");
    if (python && matches!(base, b"tests.py" | b"conftest.py"))
        || (!python && nested)
    {
        return true;
    }
    (indexed && !(python && nested))
        || base.starts_with(b"test_")
        || [b"_test.".as_slice(), b".test.", b".spec."]
            .iter()
            .any(|needle| {
                base.windows(needle.len()).any(|window| window == *needle)
            })
}

fn classify(
    nodes: &mut [Node],
    text: &[NodeText<'_>],
    edges: &[Edge],
) -> ProjectionResult<c_int> {
    for (node, text) in nodes.iter_mut().zip(text) {
        node.test = test_path(text.file, node.test_property);
        node.structural = matches!(text.label, b"Project" | b"Folder");
        node.declared = matches!(
            text.label,
            b"Module"
                | b"Package"
                | b"Namespace"
                | b"Class"
                | b"Struct"
                | b"Interface"
        );
        node.route = text.label == b"Route";
        node.callable = matches!(text.label, b"Function" | b"Method");
    }

    // HANDLES points from handler to route. Exclude handlers of routes
    // registered exclusively by test code, including test-client calls.
    const NON_TEST_REGISTRAR: u8 = 1;
    const TEST_REGISTRAR: u8 = 2;
    let mut registrars = filled(nodes.len(), 0u8)?;
    let mut relationship_edges = 0;
    for edge in edges {
        let (source, target) = edge.endpoints();
        if dependency(edge.kind) {
            nodes[source].degree += 1;
            nodes[target].degree += 1;
            relationship_edges += 1;
        }
        if edge.kind == CALLS && nodes[target].route {
            registrars[target] |= if nodes[source].test {
                TEST_REGISTRAR
            } else {
                NON_TEST_REGISTRAR
            };
        }
    }
    for edge in edges {
        let (source, target) = edge.endpoints();
        if edge.kind == HANDLES
            && nodes[target].route
            && registrars[target] != TEST_REGISTRAR
        {
            let handler = &mut nodes[source];
            handler.handler |=
                handler.callable && !handler.test && !handler.entry;
        }
    }
    Ok(relationship_edges)
}

fn ownership(
    nodes: &[Node],
    text: &[NodeText<'_>],
    edges: &[Edge],
    stop: &Stop,
) -> ProjectionResult<ScratchVec<Option<usize>>> {
    let mut table_bytes = ScratchBytes::default();
    let mut declarations = HashMap::new();
    let mut files = HashMap::new();
    declarations.try_reserve(nodes.len()).map_err(|_| ERROR)?;
    table_bytes.add_table::<(&[u8], usize)>(declarations.capacity());
    files.try_reserve(nodes.len()).map_err(|_| ERROR)?;
    table_bytes.add_table::<(&[u8], usize)>(files.capacity());
    for (index, (node, text_row)) in nodes.iter().zip(text).enumerate() {
        if node.declared {
            declarations.insert(text_row.qn, index);
        }
        if !text_row.file.is_empty()
            && matches!(text_row.label, b"File" | b"Module")
        {
            let prior = files.entry(text_row.file).or_insert(index);
            if text_row.label == b"Module" && text[*prior].label != b"Module" {
                *prior = index;
            }
        }
    }

    let mut atoms = reserved(nodes.len())?;
    for (index, (node, text)) in nodes.iter().zip(text).enumerate() {
        stop.checkpoint(index)?;
        if node.structural {
            atoms.push(None);
            continue;
        }
        if node.declared {
            atoms.push(Some(index));
            continue;
        }
        let file = files.get(text.file).copied();
        let mut atom = file.unwrap_or(index);
        let mut qn = text.qn;
        // Search for the innermost indexed declaration, up to 64 segments.
        for _ in 0..64 {
            let Some(dot) = qn.iter().rposition(|&byte| byte == b'.') else {
                break;
            };
            qn = &qn[..dot];
            if let Some(&owner) = declarations.get(qn) {
                atom = owner;
                break;
            }
        }
        if node.test != nodes[atom].test {
            atom = file
                .filter(|&file| node.test == nodes[file].test)
                .unwrap_or(index);
        }
        atoms.push(Some(atom));
    }

    for edge in edges {
        let (source, target) = edge.endpoints();
        if matches!(edge.kind, DEFINES_METHOD | DEFINES | CONTAINS)
            && nodes[source].declared
            && !nodes[target].structural
            && !nodes[target].declared
            && nodes[source].test == nodes[target].test
        {
            atoms[target] = Some(source);
        }
    }
    Ok(atoms)
}

fn cluster(
    nodes: &[Node],
    text: &[NodeText<'_>],
    edges: &[Edge],
    atoms: &[Option<usize>],
    stop: &Stop,
) -> ProjectionResult<ScratchVec<usize>> {
    let length = nodes.len();
    let mut groups = reserved(length)?;
    groups.extend(0..length);
    let mut sizes = filled(length, 0)?;
    for &atom in atoms.iter().flatten() {
        sizes[atom] = 1;
    }

    let mut pairs = reserved(edges.len() * 2)?;
    for edge in edges {
        let (source, target) = edge.endpoints();
        let (Some(a), Some(b)) = (atoms[source], atoms[target]) else {
            continue;
        };
        if a != b
            && matches!(edge.kind, CALLS | IMPORTS | USAGE)
            && nodes[a].test == nodes[b].test
        {
            pairs.extend([(a, b), (b, a)]);
        }
    }
    pairs.sort_unstable();
    let mut offsets = filled(length + 1, 0)?;
    for &(source, _) in pairs.iter() {
        offsets[source + 1] += 1;
    }
    for i in 0..length {
        offsets[i + 1] += offsets[i];
    }

    let mut scores = filled(length, 0usize)?;
    let mut touched = reserved(length)?;
    for _ in 0..CLUSTER_PASSES {
        let mut changed = false;
        for i in 0..length {
            stop.checkpoint(i)?;
            let current = groups[i];
            let neighbors = &pairs[offsets[i]..offsets[i + 1]];
            if sizes[current] == 0
                || neighbors.is_empty()
                || matches!(text[i].label, b"Package" | b"Namespace")
            {
                continue;
            }
            for &(_, target) in neighbors {
                let group = groups[target];
                if scores[group] == 0 {
                    touched.push(group);
                }
                scores[group] += 1;
            }
            // Equal scores choose the smallest group index. Update in node ID
            // order so subsequent nodes in the sweep see each accepted move.
            let mut best = current;
            let mut best_score = 0;
            for &group in touched.iter() {
                if (scores[group], Reverse(group)) > (best_score, Reverse(best))
                {
                    best = group;
                    best_score = scores[group];
                }
            }
            if best != current
                && best_score >= 3
                && best_score > scores[current]
                && best_score * 100 >= neighbors.len() * 55
                && sizes[best] < GROUP_ATOMS
            {
                sizes[current] -= 1;
                sizes[best] += 1;
                groups[i] = best;
                changed = true;
            }
            for group in touched.drain(..) {
                scores[group] = 0;
            }
        }
        if !changed {
            break;
        }
    }
    Ok(groups)
}

fn collect_components(
    nodes: &mut [Node],
    text: &[NodeText<'_>],
    atoms: &[Option<usize>],
    groups: &[usize],
    components: &mut [Component],
) -> ProjectionResult<(usize, usize)> {
    let mut component_of_group = filled(nodes.len(), None)?;
    let mut table_bytes = ScratchBytes::default();
    let mut seen = HashSet::new();
    seen.try_reserve(nodes.len()).map_err(|_| ERROR)?;
    table_bytes.add_table::<(usize, &[u8])>(seen.capacity());
    let mut component_count = 0;
    let mut accounted = 0;
    for (i, &atom) in atoms.iter().enumerate() {
        let Some(atom) = atom else { continue };
        accounted += 1;
        let root = groups[atom];
        let index = *component_of_group[root].get_or_insert_with(|| {
            let index = component_count;
            components[index] = Component {
                root: root as c_int,
                representative: i as c_int,
                displayed: -1,
                ..Component::default()
            };
            component_count += 1;
            index
        });
        nodes[i].component = index as c_int;
        let part = &mut components[index];
        part.members += 1;
        part.degree += nodes[i].degree;
        part.atoms += c_int::from(atom == i);
        let rep = &nodes[part.representative as usize];
        if (nodes[i].declared, nodes[i].degree) > (rep.declared, rep.degree) {
            part.representative = i as c_int;
        }
        if !text[i].file.is_empty() && seen.insert((index, text[i].file)) {
            part.files += 1;
        }
    }
    Ok((component_count, accounted))
}

fn charge(remaining: &mut usize, bytes: usize) -> ProjectionResult<()> {
    if bytes > *remaining {
        return Err(TEXT_LIMIT);
    }
    *remaining -= bytes;
    Ok(())
}

fn common_directory(directory: &mut Vec<u8>, file_directory: &[u8]) {
    let mut common = directory
        .iter()
        .zip(file_directory)
        .take_while(|&(&old, &new)| old == normalized_separator(new))
        .count();
    let boundary = (common == directory.len()
        && (common == file_directory.len()
            || separator(file_directory[common])))
        || (common == file_directory.len()
            && directory.get(common) == Some(&b'/'));
    if !boundary {
        while common > 0 && directory[common - 1] != b'/' {
            common -= 1;
        }
    }
    if common > 0 && directory[common - 1] == b'/' {
        common -= 1;
    }
    directory.truncate(common);
}

struct Group<'a> {
    directory: Option<&'a [u8]>,
    test: bool,
    components: c_int,
    members: c_int,
    files: c_int,
}

impl Group<'_> {
    fn prefix(&self) -> &[u8] {
        match (self.directory.is_some(), self.test) {
            (true, true) => b"directory:test:",
            (true, false) => b"directory:non_test:",
            (false, true) => b"unlocated:test:",
            (false, false) => b"unlocated:non_test:",
        }
    }

    fn path(&self) -> &[u8] {
        self.directory.unwrap_or(b"@no-source")
    }

    fn label(&self) -> &[u8] {
        match self.path() {
            b"" => b"(root)",
            path => path,
        }
    }

    fn store(
        &self,
        context: *mut c_void,
        store_group: StoreGroupFn,
    ) -> ProjectionResult<()> {
        let mut id = reserved(self.prefix().len() + self.path().len() + 1)?;
        id.extend_from_slice(self.prefix());
        id.extend_from_slice(self.path());
        id.push(0);
        let mut label = reserved(self.label().len() + 1)?;
        label.extend_from_slice(self.label());
        label.push(0);
        let row = GroupRow {
            id: id.as_ptr().cast(),
            label: label.as_ptr().cast(),
            test: self.test,
            components: self.components,
            members: self.members,
            files: self.files,
        };
        // SAFETY: C copies both buffers while they are live. Lengths include NUL.
        if !unsafe { store_group(context, &row, id.len(), label.len()) } {
            return Err(ERROR);
        }
        Ok(())
    }
}

fn overview_groups(
    nodes: &[Node],
    text: &[NodeText<'_>],
    components: &mut [Component],
    mut text_remaining: usize,
    grouping_depth: &mut c_int,
    stop: &Stop,
    store_group: StoreGroupFn,
) -> ProjectionResult<()> {
    // Directories are temporary. Charge the full first path for each component,
    // and charge group text on every coarsening pass, as the native budget did.
    let mut directory_bytes = ScratchBytes::default();
    let mut directories = filled(components.len(), None::<Vec<u8>>)?;
    for (i, (node, text)) in nodes.iter().zip(text).enumerate() {
        if node.component < 0 || text.file.is_empty() {
            continue;
        }
        let end = text
            .file
            .iter()
            .rposition(|&byte| separator(byte))
            .unwrap_or(0);
        let path = &text.file[..end];
        let directory = &mut directories[node.component as usize];
        if let Some(directory) = directory {
            common_directory(directory, path);
        } else {
            charge(&mut text_remaining, text.file.len() + 1)?;
            let mut normalized = Vec::new();
            normalized
                .try_reserve_exact(path.len())
                .map_err(|_| ERROR)?;
            directory_bytes.add(normalized.capacity());
            normalized.extend(path.iter().copied().map(normalized_separator));
            // SAFETY: The normalized prefix is initialized. Rust slice equality
            // can pass it to MemorySanitizer's instrumented memcmp.
            if !normalized.is_empty() {
                unsafe {
                    mori_architecture_directory_written(
                        normalized.as_ptr(),
                        normalized.len(),
                    );
                }
            }
            *directory = Some(normalized);
        }
        stop.checkpoint(i)?;
    }

    let mut groups: ScratchVec<Group<'_>> = reserved(MAX_GROUPS)?;
    for depth in (0..=2).rev() {
        groups.clear();
        let mut retry = false;
        for (i, part) in components.iter_mut().enumerate() {
            let directory = directories[i].as_deref().map(|path| {
                let length = if depth == 0 {
                    0
                } else {
                    path.iter()
                        .enumerate()
                        .filter(|&(_, &byte)| byte == b'/')
                        .nth(depth - 1)
                        .map_or(path.len(), |(index, _)| index)
                };
                &path[..length]
            });
            let test = nodes[part.root as usize].test;
            let index = if let Some(index) = groups.iter().position(|group| {
                group.directory == directory && group.test == test
            }) {
                index
            } else {
                if groups.len() == MAX_GROUPS {
                    retry = true;
                    break;
                }
                let group = Group {
                    directory,
                    test,
                    components: 0,
                    members: 0,
                    files: 0,
                };
                charge(
                    &mut text_remaining,
                    group.prefix().len() + group.path().len() + 1,
                )?;
                charge(&mut text_remaining, group.label().len() + 1)?;
                groups.push(group);
                groups.len() - 1
            };
            part.overview_group = index as c_int;
            groups[index].components += 1;
            groups[index].members += part.members;
            stop.checkpoint(i)?;
        }
        if !retry {
            *grouping_depth = depth as c_int;
            break;
        }
    }

    let mut table_bytes = ScratchBytes::default();
    let mut seen = HashSet::new();
    seen.try_reserve(nodes.len()).map_err(|_| ERROR)?;
    table_bytes.add_table::<(usize, &[u8])>(seen.capacity());
    for (i, (node, text)) in nodes.iter().zip(text).enumerate() {
        if node.component < 0 || text.file.is_empty() {
            continue;
        }
        let group = components[node.component as usize].overview_group as usize;
        if seen.insert((group, text.file)) {
            groups[group].files += 1;
        }
        stop.checkpoint(i)?;
    }
    for group in groups.iter() {
        group.store(stop.context, store_group)?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_architecture_partition(
    input: *mut Partition,
    context: *mut c_void,
    stop: StopFn,
    store_group: StoreGroupFn,
) -> c_int {
    let result = (|| {
        // SAFETY: C provides distinct arrays of the given lengths, and the
        // callbacks do not touch them. Only successful graph loads reach here.
        let input = unsafe { &mut *input };
        let node_count = input.node_count as usize;
        let nodes =
            unsafe { slice::from_raw_parts_mut(input.nodes, node_count) };
        let edges = unsafe {
            slice::from_raw_parts(input.edges, input.edge_count as usize)
        };
        let components =
            unsafe { slice::from_raw_parts_mut(input.components, node_count) };
        let mut text = reserved(node_count)?;
        for node in nodes.iter() {
            // SAFETY: The request owns these immutable, terminated strings.
            text.push(unsafe {
                NodeText {
                    label: CStr::from_ptr(node.label).to_bytes(),
                    file: CStr::from_ptr(node.file).to_bytes(),
                    qn: CStr::from_ptr(node.qn).to_bytes(),
                }
            });
        }
        let stop = Stop {
            context,
            poll: stop,
        };
        input.relationship_edges = classify(nodes, &text, edges)?;
        let atoms = ownership(nodes, &text, edges, &stop)?;
        let groups = cluster(nodes, &text, edges, &atoms, &stop)?;
        let (count, accounted) =
            collect_components(nodes, &text, &atoms, &groups, components)?;
        input.component_count = count as c_int;
        input.accounted = accounted as c_int;
        overview_groups(
            nodes,
            &text,
            &mut components[..count],
            input.text_budget - input.text_bytes,
            &mut input.grouping_depth,
            &stop,
            store_group,
        )
    })();
    result.map_or_else(|status| status, |()| 0)
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_architecture_rank(
    components: *mut Component,
    count: c_int,
    limit: c_int,
) -> c_int {
    let count = count as usize;
    // SAFETY: C lends the populated component array exclusively for ranking.
    let components = unsafe { slice::from_raw_parts_mut(components, count) };
    let Ok(mut rank) = reserved(count) else {
        return ERROR;
    };
    rank.extend(0..count);
    rank.sort_unstable_by_key(|&index| {
        let part = &components[index];
        (Reverse(part.members), Reverse(part.degree), index)
    });
    let shown = (limit as usize).min(count);
    for (pick, &index) in rank.iter().enumerate() {
        components[index].displayed =
            if pick < shown { pick as c_int } else { -1 };
        components[index].listed = false;
    }

    // Include displayed components first, then each group's share, then fill
    // the remaining overview slots by global rank. C emits in component order.
    let mut group_entries = [0; MAX_GROUPS];
    let mut listed = 0;
    for pass in 0..3 {
        for &index in rank.iter() {
            if listed == OVERVIEW_COMPONENTS {
                return shown as c_int;
            }
            let part = &mut components[index];
            let entries = &mut group_entries[part.overview_group as usize];
            if part.listed
                || (pass == 0 && part.displayed < 0)
                || (pass == 1 && *entries >= GROUP_ENTRIES)
            {
                continue;
            }
            part.listed = true;
            *entries += 1;
            listed += 1;
        }
    }
    shown as c_int
}

fn dependency(kind: c_int) -> bool {
    (1..DEFINES).contains(&kind) || kind > CONTAINS
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_architecture_dependencies(
    nodes: *const Node,
    edges: *const Edge,
    edge_count: c_int,
    components: *const Component,
    overview: bool,
    out: *mut Dependency,
) -> c_int {
    // SAFETY: As well as the input arrays, C provides room for one result per
    // edge. Loaded endpoints are valid node indices, and assigned component
    // indices refer to the corresponding component array.
    unsafe {
        let mut count = 0;
        for (index, edge) in slice::from_raw_parts(edges, edge_count as usize)
            .iter()
            .enumerate()
        {
            let (source, target) = edge.endpoints();
            let source = (*nodes.add(source)).component;
            let target = (*nodes.add(target)).component;
            if source < 0
                || target < 0
                || (!overview && source == target)
                || !dependency(edge.kind)
            {
                continue;
            }
            let group =
                |component| (*components.add(component as usize)).overview_group;
            out.add(count).write(Dependency {
                source: if overview { group(source) } else { source },
                target: if overview { group(target) } else { target },
                kind: edge.kind,
                edge: index as c_int,
            });
            count += 1;
        }
        slice::from_raw_parts_mut(out, count).sort_unstable_by_key(|dep| {
            (dep.source, dep.target, dep.kind, dep.edge)
        });
        count as c_int
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_architecture_sort_paths(
    candidates: *mut PathCandidate,
    count: c_int,
) {
    // SAFETY: C lends its initialized candidates exclusively for sorting.
    unsafe { slice::from_raw_parts_mut(candidates, count as usize) }
        .sort_unstable_by_key(|path| {
            (Reverse(path.depth), Reverse(path.crossings), path.node)
        });
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_architecture_select_paths(
    candidates: *const PathCandidate,
    count: c_int,
    limit: c_int,
    first_hops: *mut c_int,
    node_count: c_int,
    out: *mut c_int,
) -> c_int {
    // SAFETY: C owns the disjoint buffers. Every first hop is a graph node index,
    // and there is room for limit results.
    let candidates =
        unsafe { slice::from_raw_parts(candidates, count as usize) };
    let first_hops =
        unsafe { slice::from_raw_parts_mut(first_hops, node_count as usize) };
    let limit = limit as usize;
    first_hops.fill(-1);
    let mut selected = 0;
    for (index, path) in candidates.iter().enumerate() {
        if selected == limit {
            return selected as c_int;
        }
        let first = &mut first_hops[path.first_hop as usize];
        if *first < 0 {
            *first = index as c_int;
            unsafe { out.add(selected).write(path.node) };
            selected += 1;
        }
    }
    for (index, path) in candidates.iter().enumerate() {
        if selected == limit {
            break;
        }
        if first_hops[path.first_hop as usize] != index as c_int {
            unsafe { out.add(selected).write(path.node) };
            selected += 1;
        }
    }
    selected as c_int
}
