//! The engine owns the model, source, seeds and pinned store read scope.
//! Keep this ABI in step with test_impact_engine_reach.h and the native
//! declarations in test_impact{,_seed,_source}.h and store{,_impact}.h.
use std::ffi::{CStr, c_char, c_int, c_long, c_void};
use std::mem::ManuallyDrop;
use std::ops::{Deref, DerefMut};
use std::ptr::{null, null_mut};
use std::slice;

macro_rules! opaque {
    ($($name:ident),+ $(,)?) => {$(
        #[repr(C)]
        pub(super) struct $name { _private: [u8; 0] }
    )+};
}

opaque!(
    Context, Store, Scope, Graph, Source, Model, Policy, Seeds, ImpactWalk
);

pub(super) type Text = *const c_char;
pub(super) type Id = i64;
pub(super) type Index = c_int;
pub(super) type Checked<T> = Result<T, ()>;

#[repr(C)]
pub(super) struct Input {
    pub context: *mut Context,
    pub store: *mut Store,
    pub scope: *mut Scope,
    pub graph: *mut Graph,
    pub source: *const Source,
    pub model: *const Model,
    pub policy: *const Policy,
    pub seeds: *const Seeds,
    pub matches: *mut RuleMatch,
    pub match_count: Index,
    pub fixture_paths: *const Text,
    pub fixture_rules: *const Index,
    pub fixture_count: Index,
}

#[repr(C)]
pub(super) struct RuleMatch {
    pub rule_index: Index,
    pub resolved: bool,
}

#[repr(C)]
pub(super) struct Case {
    pub file: Text,
    pub name: Text,
    pub start_line: Index,
    _end_line: Index,
    _conditional: bool,
}

#[repr(C)]
pub(super) struct Suite {
    pub file: Text,
    pub name: Text,
    pub start_line: Index,
    _end_line: Index,
    pub macro_registrations: bool,
    pub uncertain: bool,
}

#[repr(C)]
pub(super) struct Runner {
    pub name: Text,
    pub perf: bool,
    pub file: Text,
    _line: Index,
    _conditional: bool,
}

#[repr(C)]
pub(super) struct Rule {
    _id: Text,
    pub action: Index,
    _paths: *const Text,
    _path_count: Index,
    _suites: *const Text,
    _suite_count: Index,
    _lanes: *const Text,
    _lane_count: Index,
    pub key_segment: Index,
    pub key_path: bool,
    pub unreferenced_ignore: bool,
    _builtin: bool,
}

#[repr(C)]
pub(super) struct Definition {
    pub id: Id,
    pub label: Text,
    pub name: Text,
    _qualified_name: Text,
    pub start_line: Index,
    _end_line: Index,
    pub variant_span: bool,
}

#[repr(C)]
pub(super) struct Node {
    pub id: Id,
    _project: Text,
    pub label: Text,
    _name: Text,
    _qualified_name: Text,
    pub file_path: Text,
    _start_line: Index,
    _end_line: Index,
    _properties_json: Text,
}

#[repr(C)]
pub(super) struct Edge {
    _id: Id,
    _project: Text,
    pub source_id: Id,
    pub target_id: Id,
    pub kind: Text,
    _properties_json: Text,
}

#[repr(C)]
pub(super) struct CoverageRow {
    pub rel_path: Text,
    pub kind: Text,
    pub detail: Text,
}

#[repr(C)]
pub(super) struct WalkPolicy {
    pub project: Text,
    pub edge_types: *const Text,
    pub edge_type_count: Index,
    pub follow_routes: bool,
    pub follow_variant_keys: bool,
    pub max_hops: Index,
}

#[repr(C)]
pub(super) struct Hit {
    pub id: Id,
    _hop: Index,
    pub via_id: Id,
    pub via_edge: Text,
    pub qualified_name: Text,
    pub label: Text,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct ReachRow {
    pub file: Text,
    pub test: Text,
    pub mapped: bool,
    pub reached: bool,
    pub changed: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct Trigger {
    pub suite: Text,
    pub reached: bool,
    pub changed: bool,
    pub rule: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) enum Fallback {
    GraphUnavailable = 3,
    EngineSaturated = 8,
    SelectionUnavailable = 11,
    PolicyEvaluationFailed = 13,
    RunnerReached = 15,
}

unsafe extern "C" {
    fn cbm_ffi_mark_written(address: *const c_void, size: usize);
    pub fn te_oom(context: *mut Context) -> bool;
    pub fn te_fallback(
        context: *mut Context,
        code: Fallback,
        diagnostic: Text,
    ) -> bool;
    fn te_grow(
        items: *mut *mut c_void,
        capacity: *mut Index,
        count: Index,
        size: usize,
    ) -> bool;
    fn cbm_test_impact_reach_free(items: *mut c_void);
    pub fn cbm_test_impact_reach_publish(
        context: *mut Context,
        rows: *mut ReachRow,
        row_count: Index,
        triggers: *mut Trigger,
        trigger_count: Index,
    );

    pub fn cbm_test_model_cases(
        model: *const Model,
        count: *mut Index,
    ) -> *const Case;
    pub fn cbm_test_model_suites(
        model: *const Model,
        count: *mut Index,
    ) -> *const Suite;
    pub fn cbm_test_model_runner_suites(
        model: *const Model,
        count: *mut Index,
    ) -> *const Runner;
    pub fn cbm_test_policy_rules(
        policy: *const Policy,
        count: *mut Index,
    ) -> *const Rule;
    pub fn cbm_test_policy_rule_selects_suite(
        policy: *const Policy,
        rule: Index,
        suite: Text,
        perf: bool,
        selected: *mut bool,
    ) -> bool;

    pub fn cbm_ti_seeds_nodes(
        seeds: *const Seeds,
        count: *mut Index,
    ) -> *const Id;
    pub fn cbm_ti_seeds_changed_cases(
        seeds: *const Seeds,
        count: *mut Index,
    ) -> *const Index;
    pub fn cbm_ti_seeds_changed_suites(
        seeds: *const Seeds,
        count: *mut Index,
    ) -> *const Index;
    pub fn cbm_ti_seeds_file_level_headers(
        seeds: *const Seeds,
        count: *mut Index,
    ) -> *const Text;
    pub fn cbm_ti_source_file_count(source: *const Source) -> Index;
    pub fn cbm_ti_source_path(source: *const Source, file: Index) -> Text;
    pub fn cbm_ti_source_find(source: *const Source, path: Text) -> Index;
    pub fn cbm_ti_source_line_count(source: *const Source, file: Index)
    -> Index;
    pub fn cbm_ti_source_line(
        source: *const Source,
        file: Index,
        line: Index,
        len: *mut usize,
    ) -> *const u8;
    pub fn cbm_ti_source_include_closure(
        source: *const Source,
        marked: *mut bool,
    );
    pub fn cbm_ti_graph_file(
        graph: *mut Graph,
        file: Text,
        count: *mut Index,
        ok: *mut bool,
    ) -> *const Definition;
    pub fn cbm_ti_graph_enclosing(
        graph: *mut Graph,
        file: Text,
        line: Index,
        ok: *mut bool,
    ) -> *const Definition;

    pub fn cbm_store_find_nodes_by_name(
        store: *mut Store,
        project: Text,
        name: Text,
        out: *mut *mut Node,
        count: *mut Index,
    ) -> Index;
    pub fn cbm_store_find_nodes_by_label(
        store: *mut Store,
        project: Text,
        label: Text,
        out: *mut *mut Node,
        count: *mut Index,
    ) -> Index;
    pub fn cbm_store_find_edges_by_target_type(
        store: *mut Store,
        target: Id,
        kind: Text,
        out: *mut *mut Edge,
        count: *mut Index,
    ) -> Index;
    pub fn cbm_store_find_edges_by_source(
        store: *mut Store,
        source: Id,
        out: *mut *mut Edge,
        count: *mut Index,
    ) -> Index;
    pub fn cbm_store_coverage_get(
        store: *mut Store,
        project: Text,
        out: *mut *mut CoverageRow,
        count: *mut Index,
    ) -> Index;
    pub fn cbm_store_free_nodes(rows: *mut Node, count: Index);
    pub fn cbm_store_free_edges(rows: *mut Edge, count: Index);
    pub fn cbm_store_free_coverage(rows: *mut CoverageRow, count: Index);

    pub fn cbm_impact_walk_open_scoped(
        scope: *mut Scope,
        policy: *const WalkPolicy,
        out: *mut *mut ImpactWalk,
    ) -> Index;
    pub fn cbm_impact_walk_add_sinks(
        walk: *mut ImpactWalk,
        ids: *const Id,
        count: Index,
    ) -> Index;
    pub fn cbm_impact_walk_run(
        walk: *mut ImpactWalk,
        ids: *const Id,
        count: Index,
    ) -> Index;
    pub fn cbm_impact_walk_count(walk: *const ImpactWalk) -> Index;
    pub fn cbm_impact_walk_hits(walk: *const ImpactWalk) -> *const Hit;
    pub fn cbm_impact_walk_hit(walk: *const ImpactWalk, id: Id) -> *const Hit;
    pub fn cbm_impact_walk_reached(walk: *const ImpactWalk, id: Id) -> bool;
    pub fn cbm_impact_walk_close(walk: *mut ImpactWalk);

    pub fn strtol(text: Text, end: *mut *mut c_char, base: c_int) -> c_long;
}

pub(super) unsafe fn view<'a, T>(ptr: *const T, count: usize) -> &'a [T] {
    if count == 0 {
        &[]
    } else {
        unsafe { slice::from_raw_parts(ptr, count) }
    }
}

pub(super) unsafe fn bytes<'a>(text: Text) -> &'a [u8] {
    if text.is_null() {
        &[]
    } else {
        unsafe { CStr::from_ptr(text) }.to_bytes()
    }
}

// C may be instrumented by MemorySanitizer while Rust is not.
pub(super) fn mark_written<T>(items: &[T]) {
    if !items.is_empty() {
        unsafe {
            cbm_ffi_mark_written(
                items.as_ptr().cast(),
                std::mem::size_of_val(items),
            )
        };
    }
}

// Grow request scratch through the same native memory class as the C engine.
// Copy types have no destructor for the allocator to run.
pub(super) struct Scratch<T: Copy> {
    items: *mut T,
    count: Index,
    capacity: Index,
}

impl<T: Copy> Default for Scratch<T> {
    fn default() -> Self {
        Self {
            items: null_mut(),
            count: 0,
            capacity: 0,
        }
    }
}

impl<T: Copy> Scratch<T> {
    pub fn push(&mut self, item: T) -> Checked<()> {
        if self.count == self.capacity {
            let next = if self.capacity == 0 {
                16
            } else {
                self.capacity.checked_mul(2).ok_or(())?
            };
            (next as usize).checked_mul(size_of::<T>()).ok_or(())?;
            let mut items = self.items.cast();
            mark_written(slice::from_ref(&items));
            mark_written(slice::from_ref(&self.capacity));
            if !unsafe {
                te_grow(
                    &mut items,
                    &mut self.capacity,
                    self.count,
                    size_of::<T>(),
                )
            } {
                return Err(());
            }
            self.items = items.cast();
        }
        unsafe { self.items.add(self.count as usize).write(item) };
        self.count += 1;
        Ok(())
    }

    pub fn clear(&mut self) {
        self.count = 0;
    }

    pub fn into_raw_parts(self) -> (*mut T, Index) {
        let owned = ManuallyDrop::new(self);
        (owned.items, owned.count)
    }
}

impl<T: Copy> Deref for Scratch<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        unsafe { view(self.items, self.count as usize) }
    }
}

impl<T: Copy> DerefMut for Scratch<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        if self.count == 0 {
            &mut []
        } else {
            unsafe { slice::from_raw_parts_mut(self.items, self.count as usize) }
        }
    }
}

impl<T: Copy> Drop for Scratch<T> {
    fn drop(&mut self) {
        unsafe { cbm_test_impact_reach_free(self.items.cast()) };
    }
}

// Store result allocations are independent of later reads in the same scope.
pub(super) struct StoreRows<T> {
    rows: *mut T,
    count: Index,
    free: unsafe extern "C" fn(*mut T, Index),
}

impl<T> StoreRows<T> {
    pub fn read(
        free: unsafe extern "C" fn(*mut T, Index),
        query: impl FnOnce(*mut *mut T, *mut Index) -> Index,
    ) -> Checked<Self> {
        let mut rows = null_mut();
        let mut count = 0;
        if query(&mut rows, &mut count) == 0 {
            Ok(Self { rows, count, free })
        } else {
            Err(())
        }
    }
}

impl<T> Deref for StoreRows<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        unsafe { view(self.rows, self.count as usize) }
    }
}

impl<T> Drop for StoreRows<T> {
    fn drop(&mut self) {
        unsafe { (self.free)(self.rows, self.count) };
    }
}

pub(super) fn ptr_or_null<T>(items: &[T]) -> *const T {
    if items.is_empty() {
        null()
    } else {
        items.as_ptr()
    }
}
