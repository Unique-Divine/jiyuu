//! Select changed definitions and materialize impact from the request's store.

mod native;

use crate::mcp_impact_graph::{
    ImpactWalk, Node, bytes, mark_written, ptr_or_null,
};
use native::{
    Impact, NodeHop, Request, Text, in_hunks, mark_value, product_seed,
    test_file,
};
use std::alloc::{Layout, alloc};
use std::ffi::{CStr, c_int};
use std::ptr::{NonNull, null, null_mut};

const MODULE_CAP: usize = 256;

fn seedable(node: &&Node) -> bool {
    if node.label.is_null() {
        return false;
    }
    // SAFETY: The store owns a terminated label until its result is freed.
    !matches!(
        unsafe { bytes(node.label) },
        b"File" | b"Folder" | b"Project" | b"Module" | b"Package" | b"Section"
    )
}

// Keep path and graph strings as bytes through the native renderers.
fn copy_text(bytes: &[u8]) -> Result<Vec<u8>, ()> {
    let mut text = Vec::new();
    text.try_reserve_exact(bytes.len() + 1).map_err(|_| ())?;
    text.extend_from_slice(bytes);
    text.push(0);
    mark_written(&text);
    Ok(text)
}

struct Seed {
    qn: Vec<u8>,
    file: Vec<u8>,
    label: Vec<u8>,
    start: c_int,
    end: c_int,
    hunk: bool,
}

impl Seed {
    fn order(&self) -> (&[u8], &[u8], c_int, c_int, &[u8]) {
        (&self.qn, &self.file, self.start, self.end, &self.label)
    }

    fn row(&self) -> SeedRow {
        SeedRow {
            qn: self.qn.as_ptr().cast(),
            file: self.file.as_ptr().cast(),
            label: self.label.as_ptr().cast(),
            reason: if self.hunk { c"hunk" } else { c"whole_file" }.as_ptr(),
            start: self.start,
            end: self.end,
        }
    }
}

#[repr(C)]
struct SeedRow {
    qn: Text,
    label: Text,
    file: Text,
    reason: Text,
    start: c_int,
    end: c_int,
}

#[repr(C)]
struct ModuleRow {
    name: Text,
    count: c_int,
}

#[derive(Default)]
struct Modules {
    names: Vec<Vec<u8>>,
    rows: Vec<ModuleRow>,
    overflow: c_int,
}

fn module_key(path: &[u8]) -> &[u8] {
    if path.is_empty() {
        return b"(root)";
    }
    let second_slash =
        path.iter().enumerate().filter(|&(_, b)| *b == b'/').nth(1);
    &path[..second_slash.map_or(path.len(), |(position, _)| position)]
}

impl Modules {
    fn collect(impact: &[NodeHop]) -> Self {
        let mut modules = Self::default();
        if impact.is_empty()
            || modules.names.try_reserve_exact(MODULE_CAP).is_err()
            || modules.rows.try_reserve_exact(MODULE_CAP).is_err()
        {
            return modules;
        }
        for row in impact {
            // SAFETY: The node fields live for the lifetime of the impact rows.
            let key = module_key(unsafe { bytes(row.node.file_path) });
            if let Some(index) = modules
                .names
                .iter()
                .position(|name| name[..name.len() - 1] == *key)
            {
                modules.rows[index].count += 1;
            } else if modules.rows.len() < MODULE_CAP
                && let Ok(name) = copy_text(key)
            {
                modules.rows.push(ModuleRow {
                    name: name.as_ptr().cast(),
                    count: 1,
                });
                modules.names.push(name);
            } else {
                modules.overflow += 1;
            }
        }
        modules
    }
}

#[repr(C)]
#[derive(Default)]
pub struct GraphView {
    seeds: *const SeedRow,
    seed_total: c_int,
    seed_count: c_int,
    impacted: *const NodeHop,
    impacted_count: c_int,
    saturated: bool,
    via_walk: *const ImpactWalk,
    modules: *const ModuleRow,
    module_count: c_int,
    module_overflow: c_int,
    error: Text,
}

#[derive(Default)]
pub struct Graph {
    seeds: Vec<Seed>,
    seed_rows: Vec<SeedRow>,
    impact: Impact,
    modules: Modules,
    seed_count: c_int,
}

impl Graph {
    unsafe fn collect_seeds(
        &mut self,
        request: &Request,
    ) -> Result<Vec<i64>, ()> {
        // SAFETY: The caller passes live arrays, a store and any test config.
        let files = unsafe { request.files() };
        let hunks = unsafe { request.hunks() };
        let mut ids = Vec::new();
        for &file in files {
            let nodes = unsafe { request.nodes(file) }?;
            if unsafe { test_file(request.product_config, file) } {
                continue;
            }
            let path = unsafe { CStr::from_ptr(file) };
            let candidates = nodes.iter().filter(seedable);
            // Probe before filtering test symbols. A test-only hunk must not
            // fall back to the untouched product definitions in the file.
            let scoped =
                candidates.clone().any(|node| in_hunks(node, hunks, path));
            for node in candidates {
                if (scoped && !in_hunks(node, hunks, path))
                    || !unsafe { product_seed(request.product_config, node) }
                {
                    continue;
                }
                ids.try_reserve(1).map_err(|_| ())?;
                ids.push(node.id);
                if request.list_seeds {
                    self.seeds.try_reserve(1).map_err(|_| ())?;
                    self.seeds.push(Seed {
                        qn: copy_text(unsafe { bytes(node.qualified_name) })?,
                        file: copy_text(unsafe { bytes(node.file_path) })?,
                        label: copy_text(unsafe { bytes(node.label) })?,
                        start: node.start_line,
                        end: node.end_line,
                        hunk: scoped,
                    });
                }
            }
        }
        self.seed_count = c_int::try_from(ids.len()).map_err(|_| ())?;
        self.seeds
            .sort_unstable_by(|a, b| a.order().cmp(&b.order()));
        self.seed_rows
            .try_reserve_exact(self.seeds.len())
            .map_err(|_| ())?;
        self.seed_rows.extend(self.seeds.iter().map(Seed::row));
        Ok(ids)
    }

    fn view(&self, via: bool, error: Text) -> GraphView {
        let impacted = self.impact.rows();
        mark_written(impacted);
        mark_written(&self.seed_rows);
        mark_written(&self.modules.rows);
        GraphView {
            seeds: ptr_or_null(&self.seed_rows),
            seed_total: self.seed_rows.len() as c_int,
            seed_count: self.seed_count,
            impacted: ptr_or_null(impacted),
            impacted_count: impacted.len() as c_int,
            saturated: self.impact.saturated(),
            via_walk: self.impact.via_walk(via),
            modules: ptr_or_null(&self.modules.rows),
            module_count: self.modules.rows.len() as c_int,
            module_overflow: self.modules.overflow,
            error,
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_rust_detect_graph(
    request: &Request,
    view: &mut GraphView,
) -> *mut Graph {
    const SEED_ERROR: &CStr =
        c"seed_collection_failed: no partial answer is returned";
    // SAFETY: The handler borrows its store, policy and path arrays for this call.
    let seed_failure = |view: &mut GraphView| {
        *view = GraphView {
            error: SEED_ERROR.as_ptr(),
            ..GraphView::default()
        };
        mark_value(view);
        null_mut()
    };
    let mut graph = Graph::default();
    let Ok(ids) = (unsafe { graph.collect_seeds(request) }) else {
        return seed_failure(view);
    };
    // Use the same allocation layout as Box so an allocation refusal can
    // produce a request error instead of aborting the process.
    let Some(owner) =
        NonNull::new(unsafe { alloc(Layout::new::<Graph>()) }.cast::<Graph>())
    else {
        return seed_failure(view);
    };
    let mut error = null();
    match unsafe { request.traverse(&ids, graph.seed_count) } {
        Ok(impact) => {
            graph.modules = Modules::collect(impact.rows());
            graph.impact = impact;
        }
        Err(()) => {
            error = c"impact_walk_failed: the impact walk could not be completed — no partial answer is returned; rerun, or drop edge_types, fixpoint and via for the bounded default traversal".as_ptr();
        }
    }
    *view = graph.view(request.via, error);
    mark_value(view);
    // SAFETY: owner has the size and alignment of Graph and is uninitialized.
    unsafe { owner.as_ptr().write(graph) };
    owner.as_ptr()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_rust_detect_graph_free(graph: *mut Graph) {
    if !graph.is_null() {
        // SAFETY: C returns the owner once, after the last use of its view.
        drop(unsafe { Box::from_raw(graph) });
    }
}
