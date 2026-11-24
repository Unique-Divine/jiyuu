//! Javadoc and KDoc resolution over the shared JVM scope records.
//!
//! The native pipeline owns extraction, graph storage and publication. It
//! builds one immutable index for Java and Kotlin and gives resolve the graph
//! for the current run, including the proxy graph used in closure repair.
//! Lookup order is determined by the language of the reference.

// One index over the Java and the Kotlin files of a repository, built from
// their doc-link scopes (internal/cbm/doclink_jvm.c): on the JVM a package is
// one namespace whatever language declares into it, so a Kotlin import of a
// Java class and a Javadoc link to a Kotlin class must meet in one type table.
// The language of the SOURCE file decides the lookup order; where a target's
// node is comes from the target's own scope record (slot, own), never from a
// naming scheme assumed here. Graph nodes are looked up by qualified name
// only, and only their label and file path are read, so the closure-repair
// route (whose unchanged nodes are line-less proxies) resolves exactly as a
// full build does.
//
// Entities. A type entity is its binary-style name: package + '.' + the
// dotted logical path (com.acme.Outer.Inner). Several declarations of one
// name (source sets, expect/actual) are one entity. A member belongs to the
// declaration that holds it; overloads of one type share a node.
//
// Lookup (the first level with a visible candidate decides; more than one
// entity there is ambiguous, never a pick):
//   Java type name (javac's order)
//     the method's own type variables (local) -> the enclosing types,
//     innermost first (the type and its member types, its type variables, the
//     member types it inherits) -> the file's own top-level types and its
//     single imports (a static import imports a member type too) -> the
//     file's package -> on-demand imports, with java.lang among them.
//     A qualified name: the head through those levels, the rest as member
//     types; a head no level has is read from the root (packages, then a
//     type). `Type.member` is no reference: Javadoc wants a '#'.
//   Java METHOD, `#m` and `Type#m`: javadoc's own search (walk, read in
//     the JDK source), from the documented definition's type or the type
//     named: the type's own methods -> its superclass, by the whole search
//     (its superclass, its interfaces, the type around IT) -> its
//     interfaces in the order written, likewise -> the type around it,
//     likewise; each type once, the first method found. A type outside the
//     repository on the way, or a method the language gives (Object's,
//     Enum's, Annotation's, an enum's values/valueOf, a record's
//     equals/hashCode/toString), ends it as `external`. Static imports are
//     no step: javadoc does not search them for a member.
//   Java FIELD, `#x` and `Type#x` without a parameter list: javadoc looks in
//     the type, then the types around it, then the supertypes; an inherited
//     field binds only when no type further out has one of that name, else
//     it is ambiguous. ({@value Type#X}: the type and its supertypes.)
//   Java kinds
//     `#x` is the field when javadoc takes it over the method its search
//     finds: the field's class is the method's class or below it, also for
//     every type outside the repository the search passes ;
//     else that method, when it is one of the repository with nothing
//     unseen before it (javadoc links it), or external.
//     A parameter list selects callables:
//     the first type whose overloads have that signature, never another
//     overload. A declared type variable matches its own name or its erasure.
//     A method name without a list that has several signatures is ambiguous.
//     A record's component declares an accessor of its name (no node).
//     `#Type(..)` names the constructors of that type only. A bare
//     identifier that is no type is javadoc's `#identifier`.
//   Kotlin (the order KDoc resolves in)
//     the definition's own parameters, type parameters and `this` (local) ->
//     each enclosing type, innermost first: what it holds itself together
//     with its companion (one scope), its type parameters and constructor
//     parameters (local), then what it inherits and what its companion
//     inherits -> explicit imports -> the file's package (types and top-level
//     declarations) -> star imports -> default imports.
//     A FUNCTION or PROPERTY found further out than an enclosing type binds
//     only when that type is known not to have a member of the name: a type
//     that has it from the language (kotlin.Any's members, an enum class's),
//     or whose supertypes leave the repository, makes it `external`
//     . A type found further out binds.
//     A dotted name: the qualifier is a type (through those levels, or from
//     the root) and the last name what that type holds, inherits, or gets
//     from an extension in scope whose receiver it is (an extension only
//     where the type's supertypes all lie in the repository: a member comes
//     before an extension); or the qualifier is a
//     package and the last name one of its types or top-level declarations.
//   Kotlin kinds
//     a type and a function of one name at one level (a class and its factory
//     function) are the type: KDoc resolves a classifier before a function
//     . A type without a node of its own is then a graph gap,
//     never the function. A property and a function of one name are
//     ambiguous, and so is an overload group.
//     A `[name]` that resolves to nothing is plain text to KDoc (Markdown):
//     no reference at all. @see, @throws and @sample name their subject and
//     keep a row.
// Visibility: product code never binds a declaration that only test code
// holds (test_only_target, no fallback to it); `@sample` is exempt.
// External: a name a single import brings from a package the repository does
// not declare; a qualified `pkg.Type` whose package it does not declare; a
// name only an on-demand import of such a package can supply; java.lang and
// the Kotlin default imports (the languages' own implicit imports, the only
// name lists here, and consulted after the repository's own declarations); an
// all-lower-case path under `java`, `javax` or `kotlin` (the platforms' own
// namespaces; no library family is one), or under a package the file imports
// from and the repository does not declare; a
// member that a supertype outside the repository may declare; the members
// every type has without declaring them (Object's, an enum's).
// Not indexed: a package below one the repository owns, in a directory the
// indexer skips by name (discovery's own rule, no file system access here).
// Graph gaps: a declaration without a definition (slot '-'), one whose
// qualified name another declaration took (own 0), a node of another kind or
// file, a package (it has no node), a member a parse error hides, a name the
// scan could not place, a definition in a part of its file it could not
// place -- and a reference that resolves, written on a definition that has
// no node of its own to leave from.

#![allow(unsafe_op_in_unsafe_fn)]

use std::ffi::{CStr, c_char, c_ulong, c_void};
use std::io::Write;
use std::mem::{size_of, zeroed};
use std::ptr::{self, addr_of, addr_of_mut};
use std::sync::atomic::{AtomicI64, Ordering};

mod java;
mod kotlin;

use java::{java_resolve, java_type_path};
use kotlin::{ext_key, kotlin_resolve, kotlin_type_path};

// Opaque native objects. The pipeline keeps the graph and context alive
// through their respective callbacks. The index owns its native arena.
enum Graph {}
enum PipelineContext {}
enum Arena {}
enum HashTable {}

#[repr(C)]
struct GraphNode {
    id: i64,
    label: *mut c_char,
    name: *mut c_char,
    qualified_name: *mut c_char,
    file_path: *mut c_char,
    start_line: i32,
    end_line: i32,
    properties_json: *mut c_char,
}

#[repr(C)]
struct DocLink {
    source_qn: *const c_char,
    raw: *const c_char,
    line: u32,
    def_line: u32,
    syntax: u16,
    flags: u16,
}

#[repr(C)]
struct InputFile {
    rel_path: *const c_char,
    scope: *const c_char,
    run_file: i32,
}

#[repr(C)]
struct BuildInput {
    ctx: *const PipelineContext,
    graph: *const Graph,
    files: *const InputFile,
    file_count: i32,
    run_file_count: i32,
}

#[repr(C)]
struct Outcome {
    kind: i32,
    target: *const GraphNode,
    exact: bool,
    reason: i32,
    target_first: u32,
    target_last: u32,
}

type RemovedName =
    Option<unsafe extern "C" fn(*mut c_void, *const c_char, usize) -> bool>;
type Compare = unsafe fn(*const c_void, *const c_void) -> i32;
type FileBegin = unsafe extern "C" fn(*const c_void, i32) -> *mut c_void;
type FilePrepare = unsafe extern "C" fn(
    *const c_void,
    i32,
    *const DocLink,
    i32,
    *const Graph,
) -> *mut c_void;

#[repr(C)]
pub(crate) struct Resolver {
    langs: [i32; 6],
    lang_count: i32,
    scope_tag: *const c_char,
    build: unsafe extern "C" fn(*const BuildInput) -> *mut c_void,
    destroy: unsafe extern "C" fn(*mut c_void),
    file_begin: Option<FileBegin>,
    file_prepare: Option<FilePrepare>,
    file_end: Option<unsafe extern "C" fn(*mut c_void)>,
    resolve: unsafe extern "C" fn(
        *const c_void,
        *mut c_void,
        i32,
        *const DocLink,
        *const Graph,
        *mut Outcome,
    ),
    scope_input: Option<unsafe extern "C" fn(*const c_char) -> bool>,
    scope_delta: unsafe extern "C" fn(
        *const c_char,
        *const c_char,
        RemovedName,
        *mut c_void,
    ) -> i32,
    scope_accepted: Option<unsafe extern "C" fn(*const c_char) -> i32>,
    rejected_scope: Option<unsafe extern "C" fn(*const c_char) -> *mut c_char>,
    via: *const c_char,
}

// All pointers in the exported vtable are static strings or functions.
unsafe impl Sync for Resolver {}

#[unsafe(no_mangle)]
pub(crate) static cbm_doclink_jvm_resolver: Resolver = Resolver {
    langs: [CBM_LANG_JAVA, CBM_LANG_KOTLIN, 0, 0, 0, 0],
    lang_count: 2,
    scope_tag: c"jvm1".as_ptr(),
    build,
    destroy,
    file_begin: None,
    file_prepare: None,
    file_end: None,
    resolve,
    scope_input: None,
    scope_delta,
    scope_accepted: None,
    rejected_scope: None,
    via: ptr::null(),
};

unsafe extern "C" {
    fn mori_jvm_doclinks_project(
        context: *const PipelineContext,
    ) -> *const c_char;
    fn mori_jvm_doclinks_mode(context: *const PipelineContext) -> i32;
    fn mori_jvm_doclinks_arena_new() -> *mut Arena;
    fn mori_jvm_doclinks_arena_drop(arena: *mut Arena);
}

unsafe fn clear_bytes(destination: *mut c_void, value: i32, len: usize) {
    if len != 0 {
        ptr::write_bytes(destination.cast::<u8>(), value as u8, len);
    }
}

unsafe fn sort_records<T>(records: *mut T, count: usize, compare: Compare) {
    let records = std::slice::from_raw_parts_mut(records, count);
    records.sort_unstable_by(|a, b| {
        compare(ptr::from_ref(a).cast(), ptr::from_ref(b).cast()).cmp(&0)
    });
}
unsafe extern "C" {
    fn atoi(a0: *const c_char) -> i32;
    fn cbm_alloc(a0: i32, a1: usize) -> *mut c_void;
    fn cbm_arena_alloc(a0: *mut Arena, a1: usize) -> *mut c_void;
    fn cbm_arena_strdup(a0: *mut Arena, a1: *const c_char) -> *mut c_char;
    fn cbm_calloc(a0: i32, a1: usize) -> *mut c_void;
    fn cbm_doclink_java_norm_type(
        a0: *const c_char,
        a1: usize,
        a2: *mut c_char,
        a3: usize,
    ) -> usize;
    fn cbm_fqn_module_source_lang(
        a0: *mut Arena,
        a1: *const c_char,
        a2: *const c_char,
        a3: i32,
    ) -> *mut c_char;
    fn cbm_free(a0: i32, a1: *mut c_void);
    fn cbm_gbuf_find_by_qn(
        a0: *const Graph,
        a1: *const c_char,
    ) -> *const GraphNode;
    fn cbm_ht_count(a0: *const HashTable) -> u32;
    fn cbm_ht_create(a0: u32) -> *mut HashTable;
    fn cbm_ht_free(a0: *mut HashTable);
    fn cbm_ht_get(a0: *const HashTable, a1: *const c_char) -> *mut c_void;
    fn cbm_ht_set(
        a0: *mut HashTable,
        a1: *const c_char,
        a2: *mut c_void,
    ) -> *mut c_void;
    fn cbm_label_is_type_like(a0: *const c_char) -> bool;
    fn cbm_log(a0: i32, a1: *const c_char, ...);
    fn cbm_should_skip_dir(a0: *const c_char, a1: i32) -> bool;

    fn isspace(a0: i32) -> i32;
    fn isupper(a0: i32) -> i32;
    fn memchr(a0: *const c_void, a1: i32, a2: usize) -> *mut c_void;
    fn memcmp(a0: *const c_void, a1: *const c_void, a2: usize) -> i32;
    fn memcpy(a0: *mut c_void, a1: *const c_void, a2: usize) -> *mut c_void;
    fn strchr(a0: *const c_char, a1: i32) -> *mut c_char;
    fn strcmp(a0: *const c_char, a1: *const c_char) -> i32;
    fn strlen(a0: *const c_char) -> usize;
    fn strncmp(a0: *const c_char, a1: *const c_char, a2: usize) -> i32;

    fn strrchr(a0: *const c_char, a1: i32) -> *mut c_char;
    fn strtoul(
        text: *const c_char,
        end: *mut *mut c_char,
        radix: i32,
    ) -> c_ulong;
    fn tolower(a0: i32) -> i32;
}
const CBM_DOCLINK_DELTA_GLOBAL: i32 = 1;
const CBM_DOCLINK_DELTA_LOCAL: i32 = 0;
const CBM_DOCLINK_EDGE: i32 = 0;
const CBM_DOCLINK_FLAG_FILE: i32 = 1;
const CBM_DOCLINK_JAVA_VALUE: i32 = 10;
const CBM_DOCLINK_KT_KDOC_LINK: i32 = 11;
const CBM_DOCLINK_KT_SAMPLE: i32 = 14;
const CBM_DOCLINK_LOCAL: i32 = 2;
const CBM_DOCLINK_REASON_AMBIGUOUS: i32 = 1;
const CBM_DOCLINK_REASON_EXTERNAL: i32 = 2;
const CBM_DOCLINK_REASON_GRAPH_GAP: i32 = 5;
const CBM_DOCLINK_REASON_MISSING: i32 = 0;
const CBM_DOCLINK_REASON_NOT_INDEXED: i32 = 4;
const CBM_DOCLINK_REASON_TEST_ONLY: i32 = 3;
const CBM_DOCLINK_REASON_UNPARSEABLE: i32 = 6;
const CBM_DOCLINK_UNRESOLVED: i32 = 1;
const CBM_LANG_JAVA: i32 = 6;
const CBM_LANG_KOTLIN: i32 = 12;
const CBM_LOG_ERROR: i32 = 3;
const CBM_LOG_INFO: i32 = 1;
const CBM_LOG_WARN: i32 = 2;
const CBM_MEM_CLASS_OTHER: i32 = 0;
const CBM_NOT_FOUND: i32 = -1;
const CBM_SZ_16: i32 = 16;
const CBM_SZ_1K: i32 = 1024;
const CBM_SZ_2: i32 = 2;
const CBM_SZ_256: i32 = 256;
const CBM_SZ_4: i32 = 4;
const CBM_SZ_4K: i32 = 4096;
const CBM_SZ_64: i32 = 64;
const PAIR_LEN: i32 = 2;
const SKIP_ONE: i32 = 1;
const KEY_BYTES: usize = 2048;
const MAX_REFERENCE_BYTES: usize = 1000;
const JR_MAX_SEGS: i32 = 32;
const JR_T_FIELDS: i32 = 10;
const JR_M_FIELDS: i32 = 12;
const JR_NONE: i32 = -1;
const JR_CLASH_CHECKS: i32 = 16;

// ── Index data ────────────────────────────────────────────────────
#[derive(Clone, Copy)]
struct Import {
    kind: c_char,
    // s single, w on-demand, m static single, n static on-demand
    alias: *const c_char,
    target: *const c_char,
}
impl Default for Import {
    fn default() -> Self {
        unsafe { zeroed() }
    }
}

#[derive(Clone, Copy)]
struct Lines {
    from: u32,
    to: u32,
}
impl Default for Lines {
    fn default() -> Self {
        unsafe { zeroed() }
    }
}

// Line numbers are meaningful only in a file re-extracted by this run: the
// persisted scope of every other file carries zeros. They are read for the
// SOURCE file's own context, never for a target.
#[derive(Clone, Copy)]
struct TypeDecl {
    start: u32,
    end: u32,
    kind: c_char,
    // c i e r a o C t
    incomplete: bool,
    own: bool,
    // its definition is the one the graph keeps for the slot
    path: *const c_char,
    // logical path
    name: *const c_char,
    tparams: *const c_char,
    supers: *const c_char,
    slot: *const c_char,
    // NULL: no definition; "=": the logical path; else the name below the module
    pnames: *const c_char,
    // run files only
    outer: i32,
    // the enclosing type of this file, or JR_NONE
    entity: i32,
}
impl Default for TypeDecl {
    fn default() -> Self {
        unsafe { zeroed() }
    }
}

#[derive(Clone, Copy)]
struct Member {
    start: u32,
    kind: c_char,
    // c callable, v value
    ctor: bool,
    // K
    component: bool,
    // p: a record component or constructor property
    own: bool,
    owner: i32,
    // its type in this file; JR_NONE: a Kotlin top-level declaration
    order: i32,
    // position in the scope
    shares: i32,
    // the member of this file whose node it shares; JR_NONE: it has none
    recv_entity: i32,
    // a Kotlin extension's receiver type, when the repository declares it
    name: *const c_char,
    tparams: *const c_char,
    sig: *const c_char,
    recv: *const c_char,
    slot: *const c_char,
    pnames: *const c_char,
}
impl Default for Member {
    fn default() -> Self {
        unsafe { zeroed() }
    }
}

#[derive(Clone, Copy)]
struct File {
    rel_path: *const c_char,
    module_qn: *const c_char,
    package: *const c_char,
    kotlin: bool,
    is_test: bool,
    scoped: bool,
    // its scope was read: without one nothing in it resolves
    imports: *mut Import,
    // single imports, sorted by alias then target
    nimports: i32,
    wild: *mut Import,
    // on-demand imports, scope order
    nwild: i32,
    types: *mut TypeDecl,
    // scope order: an outer type before its nested ones
    ntypes: i32,
    members: *mut Member,
    // sorted by (owner, name, order)
    nmembers: i32,
    by_start: *mut i32,
    // member indices sorted by (start, order)
    unplaced: *mut Lines,
    nunplaced: i32,
}
impl Default for File {
    fn default() -> Self {
        unsafe { zeroed() }
    }
}

#[derive(Clone, Copy)]
struct Decl {
    file: i32,
    type_index: i32,
}
impl Default for Decl {
    fn default() -> Self {
        unsafe { zeroed() }
    }
}

// A step of javadoc's search for a method (walk) that is no type of the
// repository.
const JR_W_NONE: i32 = -1;
const JR_W_OBJECT: i32 = -2;
const JR_W_ENUM: i32 = -3;
const JR_W_ANNOTATION: i32 = -4;
const JR_W_OUTSIDE: i32 = -5;

#[derive(Clone, Copy)]
struct Entity {
    fqn: *const c_char,
    name: *const c_char,
    kind: c_char,
    decls: *mut Decl,
    // sorted by file (path order), then scope order
    ndecls: i32,
    dcap: i32,
    bases: *mut i32,
    // the supertypes its declarations name, as far as the repository declares them
    nbases: i32,
    closure: *mut i32,
    // every supertype the repository declares, nearest first
    nclosure: i32,
    companion: i32,
    // Kotlin: its companion object, or JR_NONE
    open: bool,
    // a supertype outside the repository, or one that could not be bound
    incomplete: bool,
    // a declaration of it has members a parse error hides
    any_prod: bool,
    // javadoc's search for a method at this type (walk): the superclass
    // step (a type, or JR_W_*), the interface steps in the order written,
    // and the type around it
    wsuper: i32,
    wifaces: *mut i32,
    nwifaces: i32,
    encl: i32,
}
impl Default for Entity {
    fn default() -> Self {
        unsafe { zeroed() }
    }
}

#[derive(Clone, Copy)]
struct IntList {
    items: *mut i32,
    count: i32,
    cap: i32,
}
impl Default for IntList {
    fn default() -> Self {
        unsafe { zeroed() }
    }
}

// What the resolve phase counts for the run's log line. The index itself is
// read-only then; these are the only words it writes.
struct Stats {
    source_gaps: AtomicI64,
    // resolved, but the documented definition has no node
    plain_text: AtomicI64,
    // KDoc `[name]` that names nothing: plain text
    type_and_function: AtomicI64,
    // KDoc name that is a type and a function at one
    // level: resolved as the type
    type_and_function_node: AtomicI64,
    // ... where the type has a node (an edge)
    // Members further on that a lookup did not take, because something
    // before them may have the member unseen (javadoc's search, walk; the
    // guards of kotlin_simple and kotlin_in_type): `external`.
    java_given: AtomicI64,
    // Javadoc method: the language gives one first
    java_open: AtomicI64,
    // ... or a type outside the repository comes first
    kotlin_given: AtomicI64,
    // KDoc name: the same two cases
    kotlin_open: AtomicI64,
    kotlin_extension: AtomicI64,
    // KDoc `Type.name`: an extension, the type's
    // hierarchy leaving the repository
    // Javadoc fields not taken, because a method that javadoc's search may
    // reach first, outside the repository, may be what it links
    // java_field.
    java_field: AtomicI64,
    // `#x`
    java_field_named: AtomicI64,
    // `Type#x`
}
impl Default for Stats {
    fn default() -> Self {
        unsafe { zeroed() }
    }
}

#[derive(Clone, Copy)]
struct Index {
    arena: *mut Arena,
    oom: bool,
    stats: *mut Stats,
    mode: i32,
    // the run's index mode: which directories discovery skips
    project: *const c_char,
    files: *mut File,
    nfiles: i32,
    run_to_file: *mut i32,
    run_count: i32,
    ents: *mut Entity,
    nents: i32,
    ecap: i32,
    ent_by_fqn: *mut HashTable,
    // fqn -> entity + 1
    pkg_types: *mut HashTable,
    // "package\x1f name" -> entity + 1 (top-level types)
    packages: *mut HashTable,
    // every package a file declares
    pkg_prefixes: *mut HashTable,
    // ... and every prefix of one
    pkg_members: *mut HashTable,
    // "package\x1f name" -> ilist_t* of files (Kotlin top level)
    extensions: *mut HashTable,
    // "<entity>\x1f name" -> ilist_t* of (file, member) pairs
    quarantine: *mut HashTable,
    // names of types declared where no scope is known
    quarantine_test: *mut HashTable,
    // the same, declared by test code only
}
impl Default for Index {
    fn default() -> Self {
        unsafe { zeroed() }
    }
}

// ── Small helpers ─────────────────────────────────────────────────
// Index memory. A failed allocation is remembered: whatever a caller does
// with its NULL, the build as a whole reports failure instead of handing out
// an index that silently lacks a declaration.
unsafe fn alloc(ix: *mut Index, n: usize) -> *mut c_void {
    let p: *mut c_void = cbm_arena_alloc(
        (*ix).arena,
        if (n) != 0 { n } else { SKIP_ONE as usize },
    );
    (*ix).oom = ((*ix).oom) || (p.is_null());
    return p;
}

unsafe fn strdup(ix: *mut Index, s: *const c_char) -> *mut c_char {
    let p: *mut c_char = cbm_arena_strdup(
        (*ix).arena,
        if !(s).is_null() { s } else { c"".as_ptr() },
    );
    (*ix).oom = ((*ix).oom) || (p.is_null());
    return p;
}

unsafe fn ilist_push(ix: *mut Index, l: *mut IntList, v: i32) -> bool {
    if (*l).count >= (*l).cap {
        let ncap: i32 = if ((*l).cap) != 0 {
            (*l).cap * PAIR_LEN
        } else {
            CBM_SZ_4
        };
        let grown: *mut i32 =
            (alloc(ix, (ncap as usize) * size_of::<i32>())) as *mut i32;
        if grown.is_null() {
            return false;
        }
        if (*l).count > 0 {
            memcpy(
                grown as *mut c_void,
                ((*l).items) as *const c_void,
                (((*l).count) as usize) * size_of::<i32>(),
            );
        }
        (*l).items = grown;
        (*l).cap = ncap;
    }
    *((*l).items).offset(
        ({
            let value = addr_of_mut!((*l).count);
            let previous = *value;
            *value += 1;
            previous
        }) as isize,
    ) = v;
    return true;
}

unsafe fn ht_ilist(
    ix: *mut Index,
    ht: *mut HashTable,
    key: *const c_char,
    create: bool,
) -> *mut IntList {
    let mut l: *mut IntList =
        (cbm_ht_get(ht as *const HashTable, key)) as *mut IntList;
    if (!(l).is_null()) || (!(create)) {
        return l;
    }
    l = (alloc(ix, size_of::<IntList>())) as *mut IntList;
    let k: *mut c_char = strdup(ix, key);
    if (l.is_null()) || (k.is_null()) {
        return ptr::null_mut();
    }
    clear_bytes(l as *mut c_void, 0, size_of::<IntList>());
    cbm_ht_set(ht, k as *const c_char, l as *mut c_void);
    return l;
}

// Add `key` to a name set; false when memory ran out.
unsafe fn mark(ix: *mut Index, ht: *mut HashTable, key: *const c_char) -> bool {
    if !(cbm_ht_get(ht as *const HashTable, key)).is_null() {
        return true;
    }
    let k: *mut c_char = strdup(ix, key);
    if k.is_null() {
        return false;
    }
    cbm_ht_set(ht, k as *const c_char, k as *mut c_void);
    return true;
}

unsafe fn ci_equal(s: *const c_char, n: usize, word: *const c_char) -> bool {
    if strlen(word) != n {
        return false;
    }
    for i in 0..n {
        if tolower(((*s.offset(i as isize)) as u8) as i32)
            != ((*word.offset(i as isize)) as i32)
        {
            return false;
        }
    }
    return true;
}

unsafe fn ci_contains(s: *const c_char, n: usize, word: *const c_char) -> bool {
    let wl: usize = strlen(word);
    {
        let mut i: usize = 0;
        while (i + wl) <= n {
            let mut k: usize = 0;
            while (k < wl)
                && (tolower(((*s.offset((i + k) as isize)) as u8) as i32)
                    == ((*word.offset(k as isize)) as i32))
            {
                k += 1;
            }
            if k == wl {
                return true;
            }

            i += 1;
        }
    }
    return false;
}

// True when one word of the directory name s[0, n) says test code: the name
// is split at '-', '_', '.' and at a lower-to-upper case step (`commonTest`,
// `json-tests`, `testFixtures`, `smoke-test-http`), and a word is compared
// whole, so `latest` and `attestation` say nothing.
unsafe fn test_dir_name(s: *const c_char, n: usize) -> bool {
    let words: [*const c_char; 8] = [
        c"test".as_ptr(),
        c"tests".as_ptr(),
        c"fixture".as_ptr(),
        c"fixtures".as_ptr(),
        c"sample".as_ptr(),
        c"samples".as_ptr(),
        c"example".as_ptr(),
        c"examples".as_ptr(),
    ];
    let mut start: usize = 0;
    for i in 0..=n {
        let sep: bool = (((i == n)
            || (((*s.offset(i as isize)) as i32) == b'-' as i32))
            || (((*s.offset(i as isize)) as i32) == b'_' as i32))
            || (((*s.offset(i as isize)) as i32) == b'.' as i32);
        let step: bool = (((!(sep)) && (i > start))
            && ((isupper(((*s.offset(i as isize)) as u8) as i32)) != 0))
            && ((isupper(
                ((*s.offset((i - (SKIP_ONE as usize)) as isize)) as u8) as i32,
            )) == 0);
        if (!(sep)) && (!(step)) {
            continue;
        }
        {
            let mut w: usize = 0;
            while w
                < (size_of::<[*const c_char; 8]>() / size_of::<*const c_char>())
            {
                if ci_equal(s.offset(start as isize), i - start, words[w]) {
                    return true;
                }

                w += 1;
            }
        }
        start = if sep { i + (SKIP_ONE as usize) } else { i };
    }
    return false;
}

// Test code, by where a build puts it:
//   - the source set after the last `src` directory holds "test" (`src/test`,
//     `src/jvmTest`, `src/testFixtures`, `src/internalClusterTest`), or
//   - a directory above that source root (any directory, when there is no
//     `src`) has a word test, fixture, sample or example in its name: a test
//     project, a `commonTest/src` source set, a samples tree.
// Directories below a source set are packages: `src/main/java/org/acme/test/
// Tool.java` is product code. A file name decides nothing.
unsafe fn is_test_path(rel: *const c_char) -> bool {
    let mut src_set: *const c_char = ptr::null();
    // the segment after the last `src` directory
    let mut src_set_len: usize = 0;
    let mut last_src: *const c_char = ptr::null();
    {
        let mut p: *const c_char = rel;
        loop {
            let slash: *const c_char = (strchr(p, b'/' as i32)) as *const c_char;
            if slash.is_null() {
                break;
            }
            if (slash.offset_from(p) == 3)
                && (memcmp(
                    p as *const c_void,
                    (c"src".as_ptr() as *mut c_char) as *const c_void,
                    3 as usize,
                ) == 0)
            {
                let next: *const c_char = slash.offset(SKIP_ONE as isize);
                let end: *const c_char =
                    (strchr(next, b'/' as i32)) as *const c_char;
                last_src = p;
                src_set = if !(end).is_null() { next } else { ptr::null() };
                // a file right under src is no source set
                src_set_len = if !(end).is_null() {
                    (end.offset_from(next)) as usize
                } else {
                    0
                };
            }
            p = slash.offset(SKIP_ONE as isize);
        }
    }
    if (!(src_set).is_null())
        && (ci_contains(src_set, src_set_len, c"test".as_ptr()))
    {
        return true;
    }
    {
        let mut p: *const c_char = rel;
        while (last_src.is_null()) || (p < last_src) {
            let slash: *const c_char = (strchr(p, b'/' as i32)) as *const c_char;
            if slash.is_null() {
                break;
            }
            if test_dir_name(p, (slash.offset_from(p)) as usize) {
                return true;
            }
            p = slash.offset(SKIP_ONE as isize);
        }
    }
    return false;
}

// Split `s` in place at tabs into at most `max` fields; returns the count.
unsafe fn split(s: *mut c_char, out: *mut *mut c_char, max: i32) -> i32 {
    let mut n: i32 = 0;
    *out.offset(
        ({
            let value = addr_of_mut!(n);
            let previous = *value;
            *value += 1;
            previous
        }) as isize,
    ) = s;
    {
        let mut p: *mut c_char = s;
        while ((*(p)) != 0) && (n < max) {
            if ((*(p)) as i32) == b'\t' as i32 {
                *(p) = b'\0' as c_char;
                *out.offset(
                    ({
                        let value = addr_of_mut!(n);
                        let previous = *value;
                        *value += 1;
                        previous
                    }) as isize,
                ) = p.offset(SKIP_ONE as isize);
            }

            p = p.offset(1);
        }
    }
    return n;
}

// True when name[0, nl) is one of the ','-joined names of `list`. An entry may
// carry a bound (`T:Comparable`, a Java type variable's): *bound / *blen are
// set to it (length 0 when it has none).
unsafe fn listed_n(
    list: *const c_char,
    name: *const c_char,
    nl: usize,
    bound: *mut *const c_char,
    blen: *mut usize,
) -> bool {
    {
        let mut p: *const c_char = list;
        while (!(p).is_null()) && ((*(p)) != 0) {
            let e: *const c_char = (strchr(p, b',' as i32)) as *const c_char;
            let n: usize = if !(e).is_null() {
                (e.offset_from(p)) as usize
            } else {
                strlen(p)
            };
            let colon: *const c_char =
                (memchr(p as *const c_void, b':' as i32, n)) as *const c_char;
            let name_len: usize = if !(colon).is_null() {
                (colon.offset_from(p)) as usize
            } else {
                n
            };
            if (name_len == nl)
                && (memcmp(p as *const c_void, name as *const c_void, nl) == 0)
            {
                if !(bound).is_null() {
                    *(bound) = if !(colon).is_null() {
                        colon.offset(SKIP_ONE as isize)
                    } else {
                        p.offset(n as isize)
                    };
                    *(blen) = if !(colon).is_null() {
                        (n - name_len) - (SKIP_ONE as usize)
                    } else {
                        0
                    };
                }
                return true;
            }
            p = if !(e).is_null() {
                e.offset(SKIP_ONE as isize)
            } else {
                ptr::null()
            };
        }
    }
    return false;
}

unsafe fn listed(list: *const c_char, name: *const c_char) -> bool {
    return listed_n(
        list,
        name,
        strlen(name),
        (ptr::null_mut()) as *mut *const c_char,
        ptr::null_mut(),
    );
}

// ── Scope parsing ─────────────────────────────────────────────────
unsafe fn import_cmp(a: *const c_void, b: *const c_void) -> i32 {
    let x: *const Import = a as *const Import;
    let y: *const Import = b as *const Import;
    let c: i32 = strcmp((*x).alias, (*y).alias);
    return if (c) != 0 {
        c
    } else {
        strcmp((*x).target, (*y).target)
    };
}

unsafe fn member_cmp(a: *const c_void, b: *const c_void) -> i32 {
    let x: *const Member = a as *const Member;
    let y: *const Member = b as *const Member;
    if (*x).owner != (*y).owner {
        return if (*x).owner < (*y).owner { -(1) } else { 1 };
    }
    let c: i32 = strcmp((*x).name, (*y).name);
    if (c) != 0 {
        return c;
    }
    return (i32::from((*x).order > (*y).order))
        - (i32::from((*x).order < (*y).order));
}

#[derive(Clone, Copy)]
struct StartKey {
    start: u32,
    order: i32,
    idx: i32,
}
impl Default for StartKey {
    fn default() -> Self {
        unsafe { zeroed() }
    }
}

unsafe fn start_cmp(a: *const c_void, b: *const c_void) -> i32 {
    let x: *const StartKey = a as *const StartKey;
    let y: *const StartKey = b as *const StartKey;
    if (*x).start != (*y).start {
        return if (*x).start < (*y).start { -(1) } else { 1 };
    }
    return (i32::from((*x).order > (*y).order))
        - (i32::from((*x).order < (*y).order));
}

unsafe fn lines_cmp(a: *const c_void, b: *const c_void) -> i32 {
    let x: *const Lines = a as *const Lines;
    let y: *const Lines = b as *const Lines;
    if (*x).from != (*y).from {
        return if (*x).from < (*y).from { -(1) } else { 1 };
    }
    return (i32::from((*x).to > (*y).to)) - (i32::from((*x).to < (*y).to));
}

unsafe fn parse_type(
    f: *mut File,
    fld: *mut *mut c_char,
    stack: *mut i32,
    sp: *mut i32,
) {
    let t: *mut TypeDecl =
        addr_of_mut!(*((*f).types).offset(((*f).ntypes) as isize));
    clear_bytes(t as *mut c_void, 0, size_of::<TypeDecl>());
    (*t).start = (strtoul(
        (*fld.offset(1)) as *const c_char,
        (ptr::null_mut()) as *mut *mut c_char,
        10,
    )) as u32;
    (*t).end = (strtoul(
        (*fld.offset(2)) as *const c_char,
        (ptr::null_mut()) as *mut *mut c_char,
        10,
    )) as u32;
    (*t).kind = *(*fld.offset(3)).offset(0);
    (*t).incomplete = ((*(*fld.offset(3)).offset(0)) != 0)
        && (((*(*fld.offset(3)).offset(1)) as i32) == b'!' as i32);
    (*t).path = (*fld.offset(4)) as *const c_char;
    let dot: *const c_char = (strrchr((*t).path, b'.' as i32)) as *const c_char;
    (*t).name = if !(dot).is_null() {
        dot.offset(SKIP_ONE as isize)
    } else {
        (*t).path
    };
    (*t).tparams = (*fld.offset(5)) as *const c_char;
    (*t).supers = (*fld.offset(6)) as *const c_char;
    (*t).slot = (if strcmp((*fld.offset(7)) as *const c_char, c"-".as_ptr()) == 0
    {
        ptr::null_mut()
    } else {
        *fld.offset(7)
    }) as *const c_char;
    (*t).own = ((*(*fld.offset(8)).offset(0)) as i32) == b'1' as i32;
    (*t).pnames = (*fld.offset(9)) as *const c_char;
    (*t).entity = JR_NONE;
    // the enclosing type: the nearest earlier type whose path is this path
    // without its last name (a nested type follows its outer type)
    let outer_len: usize = if !(dot).is_null() {
        (dot.offset_from((*t).path)) as usize
    } else {
        0
    };
    while *(sp) > 0 {
        let op: *const c_char = (*((*f).types)
            .offset((*stack.offset((*(sp) - SKIP_ONE) as isize)) as isize))
        .path;
        if ((!(dot).is_null()) && (strlen(op) == outer_len))
            && (memcmp(
                op as *const c_void,
                ((*t).path) as *const c_void,
                outer_len,
            ) == 0)
        {
            break;
        }
        (*(sp)) -= 1;
    }
    (*t).outer = if (!(dot).is_null()) && (*(sp) > 0) {
        *stack.offset((*(sp) - SKIP_ONE) as isize)
    } else {
        JR_NONE
    };
    *stack.offset(
        ({
            let value = addr_of_mut!((*(sp)));
            let previous = *value;
            *value += 1;
            previous
        }) as isize,
    ) = (*f).ntypes;
    (*f).ntypes += 1;
}

unsafe fn parse_member(f: *mut File, fld: *mut *mut c_char) -> bool {
    let m: *mut Member =
        addr_of_mut!(*((*f).members).offset(((*f).nmembers) as isize));
    clear_bytes(m as *mut c_void, 0, size_of::<Member>());
    (*m).start = (strtoul(
        (*fld.offset(1)) as *const c_char,
        (ptr::null_mut()) as *mut *mut c_char,
        10,
    )) as u32;
    (*m).kind = *(*fld.offset(2)).offset(0);
    (*m).ctor = strchr((*fld.offset(3)) as *const c_char, b'K' as i32)
        != (ptr::null_mut());
    (*m).component = strchr((*fld.offset(3)) as *const c_char, b'p' as i32)
        != (ptr::null_mut());
    (*m).owner = if strcmp((*fld.offset(4)) as *const c_char, c"-".as_ptr()) == 0
    {
        JR_NONE
    } else {
        atoi((*fld.offset(4)) as *const c_char)
    };
    if ((((*m).owner >= (*f).ntypes)
        || (((*m).owner < 0) && ((*m).owner != JR_NONE)))
        || ((*(*fld.offset(5)).offset(0)) == 0))
        || (((((*m).kind) as i32) != b'c' as i32)
            && ((((*m).kind) as i32) != b'v' as i32))
    {
        return false;
        // not a record this code wrote
    }
    (*m).name = (*fld.offset(5)) as *const c_char;
    (*m).tparams = (*fld.offset(6)) as *const c_char;
    (*m).sig = (*fld.offset(7)) as *const c_char;
    (*m).recv = (*fld.offset(8)) as *const c_char;
    (*m).slot = (if strcmp((*fld.offset(9)) as *const c_char, c"-".as_ptr()) == 0
    {
        ptr::null_mut()
    } else {
        *fld.offset(9)
    }) as *const c_char;
    (*m).own = ((*(*fld.offset(10)).offset(0)) as i32) == b'1' as i32;
    (*m).pnames = (*fld.offset(11)) as *const c_char;
    (*m).order = (*f).nmembers;
    (*m).shares = JR_NONE;
    (*m).recv_entity = JR_NONE;
    (*f).nmembers += 1;
    return true;
}

// Which member's node each member shares. A value has its own or none.
// Callables of one name in one type (overloads) share the node of the one
// that owns their slot.
unsafe fn link_shared_nodes(f: *mut File) {
    {
        let mut i: i32 = 0;
        while i < (*f).nmembers {
            let mut j: i32 = i;
            let mut owner: i32 = JR_NONE;
            // the callable of this run that owns the node
            while ((j < (*f).nmembers)
                && ((*((*f).members).offset(j as isize)).owner
                    == (*((*f).members).offset(i as isize)).owner))
                && (strcmp(
                    (*((*f).members).offset(j as isize)).name,
                    (*((*f).members).offset(i as isize)).name,
                ) == 0)
            {
                let m: *mut Member =
                    addr_of_mut!(*((*f).members).offset(j as isize));
                if (((((*m).kind) as i32) == b'c' as i32)
                    && (!((*m).slot).is_null()))
                    && ((*m).own)
                {
                    owner = j;
                }
                j += 1;
            }
            for k in i..j {
                let m: *mut Member =
                    addr_of_mut!(*((*f).members).offset(k as isize));
                if ((*m).slot).is_null() {
                    continue;
                }
                if (*m).own {
                    (*m).shares = k;
                } else if ((((((*m).kind) as i32) == b'c' as i32)
                    && (owner != JR_NONE))
                    && (strcmp(
                        (*((*f).members).offset(owner as isize)).slot,
                        (*m).slot,
                    ) == 0))
                    && ((*((*f).members).offset(owner as isize)).ctor
                        == (*m).ctor)
                {
                    (*m).shares = owner;
                }
            }
            i = j;
        }
    }
}

// Parse one file's scope. false when it is not a scope this code wrote, or
// memory ran out.
unsafe fn parse_scope(
    ix: *mut Index,
    f: *mut File,
    blob: *const c_char,
) -> bool {
    let buf: *mut c_char = strdup(ix, blob);
    if buf.is_null() {
        return false;
    }
    let mut ni: i32 = 0;
    let mut nt: i32 = 0;
    let mut nm: i32 = 0;
    let mut nx: i32 = 0;
    {
        let mut p: *const c_char = buf as *const c_char;
        while (*(p)) != 0 {
            ni += i32::from(((*(p)) as i32) == b'I' as i32);
            nt += i32::from(((*(p)) as i32) == b'T' as i32);
            nm += i32::from(((*(p)) as i32) == b'M' as i32);
            nx += i32::from(((*(p)) as i32) == b'X' as i32);
            let nl: *const c_char = (strchr(p, b'\n' as i32)) as *const c_char;
            if nl.is_null() {
                break;
            }
            p = nl.offset(SKIP_ONE as isize);
        }
    }
    (*f).imports =
        (alloc(ix, (ni as usize) * size_of::<Import>())) as *mut Import;
    (*f).wild = (alloc(ix, (ni as usize) * size_of::<Import>())) as *mut Import;
    (*f).types =
        (alloc(ix, (nt as usize) * size_of::<TypeDecl>())) as *mut TypeDecl;
    (*f).members =
        (alloc(ix, (nm as usize) * size_of::<Member>())) as *mut Member;
    (*f).unplaced =
        (alloc(ix, (nx as usize) * size_of::<Lines>())) as *mut Lines;
    let stack: *mut i32 = (cbm_alloc(
        CBM_MEM_CLASS_OTHER,
        ((nt + SKIP_ONE) as usize) * size_of::<i32>(),
    )) as *mut i32;
    if (((((((*f).imports).is_null()) || (((*f).wild).is_null()))
        || (((*f).types).is_null()))
        || (((*f).members).is_null()))
        || (((*f).unplaced).is_null()))
        || (stack.is_null())
    {
        cbm_free(CBM_MEM_CLASS_OTHER, stack as *mut c_void);
        (*ix).oom = true;
        return false;
    }
    let mut sp: i32 = 0;
    let mut first: bool = true;
    let mut ok: bool = true;
    {
        let mut line: *mut c_char = buf;
        while ((ok) && (!(line).is_null())) && ((*(line)) != 0) {
            let nl: *mut c_char = strchr(line as *const c_char, b'\n' as i32);
            if !(nl).is_null() {
                *(nl) = b'\0' as c_char;
            }
            let mut fld: [*mut c_char; 12] = zeroed();
            if first {
                first = false;
                ok = strcmp(line as *const c_char, c"jvm1".as_ptr()) == 0;
            } else if ((*line.offset(0)) as i32) == b'L' as i32 {
                (*f).kotlin = (split(
                    line,
                    addr_of_mut!(fld).cast::<*mut c_char>(),
                    PAIR_LEN,
                ) == PAIR_LEN)
                    && (((*(fld[1]).offset(0)) as i32) == b'k' as i32);
            } else if ((*line.offset(0)) as i32) == b'P' as i32 {
                if split(line, addr_of_mut!(fld).cast::<*mut c_char>(), PAIR_LEN)
                    == PAIR_LEN
                {
                    (*f).package = (fld[1]) as *const c_char;
                }
            } else if ((*line.offset(0)) as i32) == b'I' as i32 {
                if ((split(
                    line,
                    addr_of_mut!(fld).cast::<*mut c_char>(),
                    CBM_SZ_4,
                ) == CBM_SZ_4)
                    && ((*(fld[1]).offset(0)) != 0))
                    && ((*(fld[3]).offset(0)) != 0)
                {
                    let imp: Import = Import {
                        kind: *(fld[1]).offset(0),
                        alias: ((fld[2]) as *const c_char),
                        target: ((fld[3]) as *const c_char),
                    };
                    if (((imp.kind) as i32) == b'w' as i32)
                        || (((imp.kind) as i32) == b'n' as i32)
                    {
                        *((*f).wild).offset(
                            ({
                                let value = addr_of_mut!((*f).nwild);
                                let previous = *value;
                                *value += 1;
                                previous
                            }) as isize,
                        ) = imp;
                    } else {
                        *((*f).imports).offset(
                            ({
                                let value = addr_of_mut!((*f).nimports);
                                let previous = *value;
                                *value += 1;
                                previous
                            }) as isize,
                        ) = imp;
                    }
                }
            } else if ((*line.offset(0)) as i32) == b'T' as i32 {
                // a member names its type by position: a type record that cannot
                // be read spoils the whole scope
                ok = (split(
                    line,
                    addr_of_mut!(fld).cast::<*mut c_char>(),
                    JR_T_FIELDS,
                ) == JR_T_FIELDS)
                    && ((*(fld[4]).offset(0)) != 0);
                if ok {
                    parse_type(
                        f,
                        addr_of_mut!(fld).cast::<*mut c_char>(),
                        stack,
                        addr_of_mut!(sp),
                    );
                }
            } else if ((*line.offset(0)) as i32) == b'M' as i32 {
                ok = (split(
                    line,
                    addr_of_mut!(fld).cast::<*mut c_char>(),
                    JR_M_FIELDS,
                ) == JR_M_FIELDS)
                    && (parse_member(
                        f,
                        addr_of_mut!(fld).cast::<*mut c_char>(),
                    ));
            } else if ((*line.offset(0)) as i32) == b'X' as i32 {
                if split(line, addr_of_mut!(fld).cast::<*mut c_char>(), 3) == 3 {
                    *((*f).unplaced).offset(
                        ({
                            let value = addr_of_mut!((*f).nunplaced);
                            let previous = *value;
                            *value += 1;
                            previous
                        }) as isize,
                    ) = Lines {
                        from: ((strtoul(
                            (fld[1]) as *const c_char,
                            (ptr::null_mut()) as *mut *mut c_char,
                            10,
                        )) as u32),
                        to: ((strtoul(
                            (fld[2]) as *const c_char,
                            (ptr::null_mut()) as *mut *mut c_char,
                            10,
                        )) as u32),
                    };
                }
            } else if ((*line.offset(0)) as i32) == b'Q' as i32 {
                if (split(
                    line,
                    addr_of_mut!(fld).cast::<*mut c_char>(),
                    PAIR_LEN,
                ) == PAIR_LEN)
                    && ((*(fld[1]).offset(0)) != 0)
                {
                    ok = mark(
                        ix,
                        if (*f).is_test {
                            (*ix).quarantine_test
                        } else {
                            (*ix).quarantine
                        },
                        (fld[1]) as *const c_char,
                    );
                }
            }
            line = if !(nl).is_null() {
                nl.offset(SKIP_ONE as isize)
            } else {
                ptr::null_mut()
            };
        }
    }
    cbm_free(CBM_MEM_CLASS_OTHER, stack as *mut c_void);
    if (!(ok)) || (first) {
        return false;
    }
    if (*f).nimports > 1 {
        sort_records(
            (((*f).imports) as *mut c_void).cast::<Import>(),
            ((*f).nimports) as usize,
            import_cmp,
        );
    }
    if (*f).nmembers > 1 {
        sort_records(
            (((*f).members) as *mut c_void).cast::<Member>(),
            ((*f).nmembers) as usize,
            member_cmp,
        );
    }
    link_shared_nodes(f);
    // the unplaced line ranges: sorted and disjoint, for a binary search
    if (*f).nunplaced > 1 {
        sort_records(
            (((*f).unplaced) as *mut c_void).cast::<Lines>(),
            ((*f).nunplaced) as usize,
            lines_cmp,
        );
        let mut w: i32 = 0;
        for i in SKIP_ONE..(*f).nunplaced {
            if (*((*f).unplaced).offset(i as isize)).from
                <= (*((*f).unplaced).offset(w as isize)).to
            {
                if (*((*f).unplaced).offset(i as isize)).to
                    > (*((*f).unplaced).offset(w as isize)).to
                {
                    (*((*f).unplaced).offset(w as isize)).to =
                        (*((*f).unplaced).offset(i as isize)).to;
                }
            } else {
                *((*f).unplaced).offset(
                    ({
                        let value = addr_of_mut!(w);
                        let _previous = *value;
                        *value += 1;
                        *value
                    }) as isize,
                ) = *((*f).unplaced).offset(i as isize);
            }
        }
        (*f).nunplaced = w + SKIP_ONE;
    }
    (*f).by_start =
        (alloc(ix, (((*f).nmembers) as usize) * size_of::<i32>())) as *mut i32;
    let keys: *mut StartKey = if (*f).nmembers > 0 {
        (cbm_alloc(
            CBM_MEM_CLASS_OTHER,
            (((*f).nmembers) as usize) * size_of::<StartKey>(),
        )) as *mut StartKey
    } else {
        ptr::null_mut()
    };
    if ((*f).nmembers > 0) && ((((*f).by_start).is_null()) || (keys.is_null())) {
        cbm_free(CBM_MEM_CLASS_OTHER, keys as *mut c_void);
        (*ix).oom = true;
        return false;
    }
    for i in 0..(*f).nmembers {
        *keys.offset(i as isize) = StartKey {
            start: (*((*f).members).offset(i as isize)).start,
            order: (*((*f).members).offset(i as isize)).order,
            idx: i,
        };
    }
    if (*f).nmembers > 1 {
        sort_records(
            (keys as *mut c_void).cast::<StartKey>(),
            ((*f).nmembers) as usize,
            start_cmp,
        );
    }
    for i in 0..(*f).nmembers {
        *((*f).by_start).offset(i as isize) = (*keys.offset(i as isize)).idx;
    }
    cbm_free(CBM_MEM_CLASS_OTHER, keys as *mut c_void);
    return true;
}

// ── Entities ──────────────────────────────────────────────────────
const JR_ENT_FAIL: i32 = -1;
const JR_ENT_SKIP: i32 = -2;

// Byte-preserving formatting for keys, including non-UTF-8 source names.
unsafe fn write_key(out: *mut c_char, cap: usize, parts: &[&[u8]]) -> bool {
    let len = parts.iter().map(|part| part.len()).sum::<usize>();
    if len >= cap {
        return false;
    }
    let mut offset = 0;
    for part in parts {
        ptr::copy_nonoverlapping(
            part.as_ptr(),
            out.add(offset).cast(),
            part.len(),
        );
        offset += part.len();
    }
    out.add(offset).write(0);
    true
}

fn decimal(value: i64) -> [u8; 32] {
    let mut bytes = [0; 32];
    let result = write!(&mut bytes[..], "{value}");
    debug_assert!(result.is_ok());
    bytes
}

unsafe fn name_bytes<'a>(name: *const c_char) -> &'a [u8] {
    CStr::from_ptr(name).to_bytes()
}

unsafe fn write_fqn(
    package: *const c_char,
    path: *const c_char,
    out: *mut c_char,
    cap: usize,
) -> bool {
    if package.is_null() || *package == 0 {
        write_key(out, cap, &[name_bytes(path)])
    } else {
        write_key(out, cap, &[name_bytes(package), b".", name_bytes(path)])
    }
}

unsafe fn pair_key(
    a: *const c_char,
    b: *const c_char,
    out: *mut c_char,
    cap: usize,
) -> bool {
    let prefix = if a.is_null() { &[] } else { name_bytes(a) };
    write_key(out, cap, &[prefix, b"\x1f", name_bytes(b)])
}

unsafe fn find_entity(ix: *const Index, fqn: *const c_char) -> i32 {
    let v: isize =
        (cbm_ht_get(((*ix).ent_by_fqn) as *const HashTable, fqn)) as isize;
    return if v > 0 {
        (v - (SKIP_ONE as isize)) as i32
    } else {
        JR_NONE
    };
}

// The entity `fqn`, created on first sight.
unsafe fn entity_get(
    ix: *mut Index,
    fqn: *const c_char,
    name: *const c_char,
    kind: c_char,
) -> i32 {
    let found: i32 = find_entity(ix as *const Index, fqn);
    if found != JR_NONE {
        return found;
    }
    if (*ix).nents >= (*ix).ecap {
        let ncap: i32 = if ((*ix).ecap) != 0 {
            (*ix).ecap * PAIR_LEN
        } else {
            CBM_SZ_1K
        };
        let grown: *mut Entity = (cbm_alloc(
            CBM_MEM_CLASS_OTHER,
            (ncap as usize) * size_of::<Entity>(),
        )) as *mut Entity;
        if grown.is_null() {
            return JR_ENT_FAIL;
        }
        if (*ix).nents > 0 {
            memcpy(
                grown as *mut c_void,
                ((*ix).ents) as *const c_void,
                (((*ix).nents) as usize) * size_of::<Entity>(),
            );
        }
        cbm_free(CBM_MEM_CLASS_OTHER, ((*ix).ents) as *mut c_void);
        (*ix).ents = grown;
        (*ix).ecap = ncap;
    }
    let id: i32 = (*ix).nents;
    let e: *mut Entity = addr_of_mut!(*((*ix).ents).offset(id as isize));
    clear_bytes(e as *mut c_void, 0, size_of::<Entity>());
    (*e).fqn = (strdup(ix, fqn)) as *const c_char;
    (*e).name = name;
    (*e).kind = kind;
    (*e).companion = JR_NONE;
    (*e).wsuper = JR_W_OUTSIDE;
    // until resolve_supers has read its declarations
    (*e).encl = JR_NONE;
    if ((*e).fqn).is_null() {
        return JR_ENT_FAIL;
    }
    (*ix).nents += 1;
    cbm_ht_set(
        (*ix).ent_by_fqn,
        (*e).fqn,
        ((id + SKIP_ONE) as isize) as *mut c_void,
    );
    return id;
}

unsafe fn entity_add_decl(
    ix: *mut Index,
    e: *mut Entity,
    file: i32,
    type_index: i32,
) -> bool {
    if (*e).ndecls >= (*e).dcap {
        let ncap: i32 = if ((*e).dcap) != 0 {
            (*e).dcap * PAIR_LEN
        } else {
            CBM_SZ_2
        };
        let grown: *mut Decl =
            (alloc(ix, (ncap as usize) * size_of::<Decl>())) as *mut Decl;
        if grown.is_null() {
            return false;
        }
        if (*e).ndecls > 0 {
            memcpy(
                grown as *mut c_void,
                ((*e).decls) as *const c_void,
                (((*e).ndecls) as usize) * size_of::<Decl>(),
            );
        }
        (*e).decls = grown;
        (*e).dcap = ncap;
    }
    *((*e).decls).offset(
        ({
            let value = addr_of_mut!((*e).ndecls);
            let previous = *value;
            *value += 1;
            previous
        }) as isize,
    ) = Decl {
        file: file,
        type_index: type_index,
    };
    (*e).any_prod =
        ((*e).any_prod) || (!((*((*ix).files).offset(file as isize)).is_test));
    (*e).incomplete = ((*e).incomplete)
        || ((*((*((*ix).files).offset(file as isize)).types)
            .offset(type_index as isize))
        .incomplete);
    return true;
}

unsafe fn mark_package(ix: *mut Index, package: *const c_char) -> bool {
    if (package.is_null()) || ((*package.offset(0)) == 0) {
        return true;
    }
    if !(mark(ix, (*ix).packages, package)) {
        return false;
    }
    let mut buf: [c_char; KEY_BYTES] = zeroed();
    if !write_key(buf.as_mut_ptr(), buf.len(), &[name_bytes(package)]) {
        return true;
    }
    {
        let mut dot: *mut c_char = strrchr(
            (addr_of_mut!(buf).cast::<c_char>()) as *const c_char,
            b'.' as i32,
        );
        while !(dot).is_null() {
            *(dot) = b'\0' as c_char;
            if !(mark(
                ix,
                (*ix).pkg_prefixes,
                (addr_of_mut!(buf).cast::<c_char>()) as *const c_char,
            )) {
                return false;
            }

            dot = strrchr(
                (addr_of_mut!(buf).cast::<c_char>()) as *const c_char,
                b'.' as i32,
            );
        }
    }
    return true;
}

// The entities of every declared type and the package tables. Files come in
// path order and types in scope order, so every declaration list is in that
// order too. *skipped counts declarations whose name is too long to index;
// their names are quarantined, so a reference to one binds nothing else.
unsafe fn build_entities(ix: *mut Index, skipped: *mut i32) -> bool {
    for fi in 0..(*ix).nfiles {
        let f: *mut File = addr_of_mut!(*((*ix).files).offset(fi as isize));
        if !(mark_package(ix, (*f).package)) {
            return false;
        }
        for ti in 0..(*f).ntypes {
            let t: *mut TypeDecl =
                addr_of_mut!(*((*f).types).offset(ti as isize));
            let mut fqn: [c_char; KEY_BYTES] = zeroed();
            let mut key: [c_char; KEY_BYTES] = zeroed();
            let id: i32 = if (write_fqn(
                (*f).package,
                (*t).path,
                addr_of_mut!(fqn).cast::<c_char>(),
                size_of::<[c_char; KEY_BYTES]>(),
            )) && (pair_key(
                (*f).package,
                (*t).name,
                addr_of_mut!(key).cast::<c_char>(),
                size_of::<[c_char; KEY_BYTES]>(),
            )) {
                entity_get(
                    ix,
                    (addr_of_mut!(fqn).cast::<c_char>()) as *const c_char,
                    (*t).name,
                    (*t).kind,
                )
            } else {
                JR_ENT_SKIP
            };
            if id == JR_ENT_SKIP {
                (*(skipped)) += 1;
                if !(mark(
                    ix,
                    if (*f).is_test {
                        (*ix).quarantine_test
                    } else {
                        (*ix).quarantine
                    },
                    (*t).name,
                )) {
                    return false;
                }
                continue;
            }
            if (id < 0)
                || (!(entity_add_decl(
                    ix,
                    addr_of_mut!(*((*ix).ents).offset(id as isize)),
                    fi,
                    ti,
                )))
            {
                return false;
            }
            (*t).entity = id;
            if (((((*t).kind) as i32) == b'C' as i32) && ((*t).outer != JR_NONE))
                && ((*((*f).types).offset(((*t).outer) as isize)).entity >= 0)
            {
                (*((*ix).ents).offset(
                    ((*((*f).types).offset(((*t).outer) as isize)).entity)
                        as isize,
                ))
                .companion = id;
            }
            if ((*t).outer == JR_NONE)
                && ((cbm_ht_get(
                    ((*ix).pkg_types) as *const HashTable,
                    (addr_of_mut!(key).cast::<c_char>()) as *const c_char,
                ))
                .is_null())
            {
                let k: *mut c_char = strdup(
                    ix,
                    (addr_of_mut!(key).cast::<c_char>()) as *const c_char,
                );
                if k.is_null() {
                    return false;
                }
                cbm_ht_set(
                    (*ix).pkg_types,
                    k as *const c_char,
                    ((id + SKIP_ONE) as isize) as *mut c_void,
                );
            }
        }
        // Kotlin top-level declarations: the files of a package that declare
        // a name (each file once: its members of one name are one run)
        {
            let mut mi: i32 = 0;

            while (mi < (*f).nmembers)
                && ((*((*f).members).offset(mi as isize)).owner == JR_NONE)
            {
                'iteration_1: {
                    let m: *const Member =
                        (addr_of_mut!(*((*f).members).offset(mi as isize)))
                            as *const Member;
                    if (mi > 0)
                        && (strcmp(
                            (*((*f).members).offset((mi - SKIP_ONE) as isize))
                                .name,
                            (*m).name,
                        ) == 0)
                    {
                        break 'iteration_1;
                    }
                    let mut key: [c_char; KEY_BYTES] = zeroed();
                    if !(pair_key(
                        (*f).package,
                        (*m).name,
                        addr_of_mut!(key).cast::<c_char>(),
                        size_of::<[c_char; KEY_BYTES]>(),
                    )) {
                        break 'iteration_1;
                    }
                    let l: *mut IntList = ht_ilist(
                        ix,
                        (*ix).pkg_members,
                        (addr_of_mut!(key).cast::<c_char>()) as *const c_char,
                        true,
                    );
                    if (l.is_null()) || (!(ilist_push(ix, l, fi))) {
                        return false;
                    }
                }
                mi += 1;
            }
        }
    }
    return true;
}

// ── The languages' own implicit imports ─────────────────────────────
// java.lang (Java and Kotlin/JVM) and the Kotlin default imports are part of
// the lookup order of every file. They are consulted last, after everything
// the repository declares, and only say "this simple name is the language's":
// external. Nothing else here is a list of names.
const JR_JAVA_LANG: [*const c_char; 108] = [
    c"AbstractMethodError".as_ptr(),
    c"Appendable".as_ptr(),
    c"ArithmeticException".as_ptr(),
    c"ArrayIndexOutOfBoundsException".as_ptr(),
    c"ArrayStoreException".as_ptr(),
    c"AssertionError".as_ptr(),
    c"AutoCloseable".as_ptr(),
    c"Boolean".as_ptr(),
    c"BootstrapMethodError".as_ptr(),
    c"Byte".as_ptr(),
    c"CharSequence".as_ptr(),
    c"Character".as_ptr(),
    c"Class".as_ptr(),
    c"ClassCastException".as_ptr(),
    c"ClassCircularityError".as_ptr(),
    c"ClassFormatError".as_ptr(),
    c"ClassLoader".as_ptr(),
    c"ClassNotFoundException".as_ptr(),
    c"ClassValue".as_ptr(),
    c"CloneNotSupportedException".as_ptr(),
    c"Cloneable".as_ptr(),
    c"Comparable".as_ptr(),
    c"Compiler".as_ptr(),
    c"Deprecated".as_ptr(),
    c"Double".as_ptr(),
    c"Enum".as_ptr(),
    c"EnumConstantNotPresentException".as_ptr(),
    c"Error".as_ptr(),
    c"Exception".as_ptr(),
    c"ExceptionInInitializerError".as_ptr(),
    c"Float".as_ptr(),
    c"FunctionalInterface".as_ptr(),
    c"IllegalAccessError".as_ptr(),
    c"IllegalAccessException".as_ptr(),
    c"IllegalArgumentException".as_ptr(),
    c"IllegalCallerException".as_ptr(),
    c"IllegalMonitorStateException".as_ptr(),
    c"IllegalStateException".as_ptr(),
    c"IllegalThreadStateException".as_ptr(),
    c"IncompatibleClassChangeError".as_ptr(),
    c"IndexOutOfBoundsException".as_ptr(),
    c"InheritableThreadLocal".as_ptr(),
    c"InstantiationError".as_ptr(),
    c"InstantiationException".as_ptr(),
    c"Integer".as_ptr(),
    c"InternalError".as_ptr(),
    c"InterruptedException".as_ptr(),
    c"Iterable".as_ptr(),
    c"LayerInstantiationException".as_ptr(),
    c"LinkageError".as_ptr(),
    c"Long".as_ptr(),
    c"MatchException".as_ptr(),
    c"Math".as_ptr(),
    c"Module".as_ptr(),
    c"ModuleLayer".as_ptr(),
    c"NegativeArraySizeException".as_ptr(),
    c"NoClassDefFoundError".as_ptr(),
    c"NoSuchFieldError".as_ptr(),
    c"NoSuchFieldException".as_ptr(),
    c"NoSuchMethodError".as_ptr(),
    c"NoSuchMethodException".as_ptr(),
    c"NullPointerException".as_ptr(),
    c"Number".as_ptr(),
    c"NumberFormatException".as_ptr(),
    c"Object".as_ptr(),
    c"OutOfMemoryError".as_ptr(),
    c"Override".as_ptr(),
    c"Package".as_ptr(),
    c"Process".as_ptr(),
    c"ProcessBuilder".as_ptr(),
    c"ProcessHandle".as_ptr(),
    c"Readable".as_ptr(),
    c"Record".as_ptr(),
    c"ReflectiveOperationException".as_ptr(),
    c"Runnable".as_ptr(),
    c"Runtime".as_ptr(),
    c"RuntimeException".as_ptr(),
    c"RuntimePermission".as_ptr(),
    c"SafeVarargs".as_ptr(),
    c"ScopedValue".as_ptr(),
    c"SecurityException".as_ptr(),
    c"SecurityManager".as_ptr(),
    c"Short".as_ptr(),
    c"StackOverflowError".as_ptr(),
    c"StackTraceElement".as_ptr(),
    c"StackWalker".as_ptr(),
    c"StrictMath".as_ptr(),
    c"String".as_ptr(),
    c"StringBuffer".as_ptr(),
    c"StringBuilder".as_ptr(),
    c"StringIndexOutOfBoundsException".as_ptr(),
    c"StringTemplate".as_ptr(),
    c"SuppressWarnings".as_ptr(),
    c"System".as_ptr(),
    c"Thread".as_ptr(),
    c"ThreadDeath".as_ptr(),
    c"ThreadGroup".as_ptr(),
    c"ThreadLocal".as_ptr(),
    c"Throwable".as_ptr(),
    c"TypeNotPresentException".as_ptr(),
    c"UnknownError".as_ptr(),
    c"UnsatisfiedLinkError".as_ptr(),
    c"UnsupportedClassVersionError".as_ptr(),
    c"UnsupportedOperationException".as_ptr(),
    c"VerifyError".as_ptr(),
    c"VirtualMachineError".as_ptr(),
    c"Void".as_ptr(),
    c"WrongThreadException".as_ptr(),
];

// kotlin.*, kotlin.annotation.*, kotlin.collections.*, kotlin.comparisons.*,
// kotlin.io.*, kotlin.ranges.*, kotlin.sequences.*, kotlin.text.* and
// kotlin.jvm.*: their types and the standard scope functions.
const JR_KOTLIN_DEFAULT: [*const c_char; 189] = [
    c"AbstractCollection".as_ptr(),
    c"AbstractIterator".as_ptr(),
    c"AbstractList".as_ptr(),
    c"AbstractMap".as_ptr(),
    c"AbstractMutableCollection".as_ptr(),
    c"AbstractMutableList".as_ptr(),
    c"AbstractMutableMap".as_ptr(),
    c"AbstractMutableSet".as_ptr(),
    c"AbstractSet".as_ptr(),
    c"Annotation".as_ptr(),
    c"AnnotationRetention".as_ptr(),
    c"AnnotationTarget".as_ptr(),
    c"Any".as_ptr(),
    c"Array".as_ptr(),
    c"ArrayDeque".as_ptr(),
    c"ArrayList".as_ptr(),
    c"Boolean".as_ptr(),
    c"BooleanArray".as_ptr(),
    c"BuilderInference".as_ptr(),
    c"Byte".as_ptr(),
    c"ByteArray".as_ptr(),
    c"Char".as_ptr(),
    c"CharArray".as_ptr(),
    c"CharCategory".as_ptr(),
    c"CharProgression".as_ptr(),
    c"CharRange".as_ptr(),
    c"CharSequence".as_ptr(),
    c"Charsets".as_ptr(),
    c"ClosedFloatingPointRange".as_ptr(),
    c"ClosedRange".as_ptr(),
    c"Collection".as_ptr(),
    c"Comparable".as_ptr(),
    c"Comparator".as_ptr(),
    c"ConcurrentModificationException".as_ptr(),
    c"DeepRecursiveFunction".as_ptr(),
    c"Deprecated".as_ptr(),
    c"DeprecationLevel".as_ptr(),
    c"Double".as_ptr(),
    c"DoubleArray".as_ptr(),
    c"DslMarker".as_ptr(),
    c"Enum".as_ptr(),
    c"Float".as_ptr(),
    c"FloatArray".as_ptr(),
    c"Function".as_ptr(),
    c"Grouping".as_ptr(),
    c"HashMap".as_ptr(),
    c"HashSet".as_ptr(),
    c"IndexedValue".as_ptr(),
    c"Int".as_ptr(),
    c"IntArray".as_ptr(),
    c"IntProgression".as_ptr(),
    c"IntRange".as_ptr(),
    c"Iterable".as_ptr(),
    c"Iterator".as_ptr(),
    c"JvmField".as_ptr(),
    c"JvmInline".as_ptr(),
    c"JvmName".as_ptr(),
    c"JvmOverloads".as_ptr(),
    c"JvmRecord".as_ptr(),
    c"JvmStatic".as_ptr(),
    c"JvmSuppressWildcards".as_ptr(),
    c"JvmSynthetic".as_ptr(),
    c"JvmWildcard".as_ptr(),
    c"KotlinVersion".as_ptr(),
    c"Lazy".as_ptr(),
    c"LazyThreadSafetyMode".as_ptr(),
    c"LinkedHashMap".as_ptr(),
    c"LinkedHashSet".as_ptr(),
    c"List".as_ptr(),
    c"ListIterator".as_ptr(),
    c"Long".as_ptr(),
    c"LongArray".as_ptr(),
    c"LongProgression".as_ptr(),
    c"LongRange".as_ptr(),
    c"Map".as_ptr(),
    c"MatchGroup".as_ptr(),
    c"MatchGroupCollection".as_ptr(),
    c"MatchResult".as_ptr(),
    c"Metadata".as_ptr(),
    c"MustBeDocumented".as_ptr(),
    c"MutableCollection".as_ptr(),
    c"MutableIterable".as_ptr(),
    c"MutableIterator".as_ptr(),
    c"MutableList".as_ptr(),
    c"MutableListIterator".as_ptr(),
    c"MutableMap".as_ptr(),
    c"MutableSet".as_ptr(),
    c"NoSuchElementException".as_ptr(),
    c"NotImplementedError".as_ptr(),
    c"Nothing".as_ptr(),
    c"Number".as_ptr(),
    c"OpenEndRange".as_ptr(),
    c"OptIn".as_ptr(),
    c"Pair".as_ptr(),
    c"ParameterName".as_ptr(),
    c"PublishedApi".as_ptr(),
    c"RandomAccess".as_ptr(),
    c"Regex".as_ptr(),
    c"RegexOption".as_ptr(),
    c"Repeatable".as_ptr(),
    c"ReplaceWith".as_ptr(),
    c"RequiresOptIn".as_ptr(),
    c"Result".as_ptr(),
    c"Retention".as_ptr(),
    c"Sequence".as_ptr(),
    c"SequenceScope".as_ptr(),
    c"Set".as_ptr(),
    c"Short".as_ptr(),
    c"ShortArray".as_ptr(),
    c"SinceKotlin".as_ptr(),
    c"String".as_ptr(),
    c"StringBuilder".as_ptr(),
    c"Strictfp".as_ptr(),
    c"Suppress".as_ptr(),
    c"Synchronized".as_ptr(),
    c"Target".as_ptr(),
    c"Throws".as_ptr(),
    c"Transient".as_ptr(),
    c"Triple".as_ptr(),
    c"UByte".as_ptr(),
    c"UByteArray".as_ptr(),
    c"UInt".as_ptr(),
    c"UIntArray".as_ptr(),
    c"UIntRange".as_ptr(),
    c"ULong".as_ptr(),
    c"ULongArray".as_ptr(),
    c"ULongRange".as_ptr(),
    c"UShort".as_ptr(),
    c"UShortArray".as_ptr(),
    c"UninitializedPropertyAccessException".as_ptr(),
    c"Unit".as_ptr(),
    c"Volatile".as_ptr(),
    c"TODO".as_ptr(),
    c"also".as_ptr(),
    c"apply".as_ptr(),
    c"arrayListOf".as_ptr(),
    c"arrayOf".as_ptr(),
    c"arrayOfNulls".as_ptr(),
    c"assert".as_ptr(),
    c"buildList".as_ptr(),
    c"buildMap".as_ptr(),
    c"buildSet".as_ptr(),
    c"buildString".as_ptr(),
    c"check".as_ptr(),
    c"checkNotNull".as_ptr(),
    c"compareBy".as_ptr(),
    c"compareValues".as_ptr(),
    c"emptyArray".as_ptr(),
    c"emptyList".as_ptr(),
    c"emptyMap".as_ptr(),
    c"emptySequence".as_ptr(),
    c"emptySet".as_ptr(),
    c"enumValueOf".as_ptr(),
    c"enumValues".as_ptr(),
    c"error".as_ptr(),
    c"generateSequence".as_ptr(),
    c"hashMapOf".as_ptr(),
    c"hashSetOf".as_ptr(),
    c"lazy".as_ptr(),
    c"lazyOf".as_ptr(),
    c"let".as_ptr(),
    c"linkedMapOf".as_ptr(),
    c"linkedSetOf".as_ptr(),
    c"listOf".as_ptr(),
    c"listOfNotNull".as_ptr(),
    c"mapOf".as_ptr(),
    c"maxOf".as_ptr(),
    c"minOf".as_ptr(),
    c"mutableListOf".as_ptr(),
    c"mutableMapOf".as_ptr(),
    c"mutableSetOf".as_ptr(),
    c"print".as_ptr(),
    c"println".as_ptr(),
    c"readLine".as_ptr(),
    c"readln".as_ptr(),
    c"repeat".as_ptr(),
    c"require".as_ptr(),
    c"requireNotNull".as_ptr(),
    c"run".as_ptr(),
    c"runCatching".as_ptr(),
    c"sequence".as_ptr(),
    c"sequenceOf".as_ptr(),
    c"setOf".as_ptr(),
    c"synchronized".as_ptr(),
    c"takeIf".as_ptr(),
    c"takeUnless".as_ptr(),
    c"to".as_ptr(),
    c"use".as_ptr(),
    c"with".as_ptr(),
];

const JR_JAVA_PRIMITIVES: [*const c_char; 9] = [
    c"boolean".as_ptr(),
    c"byte".as_ptr(),
    c"char".as_ptr(),
    c"short".as_ptr(),
    c"int".as_ptr(),
    c"long".as_ptr(),
    c"float".as_ptr(),
    c"double".as_ptr(),
    c"void".as_ptr(),
];

// The members a type has without declaring them. These lists are defined by
// the two languages, not collected from a library: the methods of
// java.lang.Object with their parameter lists ('|'-joined, as a declared
// signature is normalized), the three members of kotlin.Any, and what an enum
// has in either language (the methods of java.lang.Enum with the two static
// ones the language declares; the Kotlin counterparts). Written from knowledge
// of the two language specifications; not checked online.
#[derive(Clone, Copy)]
struct Given {
    name: *const c_char,
    sig: *const c_char,
}
impl Default for Given {
    fn default() -> Self {
        unsafe { zeroed() }
    }
}

const JR_OBJECT_METHODS: [Given; 11] = [
    Given {
        name: c"equals".as_ptr(),
        sig: c"Object".as_ptr(),
    },
    Given {
        name: c"hashCode".as_ptr(),
        sig: c"".as_ptr(),
    },
    Given {
        name: c"toString".as_ptr(),
        sig: c"".as_ptr(),
    },
    Given {
        name: c"clone".as_ptr(),
        sig: c"".as_ptr(),
    },
    Given {
        name: c"finalize".as_ptr(),
        sig: c"".as_ptr(),
    },
    Given {
        name: c"getClass".as_ptr(),
        sig: c"".as_ptr(),
    },
    Given {
        name: c"notify".as_ptr(),
        sig: c"".as_ptr(),
    },
    Given {
        name: c"notifyAll".as_ptr(),
        sig: c"".as_ptr(),
    },
    Given {
        name: c"wait".as_ptr(),
        sig: c"".as_ptr(),
    },
    Given {
        name: c"wait".as_ptr(),
        sig: c"long".as_ptr(),
    },
    Given {
        name: c"wait".as_ptr(),
        sig: c"long|int".as_ptr(),
    },
];

const JR_ANY_MEMBERS: [*const c_char; 3] = [
    c"equals".as_ptr(),
    c"hashCode".as_ptr(),
    c"toString".as_ptr(),
];

const JR_JAVA_ENUM_MEMBERS: [*const c_char; 7] = [
    c"values".as_ptr(),
    c"valueOf".as_ptr(),
    c"name".as_ptr(),
    c"ordinal".as_ptr(),
    c"compareTo".as_ptr(),
    c"getDeclaringClass".as_ptr(),
    c"describeConstable".as_ptr(),
];

const JR_KOTLIN_ENUM_MEMBERS: [*const c_char; 6] = [
    c"values".as_ptr(),
    c"valueOf".as_ptr(),
    c"entries".as_ptr(),
    c"name".as_ptr(),
    c"ordinal".as_ptr(),
    c"compareTo".as_ptr(),
];

fn named(table: &[*const c_char], name: *const c_char) -> bool {
    table.iter().any(|&word| unsafe { strcmp(word, name) == 0 })
}

// ── Resolution context ────────────────────────────────────────────
// The scan nests types JV_MAX_DEPTH (128) deep at most; a chain is never
// longer.
const JR_CHAIN_MAX: i32 = 136;

const JR_OK: i32 = 0;
const JR_UNRES: i32 = 1;
const JR_LOCAL: i32 = 2;

#[derive(Clone, Copy)]
struct Resolution {
    st: i32,
    node: *const GraphNode,
    exact: bool,
    reason: i32,
}
impl Default for Resolution {
    fn default() -> Self {
        unsafe { zeroed() }
    }
}

#[derive(Clone, Copy)]
struct ResolveContext {
    ix: *const Index,
    g: *const Graph,
    file: i32,
    f: *const File,
    prod: bool,
    // test-only declarations are invisible
    sample: bool,
    // a `@sample` subject: it names a function
    inherit: bool,
    // false while supertypes are being bound
    chain: [i32; 136],
    // enclosing types, innermost first
    chain_decl: [*const TypeDecl; 136],
    // ... and their declarations in this file
    nchain: i32,
    member: *const Member,
    // the documented member, when the definition is one
    type_index: *const TypeDecl,
    // the documented type, when the definition is one
}
impl Default for ResolveContext {
    fn default() -> Self {
        unsafe { zeroed() }
    }
}

unsafe fn edge(n: *const GraphNode, exact: bool) -> Resolution {
    return Resolution {
        st: JR_OK,
        node: n,
        exact: exact,
        ..Resolution::default()
    };
}

unsafe fn unres(reason: i32) -> Resolution {
    return Resolution {
        st: JR_UNRES,
        reason: reason,
        ..Resolution::default()
    };
}

unsafe fn visible(c: &ResolveContext, ent: i32) -> bool {
    return (!(c.prod)) || ((*((*c.ix).ents).offset(ent as isize)).any_prod);
}

// The enclosing types of type `t` of the context's file, `t` first.
unsafe fn chain_from(c: *mut ResolveContext, mut t: i32) {
    while (t != JR_NONE) && ((*c).nchain < JR_CHAIN_MAX) {
        let ty: *const TypeDecl =
            (addr_of_mut!(*((*(*c).f).types).offset(t as isize)))
                as *const TypeDecl;
        if (*ty).entity >= 0 {
            ((*c).chain)[((*c).nchain) as usize] = (*ty).entity;
            ((*c).chain_decl)[((*c).nchain) as usize] = ty;
            (*c).nchain += 1;
        }
        t = (*ty).outer;
    }
}

// ── Type candidates of one scope level ────────────────────────────
#[derive(Clone, Copy)]
struct Candidates {
    kind: c_char,
    // 0 none; T a type, X a name outside the repository, G a name the
    // repository declares where no scope shows it, N a name in a
    // package of the repository that the indexer does not read
    ent: i32,
    // T: the type
    count: i32,
    // distinct candidates; 2 stands for "several"
    invisible: bool,
    // a candidate product code may not bind was passed over
    exact: bool,
    // named by an explicit import
}
impl Default for Candidates {
    fn default() -> Self {
        unsafe { zeroed() }
    }
}

unsafe fn cand(out: *mut Candidates, kind: c_char, ent: i32) {
    if (((*out).count > 0) && ((((*out).kind) as i32) == (kind as i32)))
        && ((*out).ent == ent)
    {
        return;
    }
    if (*out).count == 0 {
        (*out).kind = kind;
        (*out).ent = ent;
    }
    (*out).count += i32::from((*out).count < PAIR_LEN);
}

// A type, unless the source may not name it: product code does not see what
// only test code declares, and Java does not see a Kotlin type alias.
unsafe fn cand_type(c: &ResolveContext, out: *mut Candidates, ent: i32) {
    if (ent < 0)
        || ((!((*c.f).kotlin))
            && ((((*((*c.ix).ents).offset(ent as isize)).kind) as i32)
                == b't' as i32))
    {
        return;
    }
    if !(visible(c, ent)) {
        (*out).invisible = true;
        return;
    }
    cand(out, b'T' as c_char, ent);
}

// ── Types: lookup tables ──────────────────────────────────────────
unsafe fn pkg_type(
    ix: *const Index,
    package: *const c_char,
    name: *const c_char,
) -> i32 {
    let mut key: [c_char; KEY_BYTES] = zeroed();
    if !(pair_key(
        package,
        name,
        addr_of_mut!(key).cast::<c_char>(),
        size_of::<[c_char; KEY_BYTES]>(),
    )) {
        return JR_NONE;
    }
    let v: isize = (cbm_ht_get(
        ((*ix).pkg_types) as *const HashTable,
        (addr_of_mut!(key).cast::<c_char>()) as *const c_char,
    )) as isize;
    return if v > 0 {
        (v - (SKIP_ONE as isize)) as i32
    } else {
        JR_NONE
    };
}

// The type `name` nested directly in entity `ent`.
unsafe fn nested(ix: *const Index, ent: i32, name: *const c_char) -> i32 {
    let mut fqn: [c_char; KEY_BYTES] = zeroed();
    if !(write_fqn(
        (*((*ix).ents).offset(ent as isize)).fqn,
        name,
        addr_of_mut!(fqn).cast::<c_char>(),
        size_of::<[c_char; KEY_BYTES]>(),
    )) {
        return JR_NONE;
    }
    return find_entity(
        ix,
        (addr_of_mut!(fqn).cast::<c_char>()) as *const c_char,
    );
}

// The member type `name` of `ent`: its own, else the one of the nearest
// supertype that declares one.
unsafe fn member_type(c: &ResolveContext, ent: i32, name: *const c_char) -> i32 {
    let own: i32 = nested(c.ix, ent, name);
    if (own != JR_NONE) || (!(c.inherit)) {
        return own;
    }
    let e: *const Entity =
        (addr_of_mut!(*((*c.ix).ents).offset(ent as isize))) as *const Entity;
    for i in 0..(*e).nclosure {
        let n: i32 = nested(c.ix, *((*e).closure).offset(i as isize), name);
        if n != JR_NONE {
            return n;
        }
    }
    return JR_NONE;
}

unsafe fn open(ix: *const Index, ent: i32) -> bool {
    let e: *const Entity =
        (addr_of_mut!(*((*ix).ents).offset(ent as isize))) as *const Entity;
    if (*e).open {
        return true;
    }
    for i in 0..(*e).nclosure {
        if (*((*ix).ents).offset((*((*e).closure).offset(i as isize)) as isize))
            .open
        {
            return true;
        }
    }
    return false;
}

// True when `sup` is `ent` or one of its supertypes.
unsafe fn is_or_extends(ix: *const Index, ent: i32, sup: i32) -> bool {
    if ent == sup {
        return true;
    }
    let e: *const Entity =
        (addr_of_mut!(*((*ix).ents).offset(ent as isize))) as *const Entity;
    for i in 0..(*e).nclosure {
        if *((*e).closure).offset(i as isize) == sup {
            return true;
        }
    }
    return false;
}

// ── Node binding ──────────────────────────────────────────────────
// Representative ordering of the declarations of one entity, without node
// ids or lines: the source file itself, the longest common directory prefix
// with it, then path and declaration order.
unsafe fn common_dirs(mut a: *const c_char, mut b: *const c_char) -> i32 {
    let mut n: i32 = 0;
    loop {
        let sa: *const c_char = (strchr(a, b'/' as i32)) as *const c_char;
        let sb: *const c_char = (strchr(b, b'/' as i32)) as *const c_char;
        if (sa.is_null()) || (sb.is_null()) {
            return n;
        }
        let la: usize = (sa.offset_from(a)) as usize;
        if (la != ((sb.offset_from(b)) as usize))
            || (memcmp(a as *const c_void, b as *const c_void, la) != 0)
        {
            return n;
        }
        n += 1;
        a = sa.offset(SKIP_ONE as isize);
        b = sb.offset(SKIP_ONE as isize);
    }
}

unsafe fn decl_better(
    ix: *const Index,
    fa: i32,
    oa: i32,
    fb: i32,
    ob: i32,
    src: *const c_char,
) -> bool {
    let ra: *const c_char = (*((*ix).files).offset(fa as isize)).rel_path;
    let rb: *const c_char = (*((*ix).files).offset(fb as isize)).rel_path;
    let sa: bool = strcmp(ra, src) == 0;
    let sb: bool = strcmp(rb, src) == 0;
    if sa != sb {
        return sa;
    }
    let pa: i32 = common_dirs(ra, src);
    let pb: i32 = common_dirs(rb, src);
    if pa != pb {
        return pa > pb;
    }
    let cmp: i32 = strcmp(ra, rb);
    if cmp != 0 {
        return cmp < 0;
    }
    return oa < ob;
}

unsafe fn label_callable(l: *const c_char) -> bool {
    return (!(l).is_null())
        && ((strcmp(l, c"Method".as_ptr()) == 0)
            || (strcmp(l, c"Function".as_ptr()) == 0));
}

unsafe fn label_value(l: *const c_char) -> bool {
    return (!(l).is_null())
        && ((strcmp(l, c"Field".as_ptr()) == 0)
            || (strcmp(l, c"Variable".as_ptr()) == 0));
}

// The node at "<module>.<tail>" when it is this file's own: one node per
// qualified name, and across files the graph keeps one file's.
unsafe fn node_at(
    c: &ResolveContext,
    f: *const File,
    tail: *const c_char,
) -> *const GraphNode {
    let mut qn: [c_char; KEY_BYTES] = zeroed();
    if !write_key(
        qn.as_mut_ptr(),
        qn.len(),
        &[name_bytes((*f).module_qn), b".", name_bytes(tail)],
    ) {
        return ptr::null();
    }
    let n: *const GraphNode = cbm_gbuf_find_by_qn(
        c.g,
        (addr_of_mut!(qn).cast::<c_char>()) as *const c_char,
    );
    return if ((!(n).is_null()) && (!((*n).file_path).is_null()))
        && (strcmp(((*n).file_path) as *const c_char, (*f).rel_path) == 0)
    {
        n
    } else {
        ptr::null()
    };
}

// The node of a type declaration; NULL when it has none of its own.
unsafe fn type_node(c: &ResolveContext, d: Decl) -> *const GraphNode {
    let f: *const File =
        (addr_of_mut!(*((*c.ix).files).offset((d.file) as isize)))
            as *const File;
    let t: *const TypeDecl =
        (addr_of_mut!(*((*f).types).offset((d.type_index) as isize)))
            as *const TypeDecl;
    if (((*t).slot).is_null()) || (!((*t).own)) {
        return ptr::null();
    }
    let n: *const GraphNode = node_at(
        c,
        f,
        if strcmp((*t).slot, c"=".as_ptr()) == 0 {
            (*t).path
        } else {
            (*t).slot
        },
    );
    return if (!(n).is_null())
        && (cbm_label_is_type_like(((*n).label) as *const c_char))
    {
        n
    } else {
        ptr::null()
    };
}

// The node of a member: its own, or the one the overloads of its name share.
// NULL when it has none, or when the slot holds a node of the other kind.
unsafe fn member_node(
    c: &ResolveContext,
    file: i32,
    m: *const Member,
) -> *const GraphNode {
    let f: *const File =
        (addr_of_mut!(*((*c.ix).files).offset(file as isize))) as *const File;
    if (*m).shares == JR_NONE {
        return ptr::null();
    }
    let o: *const Member =
        (addr_of_mut!(*((*f).members).offset(((*m).shares) as isize)))
            as *const Member;
    let mut tail: [c_char; KEY_BYTES] = zeroed();
    let mut slot: *const c_char = (*o).slot;
    if strcmp(slot, c"=".as_ptr()) == 0 {
        let owner = if (*o).owner == JR_NONE {
            ptr::null()
        } else {
            (*(*f).types.add((*o).owner as usize)).path
        };
        if !write_fqn(owner, (*o).name, tail.as_mut_ptr(), tail.len()) {
            return ptr::null();
        }
        slot = tail.as_ptr();
    }
    let n: *const GraphNode = node_at(c, f, slot);
    if n.is_null() {
        return ptr::null();
    }
    return if (if (((*o).kind) as i32) == b'c' as i32 {
        i32::from(label_callable(((*n).label) as *const c_char))
    } else {
        i32::from(label_value(((*n).label) as *const c_char))
    }) != 0
    {
        n
    } else {
        ptr::null()
    };
}

// The entity's representative node: never a test declaration for product
// code. An entity none of whose eligible declarations has a node is a graph
// gap.
unsafe fn type_result(c: &ResolveContext, ent: i32, exact: bool) -> Resolution {
    let ix: *const Index = c.ix;
    let e: *const Entity =
        (addr_of_mut!(*((*ix).ents).offset(ent as isize))) as *const Entity;
    let mut best: *const GraphNode = ptr::null();
    let mut bf: i32 = 0;
    let mut bo: i32 = 0;
    for d in 0..(*e).ndecls {
        let file: i32 = (*((*e).decls).offset(d as isize)).file;
        if (c.prod) && ((*((*ix).files).offset(file as isize)).is_test) {
            continue;
        }
        let n: *const GraphNode = type_node(c, *((*e).decls).offset(d as isize));
        if (!(n).is_null())
            && ((best.is_null())
                || (decl_better(
                    ix,
                    file,
                    (*((*e).decls).offset(d as isize)).type_index,
                    bf,
                    bo,
                    (*c.f).rel_path,
                )))
        {
            best = n;
            bf = file;
            bo = (*((*e).decls).offset(d as isize)).type_index;
        }
    }
    return if !(best).is_null() {
        edge(best, exact)
    } else {
        unres(CBM_DOCLINK_REASON_GRAPH_GAP)
    };
}

// ── Reference syntax ──────────────────────────────────────────────
#[derive(Clone, Copy)]
struct Reference {
    text: [c_char; MAX_REFERENCE_BYTES + 8],
    // the segments point in here
    segs: [*mut c_char; 32],
    nsegs: i32,
    member: *mut c_char,
    // Java: the name after '#' (or of a bare `name(..)`); NULL when none
    has_params: bool,
    params: [[c_char; 160]; 32],
    nparams: i32,
}
impl Default for Reference {
    fn default() -> Self {
        unsafe { zeroed() }
    }
}

// Reference segments point into the caller's scratch buffer. The caller
// leaves it in place until resolution finishes.
fn java_identifier(segment: &[u8]) -> bool {
    let Some((&first, rest)) = segment.split_first() else {
        return false;
    };
    (first.is_ascii_alphabetic()
        || matches!(first, b'_' | b'$')
        || first >= 0x80)
        && rest.iter().all(|&byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'_' | b'$')
                || byte >= 0x80
        })
}

fn kotlin_identifier(segment: &[u8]) -> bool {
    !segment.is_empty()
        && !segment
            .iter()
            .any(|byte| b"\t()#<>[]{}/\\:;,\"'".contains(byte))
}

unsafe fn write_segments(
    r: *mut Reference,
    path: &[u8],
    valid: fn(&[u8]) -> bool,
) -> bool {
    if path.is_empty() {
        return true;
    }
    let mut start = 0;
    let text = addr_of_mut!((*r).text).cast::<c_char>();
    for segment in path.split(|&byte| byte == b'.') {
        let index = (*r).nsegs as usize;
        if index == (*r).segs.len() || !valid(segment) {
            return false;
        }
        let dest = text.add(start);
        ptr::copy_nonoverlapping(segment.as_ptr(), dest.cast(), segment.len());
        *dest.add(segment.len()) = 0;
        (*r).segs[index] = dest;
        (*r).nsegs += 1;
        start += segment.len() + 1;
    }
    true
}

// The extractor and resolver share normalization so signatures compare
// consistently for generics, varargs, arrays and parameter names.
unsafe fn parse_params(params: &[u8], r: *mut Reference) -> bool {
    (*r).has_params = true;
    if params.iter().all(|&byte| isspace(i32::from(byte)) != 0) {
        return true;
    }
    let mut start = 0;
    let mut depth = 0;
    for (end, byte) in params.iter().copied().chain([b',']).enumerate() {
        match byte {
            b'<' | b'(' | b'[' => depth += 1,
            b'>' | b')' | b']' => depth -= 1,
            b',' if depth <= 0 => {
                let Some(normalized) =
                    (&mut (*r).params).get_mut((*r).nparams as usize)
                else {
                    return false;
                };
                cbm_doclink_java_norm_type(
                    params[start..end].as_ptr().cast(),
                    end - start,
                    normalized.as_mut_ptr(),
                    normalized.len(),
                );
                if normalized[0] == b'?' as c_char && normalized[1] == 0 {
                    return false;
                }
                (*r).nparams += 1;
                start = end + 1;
            }
            _ => {}
        }
    }
    true
}

// `[module/]Type[#member[(params)]]`, `#member[(params)]`, `member(params)`.
unsafe fn parse_java(raw: *const c_char, r: *mut Reference) -> bool {
    let raw = CStr::from_ptr(raw).to_bytes();
    if raw.is_empty() || raw.len() > MAX_REFERENCE_BYTES {
        return false;
    }
    let hash = raw.iter().position(|&byte| byte == b'#');
    let paren = raw.iter().position(|&byte| byte == b'(');
    let start = raw
        .iter()
        .position(|&byte| byte == b'/')
        .filter(|&slash| {
            hash.is_none_or(|hash| slash < hash)
                && paren.is_none_or(|paren| slash < paren)
        })
        .map_or(0, |slash| slash + 1);
    let (path, member) = match (hash, paren) {
        (Some(hash), _) => (&raw[start..hash], Some(&raw[hash + 1..])),
        (None, Some(_)) => (&raw[..0], Some(&raw[start..])),
        (None, None) => (&raw[start..], None),
    };
    let path = path.split(|&byte| byte == b'<').next().unwrap_or_default();
    let mut path_end = path.len();
    while path[..path_end].ends_with(b"[]") {
        path_end -= 2;
    }
    if !write_segments(r, &path[..path_end], java_identifier) {
        return false;
    }

    if let Some(member) = member {
        let name = match member.iter().position(|&byte| byte == b'(') {
            Some(open) => {
                let Some(params) = member[open + 1..].strip_suffix(b")") else {
                    return false;
                };
                if !parse_params(params, r) {
                    return false;
                }
                &member[..open]
            }
            None => member,
        };
        if !java_identifier(name) {
            return false;
        }
        let offset = if path_end == 0 { 0 } else { path_end + 1 };
        let dest = addr_of_mut!((*r).text).cast::<c_char>().add(offset);
        ptr::copy_nonoverlapping(name.as_ptr(), dest.cast(), name.len());
        *dest.add(name.len()) = 0;
        (*r).member = dest;
    }
    (*r).nsegs > 0 || !(*r).member.is_null()
}

// A KDoc name is dotted. Backticks can quote a segment, with an embedded
// dot rejected because it cannot be a JVM scope path segment here.
unsafe fn parse_kotlin(raw: *const c_char, r: *mut Reference) -> bool {
    let raw = CStr::from_ptr(raw).to_bytes();
    if raw.is_empty() || raw.len() > MAX_REFERENCE_BYTES {
        return false;
    }
    let mut unquoted = [0; MAX_REFERENCE_BYTES];
    let mut len = 0;
    let mut quoted = false;
    for &byte in raw {
        match byte {
            b'`' => quoted = !quoted,
            b'.' if quoted => return false,
            _ => {
                unquoted[len] = byte;
                len += 1;
            }
        }
    }
    if quoted {
        return false;
    }
    let mut path = &unquoted[..len];
    if len > 2 {
        path = path.strip_suffix(b"()").unwrap_or(path);
    }
    !path.is_empty() && write_segments(r, path, kotlin_identifier)
}

// ── Members of a type ─────────────────────────────────────────────
const JR_SEL_VALUES: i32 = 1;
const JR_SEL_CALLABLES: i32 = 2;
const JR_SEL_CTORS: i32 = 4;
const JR_SEL_SIG: i32 = 8;

#[derive(Clone, Copy)]
struct Found {
    count: i32,
    // the selected members
    several: bool,
    // ... are more than one thing: different signatures,
    // or members of different owners
    component: bool,
    // one is a record component or constructor property
    invisible: bool,
    // members left out because product code may not bind
    // what only test code declares
    uncompared: bool,
    // callables left out because a Javadoc signature cannot
    // be compared with a Kotlin declaration's
    sig: *const c_char,
    // the first one's
    node: *const GraphNode,
    // the representative node of those that have one
    node_file: i32,
    node_order: i32,
}
impl Default for Found {
    fn default() -> Self {
        unsafe { zeroed() }
    }
}

// Index of the first member of type `type` in `f` named `name` (the members
// are sorted by owner, name, order); f->nmembers when there is none before
// the end.
unsafe fn member_lower(
    f: *const File,
    type_index: i32,
    name: *const c_char,
) -> i32 {
    let mut lo: i32 = 0;
    let mut hi: i32 = (*f).nmembers;
    while lo < hi {
        let mid: i32 = lo + ((hi - lo) / PAIR_LEN);
        let m: *const Member =
            (addr_of_mut!(*((*f).members).offset(mid as isize)))
                as *const Member;
        let below: bool = (if (*m).owner != type_index {
            i32::from((*m).owner < type_index)
        } else {
            i32::from(strcmp((*m).name, name) < 0)
        }) != 0;
        if below {
            lo = mid + SKIP_ONE;
        } else {
            hi = mid;
        }
    }
    return lo;
}

// The suffix of a normalized type that says how many array levels it has.
unsafe fn array_suffix(s: *const c_char, n: usize) -> usize {
    let mut e: usize = n;
    while ((e >= (PAIR_LEN as usize))
        && (((*s.offset((e - (PAIR_LEN as usize)) as isize)) as i32)
            == b'[' as i32))
        && (((*s.offset((e - (SKIP_ONE as usize)) as isize)) as i32)
            == b']' as i32)
    {
        e -= PAIR_LEN as usize;
    }
    return e;
}

// True when `name[0, n)` is a type variable in scope of a member of type
// `owner` whose own type parameters are `mtparams`: the member's, its type's
// or an enclosing type's. *bound / *blen: its first bound.
unsafe fn is_tvar(
    f: *const File,
    mtparams: *const c_char,
    owner: i32,
    name: *const c_char,
    n: usize,
    bound: *mut *const c_char,
    blen: *mut usize,
) -> bool {
    if n == 0 {
        return false;
    }
    if listed_n(mtparams, name, n, bound, blen) {
        return true;
    }
    {
        let mut t: i32 = owner;
        while t != JR_NONE {
            if listed_n(
                (*((*f).types).offset(t as isize)).tparams,
                name,
                n,
                bound,
                blen,
            ) {
                return true;
            }

            t = (*((*f).types).offset(t as isize)).outer;
        }
    }
    return false;
}

// One written parameter type against one declared type: the same name, or
// the erasure of a declared type variable (its first bound, Object when it
// has none) with the same array rank. Nothing else: javadoc matches a
// signature against the declared types and against their erasures.
unsafe fn param_match(
    f: *const File,
    mtparams: *const c_char,
    owner: i32,
    written: *const c_char,
    declared: *const c_char,
    dn: usize,
) -> bool {
    let object: [c_char; 7] = [79, 98, 106, 101, 99, 116, 0];
    let wn: usize = strlen(written);
    if (wn == (SKIP_ONE as usize))
        && (((*written.offset(0)) as i32) == b'?' as i32)
    {
        return false;
        // a written type that is no type
    }
    if (wn == dn)
        && (memcmp(written as *const c_void, declared as *const c_void, wn) == 0)
    {
        return true;
    }
    let wb: usize = array_suffix(written, wn);
    let db: usize = array_suffix(declared, dn);
    let mut bound: *const c_char = ptr::null();
    let mut blen: usize = 0;
    if ((wn - wb) != (dn - db))
        || (!(is_tvar(
            f,
            mtparams,
            owner,
            declared,
            db,
            addr_of_mut!(bound),
            addr_of_mut!(blen),
        )))
    {
        return false;
    }
    if blen == 0 {
        bound = addr_of!(object).cast::<c_char>();
        blen = size_of::<[c_char; 7]>() - (SKIP_ONE as usize);
    }
    return (wb == blen)
        && (memcmp(written as *const c_void, bound as *const c_void, blen)
            == 0);
}

// True when the written parameter list is the canonical one of the record
// `owner` declares: its components' types, in declaration order. The written
// list is at most JR_MAX_PARAMS long, which bounds the passes over the
// record's members.
unsafe fn canonical_match(
    f: *const File,
    owner: i32,
    r: *const Reference,
) -> bool {
    let first: i32 = member_lower(f, owner, c"".as_ptr());
    let mut components: i32 = 0;
    {
        let mut i: i32 = first;
        while (i < (*f).nmembers)
            && ((*((*f).members).offset(i as isize)).owner == owner)
        {
            components +=
                i32::from((*((*f).members).offset(i as isize)).component);

            i += 1;
        }
    }
    if components != (*r).nparams {
        return false;
    }
    let mut last_order: i32 = JR_NONE;
    for k in 0..(*r).nparams {
        let mut next: *const Member = ptr::null();
        // the component with the next larger order
        {
            let mut i: i32 = first;
            while (i < (*f).nmembers)
                && ((*((*f).members).offset(i as isize)).owner == owner)
            {
                let x: *const Member =
                    (addr_of_mut!(*((*f).members).offset(i as isize)))
                        as *const Member;
                if (((*x).component) && ((*x).order > last_order))
                    && ((next.is_null()) || ((*x).order < (*next).order))
                {
                    next = x;
                }

                i += 1;
            }
        }
        if (next.is_null())
            || (!(param_match(
                f,
                c"".as_ptr(),
                owner,
                addr_of!(((*r).params)[k as usize]).cast::<c_char>(),
                (*next).sig,
                strlen((*next).sig),
            )))
        {
            return false;
        }
        last_order = (*next).order;
    }
    return true;
}

// The written parameter types against a declared signature.
unsafe fn sig_match(
    f: *const File,
    m: *const Member,
    r: *const Reference,
) -> bool {
    if ((*m).ctor) && (strcmp((*m).sig, c"*".as_ptr()) == 0) {
        return canonical_match(f, (*m).owner, r);
    }
    let mut p: *const c_char = (*m).sig;
    for i in 0..(*r).nparams {
        if (p.is_null()) || ((*(p)) == 0) {
            return false;
            // fewer declared than written
        }
        let bar: *const c_char = (strchr(p, b'|' as i32)) as *const c_char;
        let dn: usize = if !(bar).is_null() {
            (bar.offset_from(p)) as usize
        } else {
            strlen(p)
        };
        if !(param_match(
            f,
            (*m).tparams,
            (*m).owner,
            addr_of!(((*r).params)[i as usize]).cast::<c_char>(),
            p,
            dn,
        )) {
            return false;
        }
        p = if !(bar).is_null() {
            bar.offset(SKIP_ONE as isize)
        } else {
            ptr::null()
        };
    }
    return (if (*r).nparams == 0 {
        i32::from(((*((*m).sig).offset(0)) as i32) == b'\0' as i32)
    } else {
        i32::from(p == (ptr::null()))
    }) != 0;
}

// Count member `m` of `file` among the selected ones.
unsafe fn found_take(
    c: &ResolveContext,
    out: *mut Found,
    file: i32,
    m: *const Member,
) {
    if (*out).count == 0 {
        (*out).sig = (*m).sig;
    } else if strcmp((*out).sig, (*m).sig) != 0 {
        (*out).several = true;
    }
    (*out).count += 1;
    (*out).component = ((*out).component) || ((*m).component);
    let n: *const GraphNode = member_node(c, file, m);
    if (!(n).is_null())
        && ((((*out).node).is_null())
            || (decl_better(
                c.ix,
                file,
                (*m).order,
                (*out).node_file,
                (*out).node_order,
                (*c.f).rel_path,
            )))
    {
        (*out).node = n;
        (*out).node_file = file;
        (*out).node_order = (*m).order;
    }
}

// Add to `out` the members named `name` that `sel` selects among those `type`
// of `file` declares (JR_NONE: the file's top-level declarations).
unsafe fn members_in(
    c: &ResolveContext,
    file: i32,
    type_index: i32,
    name: *const c_char,
    sel: i32,
    r: *const Reference,
    out: *mut Found,
) {
    let f: *const File =
        (addr_of_mut!(*((*c.ix).files).offset(file as isize))) as *const File;
    for i in member_lower(f, type_index, name)..(*f).nmembers {
        let m: *const Member =
            (addr_of_mut!(*((*f).members).offset(i as isize))) as *const Member;
        if ((*m).owner != type_index) || (strcmp((*m).name, name) != 0) {
            break;
        }
        let take: bool = (if (((*m).kind) as i32) == b'v' as i32 {
            i32::from((sel & JR_SEL_VALUES) != 0)
        } else {
            i32::from(
                (sel & (if (*m).ctor {
                    JR_SEL_CTORS
                } else {
                    JR_SEL_CALLABLES
                })) != 0,
            )
        }) != 0;
        if !(take) {
            continue;
        }
        if (c.prod) && ((*f).is_test) {
            (*out).invisible = true;
            continue;
        }
        if ((sel & JR_SEL_SIG) != 0) && ((((*m).kind) as i32) == b'c' as i32) {
            if (*f).kotlin {
                (*out).uncompared = true;
                // Kotlin writes its types another way
                continue;
            }
            if !(sig_match(f, m, r)) {
                continue;
            }
        }
        found_take(c, out, file, m);
    }
}

// The members of entity `ent` named `name` that `sel` selects, across its
// declarations.
unsafe fn members(
    c: &ResolveContext,
    ent: i32,
    name: *const c_char,
    sel: i32,
    r: *const Reference,
) -> Found {
    let e: *const Entity =
        (addr_of_mut!(*((*c.ix).ents).offset(ent as isize))) as *const Entity;
    let mut out: Found = Found::default();
    for d in 0..(*e).ndecls {
        members_in(
            c,
            (*((*e).decls).offset(d as isize)).file,
            (*((*e).decls).offset(d as isize)).type_index,
            name,
            sel,
            r,
            addr_of_mut!(out),
        );
    }
    return out;
}

// The top-level declarations named `name` of package `pkg`, across its
// files.
unsafe fn top_members(
    c: &ResolveContext,
    pkg: *const c_char,
    name: *const c_char,
    sel: i32,
) -> Found {
    let mut out: Found = Found::default();
    let mut key: [c_char; KEY_BYTES] = zeroed();
    if !(pair_key(
        pkg,
        name,
        addr_of_mut!(key).cast::<c_char>(),
        size_of::<[c_char; KEY_BYTES]>(),
    )) {
        return out;
    }
    let l: *const IntList = (cbm_ht_get(
        ((*c.ix).pkg_members) as *const HashTable,
        (addr_of_mut!(key).cast::<c_char>()) as *const c_char,
    )) as *const IntList;
    {
        let mut i: i32 = 0;
        while (!(l).is_null()) && (i < (*l).count) {
            members_in(
                c,
                *((*l).items).offset(i as isize),
                JR_NONE,
                name,
                sel,
                ptr::null(),
                addr_of_mut!(out),
            );

            i += 1;
        }
    }
    return out;
}

// Join the members found under one name in two places. Members of different
// owners are different things, whatever their signatures.
unsafe fn found_add(
    c: &ResolveContext,
    into: *mut Found,
    add: *const Found,
    same_owner: bool,
) {
    (*into).invisible = ((*into).invisible) || ((*add).invisible);
    (*into).uncompared = ((*into).uncompared) || ((*add).uncompared);
    if (*add).count == 0 {
        return;
    }
    if (*into).count == 0 {
        let invisible: bool = (*into).invisible;
        let uncompared: bool = (*into).uncompared;
        *(into) = *(add);
        (*into).invisible = invisible;
        (*into).uncompared = uncompared;
        return;
    }
    (*into).several = ((((*into).several) || ((*add).several))
        || (!(same_owner)))
        || (strcmp((*into).sig, (*add).sig) != 0);
    (*into).count += (*add).count;
    (*into).component = ((*into).component) || ((*add).component);
    if (!((*add).node).is_null())
        && ((((*into).node).is_null())
            || (decl_better(
                c.ix,
                (*add).node_file,
                (*add).node_order,
                (*into).node_file,
                (*into).node_order,
                (*c.f).rel_path,
            )))
    {
        (*into).node = (*add).node;
        (*into).node_file = (*add).node_file;
        (*into).node_order = (*add).node_order;
    }
}

// The selected members as a result: their node, or a graph gap when the
// source declares them and the graph has no node for them.
unsafe fn found_result(found: *const Found, exact: bool) -> Resolution {
    return if !((*found).node).is_null() {
        edge((*found).node, exact)
    } else {
        unres(CBM_DOCLINK_REASON_GRAPH_GAP)
    };
}

// ── Type paths ────────────────────────────────────────────────────
type TypePathKind = i32;
const TP_OK: TypePathKind = 0;
const TP_PARTIAL: TypePathKind = 1;
const TP_RES: TypePathKind = 2;

#[derive(Clone, Copy)]
struct TypePath {
    st: TypePathKind,
    ent: i32,
    // OK: the type; PARTIAL: the last type reached
    exact: bool,
    unfound: bool,
    // RES: no scope level has the head (the result says why not)
    res: Resolution,
    // RES
}
impl Default for TypePath {
    fn default() -> Self {
        unsafe { zeroed() }
    }
}

unsafe fn tp_res(r: Resolution) -> TypePath {
    return TypePath {
        st: (TP_RES),
        res: r,
        ..TypePath::default()
    };
}

unsafe fn join(
    segs: *const *mut c_char,
    from: i32,
    to: i32,
    out: *mut c_char,
    cap: usize,
) -> bool {
    let mut written = 0;
    out.write(0);
    for i in from..to {
        let name = name_bytes(*segs.offset(i as isize));
        let sep: &[u8] = if written > 0 { b"." } else { b"" };
        if !write_key(out.add(written), cap - written, &[sep, name]) {
            return false;
        }
        written += sep.len() + name.len();
    }
    true
}

// Walk segs[from, to) as member types of `cur` (own, then inherited).
unsafe fn walk_nested(
    c: &ResolveContext,
    mut cur: i32,
    segs: *const *mut c_char,
    from: i32,
    to: i32,
    exact: bool,
) -> TypePath {
    for i in from..to {
        let next: i32 =
            member_type(c, cur, (*segs.offset(i as isize)) as *const c_char);
        if next == JR_NONE {
            return TypePath {
                st: (TP_PARTIAL),
                ent: cur,

                exact: exact,
                ..TypePath::default()
            };
        }
        if !(visible(c, next)) {
            return tp_res(unres(CBM_DOCLINK_REASON_TEST_ONLY));
        }
        cur = next;
    }
    return TypePath {
        st: (TP_OK),
        ent: cur,
        exact: exact,
        ..TypePath::default()
    };
}

// The file's single import of the simple name `name` with one of `kinds`;
// the next one after `prev` (the imports are sorted by alias).
unsafe fn import_next(
    f: *const File,
    name: *const c_char,
    prev: *const Import,
) -> *const Import {
    let i: i32;
    if !(prev).is_null() {
        i = ((prev.offset_from((*f).imports)) as i32) + SKIP_ONE;
    } else {
        let mut lo: i32 = 0;
        let mut hi: i32 = (*f).nimports;
        while lo < hi {
            let mid: i32 = lo + ((hi - lo) / PAIR_LEN);
            if strcmp((*((*f).imports).offset(mid as isize)).alias, name) < 0 {
                lo = mid + SKIP_ONE;
            } else {
                hi = mid;
            }
        }
        i = lo;
    }
    return (if (i < (*f).nimports)
        && (strcmp((*((*f).imports).offset(i as isize)).alias, name) == 0)
    {
        addr_of_mut!(*((*f).imports).offset(i as isize))
    } else {
        ptr::null_mut()
    }) as *const Import;
}

// True when the repository declares package `p`.
unsafe fn is_package(ix: *const Index, p: *const c_char) -> bool {
    return ((*p.offset(0)) != 0)
        && (cbm_ht_get(((*ix).packages) as *const HashTable, p)
            != ptr::null_mut());
}

// The package part of a qualified name: everything before its last name.
// false for a simple name.
unsafe fn package_of(fqn: *const c_char, out: *mut c_char, cap: usize) -> bool {
    let dot: *const c_char = (strrchr(fqn, b'.' as i32)) as *const c_char;
    if (dot.is_null()) || (((dot.offset_from(fqn)) as usize) >= cap) {
        return false;
    }
    memcpy(
        out as *mut c_void,
        fqn as *const c_void,
        (dot.offset_from(fqn)) as usize,
    );
    *out.offset(dot.offset_from(fqn)) = b'\0' as c_char;
    return true;
}

// True when the file's scope is open: an on-demand import of a package the
// repository does not declare (or of a type it does not declare) can supply
// any simple name.
unsafe fn scope_open(c: &ResolveContext) -> bool {
    for i in 0..(*c.f).nwild {
        let target: *const c_char = (*((*c.f).wild).offset(i as isize)).target;
        if (!(is_package(c.ix, target)))
            && (find_entity(c.ix, target) == JR_NONE)
        {
            return true;
        }
    }
    return false;
}

// True when package `pkg` lies in a directory the indexer never reads, below
// a package the repository owns. A JVM package is a directory path, and
// discovery skips some directory names wherever they occur (`env`, `target`,
// `tmp` ...): what is declared there is on disk, but not in the graph.
unsafe fn not_indexed(ix: *const Index, pkg: *const c_char) -> bool {
    let mut buf: [c_char; KEY_BYTES] = zeroed();
    let n: usize = strlen(pkg);
    if (n == 0) || (n >= size_of::<[c_char; KEY_BYTES]>()) {
        return false;
    }
    memcpy(
        (addr_of_mut!(buf).cast::<c_char>()) as *mut c_void,
        pkg as *const c_void,
        n + (SKIP_ONE as usize),
    );
    let mut owned: bool = false;
    // a prefix before this segment is the repository's
    let mut seg: usize = 0;
    for i in 0..=n {
        if (((buf[i]) as i32) != b'.' as i32)
            && (((buf[i]) as i32) != b'\0' as i32)
        {
            continue;
        }
        let keep: c_char = buf[i];
        buf[i] = b'\0' as c_char;
        // buf: the prefix through this segment; buf + seg: the segment
        if (owned)
            && (cbm_should_skip_dir(
                ((addr_of_mut!(buf).cast::<c_char>()).offset(seg as isize))
                    as *const c_char,
                (*ix).mode,
            ))
        {
            return true;
        }
        owned = ((owned)
            || (!(cbm_ht_get(
                ((*ix).packages) as *const HashTable,
                (addr_of_mut!(buf).cast::<c_char>()) as *const c_char,
            ))
            .is_null()))
            || (!(cbm_ht_get(
                ((*ix).pkg_prefixes) as *const HashTable,
                (addr_of_mut!(buf).cast::<c_char>()) as *const c_char,
            ))
            .is_null());
        buf[i] = keep;
        seg = i + (SKIP_ONE as usize);
    }
    return false;
}

// What a name that no scope shows is, by its package: declared by the
// repository without a scope record (G: generated code, a declaration the
// scan does not see), in a part of it the indexer does not read (N), or
// outside the repository X. `pkg`: the package, or the type, the name is
// said to be in.
unsafe fn unseen_kind(ix: *const Index, pkg: *const c_char) -> c_char {
    if ((*pkg.offset(0)) != 0)
        && ((is_package(ix, pkg)) || (find_entity(ix, pkg) != JR_NONE))
    {
        return b'G' as c_char;
    }
    return (if not_indexed(ix, pkg) {
        b'N' as i32
    } else {
        b'X' as i32
    }) as c_char;
}

unsafe fn kind_reason(kind: c_char) -> i32 {
    return if (kind as i32) == b'G' as i32 {
        CBM_DOCLINK_REASON_GRAPH_GAP
    } else {
        if (kind as i32) == b'N' as i32 {
            CBM_DOCLINK_REASON_NOT_INDEXED
        } else {
            CBM_DOCLINK_REASON_EXTERNAL
        }
    };
}

// What an explicit import of `fqn` names: the type, or what unseen_kind
// says of its package.
unsafe fn cand_import_type(
    c: &ResolveContext,
    out: *mut Candidates,
    fqn: *const c_char,
) {
    let ent: i32 = find_entity(c.ix, fqn);
    if ent != JR_NONE {
        cand_type(c, out, ent);
        return;
    }
    let mut pkg: [c_char; KEY_BYTES] = zeroed();
    if !(package_of(
        fqn,
        addr_of_mut!(pkg).cast::<c_char>(),
        size_of::<[c_char; KEY_BYTES]>(),
    )) {
        pkg[0] = b'\0' as c_char;
    }
    cand(
        out,
        unseen_kind(c.ix, (addr_of_mut!(pkg).cast::<c_char>()) as *const c_char),
        JR_NONE,
    );
}

// ── Why a name was not found ──────────────────────────────────────
// A simple name no scope level has: the scope can still explain it.
unsafe fn unfound_simple(c: &ResolveContext, name: *const c_char) -> Resolution {
    if (!((*c.f).kotlin)) && (named(&JR_JAVA_PRIMITIVES, name)) {
        return unres(CBM_DOCLINK_REASON_EXTERNAL);
    }
    if scope_open(c) {
        return unres(CBM_DOCLINK_REASON_EXTERNAL);
    }
    {
        let mut i: i32 = 0;
        while (c.inherit) && (i < c.nchain) {
            if open(c.ix, (c.chain)[i as usize]) {
                return unres(CBM_DOCLINK_REASON_EXTERNAL);
                // a supertype outside may declare it
            }

            i += 1;
        }
    }
    return unres(CBM_DOCLINK_REASON_MISSING);
}

// True when the file imports something of package `pkg` (or the package
// itself on demand) and the repository does not declare that package.
unsafe fn imports_outside(c: &ResolveContext, pkg: *const c_char) -> bool {
    if is_package(c.ix, pkg) {
        return false;
    }
    let pl: usize = strlen(pkg);
    for i in 0..(*c.f).nwild {
        if strcmp((*((*c.f).wild).offset(i as isize)).target, pkg) == 0 {
            return true;
        }
    }
    for i in 0..(*c.f).nimports {
        let t: *const c_char = (*((*c.f).imports).offset(i as isize)).target;
        if (strncmp(t, pkg, pl) == 0)
            && (((*t.offset(pl as isize)) as i32) == b'.' as i32)
        {
            return true;
        }
    }
    return false;
}

// A qualified name that is no type, no member and no top-level declaration.
//   - a package of the repository (or the start of one): packages have no node
//   - `pkg.Type..` (a capitalized name after the package part): the type is
//     missing when the repository declares that package, else it is outside
//   - all lower case: the languages' own namespaces and what the file imports
//     from outside are external; anything else names nothing
//   - `Type.x` with an unknown head: as the simple name
// The roots are the namespaces the two platforms define themselves (`java`,
// `javax`, `kotlin`). A library family is none, however common: a path under
// `kotlinx` or `org` is external only where the file imports from it.
unsafe fn unfound_path(
    c: &ResolveContext,
    segs: *const *mut c_char,
    n: i32,
) -> Resolution {
    let roots: [*const c_char; 3] =
        [c"java".as_ptr(), c"javax".as_ptr(), c"kotlin".as_ptr()];
    let ix: *const Index = c.ix;
    let mut buf: [c_char; KEY_BYTES] = zeroed();
    if !(join(
        segs,
        0,
        n,
        addr_of_mut!(buf).cast::<c_char>(),
        size_of::<[c_char; KEY_BYTES]>(),
    )) {
        return unres(CBM_DOCLINK_REASON_UNPARSEABLE);
    }
    if (!(cbm_ht_get(
        ((*ix).packages) as *const HashTable,
        (addr_of_mut!(buf).cast::<c_char>()) as *const c_char,
    ))
    .is_null())
        || (!(cbm_ht_get(
            ((*ix).pkg_prefixes) as *const HashTable,
            (addr_of_mut!(buf).cast::<c_char>()) as *const c_char,
        ))
        .is_null())
    {
        return unres(CBM_DOCLINK_REASON_GRAPH_GAP);
    }
    if n == SKIP_ONE {
        return unfound_simple(c, (*segs.offset(0)) as *const c_char);
    }
    let mut k: i32 = 0;
    while (k < n)
        && ((isupper(((*(*segs.offset(k as isize)).offset(0)) as u8) as i32))
            == 0)
    {
        k += 1;
    }
    if k == 0 {
        return unfound_simple(c, (*segs.offset(0)) as *const c_char);
    }
    if k < n {
        let _ = join(
            segs,
            0,
            k,
            addr_of_mut!(buf).cast::<c_char>(),
            size_of::<[c_char; KEY_BYTES]>(),
        );
        if is_package(ix, (addr_of_mut!(buf).cast::<c_char>()) as *const c_char)
        {
            return unres(CBM_DOCLINK_REASON_MISSING);
        }
        return unres(kind_reason(unseen_kind(
            ix,
            (addr_of_mut!(buf).cast::<c_char>()) as *const c_char,
        )));
    }
    if named(&roots, (*segs.offset(0)) as *const c_char) {
        return unres(CBM_DOCLINK_REASON_EXTERNAL);
    }
    if not_indexed(ix, (addr_of_mut!(buf).cast::<c_char>()) as *const c_char) {
        return unres(CBM_DOCLINK_REASON_NOT_INDEXED);
        // a package the indexer skips
    }
    {
        let mut i: i32 = n;
        while i >= SKIP_ONE {
            let _ = join(
                segs,
                0,
                i,
                addr_of_mut!(buf).cast::<c_char>(),
                size_of::<[c_char; KEY_BYTES]>(),
            );
            if imports_outside(
                c,
                (addr_of_mut!(buf).cast::<c_char>()) as *const c_char,
            ) {
                return unres(CBM_DOCLINK_REASON_EXTERNAL);
            }

            i -= 1;
        }
    }
    return unres(CBM_DOCLINK_REASON_MISSING);
}

// The members every type has without declaring them, in either language:
// Object's, an enum's, an annotation's. By name alone, and wider than any one
// language: it only tells `external` from `missing` once nothing was found.
unsafe fn implicit_member(
    ix: *const Index,
    ent: i32,
    name: *const c_char,
) -> bool {
    let kind: c_char = (*((*ix).ents).offset(ent as isize)).kind;
    if object_method(name, ptr::null()) {
        return true;
    }
    if (kind as i32) == b'e' as i32 {
        return (named(&JR_JAVA_ENUM_MEMBERS, name))
            || (named(&JR_KOTLIN_ENUM_MEMBERS, name));
    }
    return ((kind as i32) == b'a' as i32)
        && (strcmp(name, c"annotationType".as_ptr()) == 0);
}

// True when the type `ent`, a type around the documented definition of a
// Kotlin file, has a member `name` without declaring it: a member of
// kotlin.Any, at an enum class what an enum class has (the name alone
// decides). Java's counterpart is javadoc's own search walk.
unsafe fn given_member(
    c: &ResolveContext,
    ent: i32,
    name: *const c_char,
) -> bool {
    let kind: c_char = (*((*c.ix).ents).offset(ent as isize)).kind;
    return (named(&JR_ANY_MEMBERS, name))
        || (((kind as i32) == b'e' as i32)
            && (named(&JR_KOTLIN_ENUM_MEMBERS, name)));
}

type LookupKind = i32;
const LK_NONE: LookupKind = 0;
const LK_FOUND: LookupKind = 1;
const LK_INVISIBLE: LookupKind = 2;
const LK_LOCAL: LookupKind = 3;

unsafe fn object_method(name: *const c_char, r: *const Reference) -> bool {
    {
        let mut i: usize = 0;
        while i < (size_of::<[Given; 11]>() / size_of::<Given>()) {
            let mut p: *const c_char = (JR_OBJECT_METHODS[i]).sig;
            let mut same: bool = strcmp((JR_OBJECT_METHODS[i]).name, name) == 0;
            {
                let mut k: i32 = 0;
                while ((same) && (!(r).is_null())) && (k < (*r).nparams) {
                    let n: usize = strlen(
                        addr_of!(((*r).params)[k as usize]).cast::<c_char>(),
                    );
                    same = ((n > 0)
                        && (strncmp(
                            p,
                            addr_of!(((*r).params)[k as usize]).cast::<c_char>(),
                            n,
                        ) == 0))
                        && ((((*p.offset(n as isize)) as i32) == b'|' as i32)
                            || (((*p.offset(n as isize)) as i32)
                                == b'\0' as i32));
                    p = p.offset(if same {
                        (n + ((if ((*p.offset(n as isize)) as i32) == b'|' as i32
                        {
                            SKIP_ONE
                        } else {
                            0
                        }) as usize)) as isize
                    } else {
                        0
                    });

                    k += 1;
                }
            }
            if (same) && ((r.is_null()) || (((*(p)) as i32) == b'\0' as i32)) {
                return true;
            }

            i += 1;
        }
    }
    return false;
}

// ── Context ───────────────────────────────────────────────────────
// True when a declaration with `slot` (see type_t) is the definition whose
// name below the module is `tail`. `path`: its logical path.
unsafe fn slot_is(
    slot: *const c_char,
    owner_path: *const c_char,
    name: *const c_char,
    tail: *const c_char,
) -> bool {
    if slot.is_null() {
        return false;
    }
    if strcmp(slot, c"=".as_ptr()) != 0 {
        return strcmp(slot, tail) == 0;
    }
    if owner_path.is_null() {
        return strcmp(name, tail) == 0;
    }
    let ol: usize = strlen(owner_path);
    return ((strncmp(tail, owner_path, ol) == 0)
        && (((*tail.offset(ol as isize)) as i32) == b'.' as i32))
        && (strcmp((tail.offset(ol as isize)).offset(SKIP_ONE as isize), name)
            == 0);
}

// The scope of the definition a reference is written on: the documented
// member or type (found by its line and its name below the module), and the
// types around it.
unsafe fn ctx_init(
    c: *mut ResolveContext,
    ix: *const Index,
    g: *const Graph,
    file: i32,
    link: *const DocLink,
) {
    clear_bytes(c as *mut c_void, 0, size_of::<ResolveContext>());
    (*c).ix = ix;
    (*c).g = g;
    (*c).file = file;
    (*c).f = (addr_of_mut!(*((*ix).files).offset(file as isize))) as *const File;
    (*c).prod = !((*(*c).f).is_test);
    (*c).inherit = true;
    if ((((*link).flags) as i32) & CBM_DOCLINK_FLAG_FILE) != 0 {
        return;
        // a file-level doc: the file's scope alone
    }
    let f: *const File = (*c).f;
    let line: u32 = (*link).def_line;
    let ml: usize = strlen((*f).module_qn);
    let qn: *const c_char = (*link).source_qn;
    let tail: *const c_char = if ((!(qn).is_null())
        && (strncmp(qn, (*f).module_qn, ml) == 0))
        && (((*qn.offset(ml as isize)) as i32) == b'.' as i32)
    {
        (qn.offset(ml as isize)).offset(SKIP_ONE as isize)
    } else {
        ptr::null()
    };
    let mut lo: i32 = 0;
    let mut hi: i32 = (*f).nmembers;
    while lo < hi {
        let mid: i32 = lo + ((hi - lo) / PAIR_LEN);
        if (*((*f).members)
            .offset((*((*f).by_start).offset(mid as isize)) as isize))
        .start
            < line
        {
            lo = mid + SKIP_ONE;
        } else {
            hi = mid;
        }
    }
    {
        let mut i: i32 = lo;
        while (!(tail).is_null()) && (i < (*f).nmembers) {
            let m: *const Member = (addr_of_mut!(
                *((*f).members)
                    .offset((*((*f).by_start).offset(i as isize)) as isize)
            )) as *const Member;
            if (*m).start != line {
                break;
            }
            if slot_is(
                (*m).slot,
                if (*m).owner == JR_NONE {
                    ptr::null()
                } else {
                    (*((*f).types).offset(((*m).owner) as isize)).path
                },
                (*m).name,
                tail,
            ) {
                (*c).member = m;
                chain_from(c, (*m).owner);
                return;
            }

            i += 1;
        }
    }
    // types are in source order: the last one that starts at or before the line
    lo = 0;
    hi = (*f).ntypes;
    while lo < hi {
        let mid: i32 = lo + ((hi - lo) / PAIR_LEN);
        if (*((*f).types).offset(mid as isize)).start <= line {
            lo = mid + SKIP_ONE;
        } else {
            hi = mid;
        }
    }
    let mut t: i32 = lo - SKIP_ONE;
    {
        let mut k: i32 = t;
        while ((!(tail).is_null()) && (k >= 0))
            && ((*((*f).types).offset(k as isize)).start == line)
        {
            if slot_is(
                (*((*f).types).offset(k as isize)).slot,
                ptr::null(),
                (*((*f).types).offset(k as isize)).path,
                tail,
            ) {
                (*c).type_index =
                    (addr_of_mut!(*((*f).types).offset(k as isize)))
                        as *const TypeDecl;
                chain_from(c, k);
                return;
            }

            k -= 1;
        }
    }
    // neither: a definition the scan has no record of. The innermost type
    // around its line is its scope.
    while (t >= 0) && ((*((*f).types).offset(t as isize)).end < line) {
        t = (*((*f).types).offset(t as isize)).outer;
    }
    if t >= 0 {
        chain_from(c, t);
    }
}

// The scope of a type DECLARATION, for the names its header writes
// (supertypes, an extension's receiver): the types around it. From structure
// alone, since a stored scope has no lines.
unsafe fn ctx_decl(
    c: *mut ResolveContext,
    ix: *const Index,
    g: *const Graph,
    file: i32,
    outer: i32,
    inherit: bool,
) {
    clear_bytes(c as *mut c_void, 0, size_of::<ResolveContext>());
    (*c).ix = ix;
    (*c).g = g;
    (*c).file = file;
    (*c).f = (addr_of_mut!(*((*ix).files).offset(file as isize))) as *const File;
    (*c).prod = false;
    // a declared supertype is whatever the compiler bound
    (*c).inherit = inherit;
    chain_from(c, outer);
}

// ── Supertypes, extensions ────────────────────────────────────────
// Split a dotted name in place. Returns the number of names, 0 when there
// are more than a reference may have.
unsafe fn split_dots(s: *mut c_char, segs: *mut *mut c_char) -> i32 {
    let mut n: i32 = 0;
    {
        let mut p: *mut c_char = s;
        loop {
            let dot: *mut c_char = strchr(p as *const c_char, b'.' as i32);
            if !(dot).is_null() {
                *(dot) = b'\0' as c_char;
            }
            if (n >= JR_MAX_SEGS) || ((*p.offset(0)) == 0) {
                return 0;
            }
            *segs.offset(
                ({
                    let value = addr_of_mut!(n);
                    let previous = *value;
                    *value += 1;
                    previous
                }) as isize,
            ) = p;
            if dot.is_null() {
                return n;
            }
            p = dot.offset(SKIP_ONE as isize);
        }
    }
}

// The type a declaration header of `file` names with `name[0, len)`, in the
// scope `c`; JR_NONE when the repository declares no such type.
unsafe fn header_type(
    c: &ResolveContext,
    name: *const c_char,
    len: usize,
) -> i32 {
    let mut buf: [c_char; KEY_BYTES] = zeroed();
    let mut segs: [*mut c_char; 32] = zeroed();
    if (len == 0) || (len >= size_of::<[c_char; KEY_BYTES]>()) {
        return JR_NONE;
    }
    memcpy(
        (addr_of_mut!(buf).cast::<c_char>()) as *mut c_void,
        name as *const c_void,
        len,
    );
    buf[len] = b'\0' as c_char;
    let n: i32 = split_dots(
        addr_of_mut!(buf).cast::<c_char>(),
        addr_of_mut!(segs).cast::<*mut c_char>(),
    );
    if n == 0 {
        return JR_NONE;
    }
    let tp: TypePath = if (*c.f).kotlin {
        kotlin_type_path(
            c,
            (addr_of_mut!(segs).cast::<*mut c_char>()) as *const *mut c_char,
            n,
        )
    } else {
        java_type_path(
            c,
            (addr_of_mut!(segs).cast::<*mut c_char>()) as *const *mut c_char,
            n,
        )
    };
    return if ((tp.st) as u32) == (TP_OK as u32) {
        tp.ent
    } else {
        JR_NONE
    };
}

// True when a declaration header of the context's file writes, with
// `name[0, len)`, the root type every type has anyway: java.lang.Object,
// kotlin.Any. Written out it adds no member (given_member knows them), so
// it does not leave the hierarchy open. The simple name counts only where
// nothing else can supply it: no import of that name, no on-demand import of
// a package the repository does not declare.
unsafe fn root_super(
    c: &ResolveContext,
    name: *const c_char,
    len: usize,
) -> bool {
    let full: *const c_char = (if (*c.f).kotlin {
        c"kotlin.Any".as_ptr() as *mut c_char
    } else {
        c"java.lang.Object".as_ptr() as *mut c_char
    }) as *const c_char;
    let simple: *const c_char = ((strrchr(full, b'.' as i32))
        .offset(SKIP_ONE as isize))
        as *const c_char;
    if (len == strlen(full))
        && (memcmp(name as *const c_void, full as *const c_void, len) == 0)
    {
        return true;
    }
    return (((len == strlen(simple))
        && (memcmp(name as *const c_void, simple as *const c_void, len) == 0))
        && ((import_next(c.f, simple, ptr::null())).is_null()))
        && (!(scope_open(c)));
}

// True when the repository's type `step` is an interface to javadoc (an
// annotation type is one).
unsafe fn walk_iface(ix: *const Index, step: i32) -> bool {
    return (step >= 0)
        && (((((*((*ix).ents).offset(step as isize)).kind) as i32)
            == b'i' as i32)
            || ((((*((*ix).ents).offset(step as isize)).kind) as i32)
                == b'a' as i32));
}

// The steps of javadoc's search for a method (walk) at one declaration of
// kind `kind`, from the supertypes its header writes, in the order written
// (`steps[0, nd)`: types of the repository, JR_W_OBJECT for Object or Any
// written out, JR_W_OUTSIDE for any other). Returns the superclass step and
// leaves the interface steps, in order, in steps[0, *n).
//   - An interface has no superclass; an annotation type has the interface
//     java.lang.annotation.Annotation, an enum the superclass java.lang.Enum,
//     a record java.lang.Record (which adds nothing to what the record has
//     itself and Object's), any other class Object unless its header names
//     another.
//   - Java writes the superclass first: a first supertype that is no
//     interface of the repository is it. One outside the repository may be
//     the superclass or the first interface; it is taken as the superclass,
//     the earlier of the two places.
//   - Kotlin writes the superclass anywhere in the list: the one class of the
//     repository there is it. Without one, a supertype outside the
//     repository may be the superclass, which javadoc searches before every
//     interface: the superclass step is then outside.
//   - A Kotlin type alias is outside to this search.
unsafe fn walk_steps(
    ix: *const Index,
    kind: c_char,
    kotlin: bool,
    steps: *mut i32,
    nd: i32,
    n: *mut i32,
) -> i32 {
    let mut sup: i32 = JR_W_OBJECT;
    let mut skip: i32 = JR_NONE;
    // the entry that is the superclass
    for i in 0..nd {
        if (*steps.offset(i as isize) >= 0)
            && ((((*((*ix).ents).offset((*steps.offset(i as isize)) as isize))
                .kind) as i32)
                == b't' as i32)
        {
            *steps.offset(i as isize) = JR_W_OUTSIDE;
        }
    }
    *(n) = 0;
    if (kind as i32) == b'i' as i32 {
        sup = JR_W_NONE;
    } else if (kind as i32) == b'a' as i32 {
        sup = JR_W_ANNOTATION;
    } else if (kind as i32) == b'e' as i32 {
        sup = JR_W_ENUM;
    } else if (kind as i32) == b't' as i32 {
        return JR_W_OUTSIDE;
    } else if ((kind as i32) != b'r' as i32) && (!(kotlin)) {
        if ((nd > 0) && (*steps.offset(0) != JR_W_OBJECT))
            && (!(walk_iface(ix, *steps.offset(0))))
        {
            sup = *steps.offset(0);
            skip = 0;
        }
    } else if (kind as i32) != b'r' as i32 {
        let mut classes: i32 = 0;
        let mut outside: bool = false;
        for i in 0..nd {
            if (*steps.offset(i as isize) >= 0)
                && (!(walk_iface(ix, *steps.offset(i as isize))))
            {
                classes += 1;
                skip = i;
            }
            outside = (outside) || (*steps.offset(i as isize) == JR_W_OUTSIDE);
        }
        if classes > SKIP_ONE {
            return JR_W_OUTSIDE;
            // two superclasses: no order to follow
        }
        if classes == SKIP_ONE {
            sup = *steps.offset(skip as isize);
        } else if outside {
            sup = JR_W_OUTSIDE;
        }
    }
    let mut w: i32 = 0;
    for i in 0..nd {
        if (i != skip) && (*steps.offset(i as isize) != JR_W_OBJECT) {
            *steps.offset(
                ({
                    let value = addr_of_mut!(w);
                    let previous = *value;
                    *value += 1;
                    previous
                }) as isize,
            ) = *steps.offset(i as isize);
        }
    }
    *(n) = w;
    return sup;
}

// One declaration's steps into its type's: the first declaration sets them,
// a later one that has other steps or another order leaves the type's place
// in javadoc's search unknown (its superclass step outside).
unsafe fn walk_decl(
    ix: *mut Index,
    e: *mut Entity,
    t: *const TypeDecl,
    kotlin: bool,
    steps: *mut i32,
    nd: i32,
    merge: bool,
) -> bool {
    let mut n: i32 = 0;
    let sup: i32 = walk_steps(
        ix as *const Index,
        (*t).kind,
        kotlin,
        steps,
        nd,
        addr_of_mut!(n),
    );
    if !(merge) {
        (*e).wsuper = sup;
        if n > 0 {
            (*e).wifaces =
                (alloc(ix, (n as usize) * size_of::<i32>())) as *mut i32;
            if ((*e).wifaces).is_null() {
                return false;
            }
            memcpy(
                ((*e).wifaces) as *mut c_void,
                steps as *const c_void,
                (n as usize) * size_of::<i32>(),
            );
            (*e).nwifaces = n;
        }
        return true;
    }
    if ((sup != (*e).wsuper) || (n != (*e).nwifaces))
        || ((n > 0)
            && (memcmp(
                steps as *const c_void,
                ((*e).wifaces) as *const c_void,
                (n as usize) * size_of::<i32>(),
            ) != 0))
    {
        (*e).wsuper = JR_W_OUTSIDE;
        (*e).nwifaces = 0;
    }
    return true;
}

// The type around declaration `t` of `f`, as the scope chain has it.
unsafe fn walk_encl(f: *const File, t: *const TypeDecl) -> i32 {
    let mut o: i32 = (*t).outer;
    while (o != JR_NONE) && ((*((*f).types).offset(o as isize)).entity < 0) {
        o = (*((*f).types).offset(o as isize)).outer;
    }
    return if o == JR_NONE {
        JR_NONE
    } else {
        (*((*f).types).offset(o as isize)).entity
    };
}

// The direct supertypes of every type (each declaration's list, bound in that
// declaration's own scope), then every type's closure: breadth-first, the
// nearest first, each supertype once. A supertype that is no type of the
// repository leaves the hierarchy open. Alongside, each type's steps of
// javadoc's search for a method walk_steps.
unsafe fn resolve_supers(ix: *mut Index, g: *const Graph) -> bool {
    let n: usize = if ((*ix).nents) != 0 {
        ((*ix).nents) as usize
    } else {
        SKIP_ONE as usize
    };
    let seen: *mut i32 =
        (cbm_calloc(CBM_MEM_CLASS_OTHER, n * size_of::<i32>())) as *mut i32;
    let queue: *mut i32 =
        (cbm_alloc(CBM_MEM_CLASS_OTHER, n * size_of::<i32>())) as *mut i32;
    let mut steps: *mut i32 = ptr::null_mut();
    // one declaration's supertypes, as javadoc's search sees them
    let mut scap: i32 = 0;
    let mut ok: bool = (!(seen).is_null()) && (!(queue).is_null());
    {
        let mut ei: i32 = 0;
        while (ok) && (ei < (*ix).nents) {
            let e: *mut Entity = addr_of_mut!(*((*ix).ents).offset(ei as isize));
            let stamp: i32 = ei + SKIP_ONE;
            let mut nb: i32 = 0;
            {
                let mut d: i32 = 0;
                while (ok) && (d < (*e).ndecls) {
                    let file: i32 = (*((*e).decls).offset(d as isize)).file;
                    let t: *const TypeDecl = (addr_of_mut!(
                        *((*((*ix).files).offset(file as isize)).types).offset(
                            ((*((*e).decls).offset(d as isize)).type_index)
                                as isize
                        )
                    ))
                        as *const TypeDecl;
                    let mut c: ResolveContext = ResolveContext::default();
                    if (*((*t).supers).offset(0)) != 0 {
                        ctx_decl(
                            addr_of_mut!(c),
                            ix as *const Index,
                            g,
                            file,
                            (*t).outer,
                            false,
                        );
                    }
                    let mut nd: i32 = 0;
                    {
                        let mut p: *const c_char = (*t).supers;
                        while ((ok) && (!(p).is_null())) && ((*(p)) != 0) {
                            let bar: *const c_char =
                                (strchr(p, b'|' as i32)) as *const c_char;
                            let len: usize = if !(bar).is_null() {
                                (bar.offset_from(p)) as usize
                            } else {
                                strlen(p)
                            };
                            let base: i32 = header_type(&c, p, len);
                            let mut step: i32 = base;
                            if base == ei {
                                (*e).open = true;
                                step = JR_W_OUTSIDE;
                            } else if base == JR_NONE {
                                let root: bool = root_super(&c, p, len);
                                (*e).open = ((*e).open) || (!(root));
                                step = if root {
                                    JR_W_OBJECT
                                } else {
                                    JR_W_OUTSIDE
                                };
                            } else if *seen.offset(base as isize) != stamp {
                                *seen.offset(base as isize) = stamp;
                                *queue.offset(
                                    ({
                                        let value = addr_of_mut!(nb);
                                        let previous = *value;
                                        *value += 1;
                                        previous
                                    })
                                        as isize,
                                ) = base;
                            }
                            if nd >= scap {
                                let ncap: i32 = if (scap) != 0 {
                                    scap * PAIR_LEN
                                } else {
                                    CBM_SZ_16
                                };
                                let grown: *mut i32 = (cbm_alloc(
                                    CBM_MEM_CLASS_OTHER,
                                    (ncap as usize) * size_of::<i32>(),
                                ))
                                    as *mut i32;
                                ok = grown != (ptr::null_mut());
                                if (ok) && (nd > 0) {
                                    memcpy(
                                        grown as *mut c_void,
                                        steps as *const c_void,
                                        (nd as usize) * size_of::<i32>(),
                                    );
                                }
                                cbm_free(
                                    CBM_MEM_CLASS_OTHER,
                                    steps as *mut c_void,
                                );
                                steps = grown;
                                scap = if ok { ncap } else { 0 };
                            }
                            if ok {
                                *steps.offset(
                                    ({
                                        let value = addr_of_mut!(nd);
                                        let previous = *value;
                                        *value += 1;
                                        previous
                                    })
                                        as isize,
                                ) = step;
                            }
                            p = if !(bar).is_null() {
                                bar.offset(SKIP_ONE as isize)
                            } else {
                                ptr::null()
                            };
                        }
                    }
                    ok = (ok)
                        && (walk_decl(
                            ix,
                            e,
                            t,
                            (*((*ix).files).offset(file as isize)).kotlin,
                            steps,
                            nd,
                            d > 0,
                        ));
                    if d == 0 {
                        (*e).encl = walk_encl(
                            (addr_of_mut!(*((*ix).files).offset(file as isize)))
                                as *const File,
                            t,
                        );
                    }

                    d += 1;
                }
            }
            if (ok) && (nb > 0) {
                (*e).bases =
                    (alloc(ix, (nb as usize) * size_of::<i32>())) as *mut i32;
                ok = (*e).bases != (ptr::null_mut());
                if ok {
                    memcpy(
                        ((*e).bases) as *mut c_void,
                        queue as *const c_void,
                        (nb as usize) * size_of::<i32>(),
                    );
                    (*e).nbases = nb;
                }
            }

            ei += 1;
        }
    }
    clear_bytes(
        seen as *mut c_void,
        0,
        if ok { n * size_of::<i32>() } else { 0 },
    );
    {
        let mut ei: i32 = 0;
        while (ok) && (ei < (*ix).nents) {
            let e: *mut Entity = addr_of_mut!(*((*ix).ents).offset(ei as isize));
            let stamp: i32 = ei + SKIP_ONE;
            let mut head: i32 = 0;
            let mut tail: i32 = 0;
            *seen.offset(ei as isize) = stamp;
            // a cycle does not come back to the type itself
            for b in 0..(*e).nbases {
                if *seen.offset((*((*e).bases).offset(b as isize)) as isize)
                    != stamp
                {
                    *(seen)
                        .offset((*((*e).bases).offset(b as isize)) as isize) =
                        stamp;
                    *queue.offset(
                        ({
                            let value = addr_of_mut!(tail);
                            let previous = *value;
                            *value += 1;
                            previous
                        }) as isize,
                    ) = *((*e).bases).offset(b as isize);
                }
            }
            while head < tail {
                let s: *const Entity = (addr_of_mut!(*((*ix).ents).offset(
                    (*queue.offset(
                        ({
                            let value = addr_of_mut!(head);
                            let previous = *value;
                            *value += 1;
                            previous
                        }) as isize
                    )) as isize
                ))) as *const Entity;
                for b in 0..(*s).nbases {
                    if *(seen)
                        .offset((*((*s).bases).offset(b as isize)) as isize)
                        != stamp
                    {
                        *seen.offset(
                            (*((*s).bases).offset(b as isize)) as isize,
                        ) = stamp;
                        *queue.offset(
                            ({
                                let value = addr_of_mut!(tail);
                                let previous = *value;
                                *value += 1;
                                previous
                            }) as isize,
                        ) = *((*s).bases).offset(b as isize);
                    }
                }
            }
            if tail > 0 {
                (*e).closure =
                    (alloc(ix, (tail as usize) * size_of::<i32>())) as *mut i32;
                ok = (*e).closure != (ptr::null_mut());
                if ok {
                    memcpy(
                        ((*e).closure) as *mut c_void,
                        queue as *const c_void,
                        (tail as usize) * size_of::<i32>(),
                    );
                    (*e).nclosure = tail;
                }
            }

            ei += 1;
        }
    }
    cbm_free(CBM_MEM_CLASS_OTHER, seen as *mut c_void);
    cbm_free(CBM_MEM_CLASS_OTHER, queue as *mut c_void);
    cbm_free(CBM_MEM_CLASS_OTHER, steps as *mut c_void);
    return ok;
}

// The receiver of every Kotlin extension, bound in its file's scope, and the
// extensions of each receiver type by name.
unsafe fn build_extensions(ix: *mut Index, g: *const Graph) -> bool {
    for fi in 0..(*ix).nfiles {
        let f: *mut File = addr_of_mut!(*((*ix).files).offset(fi as isize));
        {
            let mut mi: i32 = 0;

            while ((*f).kotlin) && (mi < (*f).nmembers) {
                'iteration_5: {
                    let m: *mut Member =
                        addr_of_mut!(*((*f).members).offset(mi as isize));
                    if (*((*m).recv).offset(0)) == 0 {
                        break 'iteration_5;
                    }
                    if strcmp((*m).recv, c"?".as_ptr()) != 0 {
                        let mut c: ResolveContext = ResolveContext::default();
                        ctx_decl(
                            addr_of_mut!(c),
                            ix as *const Index,
                            g,
                            fi,
                            (*m).owner,
                            true,
                        );
                        (*m).recv_entity =
                            header_type(&c, (*m).recv, strlen((*m).recv));
                        if (*m).recv_entity == JR_NONE {
                            break 'iteration_5;
                            // a receiver outside the repository: nobody names it
                        }
                    }
                    let mut key: [c_char; KEY_BYTES] = zeroed();
                    if !(ext_key(
                        (*m).recv_entity,
                        (*m).name,
                        addr_of_mut!(key).cast::<c_char>(),
                        size_of::<[c_char; KEY_BYTES]>(),
                    )) {
                        break 'iteration_5;
                    }
                    let l: *mut IntList = ht_ilist(
                        ix,
                        (*ix).extensions,
                        (addr_of_mut!(key).cast::<c_char>()) as *const c_char,
                        true,
                    );
                    if ((l.is_null()) || (!(ilist_push(ix, l, fi))))
                        || (!(ilist_push(ix, l, mi)))
                    {
                        return false;
                    }
                }
                mi += 1;
            }
        }
    }
    return true;
}

// ── Index lifecycle and the resolver hooks ────────────────────────
unsafe extern "C" fn destroy(index: *mut c_void) {
    let ix: *mut Index = index as *mut Index;
    if ix.is_null() {
        return;
    }
    if !((*ix).stats).is_null() {
        let st = &*(*ix).stats;
        let b = [
            &st.source_gaps,
            &st.plain_text,
            &st.type_and_function,
            &st.type_and_function_node,
            &st.java_given,
            &st.java_open,
            &st.kotlin_given,
            &st.kotlin_open,
            &st.kotlin_extension,
            &st.java_field,
            &st.java_field_named,
        ]
        .map(|counter| decimal(counter.load(Ordering::Relaxed)));
        cbm_log(
            CBM_LOG_INFO,
            c"doc_links.jvm.done".as_ptr(),
            c"source_without_node".as_ptr(),
            b[0].as_ptr().cast::<c_char>(),
            c"kdoc_plain_text".as_ptr(),
            b[1].as_ptr().cast::<c_char>(),
            c"type_and_function".as_ptr(),
            b[2].as_ptr().cast::<c_char>(),
            c"type_and_function_with_node".as_ptr(),
            b[3].as_ptr().cast::<c_char>(),
            c"not_taken_java_given".as_ptr(),
            b[4].as_ptr().cast::<c_char>(),
            c"not_taken_java_open".as_ptr(),
            b[5].as_ptr().cast::<c_char>(),
            c"not_taken_kotlin_given".as_ptr(),
            b[6].as_ptr().cast::<c_char>(),
            c"not_taken_kotlin_open".as_ptr(),
            b[7].as_ptr().cast::<c_char>(),
            c"not_taken_kotlin_extension".as_ptr(),
            b[8].as_ptr().cast::<c_char>(),
            c"not_taken_java_field".as_ptr(),
            b[9].as_ptr().cast::<c_char>(),
            c"not_taken_java_field_named".as_ptr(),
            b[10].as_ptr().cast::<c_char>(),
            ptr::null::<c_char>(),
        );
        cbm_free(CBM_MEM_CLASS_OTHER, ((*ix).stats) as *mut c_void);
    }
    cbm_ht_free((*ix).ent_by_fqn);
    cbm_ht_free((*ix).pkg_types);
    cbm_ht_free((*ix).packages);
    cbm_ht_free((*ix).pkg_prefixes);
    cbm_ht_free((*ix).pkg_members);
    cbm_ht_free((*ix).extensions);
    cbm_ht_free((*ix).quarantine);
    cbm_ht_free((*ix).quarantine_test);
    cbm_free(CBM_MEM_CLASS_OTHER, ((*ix).ents) as *mut c_void);
    cbm_free(CBM_MEM_CLASS_OTHER, ((*ix).run_to_file) as *mut c_void);
    mori_jvm_doclinks_arena_drop((*ix).arena);
    cbm_free(CBM_MEM_CLASS_OTHER, ix as *mut c_void);
}

unsafe fn build_fail(ix: *mut Index, step: *const c_char) -> *mut c_void {
    cbm_log(
        CBM_LOG_ERROR,
        c"doc_links.jvm.error".as_ptr(),
        c"step".as_ptr() as *mut c_char,
        step,
        c"reason".as_ptr(),
        c"alloc".as_ptr() as *mut c_char,
        ptr::null::<c_char>(),
    );
    destroy(ix as *mut c_void);
    return ptr::null_mut();
}

unsafe fn has_suffix(s: *const c_char, sfx: *const c_char) -> bool {
    let n: usize = strlen(s);
    let sl: usize = strlen(sfx);
    return (n >= sl)
        && (strcmp((s.offset(n as isize)).offset(-(sl as isize)), sfx) == 0);
}

unsafe extern "C" fn build(input: *const BuildInput) -> *mut c_void {
    let ix: *mut Index =
        (cbm_calloc(CBM_MEM_CLASS_OTHER, size_of::<Index>())) as *mut Index;
    if ix.is_null() {
        return ptr::null_mut();
    }
    (*ix).arena = mori_jvm_doclinks_arena_new();
    if (*ix).arena.is_null() {
        return build_fail(ix, c"arena".as_ptr());
    }
    (*ix).project = mori_jvm_doclinks_project((*input).ctx);
    (*ix).mode = mori_jvm_doclinks_mode((*input).ctx);
    (*ix).ent_by_fqn = cbm_ht_create(CBM_SZ_4K as u32);
    (*ix).pkg_types = cbm_ht_create(CBM_SZ_4K as u32);
    (*ix).packages = cbm_ht_create(CBM_SZ_1K as u32);
    (*ix).pkg_prefixes = cbm_ht_create(CBM_SZ_1K as u32);
    (*ix).pkg_members = cbm_ht_create(CBM_SZ_1K as u32);
    (*ix).extensions = cbm_ht_create(CBM_SZ_256 as u32);
    (*ix).quarantine = cbm_ht_create(CBM_SZ_64 as u32);
    (*ix).quarantine_test = cbm_ht_create(CBM_SZ_64 as u32);
    (*ix).nfiles = (*input).file_count;
    (*ix).files = (alloc(
        ix,
        (if ((*input).file_count) != 0 {
            ((*input).file_count) as usize
        } else {
            SKIP_ONE as usize
        }) * size_of::<File>(),
    )) as *mut File;
    (*ix).run_count = (*input).run_file_count;
    (*ix).run_to_file = (cbm_alloc(
        CBM_MEM_CLASS_OTHER,
        (if ((*input).run_file_count) != 0 {
            ((*input).run_file_count) as usize
        } else {
            SKIP_ONE as usize
        }) * size_of::<i32>(),
    )) as *mut i32;
    (*ix).stats =
        (cbm_calloc(CBM_MEM_CLASS_OTHER, size_of::<Stats>())) as *mut Stats;
    if ((((((((((((*ix).ent_by_fqn).is_null())
        || (((*ix).pkg_types).is_null()))
        || (((*ix).packages).is_null()))
        || (((*ix).pkg_prefixes).is_null()))
        || (((*ix).pkg_members).is_null()))
        || (((*ix).extensions).is_null()))
        || (((*ix).quarantine).is_null()))
        || (((*ix).quarantine_test).is_null()))
        || (((*ix).files).is_null()))
        || (((*ix).run_to_file).is_null()))
        || (((*ix).stats).is_null())
    {
        return build_fail(ix, c"tables".as_ptr());
    }
    for i in 0..(*input).run_file_count {
        *((*ix).run_to_file).offset(i as isize) = JR_NONE;
    }
    let mut bad_scopes: i32 = 0;
    for i in 0..(*input).file_count {
        let src: *const InputFile =
            addr_of!(*((*input).files).offset(i as isize));
        let f: *mut File = addr_of_mut!(*((*ix).files).offset(i as isize));
        clear_bytes(f as *mut c_void, 0, size_of::<File>());
        (*f).rel_path = (*src).rel_path;
        // stays valid until destroy
        (*f).is_test = is_test_path((*src).rel_path);
        (*f).scoped =
            (!((*src).scope).is_null()) && (parse_scope(ix, f, (*src).scope));
        if !((*f).scoped) {
            bad_scopes += i32::from((*src).scope != (ptr::null()));
            (*f).nimports = {
                let assigned = {
                    let assigned = {
                        let assigned = {
                            let assigned = 0;
                            (*f).nunplaced = assigned;
                            assigned
                        };
                        (*f).nmembers = assigned;
                        assigned
                    };
                    (*f).ntypes = assigned;
                    assigned
                };
                (*f).nwild = assigned;
                assigned
            };
        }
        (*f).kotlin = !(has_suffix((*src).rel_path, c".java".as_ptr()));
        if ((*f).package).is_null() {
            (*f).package = c"".as_ptr();
        }
        (*f).module_qn = (cbm_fqn_module_source_lang(
            (*ix).arena,
            (*ix).project,
            (*src).rel_path,
            if (*f).kotlin {
                CBM_LANG_KOTLIN
            } else {
                CBM_LANG_JAVA
            },
        )) as *const c_char;
        if ((*f).module_qn).is_null() {
            (*ix).oom = true;
        }
        if ((*src).run_file >= 0) && ((*src).run_file < (*input).run_file_count)
        {
            *((*ix).run_to_file).offset(((*src).run_file) as isize) = i;
        }
    }
    if (*ix).oom {
        return build_fail(ix, c"scopes".as_ptr());
    }
    if bad_scopes > 0 {
        let b = decimal(i64::from(bad_scopes));
        cbm_log(
            CBM_LOG_WARN,
            c"doc_links.jvm.bad_scope".as_ptr(),
            c"files".as_ptr(),
            b.as_ptr().cast::<c_char>(),
            ptr::null::<c_char>(),
        );
    }
    let mut skipped: i32 = 0;
    if (!(build_entities(ix, addr_of_mut!(skipped)))) || ((*ix).oom) {
        return build_fail(ix, c"entities".as_ptr());
    }
    if (!(resolve_supers(ix, (*input).graph))) || ((*ix).oom) {
        return build_fail(ix, c"supertypes".as_ptr());
    }
    if (!(build_extensions(ix, (*input).graph))) || ((*ix).oom) {
        return build_fail(ix, c"extensions".as_ptr());
    }
    let mut incomplete: i32 = 0;
    for i in 0..(*ix).nents {
        incomplete += i32::from((*((*ix).ents).offset(i as isize)).incomplete);
    }
    let b = [
        i64::from((*ix).nfiles),
        i64::from((*ix).nents),
        i64::from(skipped),
        i64::from(incomplete),
        i64::from(cbm_ht_count((*ix).quarantine))
            + i64::from(cbm_ht_count((*ix).quarantine_test)),
    ]
    .map(decimal);
    cbm_log(
        CBM_LOG_INFO,
        c"doc_links.jvm.index".as_ptr(),
        c"files".as_ptr(),
        b[0].as_ptr().cast::<c_char>(),
        c"types".as_ptr(),
        b[1].as_ptr().cast::<c_char>(),
        c"skipped_types".as_ptr(),
        b[2].as_ptr().cast::<c_char>(),
        c"incomplete_types".as_ptr(),
        b[3].as_ptr().cast::<c_char>(),
        c"quarantined_names".as_ptr(),
        b[4].as_ptr().cast::<c_char>(),
        ptr::null::<c_char>(),
    );
    return ix as *mut c_void;
}

// True when the documented definition has a node of its own (or shares the
// node of its overload group). One node per qualified name: a definition
// whose name another declaration took (a Kotlin class and the factory
// function named like it, two classes' properties of one name in one file)
// has none, and an edge written for it would leave from that other
// declaration. A definition the scan has no record of is checked against the
// node itself: the source file is extracted in this run, so its nodes carry
// their lines.
unsafe fn source_has_node(c: &ResolveContext, link: *const DocLink) -> bool {
    if ((((*link).flags) as i32) & CBM_DOCLINK_FLAG_FILE) != 0 {
        return true;
        // the File node: the core finds it
    }
    if !(c.member).is_null() {
        return member_node(c, c.file, c.member) != (ptr::null());
    }
    if !(c.type_index).is_null() {
        let d: Decl = Decl {
            file: c.file,
            type_index: (((c.type_index).offset_from((*c.f).types)) as i32),
        };
        return type_node(c, d) != (ptr::null());
    }
    let n: *const GraphNode = if !((*link).source_qn).is_null() {
        cbm_gbuf_find_by_qn(c.g, (*link).source_qn)
    } else {
        ptr::null()
    };
    return (((!(n).is_null()) && (!((*n).file_path).is_null()))
        && (strcmp(((*n).file_path) as *const c_char, (*c.f).rel_path) == 0))
        && ((*n).start_line == (((*link).def_line) as i32));
}

// True when `line` lies in a part of the file the scan could not place.
unsafe fn unplaced_line(f: *const File, line: u32) -> bool {
    let mut lo: i32 = 0;
    let mut hi: i32 = (*f).nunplaced;
    while lo < hi {
        let mid: i32 = lo + ((hi - lo) / PAIR_LEN);
        if (*((*f).unplaced).offset(mid as isize)).to < line {
            lo = mid + SKIP_ONE;
        } else {
            hi = mid;
        }
    }
    return (lo < (*f).nunplaced)
        && ((*((*f).unplaced).offset(lo as isize)).from <= line);
}

unsafe extern "C" fn resolve(
    index: *const c_void,
    state: *mut c_void,
    run_file: i32,
    link: *const DocLink,
    graph: *const Graph,
    out: *mut Outcome,
) {
    let _ = state;
    // Java and Kotlin keep no per-file working state
    let ix: *const Index = index as *const Index;
    (*out).kind = CBM_DOCLINK_UNRESOLVED;
    (*out).reason = CBM_DOCLINK_REASON_MISSING;
    (*out).target = ptr::null();
    (*out).exact = false;
    if ((((ix.is_null()) || (link.is_null())) || (run_file < 0))
        || (run_file >= (*ix).run_count))
        || (*((*ix).run_to_file).offset(run_file as isize) < 0)
    {
        return;
    }
    let file: i32 = *((*ix).run_to_file).offset(run_file as isize);
    let f: *const File =
        (addr_of_mut!(*((*ix).files).offset(file as isize))) as *const File;
    if !((*f).scoped) {
        (*out).reason = CBM_DOCLINK_REASON_GRAPH_GAP;
        // no scope to resolve in: never a guess
        return;
    }
    let mut r: Reference = Reference::default();
    if (((*link).raw).is_null())
        || ((if (*f).kotlin {
            i32::from(parse_kotlin((*link).raw, addr_of_mut!(r)))
        } else {
            i32::from(parse_java((*link).raw, addr_of_mut!(r)))
        }) == 0)
    {
        (*out).reason = CBM_DOCLINK_REASON_UNPARSEABLE;
        return;
    }
    // What could not be placed is not resolved around: a definition in a part
    // of its file where the scan lost its place has no known scope, and a
    // name some file declares at an unknown place could be that declaration.
    // Both are declared-but-unplaced: graph gaps.
    let sample: bool = (((*link).syntax) as i32) == CBM_DOCLINK_KT_SAMPLE;
    let mut unplaced: bool =
        (((((*link).flags) as i32) & CBM_DOCLINK_FLAG_FILE) == 0)
            && (unplaced_line(f, (*link).def_line));
    {
        let mut i: i32 = 0;
        while (!(unplaced)) && (i < r.nsegs) {
            unplaced = (cbm_ht_get(
                ((*ix).quarantine) as *const HashTable,
                ((r.segs)[i as usize]) as *const c_char,
            ) != ptr::null_mut())
                || ((((*f).is_test) || (sample))
                    && (cbm_ht_get(
                        ((*ix).quarantine_test) as *const HashTable,
                        ((r.segs)[i as usize]) as *const c_char,
                    ) != ptr::null_mut()));

            i += 1;
        }
    }
    if unplaced {
        (*out).reason = CBM_DOCLINK_REASON_GRAPH_GAP;
        return;
    }
    let mut c: ResolveContext = ResolveContext::default();
    ctx_init(addr_of_mut!(c), ix, graph, file, link);
    if sample {
        c.prod = false;
        // samples live in test sources
        c.sample = true;
    }
    let res: Resolution = if (*f).kotlin {
        kotlin_resolve(&c, (addr_of_mut!(r)) as *const Reference)
    } else {
        java_resolve(
            &c,
            (addr_of_mut!(r)) as *const Reference,
            (((*link).syntax) as i32) == CBM_DOCLINK_JAVA_VALUE,
        )
    };
    // KDoc `[name]` is Markdown: a name that resolves to nothing is rendered
    // as the plain text it is, so it is no reference. A block tag (@see,
    // @throws, @sample) names its subject and keeps its row.
    let plain_text: bool = (((((*link).syntax) as i32)
        == CBM_DOCLINK_KT_KDOC_LINK)
        && (res.st == JR_UNRES))
        && (res.reason == CBM_DOCLINK_REASON_MISSING);
    if (res.st == JR_LOCAL) || (plain_text) {
        if plain_text {
            (*(addr_of_mut!((*(*ix).stats).plain_text)))
                .fetch_add(1 as i64, Ordering::Relaxed);
        }
        (*out).kind = CBM_DOCLINK_LOCAL;
        return;
    }
    if (res.st == JR_OK) && (!(res.node).is_null()) {
        if !(source_has_node(&c, link)) {
            // resolved, with nowhere to leave from: the documented definition
            // is the one without a node
            (*(addr_of_mut!((*(*ix).stats).source_gaps)))
                .fetch_add(1 as i64, Ordering::Relaxed);
            (*out).reason = CBM_DOCLINK_REASON_GRAPH_GAP;
            return;
        }
        (*out).kind = CBM_DOCLINK_EDGE;
        (*out).target = res.node;
        (*out).exact = res.exact;
        return;
    }
    (*out).reason = res.reason;
}

// Incremental comparison of persisted JVM scope records. A file's own
// imports ordinarily only change its own resolution. An import can also
// rebind a supertype or an extension receiver, which other files resolve
// through, so those changes require a global rebuild.

fn scope_records(scope: &[u8], imports: bool) -> impl Iterator<Item = &[u8]> {
    scope.split(|&byte| byte == b'\n').filter(move |line| {
        line.first().is_some_and(|&kind| (kind == b'I') == imports)
    })
}

fn record_field(line: &[u8], index: usize) -> Option<&[u8]> {
    line.split(|&byte| byte == b'\t').nth(index)
}

fn import_records(
    scope: &[u8],
) -> Result<Vec<&[u8]>, std::collections::TryReserveError> {
    let mut imports = Vec::new();
    imports.try_reserve_exact(scope_records(scope, true).count())?;
    imports.extend(scope_records(scope, true));
    Ok(imports)
}

fn changed_imports_rebind(stored: &[u8], fresh: &[u8]) -> Option<bool> {
    let stored_imports = import_records(stored).ok()?;
    let fresh_imports = import_records(fresh).ok()?;
    let common_start = stored_imports
        .iter()
        .zip(&fresh_imports)
        .take_while(|(old, new)| old == new)
        .count();
    let old_tail = &stored_imports[common_start..];
    let new_tail = &fresh_imports[common_start..];
    let common_end = old_tail
        .iter()
        .rev()
        .zip(new_tail.iter().rev())
        .take_while(|(old, new)| old == new)
        .count();
    let old_changed = &old_tail[..old_tail.len() - common_end];
    let new_changed = &new_tail[..new_tail.len() - common_end];
    let change_count = old_changed.len() + new_changed.len();
    if change_count == 0 {
        return Some(false);
    }

    let changed = old_changed.iter().chain(new_changed);
    // Wildcard imports can supply any name. Preserve the same conservative
    // limit when many import records change at once.
    let any_name = change_count > 16
        || changed
            .clone()
            .any(|line| matches!(record_field(line, 1), Some(b"w" | b"n")));
    Some(scope_records(fresh, false).any(|line| {
        let dependencies = match line[0] {
            b'T' => record_field(line, 6), // supertypes
            b'M' => record_field(line, 8), // extension receiver
            _ => None,
        };
        dependencies
            .into_iter()
            .flat_map(|names| names.split(|&byte| byte == b'|'))
            .any(|name| {
                let head =
                    name.split(|&byte| byte == b'.').next().unwrap_or_default();
                !head.is_empty()
                    && head != b"?"
                    && (any_name
                        || changed
                            .clone()
                            .any(|import| record_field(import, 2) == Some(head)))
            })
    }))
}

unsafe extern "C" fn scope_delta(
    stored: *const c_char,
    fresh: *const c_char,
    removed: RemovedName,
    userdata: *mut c_void,
) -> i32 {
    // The pipeline passes NUL-terminated scope records valid for this call.
    let stored = if stored.is_null() {
        &[]
    } else {
        CStr::from_ptr(stored).to_bytes()
    };
    let fresh = if fresh.is_null() {
        &[]
    } else {
        CStr::from_ptr(fresh).to_bytes()
    };
    let mut fresh_records = scope_records(fresh, false);
    let mut next_fresh = fresh_records.next();
    for old in scope_records(stored, false) {
        if next_fresh == Some(old) {
            next_fresh = fresh_records.next();
            continue;
        }
        if old[0] != b'M' {
            return CBM_DOCLINK_DELTA_GLOBAL;
        }
        let (Some(flags), Some(name)) =
            (record_field(old, 3), record_field(old, 5))
        else {
            return CBM_NOT_FOUND;
        };
        if name.is_empty() {
            return CBM_NOT_FOUND;
        }
        // Removing a record component changes constructor matching even
        // when references never mention the component's name.
        if flags.contains(&b'p') {
            return CBM_DOCLINK_DELTA_GLOBAL;
        }
        if !removed.is_some_and(|callback| {
            callback(userdata, name.as_ptr().cast(), name.len())
        }) {
            return CBM_NOT_FOUND;
        }
    }
    if next_fresh.is_some() {
        return CBM_DOCLINK_DELTA_GLOBAL;
    }
    match changed_imports_rebind(stored, fresh) {
        Some(true) => CBM_DOCLINK_DELTA_GLOBAL,
        Some(false) => CBM_DOCLINK_DELTA_LOCAL,
        None => CBM_NOT_FOUND,
    }
}
