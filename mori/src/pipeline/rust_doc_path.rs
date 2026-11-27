//! Rust intra-doc path traversal over the resolver's native binding tables.
//!
//! The host looks up indexed bindings. Rust controls the order of those
//! lookups and decides when an incomplete scope stops the search.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::slice;

const TYPE: i32 = 1;
const VALUE: i32 = 2;
const MACRO: i32 = 4;
const NAMESPACES: [i32; 3] = [TYPE, VALUE, MACRO];
const NONE: u8 = 0;
const MODULE: u8 = 2;
const EXTERNAL: u8 = 5;
const GAP: u8 = 6;
const CAPACITY: usize = 8;

#[derive(Clone, Copy, Default)]
#[repr(C)]
pub(crate) struct Entity {
    kind: u8,
    via_import: bool,
    weak: bool,
    id: i32,
}

impl Entity {
    fn new(kind: u8, id: i32) -> Self {
        Self {
            kind,
            id,
            ..Self::default()
        }
    }

    fn weak(kind: u8) -> Self {
        Self {
            kind,
            weak: true,
            ..Self::default()
        }
    }

    fn same(self, other: Self) -> bool {
        self.kind == other.kind
            && (matches!(self.kind, EXTERNAL | GAP) || self.id == other.id)
    }
}

#[derive(Clone, Copy, Default)]
#[repr(C)]
pub(crate) struct Candidates {
    entities: [Entity; CAPACITY],
    count: i32,
    overflow: bool,
}

impl Candidates {
    fn entries(&self) -> &[Entity] {
        &self.entities[..self.count as usize]
    }

    fn empty(&self) -> bool {
        self.count == 0 && !self.overflow
    }

    fn add(&mut self, entity: Entity) {
        if entity.kind == NONE {
            return;
        }
        if let Some(old) = self.entities[..self.count as usize]
            .iter_mut()
            .find(|old| old.same(entity))
        {
            old.via_import |= entity.via_import;
            old.weak &= entity.weak;
        } else if self.count as usize == CAPACITY {
            self.overflow = true;
        } else {
            self.entities[self.count as usize] = entity;
            self.count += 1;
        }
    }

    fn merge(&mut self, other: Self) {
        for &entity in other.entries() {
            self.add(entity);
        }
        self.overflow |= other.overflow;
    }

    fn agrees_with(&self, other: &Self) -> bool {
        self.count == other.count
            && !self.overflow
            && !other.overflow
            && self
                .entries()
                .iter()
                .all(|e| other.entries().iter().any(|&o| e.same(o)))
    }
}

#[derive(Default)]
#[repr(C)]
pub(crate) struct Found {
    namespaces: [Candidates; 3],
    owners: Candidates,
    ambiguous: bool,
}

#[repr(C)]
pub(crate) struct Context {
    module: i32,
    root: i32,
    package: i32,
    edition: i32,
    import: bool,
}

type Name = *const c_char;
type ScopeLookup =
    unsafe extern "C" fn(*const c_void, Name, i32, &mut Candidates);
type NamePredicate = unsafe extern "C" fn(Name) -> bool;

#[repr(C)]
pub(crate) struct PathOps {
    own: ScopeLookup,
    textual_macro: unsafe extern "C" fn(*const c_void, Name, &mut Candidates),
    globs: unsafe extern "C" fn(
        *const c_void,
        Name,
        i32,
        &mut Candidates,
    ) -> GlobFlags,
    external: unsafe extern "C" fn(*const c_void, Name, &mut Candidates) -> bool,
    prelude: unsafe extern "C" fn(Name, i32) -> bool,
    builtin: NamePredicate,
    primitive: NamePredicate,
    parents: unsafe extern "C" fn(*const c_void, i32, &mut Candidates),
    root_member: ScopeLookup,
    member:
        unsafe extern "C" fn(*const c_void, Entity, Name, i32, &mut Candidates),
    collapse: unsafe extern "C" fn(*const c_void, &mut Candidates),
    block_import: unsafe extern "C" fn(*const c_void),
}

#[repr(C)]
struct GlobFlags {
    open: bool,
    pending: bool,
}

struct Lookup<'a> {
    host: *const c_void,
    ops: &'a PathOps,
    at: &'a Context,
}

impl Lookup<'_> {
    // The first binding wins in each namespace. An unresolved or open glob
    // can hide prelude names. An import may continue in this case because
    // rustc rejects an import whose first name binds both ways (E0659).
    fn lexical(&self, name: Name, ns: i32, out: &mut Candidates) {
        let ops = self.ops;
        let host = self.host;
        unsafe { (ops.own)(host, name, ns, out) };
        if out.empty() && ns == MACRO {
            unsafe { (ops.textual_macro)(host, name, out) };
        }
        let mut outer = Candidates::default();
        if !out.empty() {
            if out.entities[0].weak && self.at.import && ns == TYPE {
                unsafe { (ops.external)(host, name, &mut outer) };
                if !outer.empty() {
                    *out = outer;
                }
            }
            return;
        }
        let flags = unsafe { (ops.globs)(host, name, ns, out) };
        if out.count > 0 && !out.entities[0].weak {
            return;
        }
        let hidden = flags.open || flags.pending || !out.empty();
        let mut unknown = false;
        if !hidden || self.at.import {
            if ns == TYPE {
                unknown = unsafe { (ops.external)(host, name, &mut outer) };
            }
            if outer.empty() && unsafe { (ops.prelude)(name, ns) } {
                outer.add(Entity::new(EXTERNAL, 0));
            }
            if !outer.empty() {
                *out = outer;
                return;
            }
        }
        if flags.pending {
            *out = Candidates::default();
            unsafe { (ops.block_import)(host) };
            return;
        }
        if out.empty() && (flags.open || unknown) {
            out.add(Entity::weak(if flags.open { EXTERNAL } else { GAP }));
        }
        if out.empty() && self.at.import && ns == TYPE && self.at.package >= 0 {
            // The manifest may give the dependency a different name.
            out.add(Entity::weak(EXTERNAL));
        }
    }

    fn crate_relative(&self, name: Name, ns: i32, out: &mut Candidates) {
        if ns == TYPE && unsafe { (self.ops.builtin)(name) } {
            out.add(Entity::new(EXTERNAL, 0));
            return;
        }
        if self.at.root < 0 {
            out.add(Entity::weak(GAP));
            return;
        }
        unsafe { (self.ops.root_member)(self.host, name, ns, out) };
        if out.empty() && ns == TYPE {
            let unknown = unsafe { (self.ops.external)(self.host, name, out) };
            if out.empty() && unknown {
                out.add(Entity::weak(GAP));
            }
        }
    }

    fn first(&self, name: Name, ns: i32, out: &mut Candidates) {
        if !self.at.import || self.at.edition >= 2018 {
            self.lexical(name, ns, out);
            return;
        }
        let mut rooted = Candidates::default();
        self.crate_relative(name, ns, &mut rooted);
        if self.at.edition == 2015 {
            *out = rooted;
            return;
        }
        self.lexical(name, ns, out);
        if !out.agrees_with(&rooted) {
            *out = Candidates::default();
            out.add(Entity::weak(GAP));
        }
    }

    fn head(
        &self,
        segments: &[Name],
        global: bool,
        self_type: Entity,
        out: &mut Candidates,
    ) -> usize {
        let first = segments[0];
        if global {
            let unknown = unsafe { (self.ops.external)(self.host, first, out) };
            if out.empty() && (unknown || self.at.edition < 2018) {
                out.add(Entity::weak(GAP));
            }
        } else {
            match unsafe { CStr::from_ptr(first) }.to_bytes() {
                b"crate" if self.at.root < 0 => out.add(Entity::weak(GAP)),
                b"crate" => out.add(Entity::new(MODULE, self.at.root)),
                b"self" => out.add(Entity::new(MODULE, self.at.module)),
                b"super" => unsafe {
                    (self.ops.parents)(self.host, self.at.module, out)
                },
                b"Self" => out.add(self_type),
                _ => return 0,
            }
        }
        let mut consumed = 1;
        for &segment in &segments[1..] {
            if unsafe { CStr::from_ptr(segment) }.to_bytes() != b"super" {
                break;
            }
            let mut up = Candidates::default();
            for owner in out.entries() {
                if owner.kind == MODULE {
                    unsafe { (self.ops.parents)(self.host, owner.id, &mut up) };
                } else {
                    up.add(Entity::weak(GAP));
                }
            }
            up.overflow |= out.overflow;
            *out = up;
            consumed += 1;
        }
        consumed
    }

    fn walk(
        &self,
        segments: &[Name],
        global: bool,
        mask: i32,
        self_type: Entity,
    ) -> Found {
        let mut found = Found::default();
        let mut current = Candidates::default();
        let mut consumed = self.head(segments, global, self_type, &mut current);
        let first = segments[0];
        if consumed == 0 && segments.len() == 1 {
            for (ns, out) in NAMESPACES.into_iter().zip(&mut found.namespaces) {
                if mask & ns != 0 {
                    self.first(first, ns, out);
                }
            }
            return found;
        }
        if consumed == 0 {
            self.first(first, TYPE, &mut current);
            unsafe { (self.ops.collapse)(self.host, &mut current) };
            if current.count > 1 {
                found.ambiguous = true;
                return found;
            }
            if current.empty() && unsafe { (self.ops.primitive)(first) } {
                current.add(Entity::new(EXTERNAL, 0));
            }
            consumed = 1;
        }
        if current.overflow {
            found.ambiguous = true;
            return found;
        }
        if consumed == segments.len() {
            if mask & TYPE != 0 {
                found.namespaces[0].merge(current);
            }
            return found;
        }
        let last = segments.len() - 1;
        for &name in &segments[consumed..last] {
            if current.empty() {
                break;
            }
            let mut next = Candidates::default();
            for &owner in current.entries() {
                let mut one = Candidates::default();
                unsafe {
                    (self.ops.member)(self.host, owner, name, TYPE, &mut one);
                    (self.ops.collapse)(self.host, &mut one);
                }
                if one.count > 1 || one.overflow {
                    found.ambiguous = true;
                    return found;
                }
                next.merge(one);
            }
            current = next;
        }
        for (ns, out) in NAMESPACES.into_iter().zip(&mut found.namespaces) {
            if mask & ns != 0 {
                for &owner in current.entries() {
                    unsafe {
                        (self.ops.member)(
                            self.host,
                            owner,
                            segments[last],
                            ns,
                            out,
                        )
                    };
                }
            }
        }
        found.owners = current;
        found
    }
}

/// # Safety
/// The host provides a nonempty array of NUL-terminated segments and valid
/// callbacks. The array, strings, and callback context live through the call.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mori_rust_doc_walk(
    host: *const c_void,
    ops: &PathOps,
    context: &Context,
    segments: *const Name,
    count: c_int,
    global: bool,
    mask: c_int,
    self_type: Entity,
    out: &mut Found,
) {
    let segments = unsafe { slice::from_raw_parts(segments, count as usize) };
    *out = Lookup {
        host,
        ops,
        at: context,
    }
    .walk(segments, global, mask, self_type);
}

/// # Safety
/// The host, context, and name are valid for all callbacks during the call.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mori_rust_doc_lexical(
    host: *const c_void,
    ops: &PathOps,
    context: &Context,
    name: Name,
    ns: i32,
    out: &mut Candidates,
) {
    Lookup {
        host,
        ops,
        at: context,
    }
    .lexical(name, ns, out);
}

/// # Safety
/// The host and callbacks remain valid for the duration of the call. An
/// attempt returns whether the given import is complete, including when it
/// was already resolved before this pass.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mori_rust_doc_imports(
    host: *mut c_void,
    count: c_int,
    attempt: unsafe extern "C" fn(*mut c_void, i32, i32, bool) -> bool,
) {
    const MAX_PASSES: i32 = 32;
    let mut pending = count;
    let mut pass = 1;
    while pass <= MAX_PASSES && pending > 0 {
        let remaining = (0..count)
            .filter(|&id| !unsafe { attempt(host, id, pass, false) })
            .count() as i32;
        if remaining == pending {
            break;
        }
        pending = remaining;
        pass += 1;
    }
    if pending > 0 {
        for id in 0..count {
            unsafe { attempt(host, id, pass + 1, true) };
        }
    }
}

fn lines(blob: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut lines = blob.split(|b| *b == b'\n');
    if blob.is_empty() || blob.ends_with(b"\n") {
        lines.next_back();
    }
    lines
}

fn removable_member(line: &[u8]) -> Option<&[u8]> {
    if line.first() != Some(&b'M') {
        return None;
    }
    let mut fields = line.splitn(5, |b| *b == b'\t').skip(2);
    let kind = fields.next()?;
    let name = fields.next()?;
    fields.next()?; // Require the node field after the name.
    if !matches!(kind, b"f" | b"c" | b"t" | b"d") {
        return None;
    }
    let name = name
        .strip_prefix(b"r#")
        .filter(|name| !name.is_empty())
        .unwrap_or(name);
    (!name.is_empty()
        && name.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_'))
    .then_some(name)
}

type Removed = unsafe extern "C" fn(*mut c_void, *const u8, usize) -> bool;

/// Compare persisted scopes. Only removal of a member that cannot be
/// imported under another name admits a local repair. Its name still goes
/// to the caller so unresolved references can be revisited.
///
/// # Safety
/// Non-null scope strings and the callback are valid until the call returns.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mori_rust_doc_scope_delta(
    stored: *const c_char,
    fresh: *const c_char,
    removed: Option<Removed>,
    host: *mut c_void,
) -> c_int {
    let bytes = |text: *const c_char| {
        if text.is_null() {
            &[]
        } else {
            unsafe { CStr::from_ptr(text) }.to_bytes()
        }
    };
    let mut old = lines(bytes(stored));
    let mut new = lines(bytes(fresh)).peekable();
    for line in &mut old {
        if new.peek() == Some(&line) {
            new.next();
            continue;
        }
        let Some(name) = removable_member(line) else {
            return 1;
        };
        if !removed.is_some_and(|removed| unsafe {
            removed(host, name.as_ptr(), name.len())
        }) {
            return -1;
        }
    }
    c_int::from(new.next().is_some())
}

// The parser's CBM_DOCLINK_RS_DIS_* values. The native bridge checks both
// enums at compile time where it projects the parser's disambiguator.
const STRUCT: i32 = 1;
const ENUM: i32 = 2;
const TRAIT: i32 = 3;
const UNION: i32 = 4;
const ALIAS: i32 = 5;
const MOD: i32 = 6;
const CONST: i32 = 7;
const STATIC: i32 = 8;
const FN: i32 = 9;
const DERIVE: i32 = 10;
const FIELD: i32 = 11;
const VARIANT: i32 = 12;
const MACRO_BANG: i32 = 13;
const TYPE_NS: i32 = 14;
const VALUE_NS: i32 = 15;
const MACRO_NS: i32 = 16;
const PRIMITIVE: i32 = 17;

const ITEM: u8 = 1;
const MEMBER: u8 = 3;

fn namespaces(dis: i32) -> i32 {
    match dis {
        0 => TYPE | VALUE | MACRO,
        CONST | STATIC | FN | FIELD | VALUE_NS => VALUE,
        DERIVE | MACRO_BANG | MACRO_NS => MACRO,
        _ => TYPE,
    }
}

fn matches_disambiguator(dis: i32, entity: Entity, kind: u8) -> bool {
    if matches!(dis, 0 | TYPE_NS | VALUE_NS | MACRO_NS) {
        return true;
    }
    match entity.kind {
        // The native kind filters accept the terminator of their kind list.
        ITEM | MEMBER if kind == 0 => true,
        ITEM => matches!(
            (dis, kind),
            (STRUCT, b'S')
                | (ENUM, b'E')
                | (TRAIT, b'R')
                | (UNION, b'U')
                | (ALIAS, b'A')
                | (CONST, b'C')
                | (STATIC, b'G')
                | (FN, b'F')
                | (DERIVE, b'V')
                | (MACRO_BANG, b'M' | b'P')
        ),
        MEMBER => matches!(
            (dis, kind),
            (CONST, b'c') | (FN, b'f') | (FIELD, b'd') | (VARIANT, b'v')
        ),
        MODULE => dis == MOD,
        _ => true, // A gap or external entity has no known declaration kind.
    }
}

#[repr(C)]
struct TargetFacts {
    path: Name,
    prefix: Name,
    suffix: Name,
    label: Name,
    product: bool,
    external: bool,
}

#[repr(C)]
struct GraphNode {
    node: *const c_void,
    path: Name,
    label: Name,
}

#[repr(C)]
pub(crate) struct LinkContext {
    dis: i32,
    single: bool,
    product: bool,
    first: Name,
    last: Name,
}

#[repr(C)]
pub(crate) struct LinkOps {
    find: unsafe extern "C" fn(*const c_void, i32, &mut Found),
    collapse: unsafe extern "C" fn(*const c_void, &mut Candidates),
    entity_kind: unsafe extern "C" fn(*const c_void, Entity) -> u8,
    target: unsafe extern "C" fn(*const c_void, Entity) -> TargetFacts,
    by_qn: unsafe extern "C" fn(*const c_void, Name) -> GraphNode,
    hidden: unsafe extern "C" fn(*const c_void, i32, Name) -> bool,
    local: unsafe extern "C" fn(*const c_void, Name) -> bool,
    unowned: unsafe extern "C" fn(*const c_void) -> bool,
}

#[repr(i32)]
enum Reason {
    Missing = 0,
    Ambiguous = 1,
    External = 2,
    TestOnly = 3,
    GraphGap = 5,
}

#[repr(C)]
pub(crate) struct Outcome {
    target: *const c_void,
    reason: Reason,
    exact: bool,
    local: bool,
}

impl Outcome {
    fn unresolved(reason: Reason) -> Self {
        Self {
            target: std::ptr::null(),
            reason,
            exact: false,
            local: false,
        }
    }

    fn local() -> Self {
        Self {
            local: true,
            ..Self::unresolved(Reason::Missing)
        }
    }
}

struct Link<'a> {
    host: *const c_void,
    graph: *const c_void,
    ops: &'a LinkOps,
    context: &'a LinkContext,
}

fn graph_string_matches(a: Name, b: Name) -> bool {
    !a.is_null()
        && !b.is_null()
        && unsafe { CStr::from_ptr(a) == CStr::from_ptr(b) }
}

impl Link<'_> {
    fn entity_kind(&self, entity: Entity) -> u8 {
        unsafe { (self.ops.entity_kind)(self.host, entity) }
    }

    fn node(&self, target: &TargetFacts) -> *const c_void {
        const QN_BUF: usize = 2048;
        if target.label.is_null() || target.suffix.is_null() {
            return std::ptr::null();
        }
        let mut buffer = [0_u8; QN_BUF];
        let qn = if target.prefix.is_null() {
            target.suffix
        } else {
            let prefix = unsafe { CStr::from_ptr(target.prefix) }.to_bytes();
            let suffix = unsafe { CStr::from_ptr(target.suffix) }.to_bytes();
            if prefix.len() + suffix.len() >= QN_BUF {
                return std::ptr::null();
            }
            buffer[..prefix.len()].copy_from_slice(prefix);
            buffer[prefix.len()..prefix.len() + suffix.len()]
                .copy_from_slice(suffix);
            buffer.as_ptr().cast()
        };
        // Proxy nodes in incremental repairs need only a qualified name,
        // label and path. Their source line is intentionally unused.
        let node = unsafe { (self.ops.by_qn)(self.graph, qn) };
        if graph_string_matches(node.label, target.label)
            && graph_string_matches(node.path, target.path)
        {
            node.node
        } else {
            std::ptr::null()
        }
    }

    fn target(&self, entity: Entity, weak: bool) -> Outcome {
        let target = unsafe { (self.ops.target)(self.host, entity) };
        let exact = !self.context.single || entity.via_import;
        let mut outcome = Outcome {
            exact,
            ..Outcome::unresolved(Reason::GraphGap)
        };
        if target.external {
            outcome.reason = Reason::External;
        } else if self.context.product && !target.product {
            outcome.reason = Reason::TestOnly;
        } else {
            outcome.target = self.node(&target);
            if !outcome.target.is_null()
                && unsafe { (self.ops.unowned)(self.host) }
            {
                return Outcome::unresolved(Reason::GraphGap);
            }
        }
        if weak
            && entity.kind == GAP
            && self.context.single
            && unsafe { (self.ops.local)(self.host, self.context.first) }
        {
            outcome.local = true;
        }
        outcome
    }

    fn choose(&self, primitive: bool) -> Outcome {
        let context = self.context;
        let dis = context.dis;
        let mut found = Found::default();
        unsafe { (self.ops.find)(self.host, namespaces(dis), &mut found) };
        if found.ambiguous {
            return Outcome::unresolved(Reason::Ambiguous);
        }

        let mut candidates = Candidates::default();
        let mut weak_kind = NONE;
        for namespace in &found.namespaces {
            for &entity in namespace.entries() {
                if !entity.weak {
                    candidates.add(entity);
                } else if weak_kind != GAP {
                    weak_kind = entity.kind;
                }
            }
            candidates.overflow |= namespace.overflow;
        }
        unsafe { (self.ops.collapse)(self.host, &mut candidates) };
        let weak = candidates.empty() && weak_kind != NONE;
        if weak {
            candidates.add(Entity::new(weak_kind, 0));
        }
        if dis == PRIMITIVE || (primitive && (weak || candidates.empty())) {
            return Outcome::unresolved(if primitive {
                Reason::External
            } else {
                Reason::Missing
            });
        }
        if primitive && matches!(dis, 0 | TYPE_NS) {
            return Outcome::unresolved(Reason::Ambiguous);
        }

        let had_candidates = !candidates.empty();
        let mut kept = candidates.entries().iter().copied().filter(|&entity| {
            matches_disambiguator(dis, entity, self.entity_kind(entity))
        });
        let first = kept.next();
        let second = kept.next();
        if candidates.overflow || kept.next().is_some() {
            return Outcome::unresolved(Reason::Ambiguous);
        }
        match (first, second) {
            (Some(a), Some(b)) => {
                // Rustdoc prefers a trait over a same-named derive macro.
                for (a, b) in [(a, b), (b, a)] {
                    if a.kind == ITEM
                        && self.entity_kind(a) == b'R'
                        && (b.kind == EXTERNAL
                            || (b.kind == ITEM && self.entity_kind(b) == b'V'))
                    {
                        return self.target(a, weak);
                    }
                }
                Outcome::unresolved(Reason::Ambiguous)
            }
            (Some(entity), None) => self.target(entity, weak),
            (None, _) => {
                if !had_candidates {
                    if found.owners.entries().iter().any(|owner| {
                        owner.kind == ITEM
                            && unsafe {
                                (self.ops.hidden)(
                                    self.host,
                                    owner.id,
                                    context.last,
                                )
                            }
                    }) {
                        return Outcome::unresolved(Reason::GraphGap);
                    }
                    if context.single
                        && unsafe { (self.ops.local)(self.host, context.first) }
                    {
                        return Outcome::local();
                    }
                }
                Outcome::unresolved(Reason::Missing)
            }
        }
    }
}

/// Resolve an intra-doc link and verify its declaration against the graph.
///
/// # Safety
/// The host, graph, and context contain valid views for the duration of the
/// call. Any node returned by the graph callbacks is borrowed from the graph.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mori_rust_doc_choose(
    host: *const c_void,
    graph: *const c_void,
    ops: &LinkOps,
    context: &LinkContext,
    primitive: bool,
) -> Outcome {
    Link {
        host,
        graph,
        ops,
        context,
    }
    .choose(primitive)
}
