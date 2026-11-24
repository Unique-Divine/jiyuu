use super::*;

// ── Members across supertypes ─────────────────────────────────────
// How many supertypes that declare a name are compared with the nearest one.
// Past that the name is ambiguous: an unresolved outcome with its reason,
// never a pick.

#[derive(Clone, Copy)]
struct MemberSearch {
    found: Found,
    // the fields of the first type that has any
    ent: i32,
    // that type; JR_NONE when none has
    gap: bool,
    // a type with members a parse error hides was reached before
    // any was found: one of those may be the field
    open: bool,
    // nothing found, and a supertype outside the repository
    clash: bool,
    // a second type declares the name, and neither comes first
    invisible: bool,
}
impl Default for MemberSearch {
    fn default() -> Self {
        unsafe { zeroed() }
    }
}

// The fields `name` of `ent` (enum constants and record components among
// them): its own, else those of the nearest supertype that declares any. A
// second supertype with the name must be one the first extends (hidden);
// else neither comes first: a clash. Methods are javadoc's search walk.
unsafe fn fields_in(
    c: &ResolveContext,
    ent: i32,
    name: *const c_char,
    out: *mut MemberSearch,
) {
    let ix: *const Index = c.ix;
    let e: *const Entity =
        (addr_of_mut!(*((*ix).ents).offset(ent as isize))) as *const Entity;
    clear_bytes(out as *mut c_void, 0, size_of::<MemberSearch>());
    (*out).ent = JR_NONE;
    let own: Found = members(c, ent, name, JR_SEL_VALUES, ptr::null());
    (*out).invisible = own.invisible;
    if own.count > 0 {
        (*out).found = own;
        (*out).ent = ent;
        return;
    }
    if (*e).incomplete {
        (*out).gap = true;
        return;
    }
    let mut checks: i32 = 0;
    for k in 0..(*e).nclosure {
        let sup: i32 = *((*e).closure).offset(k as isize);
        let inh: Found = members(c, sup, name, JR_SEL_VALUES, ptr::null());
        (*out).invisible = ((*out).invisible) || (inh.invisible);
        if inh.count == 0 {
            if ((*out).ent == JR_NONE)
                && ((*((*ix).ents).offset(sup as isize)).incomplete)
            {
                (*out).gap = true;
                return;
            }
            continue;
        }
        if (*out).ent == JR_NONE {
            (*out).found = inh;
            (*out).ent = sup;
        } else if (!((*out).clash))
            && (({
                let value = addr_of_mut!(checks);
                let _previous = *value;
                *value += 1;
                *value
            } > JR_CLASH_CHECKS)
                || (!(is_or_extends(ix, (*out).ent, sup))))
        {
            (*out).clash = true;
        }
    }
    (*out).open = ((*out).ent == JR_NONE) && (open(ix, ent));
}

// True when java.lang.Object has a method `name`. With `r`, a reference that
// writes a parameter list: one with exactly that list.
// The fields `name` through the context's chain of types, innermost first: a
// type and what it inherits before the type around it. *level is the chain
// index of the hit. javadoc looks for a field in a type, then in the types
// around it, then in its supertypes; where the two orders find different
// fields, java_member says ambiguous.
unsafe fn fields_around(
    c: &ResolveContext,
    name: *const c_char,
    out: *mut MemberSearch,
    level: *mut i32,
) {
    let mut open: bool = false;
    let mut invisible: bool = false;
    clear_bytes(out as *mut c_void, 0, size_of::<MemberSearch>());
    (*out).ent = JR_NONE;
    *(level) = 0;
    for i in 0..c.nchain {
        let mut one: MemberSearch = MemberSearch::default();
        fields_in(c, (c.chain)[i as usize], name, addr_of_mut!(one));
        open = (open) || (one.open);
        invisible = (invisible) || (one.invisible);
        if (one.ent != JR_NONE) || (one.gap) {
            *(out) = one;
            *(level) = i;
            break;
        }
    }
    (*out).open = (((*out).ent == JR_NONE) && (!((*out).gap))) && (open);
    (*out).invisible = ((*out).invisible) || (invisible);
}

// ── Java: javadoc's search for a method ───────────────────────────
// javadoc links `#m` and `Type#m` to the first method its own search finds
// (JavacTrees.searchMethod; read in the JDK 21 source on 2026-10-04, stated
// here in our words). It is not javac's scoping. From a type it looks at
//   1. the type's own methods of the name; with a written parameter list,
//      one whose parameters match it;
//   2. the superclass, by this whole search: the superclass's own methods,
//      its superclass, its interfaces and the type around the SUPERCLASS
//      all come before step 3;
//   3. the interfaces, in the order written, each by this whole search;
//   4. the type around it, by this whole search;
// and at each type once. `#m` starts at the type of the documented
// definition, `Type#m` at the type named. The steps of each type come from
// its declarations walk_steps. java.lang.Object, java.lang.Enum and
// java.lang.annotation.Annotation are steps whose methods the language
// defines; an enum has values() and valueOf(String), a record equals,
// hashCode and toString without declaring them. A type outside the
// repository is a step the index cannot look into: a method there would be
// the one linked. These lists are the language's, written from knowledge of
// the Java SE API.
const JR_ENUM_OWN_METHODS: [*const c_char; 2] =
    [c"values".as_ptr(), c"valueOf".as_ptr()];

const JR_RECORD_OWN_METHODS: [*const c_char; 3] = [
    c"equals".as_ptr(),
    c"hashCode".as_ptr(),
    c"toString".as_ptr(),
];

const JR_ENUM_CLASS_METHODS: [*const c_char; 11] = [
    c"name".as_ptr(),
    c"ordinal".as_ptr(),
    c"compareTo".as_ptr(),
    c"getDeclaringClass".as_ptr(),
    c"describeConstable".as_ptr(),
    c"valueOf".as_ptr(),
    c"equals".as_ptr(),
    c"hashCode".as_ptr(),
    c"toString".as_ptr(),
    c"clone".as_ptr(),
    c"finalize".as_ptr(),
];

const JR_ANNOTATION_METHODS: [*const c_char; 4] = [
    c"annotationType".as_ptr(),
    c"equals".as_ptr(),
    c"hashCode".as_ptr(),
    c"toString".as_ptr(),
];

// How many types one search looks at, at most. A search that would look at
// more ends as ambiguous (WK_LIMIT): never cut short into a result.
const WALK_LIMIT: usize = 512;
const WALK_SLOTS: usize = 1024;

type WalkKind = i32;
const WK_NONE: WalkKind = 0;
const WK_FOUND: WalkKind = 1;
const WK_GIVEN: WalkKind = 2;
const WK_OUTSIDE: WalkKind = 3;
const WK_GAP: WalkKind = 4;
const WK_UNCOMPARED: WalkKind = 5;
const WK_LIMIT: WalkKind = 6;

#[derive(Clone, Copy)]
struct Walk {
    kind: WalkKind,
    // FOUND, GAP, UNCOMPARED: the type; OUTSIDE: the type whose supertype
    // lies outside; GIVEN: the type that has the method without declaring
    // it, or JR_W_OBJECT, JR_W_ENUM, JR_W_ANNOTATION
    ent: i32,
    found: Found,
    // FOUND
    invisible: bool,
    // a declaration only test code holds was passed over
    passed_outside: bool,
    // for a field: a step outside was passed (walk_ends)
    passed_gap: bool,
    // ... a type whose members a parse error hides
}
impl Default for Walk {
    fn default() -> Self {
        unsafe { zeroed() }
    }
}

#[derive(Clone, Copy)]
enum WalkStage {
    Own,
    Superclass,
    Interfaces,
    Done,
}

#[derive(Clone, Copy)]
struct WalkFrame {
    ent: i32,
    stage: WalkStage,
    next: usize,
}

impl WalkFrame {
    fn new(ent: i32) -> Self {
        Self {
            ent,
            stage: WalkStage::Own,
            next: 0,
        }
    }
}

// The table has space for every visited type plus the one which can exceed
// the walk limit. A linear probe therefore always finds an empty slot.
fn walk_mark(slots: &mut [i32; WALK_SLOTS], ent: i32) -> bool {
    let mask = slots.len() - 1;
    let mut slot = (ent as u32).wrapping_mul(2654435761) as usize & mask;
    let key = ent + 1;
    loop {
        match slots[slot] {
            0 => {
                slots[slot] = key;
                return true;
            }
            entry if entry == key => return false,
            _ => slot = (slot + 1) & mask,
        }
    }
}

unsafe fn walk_own(
    c: &ResolveContext,
    ent: i32,
    name: *const c_char,
    sel: i32,
    r: *const Reference,
    out: *mut Walk,
) -> bool {
    let e: *const Entity =
        (addr_of_mut!(*((*c.ix).ents).offset(ent as isize))) as *const Entity;
    let own: Found = members(c, ent, name, sel, r);
    (*out).invisible = ((*out).invisible) || (own.invisible);
    (*out).ent = ent;
    if own.count > 0 {
        (*out).kind = WK_FOUND;
        (*out).found = own;
        return true;
    }
    if own.uncompared {
        (*out).kind = WK_UNCOMPARED;
        return true;
    }
    if ((((*e).kind) as i32) == b'r' as i32)
        && (((sel & JR_SEL_SIG) == 0) || ((*r).nparams == 0))
    {
        let comp: Found = members(c, ent, name, JR_SEL_VALUES, ptr::null());
        if (comp.count > 0) && (comp.component) {
            // a component's accessor: the record's own method, without a node
            clear_bytes(
                (addr_of_mut!((*out).found)) as *mut c_void,
                0,
                size_of::<Found>(),
            );
            ((*out).found).count = SKIP_ONE;
            (*out).kind = WK_FOUND;
            return true;
        }
    }
    if (((((*e).kind) as i32) == b'e' as i32)
        && (named(&JR_ENUM_OWN_METHODS, name)))
        || (((((*e).kind) as i32) == b'r' as i32)
            && (named(&JR_RECORD_OWN_METHODS, name)))
    {
        (*out).kind = WK_GIVEN;
        return true;
    }
    if (*e).incomplete {
        (*out).kind = WK_GAP;
        return true;
    }
    (*out).ent = JR_NONE;
    false
}

// A step that is no type of the repository, taken from type `owner`. True
// when it ends the search, as *out says. With a written parameter list
// Object's methods are matched by it; an enum's and an annotation's by name.
unsafe fn walk_marker(
    step: i32,
    owner: i32,
    name: *const c_char,
    sel: i32,
    r: *const Reference,
    out: *mut Walk,
) -> bool {
    let sig: *const Reference = if (sel & JR_SEL_SIG) != 0 {
        r
    } else {
        ptr::null()
    };
    let mut given: i32 = JR_NONE;
    if step == JR_W_OUTSIDE {
        (*out).kind = WK_OUTSIDE;
        (*out).ent = owner;
        return true;
    }
    if (step == JR_W_OBJECT) && (object_method(name, sig)) {
        given = JR_W_OBJECT;
    } else if (step == JR_W_ENUM) && (named(&JR_ENUM_CLASS_METHODS, name)) {
        given = JR_W_ENUM;
    } else if (step == JR_W_ENUM) && (object_method(name, sig)) {
        given = JR_W_OBJECT;
    } else if (step == JR_W_ANNOTATION) && (named(&JR_ANNOTATION_METHODS, name))
    {
        given = JR_W_ANNOTATION;
    }
    if given == JR_NONE {
        return false;
    }
    (*out).kind = WK_GIVEN;
    (*out).ent = given;
    true
}

// Whether the search ends where *out says. A method is linked where it is
// found. For a field of class `cf` (java_field) a step outside the
// repository, or a type whose members a parse error hides, does not end it
// when `cf` is below the type the step belongs to: whatever method is there,
// the field comes first, so the search goes on to what may come after it.
// `peek`, for a count only, passes everything but a method of the
// repository.
unsafe fn walk_ends(
    ix: *const Index,
    out: *mut Walk,
    cf: i32,
    peek: bool,
) -> bool {
    let mut pass: bool = false;
    if peek {
        pass = (((*out).kind) as u32) != (WK_FOUND as u32);
    } else if ((cf != JR_NONE)
        && (((((*out).kind) as u32) == (WK_OUTSIDE as u32))
            || ((((*out).kind) as u32) == (WK_GAP as u32))))
        && (is_or_extends(ix, cf, (*out).ent))
    {
        pass = true;
        if (((*out).kind) as u32) == (WK_OUTSIDE as u32) {
            (*out).passed_outside = true;
        } else {
            (*out).passed_gap = true;
        }
    }
    if pass {
        (*out).kind = WK_NONE;
        (*out).ent = JR_NONE;
    }
    !(pass)
}

#[derive(Clone, Copy)]
struct MethodQuery {
    name: *const c_char,
    selection: i32,
    reference: *const Reference,
}

// Search own methods, the superclass, the interfaces in source order,
// and the enclosing type. Each step into a repository type repeats that
// order. The stack and visited table stay bounded even on cyclic input.
unsafe fn walk(
    c: &ResolveContext,
    start: i32,
    query: MethodQuery,
    cf: i32,
    peek: bool,
    out: *mut Walk,
) {
    let MethodQuery {
        name,
        selection: sel,
        reference: r,
    } = query;
    let ix = c.ix;
    let mut slots = [0; WALK_SLOTS];
    let mut stack = [WalkFrame::new(start); WALK_LIMIT];
    let mut marked = 1;
    let mut depth = 1;
    walk_mark(&mut slots, start);
    out.write(Walk {
        ent: JR_NONE,
        ..Walk::default()
    });

    while depth > 0 {
        let frame = &mut stack[depth - 1];
        let ent = frame.ent;
        let entity = &*(*ix).ents.add(ent as usize);
        let step = match frame.stage {
            WalkStage::Own => {
                frame.stage = WalkStage::Superclass;
                if walk_own(c, ent, name, sel, r, out)
                    && walk_ends(ix, out, cf, peek)
                {
                    return;
                }
                continue;
            }
            WalkStage::Superclass => {
                frame.stage = WalkStage::Interfaces;
                entity.wsuper
            }
            WalkStage::Interfaces if frame.next < entity.nwifaces as usize => {
                let step = *entity.wifaces.add(frame.next);
                frame.next += 1;
                step
            }
            WalkStage::Interfaces => {
                frame.stage = WalkStage::Done;
                entity.encl
            }
            WalkStage::Done => {
                depth -= 1;
                continue;
            }
        };
        if step < 0 {
            if walk_marker(step, ent, name, sel, r, out)
                && walk_ends(ix, out, cf, peek)
            {
                return;
            }
        } else if walk_mark(&mut slots, step) {
            if marked == WALK_LIMIT {
                (*out).kind = WK_LIMIT;
                (*out).ent = JR_NONE;
                return;
            }
            marked += 1;
            stack[depth] = WalkFrame::new(step);
            depth += 1;
        }
    }
}

// ── Java ──────────────────────────────────────────────────────────
// Closes one scope level: true when it has a candidate, which then decides
// the lookup.
unsafe fn level_done(
    lv: *mut Candidates,
    out: *mut Candidates,
    invisible: *mut bool,
) -> bool {
    if (*lv).count > 0 {
        *(out) = *(lv);
        return true;
    }
    *(invisible) = (*(invisible)) || ((*lv).invisible);
    clear_bytes(lv as *mut c_void, 0, size_of::<Candidates>());
    false
}

// True when the type `ent` has a declaration in file `file`.
unsafe fn declared_in(ix: *const Index, ent: i32, file: i32) -> bool {
    let e: *const Entity =
        (addr_of_mut!(*((*ix).ents).offset(ent as isize))) as *const Entity;
    for d in 0..(*e).ndecls {
        if (*((*e).decls).offset(d as isize)).file == file {
            return true;
        }
    }
    false
}

// The member types an on-demand import of `target` offers under `name`.
unsafe fn java_on_demand(
    c: &ResolveContext,
    lv: *mut Candidates,
    imp: *const Import,
    name: *const c_char,
) {
    let ix: *const Index = c.ix;
    if ((((*imp).kind) as i32) == b'w' as i32) && (is_package(ix, (*imp).target))
    {
        cand_type(c, lv, pkg_type(ix, (*imp).target, name));
    }
    let type_index: i32 = find_entity(ix, (*imp).target);
    // `import a.Outer.*;`, `import static a.B.*;`
    if type_index != JR_NONE {
        cand_type(c, lv, member_type(c, type_index, name));
    }
}

// A simple type name in a Java file, in javac's order.
unsafe fn java_type_lookup(
    c: &ResolveContext,
    name: *const c_char,
    out: *mut Candidates,
) -> LookupKind {
    let ix: *const Index = c.ix;
    let f: *const File = c.f;
    let mut lv: Candidates = Candidates {
        kind: (0 as c_char),
        ..Candidates::default()
    };
    let mut invisible: bool = false;
    clear_bytes(out as *mut c_void, 0, size_of::<Candidates>());
    if (!(c.member).is_null()) && (listed((*c.member).tparams, name)) {
        return LK_LOCAL;
        // the method's own type variable
    }
    // enclosing types, innermost first: the type and its member types, its
    // type variables, the member types it inherits
    for i in 0..c.nchain {
        let ent: i32 = (c.chain)[i as usize];
        if strcmp((*((*ix).ents).offset(ent as isize)).name, name) == 0 {
            cand_type(c, addr_of_mut!(lv), ent);
        }
        cand_type(c, addr_of_mut!(lv), nested(ix, ent, name));
        if level_done(addr_of_mut!(lv), out, addr_of_mut!(invisible)) {
            return LK_FOUND;
        }
        if listed((*(c.chain_decl)[i as usize]).tparams, name) {
            return LK_LOCAL;
        }
        let e: *const Entity =
            (addr_of_mut!(*((*ix).ents).offset(ent as isize))) as *const Entity;
        {
            let mut k: i32 = 0;
            while ((c.inherit) && (k < (*e).nclosure)) && (lv.count == 0) {
                cand_type(
                    c,
                    addr_of_mut!(lv),
                    nested(ix, *((*e).closure).offset(k as isize), name),
                );

                k += 1;
            }
        }
        if level_done(addr_of_mut!(lv), out, addr_of_mut!(invisible)) {
            return LK_FOUND;
        }
    }
    // the file's own top-level types and its single imports
    let same: i32 = pkg_type(ix, (*f).package, name);
    if (same != JR_NONE) && (declared_in(ix, same, c.file)) {
        cand_type(c, addr_of_mut!(lv), same);
    }
    {
        let mut imp: *const Import = import_next(f, name, ptr::null());

        while !(imp).is_null() {
            'iteration_2: {
                if (((*imp).kind) as i32) == b's' as i32 {
                    cand_import_type(c, addr_of_mut!(lv), (*imp).target);
                    break 'iteration_2;
                }
                // `import static a.B.name;` imports the member TYPE of that name too
                let owner: i32 = find_entity(ix, (*imp).target);
                if owner != JR_NONE {
                    cand_type(c, addr_of_mut!(lv), member_type(c, owner, name));
                } else {
                    // a member of a type no scope shows: whatever it is, it is not
                    // one of the repository's known declarations
                    let mut pkg: [c_char; KEY_BYTES] = zeroed();
                    if !(package_of(
                        (*imp).target,
                        addr_of_mut!(pkg).cast::<c_char>(),
                        size_of::<[c_char; KEY_BYTES]>(),
                    )) {
                        pkg[0] = b'\0' as c_char;
                    }
                    cand(
                        addr_of_mut!(lv),
                        unseen_kind(
                            ix,
                            (addr_of_mut!(pkg).cast::<c_char>())
                                as *const c_char,
                        ),
                        JR_NONE,
                    );
                }
            }
            imp = import_next(f, name, imp);
        }
    }
    if lv.count > 0 {
        lv.exact = true;
    }
    if level_done(addr_of_mut!(lv), out, addr_of_mut!(invisible)) {
        return LK_FOUND;
    }
    // the file's package
    cand_type(c, addr_of_mut!(lv), same);
    if level_done(addr_of_mut!(lv), out, addr_of_mut!(invisible)) {
        return LK_FOUND;
    }
    // on-demand imports; java.lang is one of them
    for i in 0..(*f).nwild {
        java_on_demand(
            c,
            addr_of_mut!(lv),
            (addr_of_mut!(*((*f).wild).offset(i as isize))) as *const Import,
            name,
        );
    }
    let lang: i32 = pkg_type(ix, c"java.lang".as_ptr(), name);
    if lang != JR_NONE {
        cand_type(c, addr_of_mut!(lv), lang);
    } else if named(&JR_JAVA_LANG, name) {
        cand(addr_of_mut!(lv), b'X' as c_char, JR_NONE);
    }
    if level_done(addr_of_mut!(lv), out, addr_of_mut!(invisible)) {
        return LK_FOUND;
    }
    if invisible { LK_INVISIBLE } else { LK_NONE }
}

// A (qualified) type name in a Java file: the head through the scope, the
// rest as member types; a head no scope level has is read from the root, as
// packages and then a type.
pub(super) unsafe fn java_type_path(
    c: &ResolveContext,
    segs: *const *mut c_char,
    n: i32,
) -> TypePath {
    let ix: *const Index = c.ix;
    let mut cands: Candidates = Candidates::default();
    let lk: LookupKind = java_type_lookup(
        c,
        (*segs.offset(0)) as *const c_char,
        addr_of_mut!(cands),
    );
    if (lk as u32) == (LK_LOCAL as u32) {
        return tp_res(Resolution {
            st: JR_LOCAL,
            ..Resolution::default()
        });
    }
    if (lk as u32) == (LK_INVISIBLE as u32) {
        return tp_res(unres(CBM_DOCLINK_REASON_TEST_ONLY));
    }
    if (lk as u32) == (LK_FOUND as u32) {
        if cands.count > SKIP_ONE {
            return tp_res(unres(CBM_DOCLINK_REASON_AMBIGUOUS));
        }
        if ((cands.kind) as i32) != b'T' as i32 {
            return tp_res(unres(kind_reason(cands.kind)));
        }
        return walk_nested(c, cands.ent, segs, SKIP_ONE, n, cands.exact);
    }
    for i in SKIP_ONE..n {
        let mut pkg: [c_char; KEY_BYTES] = zeroed();
        if !(join(
            segs,
            0,
            i,
            addr_of_mut!(pkg).cast::<c_char>(),
            size_of::<[c_char; KEY_BYTES]>(),
        )) {
            break;
        }
        let ent: i32 = pkg_type(
            ix,
            (addr_of_mut!(pkg).cast::<c_char>()) as *const c_char,
            (*segs.offset(i as isize)) as *const c_char,
        );
        if (ent == JR_NONE)
            || ((((*((*ix).ents).offset(ent as isize)).kind) as i32)
                == b't' as i32)
        {
            continue;
        }
        if !(visible(c, ent)) {
            return tp_res(unres(CBM_DOCLINK_REASON_TEST_ONLY));
        }
        return walk_nested(c, ent, segs, i + SKIP_ONE, n, true);
    }
    tp_res(unfound_path(c, segs, n))
}

// True when a record declaration of `ent` has exactly the written parameter
// list as its components.
unsafe fn record_canonical(
    c: &ResolveContext,
    ent: i32,
    r: *const Reference,
) -> bool {
    let ix: *const Index = c.ix;
    let e: *const Entity =
        (addr_of_mut!(*((*ix).ents).offset(ent as isize))) as *const Entity;
    for d in 0..(*e).ndecls {
        let f: *const File = (addr_of_mut!(
            *((*ix).files)
                .offset(((*((*e).decls).offset(d as isize)).file) as isize)
        )) as *const File;
        if (((((*((*f).types)
            .offset(((*((*e).decls).offset(d as isize)).type_index) as isize))
        .kind) as i32)
            == b'r' as i32)
            && (!((c.prod) && ((*f).is_test))))
            && (canonical_match(
                f,
                (*((*e).decls).offset(d as isize)).type_index,
                r,
            ))
        {
            return true;
        }
    }
    false
}

// `Type#Type(..)` and `#Type(..)`: the constructors of that type alone; none
// is inherited. A constructor the type has without declaring it (the default
// one, a record's canonical one) has no node.
unsafe fn java_ctor(
    c: &ResolveContext,
    ent: i32,
    r: *const Reference,
    exact: bool,
) -> Resolution {
    let e: *const Entity =
        (addr_of_mut!(*((*c.ix).ents).offset(ent as isize))) as *const Entity;
    let params: bool = (*r).has_params;
    let mut all: Found = members(c, ent, (*e).name, JR_SEL_CTORS, r);
    if (!(params)) && (all.count > 0) {
        return if all.several {
            unres(CBM_DOCLINK_REASON_AMBIGUOUS)
        } else {
            found_result((addr_of_mut!(all)) as *const Found, exact)
        };
    }
    if params {
        let mut sel: Found =
            members(c, ent, (*e).name, JR_SEL_CTORS | JR_SEL_SIG, r);
        if sel.count > 0 {
            return found_result((addr_of_mut!(sel)) as *const Found, exact);
        }
        if sel.uncompared {
            return unres(CBM_DOCLINK_REASON_AMBIGUOUS);
        }
    }
    if (*e).incomplete {
        return unres(CBM_DOCLINK_REASON_GRAPH_GAP);
    }
    if (all.count == 0) && (all.invisible) {
        return unres(CBM_DOCLINK_REASON_TEST_ONLY);
    }
    let has_ctors: bool = (((((*e).kind) as i32) == b'c' as i32)
        || ((((*e).kind) as i32) == b'e' as i32))
        || ((((*e).kind) as i32) == b'r' as i32);
    if ((has_ctors) && (params)) && (record_canonical(c, ent, r)) {
        return unres(CBM_DOCLINK_REASON_GRAPH_GAP);
    }
    if ((has_ctors) && (all.count == 0))
        && ((!(params))
            || (((*r).nparams == 0) && ((((*e).kind) as i32) != b'r' as i32)))
    {
        return unres(CBM_DOCLINK_REASON_GRAPH_GAP);
    }
    unres(CBM_DOCLINK_REASON_MISSING)
}

// A method reference's outcome from javadoc's search `w` from `base`; `vs`:
// the field search that came first, if any. A search that ended at a method
// javadoc would link that the repository does not show is external; when a
// method of the repository lies further on, the count says so.
unsafe fn java_method(
    c: &ResolveContext,
    base: i32,
    query: MethodQuery,
    w: *const Walk,
    vs: *const MemberSearch,
    exact: bool,
) -> Resolution {
    let MethodQuery {
        name,
        selection: sel,
        ..
    } = query;
    if (((*w).kind) as u32) == (WK_FOUND as u32) {
        // without a list, several signatures are an overload group
        return if ((sel & JR_SEL_SIG) == 0) && (((*w).found).several) {
            unres(CBM_DOCLINK_REASON_AMBIGUOUS)
        } else {
            found_result(addr_of!((*w).found), exact)
        };
    }
    if (((*w).kind) as u32) == (WK_GAP as u32) {
        return unres(CBM_DOCLINK_REASON_GRAPH_GAP);
    }
    if ((*w).invisible) || ((!(vs).is_null()) && ((*vs).invisible)) {
        return unres(CBM_DOCLINK_REASON_TEST_ONLY);
    }
    if ((((*w).kind) as u32) == (WK_UNCOMPARED as u32))
        || ((((*w).kind) as u32) == (WK_LIMIT as u32))
    {
        return unres(CBM_DOCLINK_REASON_AMBIGUOUS);
    }
    if ((((*w).kind) as u32) == (WK_GIVEN as u32))
        || ((((*w).kind) as u32) == (WK_OUTSIDE as u32))
    {
        let mut further: Walk = Walk::default();
        walk(c, base, query, JR_NONE, true, addr_of_mut!(further));
        if ((further.kind) as u32) == (WK_FOUND as u32) {
            let st: *mut Stats = (*c.ix).stats;
            (*(if (((*w).kind) as u32) == (WK_GIVEN as u32) {
                addr_of_mut!((*st).java_given)
            } else {
                addr_of_mut!((*st).java_open)
            }))
            .fetch_add(1_i64, Ordering::Relaxed);
        }
        return unres(CBM_DOCLINK_REASON_EXTERNAL);
    }
    if (implicit_member(c.ix, base, name)) || ((!(vs).is_null()) && ((*vs).open))
    {
        return unres(CBM_DOCLINK_REASON_EXTERNAL);
    }
    unres(CBM_DOCLINK_REASON_MISSING)
}

// A field `vs` that javadoc's field search found for a reference without a
// parameter list, weighed against javadoc's search for a METHOD of the name
// from `start` walk. javadoc takes the field when no method is found, or
// when the field's class is the method's class or below it
// (JavacTrees.attributeDocReference, read in the JDK 21 source on
// 2026-10-04). So the field binds only when that holds whatever the steps the
// index cannot see hold:
//   - a step outside the repository may hold the method; the field's class
//     must be below the type the step belongs to (then it is below the
//     outside type as well), and the search goes on past it;
//   - a method the language gives is weighed like a declared one: Object's
//     never stops the field; Enum's or Annotation's only an enum's or an
//     annotation type's; a record's or an enum's own implicit one, a field
//     of that type or below it;
//   - against a method of the repository the field binds when its class is
//     below the method's class. Otherwise, with nothing the index cannot see
//     before that method, javadoc links the method, and so does the reference
//     (the lead's ruling of 2026-10-04: a certain binding), with the tier of
//     a member the search finds; an overload group without a list stays
//     ambiguous, as for any method. If a step outside was passed, the method
//     there may be the one: external;
//   - a type whose members a parse error hides may hold the method: a graph
//     gap, unless the field's class is below it (then it is passed, and only
//     a field outcome survives it).
// *yielded: the field was not taken because of what lies outside.
unsafe fn java_field(
    c: &ResolveContext,
    start: i32,
    name: *const c_char,
    r: *const Reference,
    vs: *const MemberSearch,
    exact: bool,
    yielded: *mut bool,
) -> Resolution {
    let ix: *const Index = c.ix;
    let cf: i32 = (*vs).ent;
    let cf_kind: c_char = (*((*ix).ents).offset(cf as isize)).kind;
    let mut w: Walk = Walk::default();
    walk(
        c,
        start,
        MethodQuery {
            name,
            selection: JR_SEL_CALLABLES,
            reference: r,
        },
        cf,
        false,
        addr_of_mut!(w),
    );
    let mut field: bool = false;
    let mut reason: i32 = CBM_DOCLINK_REASON_EXTERNAL;
    match w.kind {
        WK_NONE => {
            field = true;
        }
        WK_FOUND => {
            field = is_or_extends(ix, cf, w.ent);
            if ((!(field)) && (!(w.passed_outside))) && (!(w.passed_gap)) {
                return if (w.found).several {
                    unres(CBM_DOCLINK_REASON_AMBIGUOUS)
                } else {
                    found_result((addr_of_mut!(w.found)) as *const Found, exact)
                };
            }
        }
        WK_GIVEN => {
            field = (((w.ent == JR_W_OBJECT)
                || ((w.ent == JR_W_ENUM) && ((cf_kind as i32) == b'e' as i32)))
                || ((w.ent == JR_W_ANNOTATION)
                    && ((cf_kind as i32) == b'a' as i32)))
                || ((w.ent >= 0) && (is_or_extends(ix, cf, w.ent)));
        }
        WK_OUTSIDE => {}
        WK_GAP => {
            reason = CBM_DOCLINK_REASON_GRAPH_GAP;
        }
        _ => {
            reason = CBM_DOCLINK_REASON_AMBIGUOUS;
        }
    }
    if field {
        return found_result(addr_of!((*vs).found), exact);
    }
    if w.passed_gap {
        reason = CBM_DOCLINK_REASON_GRAPH_GAP;
    }
    *(yielded) = reason == CBM_DOCLINK_REASON_EXTERNAL;
    unres(reason)
}

// The scope of `Type#x`: the type named and the types around it, innermost
// first. javadoc's field search goes through them as it does for `#x`.
unsafe fn ctx_named(
    tc: *mut ResolveContext,
    c: &ResolveContext,
    type_index: i32,
) {
    *(tc) = *(c);
    (*tc).nchain = 0;
    {
        let mut t: i32 = type_index;
        while (t != JR_NONE) && ((*tc).nchain < JR_CHAIN_MAX) {
            ((*tc).chain)[((*tc).nchain) as usize] = t;
            ((*tc).chain_decl)[((*tc).nchain) as usize] = ptr::null();
            (*tc).nchain += 1;

            t = (*((*c.ix).ents).offset(t as isize)).encl;
        }
    }
}

// A Javadoc member reference. `type` is the type it names, or JR_NONE for
// `#member`, which starts at the type of the documented definition.
//   - a METHOD is javadoc's search (walk): the first method it finds is the
//     one; a type outside the repository on the way, or a method the language
//     gives, ends it as external; a parameter list selects the overloads with
//     that signature, never another overload, and without one several
//     signatures are ambiguous
//   - without a list a FIELD comes first. javadoc looks for one in the type,
//     then in the types around it, then in the supertypes; javac scoping
//     takes the inherited one first: an inherited field binds only when no
//     type further out has one of that name (else ambiguous). The field is
//     then weighed against the method (java_field)
//   - {@value} names a field and nothing else; for `Type#x` it searches the
//     type and its supertypes only, as before
unsafe fn java_member(
    c: &ResolveContext,
    name: *const c_char,
    r: *const Reference,
    type_index: i32,
    params: bool,
    exact: bool,
    values_only: bool,
) -> Resolution {
    let ix: *const Index = c.ix;
    let in_scope: bool = type_index == JR_NONE;
    let base: i32 = if in_scope {
        if c.nchain > 0 { (c.chain)[0] } else { JR_NONE }
    } else {
        type_index
    };
    if base == JR_NONE {
        return unres(CBM_DOCLINK_REASON_MISSING);
        // no type to be a member of
    }
    if (!(values_only))
        && (strcmp(name, (*((*ix).ents).offset(base as isize)).name) == 0)
    {
        return java_ctor(c, base, r, exact);
    }
    let query = MethodQuery {
        name,
        selection: JR_SEL_CALLABLES | if params { JR_SEL_SIG } else { 0 },
        reference: r,
    };
    let mut mw: Walk = Walk::default();
    // the method, as javadoc searches for it
    clear_bytes((addr_of_mut!(mw)) as *mut c_void, 0, size_of::<Walk>());
    mw.ent = JR_NONE;
    if params {
        walk(c, base, query, JR_NONE, false, addr_of_mut!(mw));
        return java_method(
            c,
            base,
            query,
            (addr_of_mut!(mw)) as *const Walk,
            ptr::null(),
            exact,
        );
    }
    let mut named: ResolveContext = ResolveContext::default();
    let mut fc = c;
    // the chain the field is searched through
    if (!(in_scope)) && (!(values_only)) {
        ctx_named(addr_of_mut!(named), c, base);
        fc = &named;
    }
    let chain: bool = (in_scope) || (!(values_only));
    let mut vs: MemberSearch = MemberSearch::default();
    let mut vlevel: i32 = 0;
    if chain {
        fields_around(fc, name, addr_of_mut!(vs), addr_of_mut!(vlevel));
    } else {
        fields_in(c, base, name, addr_of_mut!(vs));
    }
    if vs.ent != JR_NONE {
        {
            let mut j: i32 = vlevel + SKIP_ONE;
            while (((chain) && (vs.ent != (fc.chain)[vlevel as usize]))
                && (!(vs.clash)))
                && (j < fc.nchain)
            {
                let mut outer: MemberSearch = MemberSearch::default();
                fields_in(fc, (fc.chain)[j as usize], name, addr_of_mut!(outer));
                vs.clash = (outer.ent != JR_NONE) || (outer.gap);

                j += 1;
            }
        }
        if vs.clash {
            return unres(CBM_DOCLINK_REASON_AMBIGUOUS);
        }
        if values_only {
            return found_result(
                (addr_of_mut!(vs.found)) as *const Found,
                exact,
            );
        }
        let mut yielded: bool = false;
        let res: Resolution = java_field(
            c,
            base,
            name,
            r,
            (addr_of_mut!(vs)) as *const MemberSearch,
            exact,
            addr_of_mut!(yielded),
        );
        if yielded {
            (*(if in_scope {
                addr_of_mut!((*(*ix).stats).java_field)
            } else {
                addr_of_mut!((*(*ix).stats).java_field_named)
            }))
            .fetch_add(1_i64, Ordering::Relaxed);
        }
        return res;
    }
    if vs.gap {
        return unres(CBM_DOCLINK_REASON_GRAPH_GAP);
        // a hidden field would come first
    }
    if !(values_only) {
        walk(c, base, query, JR_NONE, false, addr_of_mut!(mw));
    }
    java_method(
        c,
        base,
        query,
        (addr_of_mut!(mw)) as *const Walk,
        (addr_of_mut!(vs)) as *const MemberSearch,
        exact,
    )
}

pub(super) unsafe fn java_resolve(
    c: &ResolveContext,
    r: *const Reference,
    values_only: bool,
) -> Resolution {
    if (*r).nsegs == 0 {
        return java_member(
            c,
            ((*r).member) as *const c_char,
            r,
            JR_NONE,
            (*r).has_params,
            false,
            values_only,
        );
    }
    let tp: TypePath =
        java_type_path(c, addr_of!((*r).segs).cast::<*mut c_char>(), (*r).nsegs);
    if !((*r).member).is_null() {
        if ((tp.st) as u32) == (TP_OK as u32) {
            return java_member(
                c,
                ((*r).member) as *const c_char,
                r,
                tp.ent,
                (*r).has_params,
                tp.exact,
                values_only,
            );
        }
        // a member type the named type does not have: javadoc finds no type
        return if ((tp.st) as u32) == (TP_PARTIAL as u32) {
            unres(CBM_DOCLINK_REASON_MISSING)
        } else {
            tp.res
        };
    }
    if ((tp.st) as u32) == (TP_OK as u32) {
        return if values_only {
            unres(CBM_DOCLINK_REASON_MISSING)
        } else {
            type_result(c, tp.ent, tp.exact)
        };
    }
    if ((tp.st) as u32) == (TP_PARTIAL as u32) {
        return unres(CBM_DOCLINK_REASON_MISSING);
        // `Type.member`: Javadoc wants a '#'
    }
    if (((*r).nsegs == SKIP_ONE) && ((tp.res).st == JR_UNRES))
        && ((tp.res).reason == CBM_DOCLINK_REASON_MISSING)
    {
        // javadoc's own fixup: a bare identifier that is no type is read as
        // `#identifier`, a member in the enclosing types. Only where the scope
        // is closed: an open one may hold a type of that name, which comes
        // first.
        let m: Resolution = java_member(
            c,
            (((*r).segs)[0]) as *const c_char,
            r,
            JR_NONE,
            false,
            false,
            values_only,
        );
        if !((m.st == JR_UNRES) && (m.reason == CBM_DOCLINK_REASON_MISSING)) {
            return m;
        }
    }
    tp.res
}
