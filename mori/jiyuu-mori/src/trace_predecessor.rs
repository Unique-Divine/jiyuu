//! Select the captured edge that precedes each visible trace hop.

use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, c_char, c_int};

const NO_EDGE: c_int = -1;

#[repr(C)]
struct Hop {
    id: i64,
    hop: c_int,
    edge_index: c_int,
}

#[repr(C)]
struct Edge {
    source_id: i64,
    target_id: i64,
    kind: *const c_char,
}

impl Edge {
    fn endpoints(&self, inbound: bool) -> (i64, i64) {
        if inbound {
            (self.target_id, self.source_id)
        } else {
            (self.source_id, self.target_id)
        }
    }

    // The input lends a NUL-terminated type or NULL through the projection.
    unsafe fn type_bytes(&self) -> &[u8] {
        if self.kind.is_null() {
            return b"";
        }
        // SAFETY: The planner's caller supplies the borrowed type string.
        unsafe { CStr::from_ptr(self.kind) }.to_bytes()
    }
}

// Each edge's type pointer is borrowed through the call.
unsafe fn project(
    inbound: bool,
    roots: &[i64],
    hops: &mut [Hop],
    edges: &[Edge],
) -> bool {
    let mut roots_by_id = HashSet::new();
    let mut hops_by_id = HashMap::new();
    if roots_by_id.try_reserve(roots.len()).is_err()
        || hops_by_id.try_reserve(hops.len()).is_err()
        || c_int::try_from(edges.len()).is_err()
    {
        return false;
    }
    roots_by_id.extend(roots.iter().copied());
    for (index, row) in hops.iter_mut().enumerate() {
        if row.hop <= 0 || hops_by_id.insert(row.id, (index, row.hop)).is_some()
        {
            return false;
        }
        row.edge_index = NO_EDGE;
    }

    for (edge_index, edge) in edges.iter().enumerate() {
        let (before, after) = edge.endpoints(inbound);
        let Some(&(index, hop)) = hops_by_id.get(&after) else {
            continue;
        };
        let previous_hop = if hop == 1 {
            roots_by_id.contains(&before)
        } else {
            hops_by_id
                .get(&before)
                .is_some_and(|&(_, distance)| distance == hop - 1)
        };
        if !previous_hop {
            continue;
        }

        let row = &mut hops[index];
        if row.edge_index != NO_EDGE {
            let best = &edges[row.edge_index as usize];
            let (best_before, _) = best.endpoints(inbound);
            // SAFETY: Both edge types are borrowed for this call. Strict
            // ordering keeps the first captured edge on an exact tie.
            if before > best_before
                || (before == best_before
                    && unsafe { edge.type_bytes() >= best.type_bytes() })
            {
                continue;
            }
        }
        row.edge_index = edge_index as c_int;
    }
    hops.sort_unstable_by_key(|row| row.id);
    true
}

unsafe extern "C" fn plan(
    inbound: bool,
    roots: *const i64,
    root_count: usize,
    hops: *mut Hop,
    hop_count: usize,
    edges: *const Edge,
    edge_count: usize,
) -> bool {
    // SAFETY: C lends three initialized, disjoint, nonempty arrays and
    // their edge type strings. The callback retains nothing.
    unsafe {
        project(
            inbound,
            std::slice::from_raw_parts(roots, root_count),
            std::slice::from_raw_parts_mut(hops, hop_count),
            std::slice::from_raw_parts(edges, edge_count),
        )
    }
}

#[cfg(not(test))]
unsafe extern "C" {
    fn cbm_mcp_set_trace_predecessor_planner(
        planner: unsafe extern "C" fn(
            bool,
            *const i64,
            usize,
            *mut Hop,
            usize,
            *const Edge,
            usize,
        ) -> bool,
    );
}

#[cfg(not(test))]
pub(crate) unsafe fn register() {
    // SAFETY: Registration happens before native worker threads start.
    unsafe { cbm_mcp_set_trace_predecessor_planner(plan) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declines_repeated_node_ids() {
        let mut hops = [1, 2].map(|hop| Hop {
            id: 20,
            hop,
            edge_index: NO_EDGE,
        });
        // SAFETY: There are no borrowed edge type strings.
        assert!(!unsafe { project(false, &[10], &mut hops, &[]) });
    }

    #[test]
    fn projects_only_previous_visible_hops_and_preserves_capture_order_on_ties()
    {
        let roots = [10, 11];
        for inbound in [false, true] {
            let mut hops = [(21, 1), (20, 1), (30, 2), (10, 3), (40, 2)].map(
                |(id, hop)| Hop {
                    id,
                    hop,
                    edge_index: NO_EDGE,
                },
            );
            let edges = [
                (11, 20, c"CALLS".as_ptr()),
                (10, 20, c"Z".as_ptr()),
                (10, 20, c"A".as_ptr()),
                (10, 20, c"A".as_ptr()),
                (10, 21, c"CALLS".as_ptr()),
                (21, 30, c"A".as_ptr()),
                (20, 30, c"CALLS".as_ptr()),
                (30, 10, c"CALLS".as_ptr()), // return to the root at hop 3
                (30, 40, c"CALLS".as_ptr()), // sideways, never a predecessor
                (99, 40, c"CALLS".as_ptr()), // hidden or missing predecessor
            ]
            .map(|(before, after, kind)| {
                let (source_id, target_id) = if inbound {
                    (after, before)
                } else {
                    (before, after)
                };
                Edge {
                    source_id,
                    target_id,
                    kind,
                }
            });

            // SAFETY: All arrays and type strings remain live through the call.
            assert!(unsafe {
                plan(
                    inbound,
                    roots.as_ptr(),
                    roots.len(),
                    hops.as_mut_ptr(),
                    hops.len(),
                    edges.as_ptr(),
                    edges.len(),
                )
            });
            assert_eq!(
                hops.map(|row| (row.id, row.edge_index)),
                [(10, 7), (20, 2), (21, 4), (30, 6), (40, NO_EDGE)]
            );
        }
    }
}
