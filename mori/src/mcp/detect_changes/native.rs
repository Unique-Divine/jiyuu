use crate::mcp_impact_graph::{
    ImpactWalk, Node, Store, StoreRows, WalkPolicy, cbm_impact_walk_close,
    cbm_impact_walk_count, cbm_impact_walk_hits, cbm_impact_walk_run,
    cbm_store_free_nodes, mark_written, view,
};
use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr::{null, null_mut};
use std::slice;

pub(super) type Text = *const c_char;

#[repr(C)]
pub struct Hunk {
    pub(super) path: [c_char; 512],
    pub(super) start_line: c_int,
    pub(super) end_line: c_int,
}

#[repr(C)]
pub struct Request {
    pub(super) store: *mut Store,
    pub(super) project: Text,
    files: *const Text,
    file_count: c_int,
    hunks: *const Hunk,
    hunk_count: c_int,
    pub(super) product_config: *const c_void,
    pub(super) list_seeds: bool,
    pub(super) want_symbols: bool,
    direction: Text,
    depth: c_int,
    bfs_limit: c_int,
    policy: *const WalkPolicy,
    pub(super) via: bool,
}

impl Request {
    // These views and the request's store are live for the duration of the call.
    pub(super) unsafe fn files(&self) -> &[Text] {
        unsafe { view(self.files, self.file_count as usize) }
    }

    pub(super) unsafe fn hunks(&self) -> &[Hunk] {
        unsafe { view(self.hunks, self.hunk_count as usize) }
    }

    pub(super) unsafe fn nodes(
        &self,
        file: Text,
    ) -> Result<StoreRows<Node>, ()> {
        StoreRows::read(cbm_store_free_nodes, |rows, count| unsafe {
            cbm_store_find_nodes_by_file(
                self.store,
                self.project,
                file,
                rows,
                count,
            )
        })
    }

    pub(super) unsafe fn traverse(
        &self,
        seeds: &[i64],
        count: c_int,
    ) -> Result<Impact, ()> {
        if seeds.is_empty() || !self.want_symbols {
            return Ok(Impact::default());
        }
        mark_written(seeds);
        if self.policy.is_null() {
            let mut result = Traversal::default();
            let mut saturated = false;
            // Match the default path: even a failed scan can publish rows.
            unsafe {
                cbm_store_bfs_multi(
                    self.store,
                    seeds.as_ptr(),
                    count,
                    self.direction,
                    null(),
                    0,
                    self.depth,
                    self.bfs_limit,
                    &mut result,
                    &mut saturated,
                )
            };
            Ok(Impact::Bounded(result, saturated))
        } else {
            let mut ptr = null_mut();
            if unsafe { cbm_impact_walk_open(self.store, self.policy, &mut ptr) }
                != 0
            {
                return Err(());
            }
            let walk = Walk(ptr);
            if unsafe { cbm_impact_walk_run(walk.0, seeds.as_ptr(), count) } != 0
            {
                return Err(());
            }
            let total = unsafe { cbm_impact_walk_count(walk.0) } as usize;
            let hits = unsafe { view(cbm_impact_walk_hits(walk.0), total) };
            let mut rows = WalkRows(Vec::new());
            rows.0.try_reserve_exact(total).map_err(|_| ())?;
            for hit in hits.iter().filter(|hit| hit.hop != 0) {
                let mut node = Node::default();
                if unsafe {
                    cbm_store_find_node_by_id(self.store, hit.id, &mut node)
                } != 0
                {
                    return Err(());
                }
                rows.0.push(NodeHop { node, hop: hit.hop });
            }
            Ok(Impact::Walk(rows, walk))
        }
    }
}

#[repr(C)]
pub(super) struct NodeHop {
    pub(super) node: Node,
    hop: c_int,
}

#[repr(C)]
#[derive(Default)]
pub(super) struct Traversal {
    root: Node,
    visited: *mut NodeHop,
    visited_count: c_int,
    edges: *mut c_void,
    edge_count: c_int,
    truncated: bool,
    edges_truncated: bool,
}

impl Drop for Traversal {
    fn drop(&mut self) {
        // SAFETY: Native BFS populated this result and owns its allocations.
        unsafe { cbm_store_traverse_free(self) };
    }
}

pub(super) struct WalkRows(Vec<NodeHop>);

impl Drop for WalkRows {
    fn drop(&mut self) {
        // Only the fields in each row were allocated by C.
        for row in &mut self.0 {
            unsafe { cbm_node_free_fields(&mut row.node) };
        }
    }
}

pub(super) struct Walk(*mut ImpactWalk);

impl Drop for Walk {
    fn drop(&mut self) {
        unsafe { cbm_impact_walk_close(self.0) };
    }
}

#[derive(Default)]
pub(super) enum Impact {
    #[default]
    Empty,
    Bounded(Traversal, bool),
    Walk(WalkRows, Walk),
}

impl Impact {
    pub(super) fn rows(&self) -> &[NodeHop] {
        match self {
            Self::Empty => &[],
            Self::Bounded(result, _) => unsafe {
                view(result.visited, result.visited_count as usize)
            },
            Self::Walk(rows, _) => &rows.0,
        }
    }

    pub(super) fn saturated(&self) -> bool {
        matches!(self, Self::Bounded(_, true))
    }

    pub(super) fn via_walk(&self, via: bool) -> *const ImpactWalk {
        match self {
            Self::Walk(_, walk) if via => walk.0,
            _ => null(),
        }
    }
}

unsafe extern "C" {
    fn cbm_store_find_nodes_by_file(
        store: *mut Store,
        project: Text,
        file: Text,
        rows: *mut *mut Node,
        count: *mut c_int,
    ) -> c_int;
    fn cbm_store_find_node_by_id(
        store: *mut Store,
        id: i64,
        out: *mut Node,
    ) -> c_int;
    fn cbm_store_bfs_multi(
        store: *mut Store,
        seeds: *const i64,
        count: c_int,
        direction: Text,
        types: *const Text,
        type_count: c_int,
        depth: c_int,
        limit: c_int,
        out: *mut Traversal,
        saturated: *mut bool,
    ) -> c_int;
    fn cbm_store_traverse_free(result: *mut Traversal);
    fn cbm_node_free_fields(node: *mut Node);
    fn cbm_impact_walk_open(
        store: *mut Store,
        policy: *const WalkPolicy,
        out: *mut *mut ImpactWalk,
    ) -> c_int;
    fn cbm_test_config_is_test_file(config: *const c_void, file: Text) -> bool;
    fn cbm_detect_seed_is_test(node: *const Node) -> bool;
}

pub(super) unsafe fn product_seed(config: *const c_void, node: &Node) -> bool {
    config.is_null() || !unsafe { cbm_detect_seed_is_test(node) }
}

pub(super) unsafe fn test_file(config: *const c_void, file: Text) -> bool {
    !config.is_null() && unsafe { cbm_test_config_is_test_file(config, file) }
}

pub(super) fn in_hunks(node: &Node, hunks: &[Hunk], file: &CStr) -> bool {
    hunks.iter().any(|hunk| {
        node.start_line <= hunk.end_line
            && node.end_line >= hunk.start_line
            // SAFETY: The native parser terminates each path inside its buffer.
            && unsafe { CStr::from_ptr(hunk.path.as_ptr()) } == file
    })
}

// Existing native tests call the same predicate as production.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_detect_node_in_hunks(
    node: *const Node,
    hunks: *const Hunk,
    hunk_count: c_int,
    file: Text,
) -> bool {
    if hunk_count <= 0 {
        return false;
    }
    // SAFETY: C passes a live node, hunk array and terminated path.
    in_hunks(
        unsafe { &*node },
        unsafe { view(hunks, hunk_count as usize) },
        unsafe { CStr::from_ptr(file) },
    )
}

pub(super) fn mark_value<T>(value: &T) {
    mark_written(slice::from_ref(value));
}
