//! Decorator tags: the decorator words that two or more definitions share.
//!
//! Each Function, Method, Class and Struct node with a `decorators`
//! property is split into words (`@login_required` gives `login` and
//! `required`). A word on two or more of those nodes becomes a tag, and each
//! node that has a tag gets a sorted `decorator_tags` array in its
//! properties.

use std::collections::{HashMap, HashSet};

mod json;

// Buffer and array limits of the C pass, kept for identical output.
const DECORATOR_MAX: usize = 255;
const CAMEL_MAX: usize = 16;
const TOKENS_MAX: usize = 32;
const WORDS_MAX: usize = 256;
const MIN_WORD: usize = 2;

const STOPWORDS: &[&[u8]] = &[
    b"get",
    b"set",
    b"new",
    b"class",
    b"method",
    b"function",
    b"value",
    b"type",
    b"param",
    b"return",
    b"public",
    b"private",
    b"for",
    b"if",
    b"the",
    b"and",
    b"or",
    b"not",
    b"with",
    b"from",
    b"app",
    b"router",
];

/// The labels whose nodes give and receive tags, in collection order.
/// Struct sits beside Class so Go, Rust, Swift and D structs keep the tags
/// they had when structs were labelled Class.
pub(super) const LABELS: [&[u8]; 4] =
    [b"Function", b"Method", b"Class", b"Struct"];

/// Splits `s` where a lowercase ASCII letter meets an uppercase one, keeping
/// at most `max` parts. Later parts are dropped, not merged.
fn split_camel_case(s: &[u8], max: usize) -> Vec<&[u8]> {
    let mut parts = Vec::new();
    if s.is_empty() || max == 0 {
        return parts;
    }
    let mut start = 0;
    for i in 1..s.len() {
        if s[i].is_ascii_uppercase() && s[i - 1].is_ascii_lowercase() {
            if parts.len() < max {
                parts.push(&s[start..i]);
            }
            start = i;
        }
    }
    if parts.len() < max {
        parts.push(&s[start..]);
    }
    parts
}

/// Removes a leading `@`, a `#[...]` attribute wrapper and the arguments,
/// and turns `.`, `_`, `-`, `:` and `/` into word breaks.
fn strip_decorator_syntax(decorator: &[u8]) -> Vec<u8> {
    let mut p = decorator;
    if let [b'@', rest @ ..] = p {
        p = rest;
    }
    if let [b'#', b'[', rest @ ..] = p {
        p = rest;
        if let [inner @ .., b']'] = p {
            p = inner;
        }
    }
    if let Some(paren) = p.iter().position(|&b| b == b'(') {
        p = &p[..paren];
    }
    p.iter()
        .map(|&b| match b {
            b'.' | b'_' | b'-' | b':' | b'/' => b' ',
            _ => b,
        })
        .collect()
}

/// Lowercase words of one decorator, without stopwords or one-letter words,
/// at most `max` of them. Only the decorator's first 255 bytes count.
fn tokenize_decorator(decorator: &[u8], max: usize) -> Vec<Vec<u8>> {
    let mut words = Vec::new();
    if max == 0 {
        return words;
    }
    let decorator = &decorator[..decorator.len().min(DECORATOR_MAX)];
    let stripped = strip_decorator_syntax(decorator);
    for part in stripped.split(|&b| b == b' ').filter(|p| !p.is_empty()) {
        if words.len() >= max {
            break;
        }
        for camel in split_camel_case(part, CAMEL_MAX) {
            if words.len() >= max {
                break;
            }
            let word = camel.to_ascii_lowercase();
            if word.len() >= MIN_WORD && !STOPWORDS.contains(&word.as_slice()) {
                words.push(word);
            }
        }
    }
    words
}

/// The distinct words of a node's decorators in first-seen order, or none
/// when its properties have no `decorators` strings.
fn decorator_words(props: &[u8]) -> Vec<Vec<u8>> {
    let Some(decorators) = json::decorators(props) else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    let mut words = Vec::new();
    for decorator in &decorators {
        for word in tokenize_decorator(decorator, TOKENS_MAX) {
            if words.len() < WORDS_MAX && seen.insert(word.clone()) {
                words.push(word);
            }
        }
    }
    words
}

/// The tags of each tagged node, by the node's position in `props`, in
/// node order. `props` lists the candidate nodes' properties in collection
/// order (`None` for a node without properties).
pub(super) fn plan(props: &[Option<&[u8]>]) -> Plan {
    let nodes: Vec<(usize, Vec<Vec<u8>>)> = props
        .iter()
        .enumerate()
        .filter_map(|(index, props)| {
            let words = decorator_words((*props)?);
            (!words.is_empty()).then_some((index, words))
        })
        .collect();
    let mut counts: HashMap<&[u8], usize> = HashMap::new();
    for (_, words) in &nodes {
        for word in words {
            *counts.entry(word.as_slice()).or_default() += 1;
        }
    }
    let shared = |word: &[u8]| counts.get(word).is_some_and(|&n| n >= 2);
    let has_candidates = nodes.iter().any(|(_, w)| w.iter().any(|w| shared(w)));
    let mut tagged = Vec::new();
    if has_candidates {
        for (index, words) in &nodes {
            let mut tags: Vec<Vec<u8>> = words
                .iter()
                .filter(|word| shared(word))
                .take(WORDS_MAX)
                .cloned()
                .collect();
            if tags.is_empty() {
                continue;
            }
            tags.sort();
            tagged.push((*index, tags));
        }
    }
    Plan {
        decorated: nodes.len(),
        has_candidates,
        tagged,
    }
}

pub(super) struct Plan {
    /// Nodes with at least one decorator word.
    pub(super) decorated: usize,
    /// Whether any word is on two or more nodes.
    pub(super) has_candidates: bool,
    /// (node position, sorted tags) for each node that gets tags.
    pub(super) tagged: Vec<(usize, Vec<Vec<u8>>)>,
}

/// `props` with its `decorator_tags` replaced by `tags`, written the way the
/// C pass's yyjson writer wrote it. Properties that are not a JSON object
/// become an object holding only the tags. `None` where yyjson refused to
/// write: a tag cut inside a UTF-8 sequence.
pub(super) fn with_tags(props: &[u8], tags: &[Vec<u8>]) -> Option<Vec<u8>> {
    json::with_tags(props, tags)
}

#[cfg(feature = "native-engine-abi")]
mod native {
    use super::super::native_graph_buffer::{
        Node, cbm_gbuf_find_by_label, cbm_gbuf_find_by_qn,
    };
    use super::{LABELS, plan, with_tags};
    use std::ffi::{CStr, CString, c_char, c_int, c_void};
    use std::{ptr, slice};

    unsafe extern "C" {
        fn cbm_gbuf_upsert_node(
            graph: *mut c_void,
            label: *const c_char,
            name: *const c_char,
            qn: *const c_char,
            file: *const c_char,
            start: c_int,
            end: c_int,
            props: *const c_char,
        ) -> i64;
        fn cbm_log(level: c_int, event: *const c_char, ...);
    }

    const LOG_INFO: c_int = 1;

    // SAFETY: a non-null ptr is NUL-terminated and outlives the copy.
    unsafe fn owned(ptr: *const c_char) -> Option<CString> {
        (!ptr.is_null()).then(|| unsafe { CStr::from_ptr(ptr) }.to_owned())
    }

    /// Tags the decorated nodes of `graph` and returns how many it updated.
    ///
    /// # Safety
    ///
    /// `graph` is a live graph buffer or null; `project` is a NUL-terminated
    /// string or null. No other thread uses the buffer during the call.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn mori_pipeline_pass_decorator_tags(
        graph: *mut c_void,
        project: *const c_char,
    ) -> c_int {
        if graph.is_null() || project.is_null() {
            return 0;
        }
        // Snapshot the candidates first: updating a node replaces its strings.
        let mut nodes: Vec<(CString, Option<Vec<u8>>)> = Vec::new();
        for label in LABELS {
            let label = CString::new(label).expect("labels have no NUL");
            let mut found: *const *const Node = ptr::null();
            let mut count: c_int = 0;
            // SAFETY: the buffer owns the returned array for this call.
            unsafe {
                cbm_gbuf_find_by_label(
                    graph,
                    label.as_ptr(),
                    &mut found,
                    &mut count,
                );
            }
            if found.is_null() || count <= 0 {
                continue;
            }
            let found = unsafe { slice::from_raw_parts(found, count as usize) };
            for &node in found {
                let node = unsafe { &*node };
                let Some(qn) = (unsafe { owned(node.qualified_name) }) else {
                    continue;
                };
                let props = unsafe { owned(node.properties_json) }
                    .map(CString::into_bytes);
                nodes.push((qn, props));
            }
        }
        let props: Vec<Option<&[u8]>> =
            nodes.iter().map(|(_, props)| props.as_deref()).collect();
        let plan = plan(&props);
        if plan.decorated == 0 {
            return 0;
        }
        let mut tagged: c_int = 0;
        for (index, tags) in &plan.tagged {
            let qn = &nodes[*index].0;
            // SAFETY: the node stays valid until the buffer changes below.
            let node = unsafe { cbm_gbuf_find_by_qn(graph, qn.as_ptr()) };
            if node.is_null() {
                continue;
            }
            let node = unsafe { &*node };
            let current = unsafe { owned(node.properties_json) };
            let current = current.as_ref().map_or(&b"{}"[..], |p| p.to_bytes());
            let Some(updated) = with_tags(current, tags) else {
                continue;
            };
            let updated =
                CString::new(updated).expect("written JSON escapes NUL");
            let (label, name, qn, file) = unsafe {
                (
                    owned(node.label),
                    owned(node.name),
                    owned(node.qualified_name),
                    owned(node.file_path),
                )
            };
            let text = |s: &Option<CString>| {
                s.as_ref().map_or(ptr::null(), |s| s.as_ptr())
            };
            unsafe {
                cbm_gbuf_upsert_node(
                    graph,
                    text(&label),
                    text(&name),
                    text(&qn),
                    text(&file),
                    node.start_line,
                    node.end_line,
                    updated.as_ptr(),
                );
            }
            tagged += 1;
        }
        let yes_or_zero = |yes: bool| if yes { c"yes" } else { c"0" };
        unsafe {
            cbm_log(
                LOG_INFO,
                c"pass.decorator_tags".as_ptr(),
                c"candidates".as_ptr(),
                yes_or_zero(plan.has_candidates).as_ptr(),
                c"tagged".as_ptr(),
                yes_or_zero(tagged > 0).as_ptr(),
                ptr::null::<c_char>(),
            );
        }
        tagged
    }
}

#[cfg(test)]
mod tests {
    use super::{plan, split_camel_case, tokenize_decorator, with_tags};

    fn words(decorator: &str) -> Vec<String> {
        tokenize_decorator(decorator.as_bytes(), 16)
            .into_iter()
            .map(|w| String::from_utf8(w).unwrap())
            .collect()
    }

    fn camel(s: &str, max: usize) -> Vec<&str> {
        split_camel_case(s.as_bytes(), max)
            .into_iter()
            .map(|p| std::str::from_utf8(p).unwrap())
            .collect()
    }

    #[test]
    fn splits_camel_case() {
        assert_eq!(camel("GetMapping", 8), ["Get", "Mapping"]);
        assert_eq!(camel("getMessage", 8), ["get", "Message"]);
        assert_eq!(camel("getCamelCase", 16), ["get", "Camel", "Case"]);
        assert_eq!(camel("cache", 8), ["cache"]);
        assert_eq!(camel("HTMLParser", 8), ["HTMLParser"]);
        assert!(camel("", 8).is_empty());
        // Parts past the limit are dropped, not merged into the last one.
        assert_eq!(camel("aBcDeF", 2), ["a", "Bc"]);
    }

    #[test]
    fn tokenizes_decorators() {
        assert_eq!(words("@Override"), ["override"]);
        assert_eq!(words("@Deprecated"), ["deprecated"]);
        assert_eq!(words("@Test"), ["test"]);
        assert_eq!(words("@login_required"), ["login", "required"]);
        assert_eq!(words("@cache"), ["cache"]);
        assert_eq!(words("@pytest.fixture"), ["pytest", "fixture"]);
        assert_eq!(words("@GetMapping(\"/api\")"), ["mapping"]);
        assert_eq!(words("@PostMapping(\"/api\")"), ["post", "mapping"]);
        assert_eq!(words("@Transactional"), ["transactional"]);
        assert_eq!(words("@MessageMapping(\"/chat\")"), ["message", "mapping"]);
        assert_eq!(words("#[test]"), ["test"]);
        assert_eq!(words("#[derive(Debug)]"), ["derive"]);
        assert!(words("@app.get(\"/api\")").is_empty());
        assert_eq!(words("@router.post(\"/api\")"), ["post"]);
        assert!(words("@x").is_empty());
        assert!(words("").is_empty());
        assert_eq!(words("@click.command"), ["click", "command"]);
        assert_eq!(words("@celery.task"), ["celery", "task"]);
    }

    #[test]
    fn tags_words_on_two_nodes() {
        let props: Vec<&[u8]> = vec![
            br#"{"decorators":["@login_required"]}"#,
            br#"{"decorators":["@login_required","@cache"]}"#,
            br#"{"decorators":["@cache"]}"#,
            br#"{"decorators":["@unique_helper"]}"#,
            br#"{"name":"plain"}"#,
        ];
        let props: Vec<Option<&[u8]>> = props.into_iter().map(Some).collect();
        let plan = plan(&props);
        assert_eq!(plan.decorated, 4);
        assert!(plan.has_candidates);
        let tagged: Vec<(usize, Vec<&str>)> = plan
            .tagged
            .iter()
            .map(|(i, tags)| {
                (
                    *i,
                    tags.iter()
                        .map(|t| std::str::from_utf8(t).unwrap())
                        .collect(),
                )
            })
            .collect();
        assert_eq!(
            tagged,
            [
                (0, vec!["login", "required"]),
                (1, vec!["cache", "login", "required"]),
                (2, vec!["cache"]),
            ]
        );
    }

    #[test]
    fn rewrites_properties_like_yyjson() {
        let tags = vec![b"cache".to_vec(), b"login".to_vec()];
        assert_eq!(
            with_tags(
                br#"{"a":1,"decorator_tags":["x"],"b":[1.5,true]}"#,
                &tags
            ),
            Some(
                br#"{"a":1,"b":[1.5,true],"decorator_tags":["cache","login"]}"#
                    .to_vec()
            )
        );
        let only = Some(br#"{"decorator_tags":["cache","login"]}"#.to_vec());
        assert_eq!(with_tags(b"[1]", &tags), only);
        assert_eq!(with_tags(b"{bad", &tags), only);
    }
}
