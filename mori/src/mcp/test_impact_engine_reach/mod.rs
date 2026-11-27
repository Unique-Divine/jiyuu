//! Static test-impact selection inputs for the engine's pinned HEAD graph.
mod native;
mod scan;
mod score;

use native::*;
use std::ffi::CStr;
use std::ptr::{null, null_mut};

const PROJECT: &CStr = c"test-impact";
// Edge order also ranks competing parents in the native walk.
const WALK_TYPES: [&CStr; 6] = [
    c"CALLS",
    c"ASYNC_CALLS",
    c"USAGE",
    c"CALL_REFERENCE",
    c"READS",
    c"SPAWNS",
];

struct Reach<'a> {
    input: &'a Input,
    cases: &'a [Case],
    suites: &'a [Suite],
    runners: &'a [Runner],
    rules: &'a [Rule],
}

#[derive(Default)]
struct Sinks<'a> {
    all: Scratch<Id>,
    main: Scratch<Id>,
    roles: Scratch<scan::Role<'a>>,
}

struct Walk(*mut ImpactWalk);

impl Walk {
    fn reached(&self, id: Id) -> bool {
        unsafe { cbm_impact_walk_reached(self.0, id) }
    }

    fn count(&self) -> Index {
        unsafe { cbm_impact_walk_count(self.0) }
    }

    fn hits(&self) -> &[Hit] {
        let count = self.count();
        let hits = unsafe { cbm_impact_walk_hits(self.0) };
        // The scope can become cancelled between the two accessor calls.
        if hits.is_null() {
            &[]
        } else {
            unsafe { view(hits, count as usize) }
        }
    }

    fn hit(&self, id: Id) -> Option<&Hit> {
        unsafe { cbm_impact_walk_hit(self.0, id).as_ref() }
    }

    fn run(&mut self, reach: &Reach<'_>, seeds: &[Id]) -> Checked<()> {
        mark_written(seeds);
        if !seeds.is_empty()
            && unsafe {
                cbm_impact_walk_run(self.0, seeds.as_ptr(), seeds.len() as Index)
            } != 0
        {
            return reach.walk_failed();
        }
        Ok(())
    }
}

impl Drop for Walk {
    fn drop(&mut self) {
        unsafe { cbm_impact_walk_close(self.0) };
    }
}

// Diagnostics carry raw path/name bytes, with the same caps as the native
// engine's snprintf buffers. Tails are truncated even for non-UTF-8 paths.
struct Note<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> Note<N> {
    fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }

    fn append(&mut self, text: &[u8]) -> bool {
        let n = text.len().min(N - 1 - self.len);
        self.bytes[self.len..self.len + n].copy_from_slice(&text[..n]);
        self.len += n;
        n == text.len()
    }

    fn text(&self) -> Text {
        mark_written(&self.bytes[..=self.len]);
        self.bytes.as_ptr().cast()
    }

    fn location(&mut self, path: &[u8], line: Index) {
        use std::fmt::Write;
        self.append(path);
        let _ = write!(self, ":{line}");
    }

    fn chain(&mut self, walk: &Walk, mut id: Id, prefix: &[u8]) {
        let start = self.len;
        if !self.append(prefix) {
            return;
        }
        for step in 0..64 {
            if id == 0 {
                break;
            }
            let Some(hit) = walk.hit(id) else { break };
            let name = if hit.qualified_name.is_null() {
                b"?"
            } else {
                short_name(unsafe { bytes(hit.qualified_name) })
            };
            if (step > 0 && !self.append(b" -> ")) || !self.append(name) {
                if N - start > 4 {
                    self.bytes[N - 4..N - 1].copy_from_slice(b"...");
                }
                break;
            }
            id = hit.via_id;
        }
    }
}

impl<const N: usize> std::fmt::Write for Note<N> {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        self.append(text.as_bytes());
        Ok(())
    }
}

fn short_name(name: &[u8]) -> &[u8] {
    name.rsplitn(3, |&c| c == b'.')
        .nth(2)
        .map_or(name, |prefix| &name[prefix.len() + 1..])
}

fn edge_walked(edge: Text) -> bool {
    let edge = unsafe { bytes(edge) };
    WALK_TYPES.iter().any(|kind| kind.to_bytes() == edge)
}

fn is_function(label: Text) -> bool {
    matches!(unsafe { bytes(label) }, b"Function" | b"Method")
}

impl<'a> Reach<'a> {
    fn fallback(&self, reason: Fallback, note: Text) -> Checked<()> {
        unsafe { te_fallback(self.input.context, reason, note) }
            .then_some(())
            .ok_or(())
    }

    fn graph_error<T>(&self) -> Checked<T> {
        self.fallback(
            Fallback::GraphUnavailable,
            c"graph read failed".as_ptr(),
        )?;
        Err(())
    }

    fn walk_failed<T>(&self) -> Checked<T> {
        self.fallback(
            Fallback::EngineSaturated,
            c"the impact walk failed".as_ptr(),
        )?;
        Err(())
    }

    fn test_file(&self, path: &[u8]) -> bool {
        self.suites
            .iter()
            .any(|suite| unsafe { bytes(suite.file) } == path)
    }

    fn runner_file(&self, path: &[u8]) -> bool {
        self.runners.iter().any(|runner| {
            !runner.file.is_null() && unsafe { bytes(runner.file) } == path
        })
    }

    fn runner_suite(&self, name: &[u8]) -> bool {
        self.runners
            .iter()
            .any(|runner| unsafe { bytes(runner.name) } == name)
    }

    fn definitions(&self, file: Text) -> Checked<&'a [Definition]> {
        let mut count = 0;
        let mut ok = false;
        let ptr = unsafe {
            cbm_ti_graph_file(self.input.graph, file, &mut count, &mut ok)
        };
        ok.then(|| unsafe { view(ptr, count as usize) }).ok_or(())
    }

    fn enclosing(
        &self,
        file: Text,
        line: Index,
    ) -> Checked<Option<&'a Definition>> {
        let mut ok = false;
        let ptr = unsafe {
            cbm_ti_graph_enclosing(self.input.graph, file, line, &mut ok)
        };
        ok.then(|| unsafe { ptr.as_ref() }).ok_or(())
    }

    // Prefer spans starting at TEST/SUITE over names. Count each definition
    // once among matching spans, including preprocessor/platform variants.
    fn named(
        &self,
        file: Text,
        name: Text,
        line: Index,
        ids: &mut Scratch<Id>,
    ) -> Checked<()> {
        let Ok(defs) = self.definitions(file) else {
            return self.graph_error();
        };
        let start = ids.len();
        for by_line in [true, false] {
            for def in defs {
                let matches = if by_line {
                    def.start_line == line
                } else {
                    unsafe { bytes(def.name) == bytes(name) }
                };
                if matches
                    && is_function(def.label)
                    && !ids[start..].contains(&def.id)
                {
                    self.allocate(ids.push(def.id))?;
                }
            }
            if ids.len() != start {
                break;
            }
        }
        Ok(())
    }

    fn allocate<T>(&self, result: Checked<T>) -> Checked<T> {
        result.inspect_err(|()| {
            unsafe { te_oom(self.input.context) };
        })
    }

    fn nodes_by_name(&self, name: &CStr) -> Checked<StoreRows<Node>> {
        StoreRows::read(cbm_store_free_nodes, |out, count| unsafe {
            cbm_store_find_nodes_by_name(
                self.input.store,
                PROJECT.as_ptr(),
                name.as_ptr(),
                out,
                count,
            )
        })
        .or_else(|()| self.graph_error())
    }

    fn walk(&self, sinks: &[Id], extra: &[Id]) -> Checked<Walk> {
        let kinds = WALK_TYPES.map(CStr::as_ptr);
        let policy = WalkPolicy {
            project: PROJECT.as_ptr(),
            edge_types: kinds.as_ptr(),
            edge_type_count: kinds.len() as Index,
            follow_routes: true,
            follow_variant_keys: true,
            max_hops: 0,
        };
        mark_written(&kinds);
        mark_written(std::slice::from_ref(&policy));
        let mut ptr = null_mut();
        if unsafe {
            cbm_impact_walk_open_scoped(self.input.scope, &policy, &mut ptr)
        } != 0
        {
            self.fallback(
                Fallback::EngineSaturated,
                c"the impact walk could not start".as_ptr(),
            )?;
            return Err(());
        }
        let mut walk = Walk(ptr);
        mark_written(sinks);
        if unsafe {
            cbm_impact_walk_add_sinks(
                ptr,
                ptr_or_null(sinks),
                sinks.len() as Index,
            )
        } != 0
        {
            return self.walk_failed();
        }
        let seeds = borrowed(|count| unsafe {
            cbm_ti_seeds_nodes(self.input.seeds, count)
        });
        walk.run(self, seeds)?;
        walk.run(self, extra)?;
        Ok(walk)
    }

    fn main_way_in(
        &self,
        walk: &Walk,
        sinks: &Sinks<'_>,
        main: Id,
    ) -> Checked<Id> {
        let Some(hit) = walk.hit(main) else {
            return Ok(0);
        };
        if !edge_walked(hit.via_edge) {
            return Ok(main);
        }
        let edges = StoreRows::read(cbm_store_free_edges, |out, count| unsafe {
            cbm_store_find_edges_by_source(self.input.store, main, out, count)
        })
        .or_else(|()| self.graph_error())?;
        for edge in &*edges {
            let Some(to) = walk.hit(edge.target_id) else {
                continue;
            };
            if !edge_walked(edge.kind) || sinks.all.contains(&edge.target_id) {
                continue;
            }
            // RUN_SELECTED_SUITE(mcp) can appear as a read of an unrelated
            // field named mcp. It is not a way into the runner.
            if unsafe { bytes(to.label) } == b"Field"
                && let Some(name) =
                    leaf_name(unsafe { bytes(to.qualified_name) })
                && self.runner_suite(name)
            {
                continue;
            }
            return Ok(edge.target_id);
        }
        Ok(0)
    }

    fn check_runner(&self, walk: &Walk, sinks: &Sinks<'_>) -> Checked<()> {
        for &main in &*sinks.main {
            let way_in = self.main_way_in(walk, sinks, main)?;
            if way_in == 0 {
                continue;
            }
            let mut note = Note::<400>::new();
            note.append(b"the change reaches the test runner's main: ");
            if way_in != main {
                let name =
                    walk.hit(main).map_or(null(), |hit| hit.qualified_name);
                note.append(if name.is_null() {
                    b"main"
                } else {
                    short_name(unsafe { bytes(name) })
                });
                note.append(b" -> ");
            }
            note.chain(walk, way_in, b"");
            return self.fallback(Fallback::RunnerReached, note.text());
        }
        Ok(())
    }

    fn spawn_lift(&self, walk: &mut Walk) -> Checked<()> {
        let mains = self.nodes_by_name(c"main")?;
        let product_main = mains.iter().any(|main| {
            (unsafe { bytes(main.label) == b"Function" })
                && !main.file_path.is_null()
                && !self.test_file(unsafe { bytes(main.file_path) })
                && walk.reached(main.id)
        });
        drop(mains);
        if !product_main {
            return Ok(());
        }
        // The graph does not resolve which program a Process node runs.
        let processes =
            StoreRows::read(cbm_store_free_nodes, |out, count| unsafe {
                cbm_store_find_nodes_by_label(
                    self.input.store,
                    PROJECT.as_ptr(),
                    c"Process".as_ptr(),
                    out,
                    count,
                )
            })
            .or_else(|()| self.graph_error())?;
        let mut seeds = Scratch::default();
        for process in &*processes {
            self.allocate(seeds.push(process.id))?;
        }
        drop(processes);
        walk.run(self, &seeds)
    }

    fn run(&self) -> Checked<()> {
        let (extra, mut fixture_cases) = self.fixtures()?;
        let mut sinks = self.collect_sinks()?;
        let mut walk = self.walk(&sinks.all, &extra)?;
        let mut starters = Scratch::default();
        self.lift_fixpoint(
            &mut walk,
            &mut sinks,
            &mut fixture_cases,
            &mut starters,
        )?;
        self.check_runner(&walk, &sinks)?;
        // A runner can spawn itself. Escalate for main before lifting SPAWNS.
        self.spawn_lift(&mut walk)?;
        self.lift_fixpoint(
            &mut walk,
            &mut sinks,
            &mut fixture_cases,
            &mut starters,
        )?;
        self.score(&walk, &fixture_cases)
    }
}

// The call sites use arrays owned for the request (model, seeds, policy).
fn borrowed<'a, T>(get: impl FnOnce(*mut Index) -> *const T) -> &'a [T] {
    let mut count = 0;
    let ptr = get(&mut count);
    unsafe { view(ptr, count as usize) }
}

fn leaf_name(name: &[u8]) -> Option<&[u8]> {
    let at = name.iter().rposition(|&ch| ch == b'.')?;
    Some(&name[at + 1..])
}

/// # Safety
/// The C engine passes a live request with exclusive access to its context,
/// graph and pinned store, and keeps the model and source alive until return.
#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_test_impact_rust_reach(input: *const Input) -> bool {
    let input = unsafe { &*input };
    let reach = Reach {
        input,
        cases: borrowed(|count| unsafe {
            cbm_test_model_cases(input.model, count)
        }),
        suites: borrowed(|count| unsafe {
            cbm_test_model_suites(input.model, count)
        }),
        runners: borrowed(|count| unsafe {
            cbm_test_model_runner_suites(input.model, count)
        }),
        rules: borrowed(|count| unsafe {
            cbm_test_policy_rules(input.policy, count)
        }),
    };
    reach.run().is_ok()
}
