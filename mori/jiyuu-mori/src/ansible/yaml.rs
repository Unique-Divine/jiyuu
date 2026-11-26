//! The Ansible pass uses the same pinned YAML grammar as extraction.

use std::ffi::{CStr, c_char, c_void};
use std::marker::PhantomData;
use std::ptr::{self, NonNull};

pub(super) const SCANNER_PADDING: usize = 16;

#[repr(C)]
#[derive(Clone, Copy)]
struct RawNode {
    context: [u32; 4],
    id: *const c_void,
    tree: *const c_void,
}

#[repr(C)]
struct Point {
    row: u32,
    column: u32,
}

unsafe extern "C" {
    fn ts_parser_new() -> *mut c_void;
    fn ts_parser_delete(parser: *mut c_void);
    fn ts_parser_set_language(
        parser: *mut c_void,
        language: *const c_void,
    ) -> bool;
    fn ts_parser_parse_string(
        parser: *mut c_void,
        old_tree: *const c_void,
        source: *const c_char,
        len: u32,
    ) -> *mut c_void;
    fn ts_tree_delete(tree: *mut c_void);
    fn ts_tree_root_node(tree: *const c_void) -> RawNode;
    fn ts_node_type(node: RawNode) -> *const c_char;
    fn ts_node_parent(node: RawNode) -> RawNode;
    fn ts_node_named_child(node: RawNode, index: u32) -> RawNode;
    fn ts_node_named_child_count(node: RawNode) -> u32;
    fn cbm_ts_child_by_field_name(
        node: RawNode,
        name: *const c_char,
        len: u32,
    ) -> RawNode;
    fn ts_node_start_byte(node: RawNode) -> u32;
    fn ts_node_end_byte(node: RawNode) -> u32;
    fn ts_node_start_point(node: RawNode) -> Point;
    fn ts_node_end_point(node: RawNode) -> Point;
}

pub(super) struct Parser(NonNull<c_void>);

impl Parser {
    /// The language pointer must be a live Tree-sitter language.
    pub(super) unsafe fn new(language: *const c_void) -> Option<Self> {
        let parser = Self(NonNull::new(unsafe { ts_parser_new() })?);
        unsafe { ts_parser_set_language(parser.0.as_ptr(), language) }
            .then_some(parser)
    }

    pub(super) fn parse(&mut self, mut source: Vec<u8>) -> Option<Document> {
        let len = u32::try_from(source.len()).ok()?;
        // Preserve the padding used by the C pass for the YAML scanner.
        source.try_reserve_exact(SCANNER_PADDING).ok()?;
        source.extend_from_slice(&[0; SCANNER_PADDING]);
        let tree = unsafe {
            ts_parser_parse_string(
                self.0.as_ptr(),
                ptr::null(),
                source.as_ptr().cast(),
                len,
            )
        };
        Some(Document {
            tree: NonNull::new(tree)?,
            source,
        })
    }
}

impl Drop for Parser {
    fn drop(&mut self) {
        unsafe { ts_parser_delete(self.0.as_ptr()) };
    }
}

pub(super) struct Document {
    tree: NonNull<c_void>,
    source: Vec<u8>,
}

impl Document {
    pub(super) fn root(&self) -> Node<'_> {
        Node(
            unsafe { ts_tree_root_node(self.tree.as_ptr()) },
            PhantomData,
        )
    }

    pub(super) fn top(&self) -> Option<Node<'_>> {
        self.root().child(b"document")?.child(b"block_node")
    }

    pub(super) fn scalar(
        &self,
        node: Node<'_>,
        limit: usize,
    ) -> Option<Vec<u8>> {
        let node = node.unwrap(&[b"flow_node", b"block_node", b"plain_scalar"]);
        let text = node.text(&self.source);
        let value = match node.kind() {
            b"single_quote_scalar" | b"double_quote_scalar" => {
                let quoted = text.get(1..text.len().saturating_sub(1))?;
                if quoted.len() >= limit {
                    return None;
                }
                unescape(quoted, node.kind() == b"double_quote_scalar")
            }
            b"string_scalar" | b"plain_scalar" if text.len() < limit => {
                text.to_vec()
            }
            _ => return None,
        };
        let end = value
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(value.len());
        (end > 0).then(|| value[..end].to_vec())
    }

    pub(super) fn key(&self, pair: Node<'_>, limit: usize) -> Option<Vec<u8>> {
        let key = self.scalar(pair.field(c"key")?, limit)?;
        Some(match key.strip_prefix(b"ansible.builtin.") {
            Some(key) => key.to_vec(),
            None => key,
        })
    }

    pub(super) fn get<'tree>(
        &self,
        mapping: Node<'tree>,
        key: &[u8],
    ) -> Option<Node<'tree>> {
        let mapping = mapping.unwrap(&[b"block_node"]);
        if mapping.kind() != b"block_mapping" {
            return None;
        }
        mapping
            .children()
            .filter(|pair| pair.kind() == b"block_mapping_pair")
            .find(|&pair| self.key(pair, 64).as_deref() == Some(key))?
            .value()
    }

    pub(super) fn string(
        &self,
        mapping: Node<'_>,
        key: &[u8],
        limit: usize,
    ) -> Option<Vec<u8>> {
        self.scalar(self.get(mapping, key)?, limit)
    }

    pub(super) fn is_true(&self, node: Node<'_>) -> bool {
        let node = node.unwrap(&[b"flow_node", b"block_node", b"plain_scalar"]);
        let scalar;
        let text = match node.kind() {
            b"boolean_scalar" | b"integer_scalar" => node.text(&self.source),
            _ => {
                scalar = self.scalar(node, 16);
                scalar.as_deref().unwrap_or_default()
            }
        };
        [b"y".as_slice(), b"yes", b"on", b"1", b"true", b"t"]
            .iter()
            .any(|word| text.eq_ignore_ascii_case(word))
    }
}

impl Drop for Document {
    fn drop(&mut self) {
        unsafe { ts_tree_delete(self.tree.as_ptr()) };
    }
}

/// A node can only be used while its document is live.
#[derive(Clone, Copy)]
pub(super) struct Node<'tree>(RawNode, PhantomData<&'tree Document>);

impl<'tree> Node<'tree> {
    fn wrap(raw: RawNode) -> Option<Self> {
        (!raw.id.is_null()).then_some(Self(raw, PhantomData))
    }

    pub(super) fn kind(self) -> &'tree [u8] {
        unsafe { CStr::from_ptr(ts_node_type(self.0)) }.to_bytes()
    }

    fn text(self, source: &[u8]) -> &[u8] {
        let start = unsafe { ts_node_start_byte(self.0) } as usize;
        let end = unsafe { ts_node_end_byte(self.0) } as usize;
        &source[start..end]
    }

    pub(super) fn line(self) -> i32 {
        unsafe { ts_node_start_point(self.0) }.row as i32 + 1
    }

    pub(super) fn last_line(self) -> i32 {
        unsafe { ts_node_end_point(self.0) }.row as i32 + 1
    }

    pub(super) fn parent(self) -> Option<Self> {
        Self::wrap(unsafe { ts_node_parent(self.0) })
    }

    pub(super) fn field(self, name: &CStr) -> Option<Self> {
        Self::wrap(unsafe {
            cbm_ts_child_by_field_name(
                self.0,
                name.as_ptr(),
                name.count_bytes() as u32,
            )
        })
    }

    pub(super) fn value(self) -> Option<Self> {
        self.field(c"value")
    }

    pub(super) fn children(
        self,
    ) -> impl DoubleEndedIterator<Item = Self> + 'tree {
        let count = unsafe { ts_node_named_child_count(self.0) };
        (0..count).filter_map(move |index| self.at(index))
    }

    pub(super) fn at(self, index: u32) -> Option<Self> {
        Self::wrap(unsafe { ts_node_named_child(self.0, index) })
    }

    pub(super) fn child(self, kind: &[u8]) -> Option<Self> {
        self.children().find(|node| node.kind() == kind)
    }

    pub(super) fn unwrap(mut self, wrappers: &[&[u8]]) -> Self {
        while wrappers.contains(&self.kind())
            && unsafe { ts_node_named_child_count(self.0) } == 1
        {
            self = self.at(0).expect("wrapper with one named child");
        }
        self
    }

    pub(super) fn bare(self) -> Self {
        self.unwrap(&[b"block_node", b"flow_node"])
    }

    pub(super) fn sequence(self) -> Option<Self> {
        let node = self.unwrap(&[b"block_node"]);
        (node.kind() == b"block_sequence").then_some(node)
    }

    pub(super) fn item_mapping(self) -> Option<Self> {
        let item = self.at(0)?;
        (item.kind() == b"block_node")
            .then(|| item.child(b"block_mapping"))
            .flatten()
    }
}

fn unescape(text: &[u8], double: bool) -> Vec<u8> {
    let mut decoded = Vec::with_capacity(text.len());
    let mut bytes = text.iter().copied().peekable();
    while let Some(byte) = bytes.next() {
        decoded.push(match (double, byte, bytes.peek()) {
            (true, b'\\', Some(_)) => match bytes.next().unwrap() {
                b'n' => b'\n',
                b't' => b'\t',
                other => other,
            },
            (false, b'\'', Some(b'\'')) => {
                bytes.next();
                b'\''
            }
            _ => byte,
        });
    }
    decoded
}

pub(super) fn starts_sequence(source: &[u8]) -> bool {
    let source = source.split(|&byte| byte == 0).next().unwrap_or_default();
    for line in source.split(|&byte| byte == b'\n') {
        let first = &line[line
            .iter()
            .take_while(|&&byte| matches!(byte, b' ' | b'\t' | b'\r'))
            .count()..];
        if !first.is_empty()
            && !first.starts_with(b"#")
            && !first.starts_with(b"---")
        {
            return line.starts_with(b"- ");
        }
    }
    false
}
