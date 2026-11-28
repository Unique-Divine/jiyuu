//! Leiden communities for architecture reports and Graph UI layout regions.

use std::collections::{HashMap, VecDeque};
use std::ffi::c_int;
use std::slice;

const MAX_LEVELS: usize = 64;
const MOVE_PASSES: usize = 100;
const UNASSIGNED: usize = usize::MAX;

#[derive(Clone, Copy)]
#[repr(C)]
struct Edge {
    source: i64,
    target: i64,
}

#[repr(C)]
struct Community {
    node_id: i64,
    community: c_int,
}

struct WeightedEdge {
    source: usize,
    target: usize,
    weight: f64,
}

#[derive(Clone, Copy, Default)]
struct Neighbor {
    node: usize,
    weight: f64,
}

/// Only inter-community edges enter the adjacency array after aggregation.
/// Degrees also retain collapsed intra-community weight for the null model.
struct Graph {
    offsets: Vec<usize>,
    neighbors: Vec<Neighbor>,
    degrees: Vec<f64>,
}

fn reserved<T>(capacity: usize) -> Option<Vec<T>> {
    let mut vec = Vec::new();
    vec.try_reserve_exact(capacity).ok()?;
    Some(vec)
}

fn filled<T: Copy>(len: usize, value: T) -> Option<Vec<T>> {
    let mut vec = reserved(len)?;
    vec.resize(len, value);
    Some(vec)
}

fn identity(len: usize) -> Option<Vec<usize>> {
    let mut order = reserved(len)?;
    order.extend(0..len);
    Some(order)
}

fn weighted_edges(nodes: &[i64], edges: &[Edge]) -> Option<Vec<WeightedEdge>> {
    let mut index = HashMap::new();
    index.try_reserve(nodes.len()).ok()?;
    for (position, &id) in nodes.iter().enumerate() {
        index.entry(id).or_insert(position);
    }

    let mut keys = HashMap::<(usize, usize), usize>::new();
    let mut unique: Vec<WeightedEdge> = Vec::new();
    for edge in edges {
        let (Some(&a), Some(&b)) =
            (index.get(&edge.source), index.get(&edge.target))
        else {
            continue;
        };
        if a == b {
            continue;
        }
        let key @ (source, target) = (a.min(b), a.max(b));
        if let Some(&position) = keys.get(&key) {
            unique[position].weight += 1.0;
        } else {
            keys.try_reserve(1).ok()?;
            unique.try_reserve(1).ok()?;
            keys.insert(key, unique.len());
            unique.push(WeightedEdge {
                source,
                target,
                weight: 1.0,
            });
        }
    }
    Some(unique)
}

fn accumulate_offsets(offsets: &mut [usize]) -> Option<()> {
    for index in 1..offsets.len() {
        offsets[index] = offsets[index - 1].checked_add(offsets[index])?;
    }
    Some(())
}

impl Graph {
    fn build(nodes: usize, edges: &[WeightedEdge]) -> Option<Self> {
        let mut offsets = filled(nodes + 1, 0)?;
        for edge in edges {
            offsets[edge.source + 1] += 1;
            offsets[edge.target + 1] += 1;
        }
        accumulate_offsets(&mut offsets)?;

        let mut neighbors = filled(offsets[nodes], Neighbor::default())?;
        let mut degrees = filled(nodes, 0.0)?;
        let mut next = reserved(nodes)?;
        next.extend_from_slice(&offsets[..nodes]);
        for &WeightedEdge {
            source,
            target,
            weight,
        } in edges
        {
            neighbors[next[source]] = Neighbor {
                node: target,
                weight,
            };
            neighbors[next[target]] = Neighbor {
                node: source,
                weight,
            };
            next[source] += 1;
            next[target] += 1;
            degrees[source] += weight;
            degrees[target] += weight;
        }
        Some(Self {
            offsets,
            neighbors,
            degrees,
        })
    }

    fn neighbors(&self, node: usize) -> &[Neighbor] {
        &self.neighbors[self.offsets[node]..self.offsets[node + 1]]
    }
}

struct Scratch {
    weights: Vec<f64>,
    touched: Vec<usize>,
}

impl Scratch {
    fn new(len: usize) -> Option<Self> {
        Some(Self {
            weights: filled(len, 0.0)?,
            touched: reserved(len)?,
        })
    }

    fn add(&mut self, index: usize, weight: f64) {
        if self.weights[index] == 0.0 {
            self.touched.push(index);
        }
        self.weights[index] += weight;
    }

    fn clear(&mut self) {
        for index in self.touched.drain(..) {
            self.weights[index] = 0.0;
        }
    }
}

fn move_nodes(
    graph: &Graph,
    labels: &mut [usize],
    gamma: f64,
    total: f64,
) -> Option<()> {
    let len = labels.len();
    let mut degrees = filled(len, 0.0)?;
    let mut scratch = Scratch::new(len)?;
    let mut pending = filled(len, true)?;
    let mut queue = VecDeque::new();
    queue.try_reserve_exact(len).ok()?;
    queue.extend(0..len);
    for (node, &cluster) in labels.iter().enumerate() {
        degrees[cluster] += graph.degrees[node];
    }

    let cap = len.saturating_mul(MOVE_PASSES).saturating_add(MAX_LEVELS);
    for _ in 0..cap {
        let Some(node) = queue.pop_front() else {
            break;
        };
        pending[node] = false;
        let current = labels[node];
        for &other in graph.neighbors(node) {
            if other.node != node {
                scratch.add(labels[other.node], other.weight);
            }
        }
        let degree = graph.degrees[node];
        degrees[current] -= degree;
        let gain = |cluster| {
            scratch.weights[cluster] - gamma * degree * degrees[cluster] / total
        };
        let mut best = current;
        let mut best_gain = gain(current);
        // Equal gain keeps the first community encountered in adjacency order.
        for &cluster in &scratch.touched {
            let candidate = gain(cluster);
            if candidate > best_gain {
                best = cluster;
                best_gain = candidate;
            }
        }
        degrees[best] += degree;
        labels[node] = best;
        if best != current {
            for &other in graph.neighbors(node) {
                if labels[other.node] != best && !pending[other.node] {
                    queue.push_back(other.node);
                    pending[other.node] = true;
                }
            }
        }
        scratch.clear();
    }
    Some(())
}

fn compact(labels: &mut [usize]) -> Option<usize> {
    let mut map = filled(labels.len(), UNASSIGNED)?;
    let mut next = 0;
    for label in labels {
        let dense = &mut map[*label];
        if *dense == UNASSIGNED {
            *dense = next;
            next += 1;
        }
        *label = *dense;
    }
    Some(next)
}

fn refine(
    graph: &Graph,
    moved: &[usize],
    gamma: f64,
    total: f64,
) -> Option<(Vec<usize>, usize)> {
    let len = moved.len();
    let mut labels = identity(len)?;
    let mut degrees = reserved(len)?;
    degrees.extend_from_slice(&graph.degrees);
    let mut sizes = filled::<usize>(len, 1)?;
    let mut scratch = Scratch::new(len)?;

    for (node, &cluster) in moved.iter().enumerate() {
        let current = labels[node];
        if sizes[current] != 1 {
            continue;
        }
        for &other in graph.neighbors(node) {
            if other.node != node && moved[other.node] == cluster {
                scratch.add(labels[other.node], other.weight);
            }
        }
        let degree = graph.degrees[node];
        degrees[current] -= degree;
        let mut best = current;
        let mut best_gain = 0.0;
        for &candidate in &scratch.touched {
            if candidate != current {
                let gain = scratch.weights[candidate]
                    - gamma * degree * degrees[candidate] / total;
                if gain > best_gain {
                    best = candidate;
                    best_gain = gain;
                }
            }
        }
        if best != current {
            labels[node] = best;
            sizes[current] -= 1;
            sizes[best] += 1;
        }
        degrees[best] += degree;
        scratch.clear();
    }
    let count = compact(&mut labels)?;
    Some((labels, count))
}

fn aggregate(
    graph: &Graph,
    refined: &[usize],
    count: usize,
    moved: &[usize],
) -> Option<(Graph, Vec<usize>)> {
    let mut degrees = filled(count, 0.0)?;
    let mut member_offsets = filled(count + 1, 0)?;
    let mut seed = filled(count, UNASSIGNED)?;
    for (node, &cluster) in refined.iter().enumerate() {
        degrees[cluster] += graph.degrees[node];
        member_offsets[cluster + 1] += 1;
        if seed[cluster] == UNASSIGNED {
            seed[cluster] = moved[node];
        }
    }
    accumulate_offsets(&mut member_offsets)?;
    let mut members = filled(refined.len(), 0)?;
    let mut next = reserved(count)?;
    next.extend_from_slice(&member_offsets[..count]);
    for (node, &cluster) in refined.iter().enumerate() {
        members[next[cluster]] = node;
        next[cluster] += 1;
    }

    let members_of =
        |cluster| &members[member_offsets[cluster]..member_offsets[cluster + 1]];
    let mut offsets = filled(count + 1, 0)?;
    let mut seen = filled(count, UNASSIGNED)?;
    for cluster in 0..count {
        for &member in members_of(cluster) {
            for &neighbor in graph.neighbors(member) {
                let other = refined[neighbor.node];
                if other != cluster && seen[other] != cluster {
                    seen[other] = cluster;
                    offsets[cluster + 1] += 1;
                }
            }
        }
    }
    accumulate_offsets(&mut offsets)?;

    let mut neighbors = reserved(offsets[count])?;
    let mut scratch = Scratch::new(count)?;
    for cluster in 0..count {
        for &member in members_of(cluster) {
            for &neighbor in graph.neighbors(member) {
                let other = refined[neighbor.node];
                if other != cluster {
                    scratch.add(other, neighbor.weight);
                }
            }
        }
        neighbors.extend(scratch.touched.iter().map(|&node| Neighbor {
            node,
            weight: scratch.weights[node],
        }));
        scratch.clear();
    }
    Some((
        Graph {
            offsets,
            neighbors,
            degrees,
        },
        seed,
    ))
}

fn partition(
    nodes: &[i64],
    edges: &[Edge],
    resolution: f64,
) -> Option<Vec<usize>> {
    let edges = weighted_edges(nodes, edges)?;
    if edges.is_empty() {
        return None;
    }
    let mut graph = Graph::build(nodes.len(), &edges)?;
    drop(edges);
    let total = graph.degrees.iter().fold(0.0, |sum, degree| sum + degree);
    let gamma = if resolution > 0.0 { resolution } else { 1.0 };
    let mut original = identity(nodes.len())?;
    let mut moved = identity(nodes.len())?;

    for _ in 0..MAX_LEVELS {
        if move_nodes(&graph, &mut moved, gamma, total).is_none() {
            break;
        }
        let Some(count) = compact(&mut moved) else {
            break;
        };
        if count >= moved.len() {
            break;
        }
        let Some((refined, count)) = refine(&graph, &moved, gamma, total) else {
            break;
        };
        if count >= moved.len() {
            break;
        }
        let Some((next, seed)) = aggregate(&graph, &refined, count, &moved)
        else {
            break;
        };
        // Keep the last completed partition usable if coarsening runs out of memory.
        for node in &mut original {
            *node = refined[*node];
        }
        graph = next;
        moved = seed;
    }
    for node in &mut original {
        *node = moved[*node];
    }
    Some(original)
}

#[cfg_attr(feature = "native-engine-abi", unsafe(no_mangle))]
unsafe extern "C" fn cbm_rust_leiden_partition(
    nodes: *const i64,
    count: c_int,
    edges: *const Edge,
    edge_count: c_int,
    resolution: f64,
    rows: *mut Community,
) {
    if count < 2 || edge_count <= 0 {
        return;
    }
    // SAFETY: The caller lends two immutable input arrays and an initialized,
    // exclusive result array of count rows for the duration of the algorithm.
    let (nodes, edges) = unsafe {
        (
            slice::from_raw_parts(nodes, count as usize),
            slice::from_raw_parts(edges, edge_count as usize),
        )
    };
    let Some(labels) = partition(nodes, edges, resolution) else {
        return; // The C-owned output already holds the singleton partition.
    };
    let rows = unsafe { slice::from_raw_parts_mut(rows, count as usize) };
    for (row, label) in rows.iter_mut().zip(labels) {
        row.community = label as c_int;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partitions_weighted_edges_in_original_row_order() {
        let nodes = [i64::MIN, 7, 2, 9, 8, 7, i64::MAX];
        let edges = [
            (i64::MIN, 7),
            (7, 2),
            (i64::MIN, 2),
            (7, i64::MIN),
            (9, 8),
            (i64::MAX, i64::MAX),
            (3, 8),
        ]
        .map(|(source, target)| Edge { source, target });
        let mut rows = std::array::from_fn::<_, 7, _>(|index| Community {
            node_id: nodes[index],
            community: index as c_int,
        });
        // SAFETY: The arrays remain live and the rows are uniquely borrowed.
        unsafe {
            cbm_rust_leiden_partition(
                nodes.as_ptr(),
                nodes.len() as c_int,
                edges.as_ptr(),
                edges.len() as c_int,
                f64::NAN,
                rows.as_mut_ptr(),
            );
        }
        assert_eq!(rows.each_ref().map(|row| row.node_id), nodes);
        assert_eq!(rows.map(|row| row.community), [0, 0, 0, 1, 1, 2, 3]);
    }

    #[test]
    fn resolution_controls_path_granularity() {
        let nodes: Vec<_> = (0..30).collect();
        let edges: Vec<_> = (0..29)
            .map(|source| Edge {
                source,
                target: source + 1,
            })
            .collect();
        let low = partition(&nodes, &edges, 0.1).unwrap();
        let high = partition(&nodes, &edges, 5.0).unwrap();
        let clusters = |labels: &[usize]| {
            labels
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>()
                .len()
        };
        assert!(clusters(&low) < clusters(&high));
        for labels in [low, high] {
            for cluster in 0..clusters(&labels) {
                let start =
                    labels.iter().position(|&label| label == cluster).unwrap();
                let end =
                    labels.iter().rposition(|&label| label == cluster).unwrap();
                assert!(
                    labels[start..=end].iter().all(|&label| label == cluster)
                );
            }
        }
    }
}
