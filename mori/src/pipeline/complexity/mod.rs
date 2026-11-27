//! Propagate local loop depth over one generation's CALLS graph.

use std::collections::HashMap;
use std::ffi::c_long;

#[cfg(feature = "native-engine-abi")]
mod native;
#[cfg(feature = "native-engine-abi")]
mod stage;

const MAX_TRAVERSAL_DEPTH: usize = 256;
const DEPTH_KEY: &[u8] = b"\"transitive_loop_depth\":";
const RECURSIVE_KEY: &[u8] = b",\"recursive\":";

#[derive(Default, PartialEq, Eq, PartialOrd, Ord)]
struct NodeKey {
    qualified_name: Vec<u8>,
    file_path: Vec<u8>,
    start_line: i32,
}

#[derive(Default)]
struct Vertex {
    id: i64,
    key: Option<NodeKey>,
    properties: Option<Vec<u8>>,
    calls: Vec<usize>,
}

#[derive(Default)]
struct CallGraph {
    // Store IDs can be sparse after closure patches. Index traversal state by
    // the number of vertices in this pass, independent of the ID ceiling.
    indices: HashMap<i64, usize>,
    vertices: Vec<Vertex>,
}

#[derive(Clone, Copy)]
enum PropertyMode {
    Append,
    Rewrite,
}

struct NodePropertyUpdate {
    id: i64,
    properties: Vec<u8>,
}

#[derive(Default)]
enum Visit {
    #[default]
    Pending,
    Visiting,
    Done(i32),
}

#[derive(Default)]
struct State {
    visit: Visit,
    local_depth: i32,
    recursive: bool,
}

impl CallGraph {
    fn index(&mut self, id: i64) -> usize {
        *self.indices.entry(id).or_insert_with(|| {
            let index = self.vertices.len();
            self.vertices.push(Vertex {
                id,
                ..Vertex::default()
            });
            index
        })
    }

    fn set_node(&mut self, id: i64, key: NodeKey, properties: Option<Vec<u8>>) {
        let index = self.index(id);
        self.vertices[index].key = Some(key);
        self.vertices[index].properties = properties;
    }

    fn add_call(&mut self, source_id: i64, target_id: i64) -> bool {
        let source = self.index(source_id);
        let target = self.index(target_id);
        self.vertices[source].calls.push(target);
        self.vertices[target].key.is_none()
    }

    fn updates(
        mut self,
        mode: PropertyMode,
    ) -> Result<Vec<NodePropertyUpdate>, MalformedPair> {
        // The first member entered in a cycle gets the flag. Seed and callee
        // order must therefore come from source content.
        let mut seeds: Vec<_> = self
            .vertices
            .iter()
            .enumerate()
            .filter_map(|(index, vertex)| {
                vertex.properties.as_ref().map(|_| index)
            })
            .collect();
        seeds.sort_by_key(|&index| &self.vertices[index].key);

        let missing_key = NodeKey::default();
        for index in 0..self.vertices.len() {
            let mut calls = std::mem::take(&mut self.vertices[index].calls);
            calls.sort_by_key(|&target| {
                self.vertices[target].key.as_ref().unwrap_or(&missing_key)
            });
            self.vertices[index].calls = calls;
        }

        let mut states: Vec<_> = self
            .vertices
            .iter()
            .map(|vertex| {
                let Some(props) = &vertex.properties else {
                    return State::default();
                };
                State {
                    local_depth: local_depth(props),
                    recursive: field_start(props, b"\"self_recursive\":")
                        .iter()
                        .find(|&&byte| byte != b' ' && byte != b'\t')
                        == Some(&b't'),
                    ..State::default()
                }
            })
            .collect();
        for &index in &seeds {
            visit(&self.vertices, &mut states, index, 0);
        }

        seeds
            .into_iter()
            .filter_map(|index| {
                let state = &states[index];
                let Visit::Done(depth) = state.visit else {
                    unreachable!("every seed was visited at depth zero");
                };
                let props = self.vertices[index].properties.as_deref().unwrap();
                match set_properties(props, depth, state.recursive, mode) {
                    Ok(Some(properties)) => Some(Ok(NodePropertyUpdate {
                        id: self.vertices[index].id,
                        properties,
                    })),
                    Ok(None) => None,
                    Err(error) => Some(Err(error)),
                }
            })
            .collect()
    }
}

fn visit(
    vertices: &[Vertex],
    states: &mut [State],
    index: usize,
    depth: usize,
) -> i32 {
    match states[index].visit {
        Visit::Done(result) => return result,
        Visit::Visiting => {
            states[index].recursive = true;
            return 0;
        }
        Visit::Pending => {}
    }
    if depth > MAX_TRAVERSAL_DEPTH {
        return states[index].local_depth;
    }

    states[index].visit = Visit::Visiting;
    let mut best = 0;
    for &target in &vertices[index].calls {
        if target == index {
            states[index].recursive = true;
            continue;
        }
        best = best.max(visit(vertices, states, target, depth + 1));
    }
    let result = states[index].local_depth.saturating_add(best);
    states[index].visit = Visit::Done(result);
    result
}

fn find(bytes: &[u8], key: &[u8]) -> Option<usize> {
    bytes.windows(key.len()).position(|window| window == key)
}

// Extraction writes flat, compact JSON. Match the reference's literal keys
// and preserve the bytes of properties this pass does not own.
fn field_start<'a>(props: &'a [u8], key: &[u8]) -> &'a [u8] {
    let Some(start) = find(props, key) else {
        return &[];
    };
    &props[start + key.len()..]
}

fn local_depth(props: &[u8]) -> i32 {
    let value = field_start(props, b"\"loop_depth\":").trim_ascii_start();
    let digits = match value.first() {
        Some(b'-' | b'+') => &value[1..],
        _ => value,
    };
    let length = digits
        .iter()
        .take_while(|digit| digit.is_ascii_digit())
        .count();
    if length == 0 {
        return 0;
    }
    // The scanned prefix contains only an optional sign and ASCII digits.
    std::str::from_utf8(&value[..value.len() - digits.len() + length])
        .ok()
        .map_or(0, |prefix| {
            prefix.parse::<c_long>().unwrap_or_else(|_| {
                if prefix.starts_with('-') {
                    c_long::MIN
                } else {
                    c_long::MAX
                }
            }) as i32
        })
}

#[derive(Debug, PartialEq, Eq)]
struct MalformedPair;

fn set_properties(
    old: &[u8],
    depth: i32,
    recursive: bool,
    mode: PropertyMode,
) -> Result<Option<Vec<u8>>, MalformedPair> {
    let value =
        format!("\"transitive_loop_depth\":{depth},\"recursive\":{recursive}");
    if let PropertyMode::Rewrite = mode
        && let Some(head) = find(old, DEPTH_KEY)
    {
        let tail = &old[head + DEPTH_KEY.len()..];
        let digits = tail.strip_prefix(b"-").unwrap_or(tail);
        let digits_end = digits
            .iter()
            .take_while(|digit| digit.is_ascii_digit())
            .count();
        let tail = digits[digits_end..]
            .strip_prefix(RECURSIVE_KEY)
            .ok_or(MalformedPair)?;
        let rest = tail
            .strip_prefix(b"true")
            .or_else(|| tail.strip_prefix(b"false"))
            .ok_or(MalformedPair)?;
        return Ok(Some([&old[..head], value.as_bytes(), rest].concat()));
    }
    if old.len() < 2 || old.last() != Some(&b'}') {
        return Ok(None);
    }
    let separator: &[u8] = if old.len() == 2 { b"" } else { b"," };
    Ok(Some(
        [&old[..old.len() - 1], separator, value.as_bytes(), b"}"].concat(),
    ))
}

#[cfg(test)]
mod tests;
