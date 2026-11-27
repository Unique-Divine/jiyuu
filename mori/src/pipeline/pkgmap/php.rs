//! PHP namespace and Composer resolution, plus the binding evidence stored
//! on PHP File nodes and IMPORTS edges.

use super::graph::*;
use super::json::{Document, Item, Value, Writer};
use super::resolve::{
    contains, import_node, namespace_nodes, replace, split_last,
};
use super::{
    HashTable, PATH_LIMIT, c_bytes, cbm_ffi_mark_written,
    cbm_pkgmap_tracked_free,
};
use crate::native_graph_buffer::cbm_gbuf_node_set_properties_json;
use crate::native_tree::RawNode;
use std::ffi::{CStr, c_char, c_int, c_uint, c_void};
use std::mem::{ManuallyDrop, MaybeUninit};
use std::ptr::{NonNull, null, null_mut};

const CLASS: c_uint = 0;
const FUNCTION: c_uint = 1;
const CONSTANT: c_uint = 2;
const COMPLETE: &[u8] = b"{\"php_imports_complete\":1,";
const COMPLETE_KEY: &CStr = c"php_imports_complete";
const BINDINGS: &CStr = c"php_import_bindings";
const LOCAL: &CStr = c"local_name";
const MODULE: &CStr = c"module_path";
const TARGET: &CStr = c"target";
const KIND: &CStr = c"kind";
const DEFAULT: &CStr = c"is_default";

fn ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80
}

fn unqualified(path: &[u8]) -> &[u8] {
    path.strip_prefix(b"\\").unwrap_or(path)
}

fn class_label(node: &Node) -> bool {
    matches!(
        node.label(),
        Some(b"Class" | b"Interface" | b"Trait" | b"Enum")
    )
}

#[derive(PartialEq, Eq)]
pub(super) enum UseShape {
    None,
    Global,
    Namespaced,
}

pub(super) fn use_shape(lang: Language, path: &[u8]) -> UseShape {
    let name = unqualified(path);
    if lang != Language::Php
        || name.is_empty()
        || !name.iter().all(|&byte| byte == b'\\' || ident(byte))
    {
        return UseShape::None;
    }
    if name.contains(&b'\\') {
        UseShape::Namespaced
    } else {
        UseShape::Global
    }
}

fn psr4_prefixes(name: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
    // A prefix must end before the final byte, leaving a class name.
    name.iter()
        .enumerate()
        .rev()
        .filter(|&(index, &byte)| byte == b'\\' && index + 1 < name.len())
        .map(|(index, _)| name.split_at(index + 1))
}

pub(super) fn psr4_class<'a>(
    graph: &'a Buffer,
    source: Option<&[u8]>,
    path: &[u8],
    kind: c_uint,
) -> Option<Option<&'a Node>> {
    let map = unsafe { pkgmap() }?;
    let name = unqualified(path);
    if kind != CLASS || name.len() > PATH_LIMIT {
        return None;
    }
    let mut covered = false;
    for (prefix, rest) in psr4_prefixes(name) {
        let Some(dir) = lookup(map, prefix) else {
            continue;
        };
        covered = true;
        let slash: &[u8] = if dir.is_empty() { b"" } else { b"/" };
        let relative = [dir, slash, rest, b".php"].concat();
        if relative.len() > PATH_LIMIT {
            continue;
        }
        let relative = replace(&relative, b'\\', b'/');
        let file = nodes(graph.name(leaf(&relative))).find(|node| {
            node.label() == Some(b"File")
                && node.path() == Some(relative.as_slice())
        });
        if let Some(file) = file.filter(|file| file.other_than(source)) {
            return Some(Some(file));
        }
    }
    covered.then_some(None)
}

#[repr(C)]
pub(crate) enum Member {
    NotCovered,
    Resolved,
    Unresolved,
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn cbm_pipeline_psr4_member_qn(
    graph: *const Buffer,
    path: *const c_char,
    member: *const c_char,
    out: *mut *const c_char,
) -> Member {
    unsafe {
        out.write(null());
        cbm_ffi_mark_written(out.cast(), std::mem::size_of::<*const c_char>());
    };
    let (Some(graph), Some(path), Some(member)) =
        (unsafe { (graph.as_ref(), c_bytes(path), c_bytes(member)) })
    else {
        return Member::NotCovered;
    };
    if member.is_empty() {
        return Member::NotCovered;
    }
    let Some(file) = psr4_class(graph, None, path, CLASS) else {
        return Member::NotCovered;
    };
    let Some(file) = file.and_then(Node::path) else {
        return Member::Unresolved;
    };
    let class = split_last(path, b'\\').map_or(path, |(_, leaf)| leaf);
    if let Some(node) = nodes(graph.name(class)).find(|node| {
        class_label(node) && node.qn().is_some() && node.path() == Some(file)
    }) {
        let qn = [node.qn().unwrap(), b".", member].concat();
        if qn.len() <= PATH_LIMIT
            && let Some(qn) = graph
                .qn(&qn)
                .map(|node| node.qualified_name)
                .filter(|qn| !qn.is_null())
        {
            unsafe { out.write(qn) };
            return Member::Resolved;
        }
    }
    Member::Unresolved
}

pub(super) fn declared_use<'a>(
    res: &Resolution<'a>,
    path: &[u8],
    namespaces: &HashTable,
) -> Option<&'a Node> {
    let name = unqualified(path);
    if name.len() > PATH_LIMIT {
        return None;
    }
    let key = replace(name, b'\\', b'.');
    let (parent, leaf) = split_last(&key, b'.')?;
    let files = |bucket| {
        namespace_nodes(res.graph, bucket).filter(|node| {
            node.label() == Some(b"File")
                && node.php()
                && node.other_than(res.source_qn)
        })
    };
    let parent = lookup(namespaces, parent);
    if parent.is_some() {
        let hits = res.graph.name(leaf);
        if let Some(found) = files(parent).find(|file| {
            file.path().is_some()
                && nodes(hits).any(|hit| {
                    hit.label().is_some_and(|label| label != b"File")
                        && hit.path() == file.path()
                })
        }) {
            return Some(found);
        }
    }
    files(lookup(namespaces, &key))
        .next()
        .or_else(|| files(parent).next())
}

fn known_absent(
    res: &Resolution<'_>,
    lang: Language,
    imp: &Import,
    namespaces: Option<&HashTable>,
) -> bool {
    let (Some(map), Some(path), Some(local)) =
        (namespaces, imp.path(), imp.local())
    else {
        return false;
    };
    if local.is_empty() || !matches!(imp.kind, CLASS | FUNCTION | CONSTANT) {
        return false;
    }
    let name = unqualified(path);
    match use_shape(lang, path) {
        UseShape::None => false,
        UseShape::Global => {
            imp.kind == CLASS
                && name == local
                && res.graph.name(name).is_some_and(|hits| {
                    !nodes(Some(hits)).any(|node| {
                        node.php()
                            && (class_label(node)
                                || matches!(
                                    node.label(),
                                    Some(b"File" | b"Module")
                                ))
                    })
                })
        }
        UseShape::Namespaced => {
            if name.len() > PATH_LIMIT
                || name.starts_with(b"\\")
                || name.ends_with(b"\\")
                || contains(name, b"\\\\")
            {
                return false;
            }
            if imp.kind == CLASS
                && unsafe { pkgmap() }.is_some_and(|packages| {
                    lookup(packages, b"").is_some()
                        || psr4_prefixes(name).any(|(prefix, _)| {
                            lookup(packages, prefix).is_some()
                        })
                })
            {
                return false;
            }
            let key = replace(name, b'\\', b'.');
            lookup(map, &key).is_none()
                && split_last(&key, b'.')
                    .is_some_and(|(parent, _)| lookup(map, parent).is_none())
        }
    }
}

fn string_eq(value: Value<'_>, text: Option<&[u8]>) -> bool {
    text.map_or_else(|| value.is_null(), |text| value.equals_string(text))
}

fn same_binding(value: Value<'_>, import: &Import) -> bool {
    value.pairs().count() == 5
        && value.get(KIND).integer() == Some(import.kind as i32)
        && value.get(DEFAULT).boolean() == Some(import.is_default)
        && string_eq(value.get(LOCAL), import.local())
        && string_eq(value.get(MODULE), import.path())
}

fn binding<'a>(
    doc: &'a Writer,
    import: &Import,
    target: Option<&[u8]>,
) -> Option<Item<'a>> {
    let result = doc.object()?;
    doc.put(result, LOCAL.to_bytes(), doc.string(import.local())?)?;
    doc.put(result, MODULE.to_bytes(), doc.string(import.path())?)?;
    doc.put(
        result,
        KIND.to_bytes(),
        doc.integer(i64::from(import.kind))?,
    )?;
    doc.put(result, DEFAULT.to_bytes(), doc.boolean(import.is_default)?)?;
    doc.put(result, TARGET.to_bytes(), doc.string(target)?)?;
    Some(result)
}

struct BindingScratch<T> {
    values: NonNull<T>,
    len: usize,
}

impl<T: Copy> BindingScratch<T> {
    fn new(len: usize, value: T) -> Option<Self> {
        let size = len.max(1).checked_mul(size_of::<T>())?;
        if size > isize::MAX as usize {
            return None;
        }
        let values = NonNull::new(
            unsafe { cbm_pkgmap_binding_scratch_alloc(size) }.cast::<T>(),
        )?;
        // The tracked allocator provides storage aligned for the binding rows.
        for index in 0..len {
            unsafe { values.as_ptr().add(index).write(value) };
        }
        unsafe {
            cbm_ffi_mark_written(values.as_ptr().cast(), len * size_of::<T>())
        };
        Some(Self { values, len })
    }

    fn as_mut_slice(&mut self) -> &mut [T] {
        unsafe { std::slice::from_raw_parts_mut(self.values.as_ptr(), self.len) }
    }

    fn into_raw(self) -> *mut T {
        ManuallyDrop::new(self).values.as_ptr()
    }
}

impl<T> Drop for BindingScratch<T> {
    fn drop(&mut self) {
        unsafe { cbm_pkgmap_tracked_free(self.values.as_ptr().cast()) };
    }
}

fn binding_snapshot(
    graph: &Buffer,
    file_id: i64,
    imports: &[Import],
    json: &[u8],
) -> Option<BindingScratch<*const c_char>> {
    if !json.starts_with(COMPLETE) {
        return None;
    }
    let doc = Document::read(json)?;
    let root = doc.root();
    for field in [COMPLETE_KEY, BINDINGS] {
        if root
            .pairs()
            .filter(|(key, _)| key.equals_string(field.to_bytes()))
            .count()
            != 1
        {
            return None;
        }
    }
    let manifest = root.get(BINDINGS).array()?;
    if manifest.len() != imports.len() {
        return None;
    }
    let edges = graph.imports(file_id)?;
    let mut snapshot = BindingScratch::new(imports.len(), null())?;
    let targets = snapshot.as_mut_slice();
    let mut seen_scratch: BindingScratch<c_int> =
        BindingScratch::new(imports.len(), 0)?;
    let seen = seen_scratch.as_mut_slice();
    for ((entry, import), slot) in manifest.zip(imports).zip(targets.iter_mut())
    {
        if !same_binding(entry, import) {
            return None;
        }
        let target = entry.get(TARGET);
        if target.is_null() {
            continue;
        }
        let node = graph.qn(target.string()?)?;
        if node.id == file_id
            || !string_eq(target, node.qn())
            || node.qualified_name.is_null()
        {
            return None;
        }
        *slot = node.qualified_name;
    }

    for &edge in edges {
        let edge = unsafe { edge.as_ref()? };
        let target = graph.id(edge.target)?.qn()?;
        let props = Document::read(unsafe { c_bytes(edge.properties) }?)?;
        let obj = props.root();
        let local = obj.get(LOCAL);
        let rows = obj.get(BINDINGS).array()?;
        if obj.pairs().count() != 2
            || local.string().is_none()
            || rows.len() == 0
        {
            return None;
        }
        for row in rows {
            let mut matching = targets.iter().zip(imports).enumerate().filter(
                |(_, (qn, import))| {
                    (unsafe { c_bytes(**qn) }) == Some(target)
                        && same_binding(row, import)
                        && string_eq(local, import.local())
                        && row.get(TARGET).equals_string(target)
                },
            );
            let (index, _) = matching.next()?;
            if matching.next().is_some() || seen[index] != 0 {
                return None;
            }
            seen[index] = 1;
        }
    }
    seen.iter()
        .zip(targets)
        .all(|(seen, qn)| (*seen != 0) != qn.is_null())
        .then_some(snapshot)
}

#[repr(C)]
pub(crate) struct VendorNames {
    imports: *const Imports,
    targets: *mut *const c_char,
}

impl VendorNames {
    fn empty() -> Self {
        Self {
            imports: null(),
            targets: null_mut(),
        }
    }

    fn contains(&self, name: &[u8], kind: Option<c_uint>) -> bool {
        let Some(imports) =
            unsafe { self.imports.as_ref() }.and_then(Imports::rows)
        else {
            return false;
        };
        let end = name
            .iter()
            .position(|&byte| !ident(byte))
            .unwrap_or(name.len());
        if self.targets.is_null() || end == 0 {
            return false;
        }
        let targets =
            unsafe { std::slice::from_raw_parts(self.targets, imports.len()) };
        imports.iter().zip(targets).any(|(import, target)| {
            target.is_null()
                && import.path().is_some()
                && import.local() == Some(&name[..end])
                && kind.is_none_or(|kind| kind == import.kind)
        })
    }
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn cbm_pipeline_php_vendor_names_build(
    graph: *const Buffer,
    project: *const c_char,
    rel: *const c_char,
    result: *const FileResult,
    out: *mut VendorNames,
) {
    unsafe {
        out.write(VendorNames::empty());
        cbm_ffi_mark_written(out.cast(), std::mem::size_of::<VendorNames>());
    };
    let (Some(graph), Some(path), Some(_)) =
        (unsafe { (graph.as_ref(), c_bytes(rel), result.as_ref()) })
    else {
        return;
    };
    if language(leaf(path)) != Language::Php {
        return;
    }
    let imports = unsafe { cbm_pkgmap_imports(result) };
    let Some(rows) = (unsafe { imports.as_ref() }).and_then(Imports::rows)
    else {
        return;
    };
    let Some(qn) = Text::new(unsafe {
        cbm_pipeline_fqn_compute(project, rel, c"__file__".as_ptr())
    }) else {
        return;
    };
    let Some(file) = graph.qn(qn.bytes()) else {
        return;
    };
    let Some(targets) = unsafe { c_bytes(file.properties_json) }
        .and_then(|json| binding_snapshot(graph, file.id, rows, json))
    else {
        return;
    };
    unsafe {
        out.write(VendorNames {
            imports,
            targets: targets.into_raw(),
        });
    }
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn cbm_pipeline_php_vendor_names_free(
    value: *mut VendorNames,
) {
    if let Some(value) = unsafe { value.as_mut() } {
        unsafe { cbm_pkgmap_tracked_free(value.targets.cast()) };
        *value = VendorNames::empty();
    }
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn cbm_pipeline_php_vendor_bound(
    names: *const VendorNames,
    name: *const c_char,
) -> bool {
    unsafe { names.as_ref().zip(c_bytes(name)) }
        .is_some_and(|(names, name)| names.contains(name, Some(CLASS)))
}

fn write_bindings(
    res: &Resolution<'_>,
    file_id: i64,
    old: Option<&[u8]>,
    imports: &[Import],
    map: Option<&HashTable>,
    written: &mut c_int,
) -> Option<Text> {
    let mut target_scratch = BindingScratch::new(imports.len(), null::<Node>())?;
    let targets = target_scratch.as_mut_slice();
    let doc = Writer::new()?;
    let root = doc.object()?;
    let manifest = doc.array()?;
    doc.put(root, COMPLETE_KEY.to_bytes(), doc.integer(1)?)?;
    doc.put(root, BINDINGS.to_bytes(), manifest)?;
    if let Some(old_json) = old {
        let old_doc = Document::read(old_json)?;
        let old_root = old_doc.root();
        if !old_root.is_object() {
            return None;
        }
        for (key, value) in old_root.pairs() {
            let key = key.string()?;
            if key != COMPLETE_KEY.to_bytes() && key != BINDINGS.to_bytes() {
                doc.put(root, key, doc.copy(value)?)?;
            }
        }
    }

    let lang = res
        .modules
        .source
        .map_or(Language::Other, |path| language(leaf(path.to_bytes())));
    let mut complete = true;
    for (import, slot) in imports.iter().zip(targets.iter_mut()) {
        let target =
            import_node(res, import, map).filter(|node| node.id != file_id);
        if target.is_none() && !known_absent(res, lang, import, map) {
            complete = false;
        }
        doc.append(manifest, binding(&doc, import, target.and_then(Node::qn))?)?;
        *slot = target.map_or(null(), std::ptr::from_ref);
    }

    let matches = |index: usize, id, name| {
        // Edge insertion keeps the target node addresses stable.
        unsafe { targets[index].as_ref() }.is_some_and(|target| target.id == id)
            && imports[index].local() == Some(name)
    };
    for (index, target) in targets.iter().enumerate() {
        let Some(target) = (unsafe { target.as_ref() }) else {
            continue;
        };
        let local = imports[index].local().filter(|local| !local.is_empty())?;
        if (0..index).any(|prior| matches(prior, target.id, local)) {
            continue;
        }
        let edge = doc.object()?;
        let rows = doc.array()?;
        doc.put(edge, LOCAL.to_bytes(), doc.string(Some(local))?)?;
        doc.put(edge, BINDINGS.to_bytes(), rows)?;
        for other in index..targets.len() {
            if matches(other, target.id, local) {
                let qn = unsafe { targets[other].as_ref() }?.qn();
                doc.append(rows, binding(&doc, &imports[other], qn)?)?;
            }
        }
        let props = doc.write(edge)?;
        let result = unsafe {
            cbm_gbuf_insert_edge(
                res.graph_ptr.cast_mut().cast(),
                file_id,
                target.id,
                c"IMPORTS".as_ptr(),
                props.as_ptr(),
            )
        };
        if result <= 0 {
            return None;
        }
        *written += 1;
    }
    if complete {
        let props = doc.write(root)?;
        binding_snapshot(res.graph, file_id, imports, props.bytes())?;
        return Some(props);
    }
    None
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn cbm_pipeline_php_create_import_edges(
    ctx: *mut Context,
    result: *const FileResult,
    rel: *const c_char,
    qn: *const c_char,
    file: *const Node,
    map: *const HashTable,
) -> c_int {
    if ctx.is_null() || result.is_null() || file.is_null() {
        return 0;
    }
    let id = unsafe { (*file).id };
    let properties = unsafe { (*file).properties_json };
    let complete = unsafe { c_bytes(properties) }
        .is_some_and(|json| json.starts_with(COMPLETE));
    if complete {
        // The coordinator owns this buffer exclusively until writes finish.
        unsafe {
            properties
                .cast_mut()
                .add(COMPLETE.len() - 2)
                .write(b'0' as c_char)
        };
    }
    let Some(res) = (unsafe { Resolution::new(ctx, rel, qn) }) else {
        return 0;
    };
    let Some(imports) =
        (unsafe { cbm_pkgmap_imports(result).as_ref() }).and_then(Imports::rows)
    else {
        return 0;
    };
    let mut written = 0;
    let props = write_bindings(
        &res,
        id,
        unsafe { c_bytes(properties) },
        imports,
        unsafe { map.as_ref() },
        &mut written,
    );
    if let Some(props) = props {
        unsafe {
            cbm_gbuf_node_set_properties_json(file.cast_mut(), props.as_ptr())
        };
    }
    written
}

// Call and usage records differ in layout. The C bridge supplies the
// occurrence and tree; classification uses the same Tree-sitter traversal
// for both.
#[repr(C)]
#[derive(Default)]
struct Occurrence {
    start: u32,
    end: u32,
    origin: c_uint,
}

fn read_node(read: impl FnOnce(*mut RawNode) -> bool) -> Option<RawNode> {
    let mut node = MaybeUninit::uninit();
    if read(node.as_mut_ptr()) {
        // The C bridge initializes the node on a successful call.
        Some(unsafe { node.assume_init() })
    } else {
        None
    }
}

fn site_kind(
    result: *const FileResult,
    site: Occurrence,
    call: bool,
) -> Option<c_uint> {
    if result.is_null() || site.origin != 0 || site.end <= site.start {
        return None;
    }
    let mut source = null();
    let mut len = 0;
    let tree = unsafe { cbm_pkgmap_site_source(result, &mut source, &mut len) };
    let mut current = if tree.is_null() {
        None
    } else {
        read_node(|out| unsafe {
            cbm_pkgmap_node_at(tree, site.start, site.end - 1, out)
        })
    };
    let covers = |node: Option<RawNode>| {
        node.is_some_and(|node| {
            let range = unsafe { node.range() };
            range.start <= site.start as usize && range.end >= site.end as usize
        })
    };
    for _ in 0..16 {
        let Some(node) = current else {
            break;
        };
        match unsafe { node.kind().to_bytes() } {
            b"named_type" | b"base_clause" | b"class_interface_clause" => {
                return Some(CLASS);
            }
            b"object_creation_expression"
            | b"scoped_call_expression"
            | b"class_constant_access_expression" => {
                if call
                    || covers(read_node(|out| unsafe {
                        cbm_pkgmap_node_first_named_child(&node, out)
                    }))
                {
                    return Some(CLASS);
                }
                break;
            }
            b"function_call_expression" => {
                if call || covers(unsafe { node.field(c"function") }) {
                    return Some(FUNCTION);
                }
                break;
            }
            b"namespace_use_declaration"
            | b"method_declaration"
            | b"function_definition" => break,
            _ => {
                current = read_node(|out| unsafe {
                    cbm_pkgmap_node_parent(&node, out)
                })
            }
        }
    }
    if call
        && !source.is_null()
        && len >= 0
        && site.end <= len as u32
        && site.end - site.start > 3
    {
        let head = unsafe {
            std::slice::from_raw_parts(
                source.cast::<u8>().add(site.start as usize),
                4,
            )
        };
        if head.starts_with(b"new")
            && matches!(head[3], b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c)
        {
            return Some(CLASS);
        }
    }
    None
}

#[repr(C)]
pub(crate) struct Call {
    _opaque: [u8; 0],
}
#[repr(C)]
pub(crate) struct Usage {
    _opaque: [u8; 0],
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn cbm_pipeline_php_vendor_call(
    names: *const VendorNames,
    result: *const FileResult,
    call: *const Call,
) -> bool {
    if call.is_null() {
        return false;
    }
    let mut site = Occurrence::default();
    let name = unsafe { cbm_pkgmap_call_occurrence(call, &mut site) };
    unsafe { names.as_ref().zip(c_bytes(name)) }.is_some_and(|(names, name)| {
        let kind = if contains(name, b"::") {
            Some(CLASS)
        } else {
            site_kind(result, site, true)
        };
        names.contains(name, kind)
    })
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn cbm_pipeline_php_vendor_usage(
    names: *const VendorNames,
    result: *const FileResult,
    usage: *const Usage,
) -> bool {
    if usage.is_null() {
        return false;
    }
    let mut site = Occurrence::default();
    let name = unsafe { cbm_pkgmap_usage_occurrence(usage, &mut site) };
    unsafe { names.as_ref().zip(c_bytes(name)) }.is_some_and(|(names, name)| {
        names.contains(name, site_kind(result, site, false))
    })
}

unsafe extern "C" {
    fn cbm_pkgmap_binding_scratch_alloc(size: usize) -> *mut c_void;
    fn cbm_pkgmap_site_source(
        result: *const FileResult,
        source: *mut *const c_char,
        len: *mut c_int,
    ) -> *const c_void;
    fn cbm_pkgmap_call_occurrence(
        call: *const Call,
        site: *mut Occurrence,
    ) -> *const c_char;
    fn cbm_pkgmap_usage_occurrence(
        usage: *const Usage,
        site: *mut Occurrence,
    ) -> *const c_char;
    fn cbm_pkgmap_node_at(
        tree: *const c_void,
        start: u32,
        end: u32,
        out: *mut RawNode,
    ) -> bool;
    fn cbm_pkgmap_node_parent(node: *const RawNode, out: *mut RawNode) -> bool;
    fn cbm_pkgmap_node_first_named_child(
        node: *const RawNode,
        out: *mut RawNode,
    ) -> bool;
}
