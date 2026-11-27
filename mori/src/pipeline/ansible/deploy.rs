//! Link kube and k8s tasks to the templates or copies they apply.

use super::yaml::{Document, Node, Parser};
use super::{
    FileNode, Location, PATH_SIZE, Run, contains, escape, join, literal, read,
    truncated,
};
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_int};

#[derive(Default)]
struct Item {
    scalar: Option<Vec<u8>>,
    pairs: Vec<(Vec<u8>, Vec<u8>)>,
}

impl Item {
    fn parse(doc: &Document, node: Node<'_>) -> Self {
        if let Some(scalar) = doc.scalar(node, PATH_SIZE) {
            return Self {
                scalar: Some(scalar),
                pairs: Vec::new(),
            };
        }
        let mapping = node.bare();
        let pairs =
            if matches!(mapping.kind(), b"block_mapping" | b"flow_mapping") {
                mapping
                    .children()
                    .filter(|pair| {
                        matches!(
                            pair.kind(),
                            b"block_mapping_pair" | b"flow_pair"
                        )
                    })
                    .filter_map(|pair| {
                        Some((
                            doc.scalar(pair.field(c"key")?, 128)?,
                            doc.scalar(pair.value()?, PATH_SIZE)?,
                        ))
                    })
                    .collect()
            } else {
                Vec::new()
            };
        Self {
            scalar: None,
            pairs,
        }
    }

    fn value(&self, key: &[u8]) -> Option<&[u8]> {
        if key.is_empty() {
            return self.scalar.as_deref();
        }
        self.pairs
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_slice())
    }
}

#[derive(Default)]
enum Loop {
    #[default]
    Empty,
    Items(Vec<Item>),
    Expression(Vec<u8>),
}

impl Loop {
    fn parse(doc: &Document, node: Node<'_>) -> Self {
        if let Some(scalar) = doc.scalar(node, PATH_SIZE) {
            return Self::from_scalar(scalar);
        }
        let sequence = node.bare();
        let block = sequence.kind() == b"block_sequence";
        if !block && sequence.kind() != b"flow_sequence" {
            return Self::Empty;
        }
        let mut items: Vec<_> = sequence
            .children()
            .filter_map(|item| match (block, item.kind()) {
                (true, b"block_sequence_item") => Some(item.at(0)),
                (false, b"flow_node") => Some(Some(item)),
                _ => None,
            })
            .map(|item| {
                item.map_or_else(Item::default, |node| Item::parse(doc, node))
            })
            .collect();
        if items.len() == 1
            && items[0].scalar.as_deref().is_some_and(|s| !literal(s))
        {
            return Self::Expression(items[0].scalar.take().unwrap());
        }
        if items.is_empty() {
            Self::Empty
        } else {
            Self::Items(items)
        }
    }

    fn from_scalar(scalar: Vec<u8>) -> Self {
        if literal(&scalar) {
            Self::Empty
        } else {
            Self::Expression(scalar)
        }
    }

    fn task(doc: &Document, mapping: Node<'_>) -> Self {
        doc.get(mapping, b"with_items")
            .or_else(|| doc.get(mapping, b"loop"))
            .map_or(Self::Empty, |value| Self::parse(doc, value))
    }

    fn items(&self) -> &[Item] {
        match self {
            Self::Items(items) => items,
            _ => &[],
        }
    }
}

struct Render {
    copy: bool,
    source: Option<Vec<u8>>,
    dest: Option<Vec<u8>>,
    register: Option<Vec<u8>>,
    items: Loop,
}

struct Apply {
    id: i64,
    line: c_int,
    module: Vec<u8>,
    filename: Option<Vec<u8>>,
    template: Option<Vec<u8>>,
    items: Loop,
}

#[derive(Default)]
pub(super) struct Deployments {
    renders: Vec<Render>,
    applies: Vec<Apply>,
    facts: HashMap<Vec<u8>, Loop>,
}

impl Deployments {
    pub(super) fn collect(
        &mut self,
        doc: &Document,
        mapping: Node<'_>,
        id: i64,
    ) {
        for pair in mapping.children() {
            let Some(key) = doc.key(pair, 128) else {
                continue;
            };
            let value = pair.value();
            let string = |key| doc.string(value?, key, PATH_SIZE);
            match key.as_slice() {
                b"template" | b"copy" => {
                    let Some(value) =
                        value.filter(|value| is_mapping(value.bare()))
                    else {
                        continue;
                    };
                    let copy = key == b"copy";
                    if copy
                        && doc
                            .get(value, b"remote_src")
                            .is_some_and(|n| doc.is_true(n))
                    {
                        continue;
                    }
                    self.renders.push(Render {
                        copy,
                        source: string(b"src"),
                        dest: string(b"dest"),
                        register: doc.string(mapping, b"register", PATH_SIZE),
                        items: Loop::task(doc, mapping),
                    });
                }
                b"set_fact" => {
                    if let Some(mapping) = value
                        .map(Node::bare)
                        .filter(|n| n.kind() == b"block_mapping")
                    {
                        for (name, value) in lists(doc, mapping) {
                            self.facts
                                .entry(name)
                                .or_insert_with(|| Loop::parse(doc, value));
                        }
                    }
                }
                b"kube"
                | b"k8s"
                | b"kubernetes.core.k8s"
                | b"community.kubernetes.k8s" => {
                    let template = string(b"template");
                    let filename = if template.is_some() {
                        None
                    } else {
                        string(b"filename").or_else(|| string(b"src"))
                    };
                    self.applies.push(Apply {
                        id,
                        line: mapping.line(),
                        module: key,
                        filename,
                        template,
                        items: Loop::task(doc, mapping),
                    });
                }
                _ => {}
            }
        }
    }

    fn renderer(&self, name: &[u8], register: bool) -> Option<&Render> {
        [false, true].into_iter().find_map(|copy| {
            self.renders.iter().find(|render| {
                let value = if register {
                    &render.register
                } else {
                    &render.dest
                };
                render.copy == copy && value.as_deref() == Some(name)
            })
        })
    }

    pub(super) fn link(
        &self,
        run: &Run,
        location: &Location<'_>,
        files: &HashMap<Vec<u8>, FileNode>,
        by_path: &HashMap<&[u8], &CStr>,
        parser: &mut Parser,
        count: &mut c_int,
    ) {
        for apply in &self.applies {
            if run.cancelled() {
                return;
            }
            let module = escape(&apply.module, &mut [0; 256]);
            let tail = format!("\",\"line\":{}}}", apply.line);
            let props = CString::new(
                [
                    b"{\"via\":\"ansible\",\"module\":\"".as_slice(),
                    &module,
                    tail.as_bytes(),
                ]
                .concat(),
            )
            .unwrap();
            let mut emit = |src: &[u8], copy| {
                if let Some(target) = source_file(location, files, src, copy) {
                    run.edge(apply.id, target, c"DEPLOYS", &props);
                    *count += 1;
                    true
                } else {
                    false
                }
            };
            if let Some(template) = &apply.template {
                emit(template, false);
                continue;
            }
            let Some(filename) = &apply.filename else {
                continue;
            };
            let direct = self.renderer(filename, false);
            if let Some(render) = direct
                && let Some(source) = &render.source
                && emit(source, render.copy)
            {
                continue;
            }

            let mut deploy_items = |render: &Render, items: &[Item]| {
                let Some(source) = &render.source else {
                    return false;
                };
                let mut found = false;
                for item in items {
                    if run.cancelled() {
                        break;
                    }
                    if let Some(path) = substitute(source, item, true) {
                        found |= emit(&path, render.copy);
                    }
                }
                found
            };

            // A registered renderer's results carry the original loop item.
            if contains(filename, b"item.item")
                && let Loop::Expression(expr) = &apply.items
                && let Some(register) = expression_name(expr, b".results")
                && let Some(render) = self.renderer(register, true)
                && deploy_items(
                    render,
                    render
                        .items
                        .resolve(&self.facts, location, by_path, parser)
                        .items(),
                )
            {
                continue;
            }

            // The task and its renderer share the same destination expression.
            if has_item_key(filename)
                && let Some(render) = direct
                && deploy_items(
                    render,
                    apply
                        .items
                        .resolve(&self.facts, location, by_path, parser)
                        .items(),
                )
            {
                continue;
            }

            // A literal destination can match each looped renderer's output.
            if !contains(filename, b"{{ item") {
                for render in &self.renders {
                    if run.cancelled() {
                        return;
                    }
                    let (Some(dest), Some(source)) =
                        (&render.dest, &render.source)
                    else {
                        continue;
                    };
                    let items = render.items.resolve(
                        &self.facts,
                        location,
                        by_path,
                        parser,
                    );
                    for item in items.items() {
                        if run.cancelled() {
                            return;
                        }
                        if substitute(dest, item, false).as_ref()
                            == Some(filename)
                            && let Some(path) = substitute(source, item, true)
                        {
                            emit(&path, render.copy);
                        }
                    }
                }
            }
        }
    }
}

fn is_mapping(node: Node<'_>) -> bool {
    matches!(node.kind(), b"block_mapping" | b"flow_mapping")
}

fn lists<'tree>(
    doc: &'tree Document,
    mapping: Node<'tree>,
) -> impl Iterator<Item = (Vec<u8>, Node<'tree>)> + 'tree {
    mapping.children().filter_map(|pair| {
        let value = pair.value()?;
        let key = doc.key(pair, 128)?;
        matches!(value.bare().kind(), b"block_sequence" | b"flow_sequence")
            .then_some((key, value))
    })
}

fn source_file(
    location: &Location<'_>,
    files: &HashMap<Vec<u8>, FileNode>,
    source: &[u8],
    copy: bool,
) -> Option<i64> {
    if !literal(source) {
        return None;
    }
    let lookup = |base: &[u8]| Some(files.get(&join(base, source)?)?.id);
    if !location.task_role.is_empty() {
        let suffix: &[u8] = if copy { b"/files" } else { b"/templates" };
        let base = truncated([location.task_role, suffix].concat(), PATH_SIZE);
        if let Some(id) = lookup(&base) {
            return Some(id);
        }
    }
    lookup(location.dir)
}

enum Resolved<'a> {
    Borrowed(&'a Loop),
    Owned(Loop),
}

impl Resolved<'_> {
    fn items(&self) -> &[Item] {
        match self {
            Self::Borrowed(items) => items.items(),
            Self::Owned(items) => items.items(),
        }
    }
}

impl Loop {
    fn resolve<'a>(
        &'a self,
        facts: &'a HashMap<Vec<u8>, Self>,
        location: &Location<'_>,
        by_path: &HashMap<&[u8], &CStr>,
        parser: &mut Parser,
    ) -> Resolved<'a> {
        let Self::Expression(expr) = self else {
            return Resolved::Borrowed(self);
        };
        let Some(name) = expression_name(expr, b"") else {
            return Resolved::Owned(Self::Empty);
        };
        if let Some(items) = facts.get(name) {
            return Resolved::Borrowed(items);
        }
        Resolved::Owned(
            role_list(location, by_path, parser, name).unwrap_or_default(),
        )
    }
}

fn role_list(
    location: &Location<'_>,
    by_path: &HashMap<&[u8], &CStr>,
    parser: &mut Parser,
    name: &[u8],
) -> Option<Loop> {
    if location.task_role.is_empty() {
        return None;
    }
    [
        b"/defaults/main.yml".as_slice(),
        b"/defaults/main.yaml",
        b"/vars/main.yml",
        b"/vars/main.yaml",
    ]
    .iter()
    .find_map(|suffix| {
        let path = truncated([location.task_role, suffix].concat(), PATH_SIZE);
        let doc = parser.parse(read(by_path.get(path.as_slice())?)?)?;
        let root = doc.top()?.bare();
        if root.kind() != b"block_mapping" {
            return None;
        }
        let (_, value) = lists(&doc, root).find(|(key, _)| key == name)?;
        Some(Loop::parse(&doc, value))
    })
}

fn identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn identifier_len(bytes: &[u8]) -> usize {
    if !bytes.first().is_some_and(|byte| identifier_start(*byte)) {
        return 0;
    }
    bytes
        .iter()
        .take_while(|&&byte| identifier_start(byte) || byte.is_ascii_digit())
        .count()
}

fn trim_horizontal_start(bytes: &[u8]) -> &[u8] {
    let n = bytes
        .iter()
        .take_while(|&&byte| matches!(byte, b' ' | b'\t'))
        .count();
    &bytes[n..]
}

fn expression_name<'a>(expr: &'a [u8], suffix: &[u8]) -> Option<&'a [u8]> {
    let value = trim_horizontal_start(expr.strip_prefix(b"{{")?);
    let len = identifier_len(value);
    if len == 0 || len >= 128 {
        return None;
    }
    let ending = trim_horizontal_start(value[len..].strip_prefix(suffix)?);
    (ending == b"}}").then_some(&value[..len])
}

fn has_item_key(text: &[u8]) -> bool {
    text.windows(2)
        .enumerate()
        .filter(|(_, pair)| *pair == b"{{")
        .any(|(offset, _)| {
            let Some(key) = trim_horizontal_start(&text[offset + 2..])
                .strip_prefix(b"item.")
            else {
                return false;
            };
            // The original detector accepts letters and underscores here.
            let len = key
                .iter()
                .take_while(|&&byte| identifier_start(byte))
                .count();
            len > 0 && trim_horizontal_start(&key[len..]).starts_with(b"}}")
        })
}

fn item_reference(expr: &[u8]) -> Option<&[u8]> {
    let expr = trim_horizontal_start(expr);
    let end = expr
        .iter()
        .rposition(|&byte| !matches!(byte, b' ' | b'\t'))?;
    let item = &expr[..=end];
    if item == b"item" {
        return Some(b"");
    }
    let key = item.strip_prefix(b"item.")?;
    (!key.is_empty() && identifier_len(key) == key.len()).then_some(key)
}

fn substitute(mut pattern: &[u8], item: &Item, strict: bool) -> Option<Vec<u8>> {
    let mut output = Vec::new();
    while let Some(start) = pattern.windows(2).position(|pair| pair == b"{{") {
        let expression = &pattern[start + 2..];
        let Some(end) = expression.windows(2).position(|pair| pair == b"}}")
        else {
            break;
        };
        let next = start + 2 + end + 2;
        output.extend_from_slice(&pattern[..start]);
        if let Some(key) = item_reference(&expression[..end]) {
            output.extend_from_slice(item.value(key)?);
        } else if strict {
            return None;
        } else {
            output.extend_from_slice(&pattern[start..next]);
        }
        if output.len() >= PATH_SIZE {
            return None;
        }
        pattern = &pattern[next..];
    }
    output.extend_from_slice(pattern);
    (output.len() < PATH_SIZE).then_some(output)
}
