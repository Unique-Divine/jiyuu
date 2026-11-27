//! Borrowed graph and extraction records. The C pipeline owns the lifetimes
//! and excludes mutation while resolution workers read the graph.

use super::{HashTable, c_bytes, cbm_ht_get, free, mark_cstr};
pub(super) use crate::native_graph_buffer::Node;
use crate::native_graph_buffer::{
    cbm_gbuf_find_by_id, cbm_gbuf_find_by_name, cbm_gbuf_find_by_qn,
};
use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::ptr::{null, null_mut};
use std::slice;

impl Node {
    pub fn label(&self) -> Option<&[u8]> {
        unsafe { c_bytes(self.label) }
    }
    pub fn qn(&self) -> Option<&[u8]> {
        unsafe { c_bytes(self.qualified_name) }
    }
    pub fn path(&self) -> Option<&[u8]> {
        unsafe { c_bytes(self.file_path) }
    }
    pub fn php(&self) -> bool {
        self.path()
            .is_some_and(|path| language(leaf(path)) == Language::Php)
    }
    pub fn generated(&self) -> bool {
        unsafe { cbm_node_props_generated(self.properties_json) }
    }
    pub fn type_like(&self) -> bool {
        unsafe { cbm_label_is_type_like(self.label) }
    }
    pub fn other_than(&self, source: Option<&[u8]>) -> bool {
        source.is_none() || self.qn() != source
    }
}

#[repr(C)]
pub(crate) struct Edge {
    id: i64,
    source: i64,
    pub target: i64,
    kind: *const c_char,
    pub properties: *const c_char,
}

#[repr(C)]
pub(crate) struct Buffer {
    _opaque: [u8; 0],
}

impl Buffer {
    pub fn qn(&self, qn: &[u8]) -> Option<&Node> {
        with_c_string(qn, |qn| unsafe {
            cbm_gbuf_find_by_qn(std::ptr::from_ref(self).cast(), qn.as_ptr())
                .as_ref()
        })
    }

    pub fn id(&self, id: i64) -> Option<&Node> {
        unsafe {
            cbm_gbuf_find_by_id(std::ptr::from_ref(self).cast(), id).as_ref()
        }
    }

    pub fn name(&self, name: &[u8]) -> Option<&[*const Node]> {
        let mut hits = null();
        let mut count = 0;
        let result = with_c_string(name, |name| unsafe {
            cbm_gbuf_find_by_name(
                std::ptr::from_ref(self).cast(),
                name.as_ptr(),
                &mut hits,
                &mut count,
            )
        });
        pointer_rows(result, hits, count)
    }

    pub fn imports(&self, id: i64) -> Option<&[*const Edge]> {
        let mut hits = null();
        let mut count = 0;
        let result = unsafe {
            cbm_gbuf_find_edges_by_source_type(
                self,
                id,
                c"IMPORTS".as_ptr(),
                &mut hits,
                &mut count,
            )
        };
        pointer_rows(result, hits, count)
    }
}

fn pointer_rows<'a, T>(
    result: c_int,
    data: *const *const T,
    count: c_int,
) -> Option<&'a [*const T]> {
    if result != 0 || count < 0 {
        return None;
    }
    if count == 0 {
        return Some(&[]);
    }
    if data.is_null() {
        return None;
    }
    Some(unsafe { slice::from_raw_parts(data, count as usize) })
}

#[repr(C)]
pub(crate) struct Context {
    _opaque: [u8; 0],
}

#[repr(C)]
pub(crate) struct FileResult {
    _opaque: [u8; 0],
}

#[repr(C)]
pub(crate) struct Import {
    local_name_synthesized: bool,
    pub local_name: *const c_char,
    pub module_path: *const c_char,
    member: *const c_char,
    pub is_default: bool,
    type_checking_only: bool,
    pub kind: c_uint,
}

impl Import {
    pub fn path(&self) -> Option<&[u8]> {
        unsafe { c_bytes(self.module_path) }
    }
    pub fn local(&self) -> Option<&[u8]> {
        unsafe { c_bytes(self.local_name) }
    }
}

#[repr(C)]
pub(crate) struct Imports {
    items: *const Import,
    count: c_int,
    capacity: c_int,
}

impl Imports {
    pub fn rows(&self) -> Option<&[Import]> {
        if self.count == 0 {
            return Some(&[]);
        }
        if self.items.is_null() || self.count < 0 {
            return None;
        }
        Some(unsafe { slice::from_raw_parts(self.items, self.count as usize) })
    }
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Language {
    Other,
    Go,
    Python,
    Rust,
    Php,
    Kustomize,
}

pub(super) fn language(base: &[u8]) -> Language {
    with_c_string(base, |name| unsafe { cbm_pkgmap_language(name.as_ptr()) })
}

pub(super) fn leaf(path: &[u8]) -> &[u8] {
    path.rsplit(|byte| matches!(byte, b'/' | b'\\'))
        .next()
        .unwrap_or_default()
}

// Graph lookups use C strings. Most names fit on the stack, with an owned
// fallback for long qualified names.
pub(super) fn with_c_string<T>(bytes: &[u8], f: impl FnOnce(&CStr) -> T) -> T {
    let bytes = super::trimmed_c_string(bytes);
    let mut stack = [0u8; 256];
    if bytes.len() < stack.len() {
        stack[..bytes.len()].copy_from_slice(bytes);
        let text = CStr::from_bytes_with_nul(&stack[..=bytes.len()]).unwrap();
        mark_cstr(text);
        return f(text);
    }
    let text = CString::new(bytes).unwrap();
    mark_cstr(&text);
    f(&text)
}

pub(super) fn lookup<'a>(map: &'a HashTable, key: &[u8]) -> Option<&'a [u8]> {
    with_c_string(key, |key| unsafe {
        c_bytes(cbm_ht_get(map, key.as_ptr()).cast())
    })
}

pub(super) unsafe fn pkgmap<'a>() -> Option<&'a HashTable> {
    // The caller is executing within a single index run and must scope the
    // borrow to that run. The coordinator frees the map after readers return.
    unsafe { cbm_pipeline_get_pkgmap().as_ref() }
}

pub(super) struct Text(*mut c_char);

impl Text {
    pub fn new(ptr: *mut c_char) -> Option<Self> {
        (!ptr.is_null()).then_some(Self(ptr))
    }

    pub fn as_ptr(&self) -> *const c_char {
        self.0
    }

    pub fn bytes(&self) -> &[u8] {
        unsafe { CStr::from_ptr(self.0) }.to_bytes()
    }
}

impl Drop for Text {
    fn drop(&mut self) {
        unsafe { free(self.0.cast()) };
    }
}

pub(super) struct ModuleContext<'a> {
    pub project: *const c_char,
    pub source: Option<&'a CStr>,
    pub aliases: *const c_void,
}

impl<'a> ModuleContext<'a> {
    // The caller keeps the pipeline context, source path, and graph alive.
    pub unsafe fn read(
        ctx: *const Context,
        source: *const c_char,
    ) -> (Self, *const Buffer) {
        let (mut project, mut aliases) = (null(), null());
        let graph =
            unsafe { cbm_pkgmap_context(ctx, &mut project, &mut aliases) };
        (
            Self {
                project,
                aliases,
                source: unsafe { super::c_string(source) },
            },
            graph,
        )
    }

    pub fn relative(&self, path: &[u8]) -> Option<Text> {
        with_c_string(path, |path| {
            Text::new(unsafe {
                cbm_pipeline_resolve_relative_import(
                    self.source.map_or(null(), CStr::as_ptr),
                    path.as_ptr(),
                )
            })
        })
    }

    pub fn folder(&self, path: &[u8]) -> Option<Text> {
        with_c_string(path, |path| {
            Text::new(unsafe {
                cbm_pipeline_fqn_folder(self.project, path.as_ptr())
            })
        })
    }
}

pub(super) struct Resolution<'a> {
    pub graph: &'a Buffer,
    pub graph_ptr: *const Buffer,
    pub modules: ModuleContext<'a>,
    pub source_qn: Option<&'a [u8]>,
}

impl<'a> Resolution<'a> {
    pub unsafe fn new(
        ctx: *const Context,
        source: *const c_char,
        source_qn: *const c_char,
    ) -> Option<Self> {
        let (modules, graph) = unsafe { ModuleContext::read(ctx, source) };
        Some(Self {
            graph: unsafe { graph.as_ref()? },
            graph_ptr: graph,
            modules,
            source_qn: unsafe { c_bytes(source_qn) },
        })
    }

    pub fn file(&self, path: &[u8]) -> Option<&'a Node> {
        with_c_string(path, |path| unsafe {
            cbm_pipeline_file_node(
                self.graph,
                self.modules.project,
                path.as_ptr(),
            )
            .as_ref()
        })
    }

    pub fn module(&self, path: &[u8]) -> Option<&'a Node> {
        Text::new(super::resolve::module_qn(&self.modules, path))
            .and_then(|qn| self.graph.qn(qn.bytes()))
    }

    pub fn module_at(&self, path: &[u8]) -> Option<&'a Node> {
        let qn = fqn_module(self.modules.project, path)?;
        self.graph.qn(qn.bytes())
    }
}

pub(super) fn fqn_module(project: *const c_char, path: &[u8]) -> Option<Text> {
    with_c_string(path, |path| {
        Text::new(unsafe {
            super::cbm_pipeline_fqn_module(project, path.as_ptr())
        })
    })
}

pub(super) fn rel_join(dir: &[u8], path: &[u8]) -> Option<Vec<u8>> {
    let mut out = [0; super::PATH_LIMIT + 1];
    let joined = with_c_string(dir, |dir| {
        with_c_string(path, |path| unsafe {
            cbm_repo_rel_join(
                dir.as_ptr(),
                path.as_ptr(),
                out.as_mut_ptr(),
                out.len(),
            )
        })
    });
    joined.then(|| unsafe { CStr::from_ptr(out.as_ptr()) }.to_bytes().to_vec())
}

pub(super) fn ptr<T>(value: Option<&T>) -> *const T {
    value.map_or(null(), std::ptr::from_ref)
}

pub(super) fn nodes(
    hits: Option<&[*const Node]>,
) -> impl Iterator<Item = &Node> {
    hits.into_iter()
        .flatten()
        .filter_map(|&node| unsafe { node.as_ref() })
}

pub(super) fn take_text(text: Option<Text>) -> *mut c_char {
    text.map_or(null_mut(), |text| {
        let text = std::mem::ManuallyDrop::new(text);
        text.0
    })
}

unsafe extern "C" {
    fn cbm_pkgmap_language(name: *const c_char) -> Language;
    fn cbm_pkgmap_context(
        ctx: *const Context,
        project: *mut *const c_char,
        aliases: *mut *const c_void,
    ) -> *const Buffer;
    pub(super) fn cbm_pkgmap_imports(
        result: *const FileResult,
    ) -> *const Imports;
    pub(super) fn cbm_pkgmap_namespace(
        result: *const FileResult,
    ) -> *const c_char;
    pub(super) fn cbm_pipeline_fqn_compute(
        project: *const c_char,
        path: *const c_char,
        name: *const c_char,
    ) -> *mut c_char;
    pub(super) fn cbm_pipeline_file_node(
        graph: *const Buffer,
        project: *const c_char,
        path: *const c_char,
    ) -> *const Node;
    pub(super) fn cbm_gbuf_insert_edge(
        graph: *mut c_void,
        src: i64,
        dst: i64,
        kind: *const c_char,
        props: *const c_char,
    ) -> i64;
    pub(super) fn cbm_path_alias_find_for_file(
        aliases: *const c_void,
        source: *const c_char,
    ) -> *const c_void;
    pub(super) fn cbm_path_alias_resolve(
        aliases: *const c_void,
        path: *const c_char,
    ) -> *mut c_char;
    fn cbm_pipeline_get_pkgmap() -> *const HashTable;
    fn cbm_pipeline_resolve_relative_import(
        source: *const c_char,
        path: *const c_char,
    ) -> *mut c_char;
    fn cbm_pipeline_fqn_folder(
        project: *const c_char,
        path: *const c_char,
    ) -> *mut c_char;
    fn cbm_repo_rel_join(
        dir: *const c_char,
        path: *const c_char,
        out: *mut c_char,
        len: usize,
    ) -> bool;
    fn cbm_node_props_generated(properties: *const c_char) -> bool;
    fn cbm_label_is_type_like(label: *const c_char) -> bool;
    fn cbm_gbuf_find_edges_by_source_type(
        graph: *const Buffer,
        src: i64,
        kind: *const c_char,
        out: *mut *const *const Edge,
        count: *mut c_int,
    ) -> c_int;
}
