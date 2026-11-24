use super::*;

// ── Kotlin ────────────────────────────────────────────────────────
// The packages every Kotlin file imports on demand.
const JR_KOTLIN_DEFAULT_PKGS: [*const c_char; 10] = [
    c"kotlin".as_ptr(),
    c"kotlin.annotation".as_ptr(),
    c"kotlin.collections".as_ptr(),
    c"kotlin.comparisons".as_ptr(),
    c"kotlin.io".as_ptr(),
    c"kotlin.ranges".as_ptr(),
    c"kotlin.sequences".as_ptr(),
    c"kotlin.text".as_ptr(),
    c"java.lang".as_ptr(),
    c"kotlin.jvm".as_ptr(),
];

// What one scope level holds under a name: types, properties, functions.
#[derive(Clone, Copy)]
struct KotlinLevel {
    types: Candidates,
    vals: Found,
    calls: Found,
    clash: bool,
    // two supertypes declare the name and neither comes first
}
impl Default for KotlinLevel {
    fn default() -> Self {
        unsafe { zeroed() }
    }
}

unsafe fn kl_any(lv: *const KotlinLevel) -> bool {
    return ((((*lv).types).count > 0) || (((*lv).vals).count > 0))
        || (((*lv).calls).count > 0);
}

unsafe fn kl_invisible(lv: *const KotlinLevel) -> bool {
    return ((((*lv).types).invisible) || (((*lv).vals).invisible))
        || (((*lv).calls).invisible);
}

// The members `name` of type `ent`, joined to the level.
unsafe fn kl_members(
    c: &ResolveContext,
    lv: *mut KotlinLevel,
    ent: i32,
    name: *const c_char,
) {
    let mut v: Found = members(c, ent, name, JR_SEL_VALUES, ptr::null());
    let mut k: Found = members(c, ent, name, JR_SEL_CALLABLES, ptr::null());
    found_add(
        c,
        addr_of_mut!((*lv).vals),
        (addr_of_mut!(v)) as *const Found,
        false,
    );
    found_add(
        c,
        addr_of_mut!((*lv).calls),
        (addr_of_mut!(k)) as *const Found,
        false,
    );
}

// The top-level declarations `name` of package `pkg`, joined to the level.
unsafe fn kl_top(
    c: &ResolveContext,
    lv: *mut KotlinLevel,
    pkg: *const c_char,
    name: *const c_char,
) {
    let mut v: Found = top_members(c, pkg, name, JR_SEL_VALUES);
    let mut k: Found = top_members(c, pkg, name, JR_SEL_CALLABLES);
    found_add(
        c,
        addr_of_mut!((*lv).vals),
        (addr_of_mut!(v)) as *const Found,
        false,
    );
    found_add(
        c,
        addr_of_mut!((*lv).calls),
        (addr_of_mut!(k)) as *const Found,
        false,
    );
}

// The type `name` nested in `ent`; through a type alias, in the type it
// names.
unsafe fn kotlin_nested(ix: *const Index, ent: i32, name: *const c_char) -> i32 {
    let mut n: i32 = nested(ix, ent, name);
    let e: *const Entity =
        (addr_of_mut!(*((*ix).ents).offset(ent as isize))) as *const Entity;
    if ((n == JR_NONE) && ((((*e).kind) as i32) == b't' as i32))
        && ((*e).nclosure > 0)
    {
        n = nested(ix, *((*e).closure).offset(0), name);
    }
    return n;
}

// What type `ent` holds under `name` itself: a nested type, its members, and
// the same of its companion object. One level: a class's member scope and its
// companion's are one scope to a KDoc name.
unsafe fn kl_type_scope(
    c: &ResolveContext,
    lv: *mut KotlinLevel,
    ent: i32,
    name: *const c_char,
    members: bool,
) {
    let ix: *const Index = c.ix;
    cand_type(c, addr_of_mut!((*lv).types), kotlin_nested(ix, ent, name));
    if members {
        kl_members(c, lv, ent, name);
    }
    let comp: i32 = (*((*ix).ents).offset(ent as isize)).companion;
    if comp != JR_NONE {
        cand_type(c, addr_of_mut!((*lv).types), nested(ix, comp, name));
        if members {
            kl_members(c, lv, comp, name);
        }
    }
}

// What the supertypes of `ent` hold under `name`: the nearest one that has
// anything (`types`: nested types, `members`: properties and functions). A
// further one that the nearest does not extend is a clash. *gap: a supertype
// with members a parse error hides came before any hit.
unsafe fn kl_inherited(
    c: &ResolveContext,
    lv: *mut KotlinLevel,
    ent: i32,
    name: *const c_char,
    types: bool,
    members: bool,
    gap: *mut bool,
) {
    let ix: *const Index = c.ix;
    let e: *const Entity =
        (addr_of_mut!(*((*ix).ents).offset(ent as isize))) as *const Entity;
    let mut first: i32 = JR_NONE;
    let mut checks: i32 = 0;
    let mut invisible: bool = false;
    for k in 0..(*e).nclosure {
        let sup: i32 = *((*e).closure).offset(k as isize);
        let mut one: KotlinLevel = KotlinLevel::default();
        clear_bytes(
            (addr_of_mut!(one)) as *mut c_void,
            0,
            size_of::<KotlinLevel>(),
        );
        if types {
            cand_type(c, addr_of_mut!(one.types), nested(ix, sup, name));
        }
        if members {
            kl_members(c, addr_of_mut!(one), sup, name);
        }
        if !(kl_any((addr_of_mut!(one)) as *const KotlinLevel)) {
            invisible = (invisible)
                || (kl_invisible((addr_of_mut!(one)) as *const KotlinLevel));
            if ((first == JR_NONE) && (members))
                && ((*((*ix).ents).offset(sup as isize)).incomplete)
            {
                *(gap) = true;
                break;
            }
            continue;
        }
        if first == JR_NONE {
            *(lv) = one;
            first = sup;
        } else if ({
            let value = addr_of_mut!(checks);
            let _previous = *value;
            *value += 1;
            *value
        } > JR_CLASH_CHECKS)
            || (!(is_or_extends(ix, first, sup)))
        {
            (*lv).clash = true;
            break;
        }
    }
    ((*lv).types).invisible = (((*lv).types).invisible) || (invisible);
}

// What an explicit import of `fqn` puts in scope: a type, the top-level
// declarations of that name, a member of an object or an enum. When no scope
// shows anything of that name it is either declared by the repository where
// no scope sees it (its package or type is the repository's), or outside.
unsafe fn kl_import(
    c: &ResolveContext,
    lv: *mut KotlinLevel,
    fqn: *const c_char,
    members: bool,
) {
    let ix: *const Index = c.ix;
    let dot: *const c_char = (strrchr(fqn, b'.' as i32)) as *const c_char;
    let last: *const c_char = if !(dot).is_null() {
        dot.offset(SKIP_ONE as isize)
    } else {
        fqn
    };
    let mut pkg: [c_char; KEY_BYTES] = zeroed();
    let has_pkg: bool = package_of(
        fqn,
        addr_of_mut!(pkg).cast::<c_char>(),
        size_of::<[c_char; KEY_BYTES]>(),
    );
    if !(has_pkg) {
        pkg[0] = b'\0' as c_char;
    }
    let mut one: KotlinLevel = KotlinLevel::default();
    clear_bytes(
        (addr_of_mut!(one)) as *mut c_void,
        0,
        size_of::<KotlinLevel>(),
    );
    cand_type(c, addr_of_mut!(one.types), find_entity(ix, fqn));
    kl_top(
        c,
        addr_of_mut!(one),
        (addr_of_mut!(pkg).cast::<c_char>()) as *const c_char,
        last,
    );
    let owner: i32 = if has_pkg {
        find_entity(ix, (addr_of_mut!(pkg).cast::<c_char>()) as *const c_char)
    } else {
        JR_NONE
    };
    if owner != JR_NONE {
        kl_members(c, addr_of_mut!(one), owner, last);
    }
    if !(kl_any((addr_of_mut!(one)) as *const KotlinLevel)) {
        if kl_invisible((addr_of_mut!(one)) as *const KotlinLevel) {
            ((*lv).types).invisible = true;
            return;
        }
        cand(
            addr_of_mut!((*lv).types),
            unseen_kind(
                ix,
                (addr_of_mut!(pkg).cast::<c_char>()) as *const c_char,
            ),
            JR_NONE,
        );
        return;
    }
    if (one.types).count > 0 {
        cand(addr_of_mut!((*lv).types), (one.types).kind, (one.types).ent);
    }
    if members {
        found_add(
            c,
            addr_of_mut!((*lv).vals),
            (addr_of_mut!(one.vals)) as *const Found,
            false,
        );
        found_add(
            c,
            addr_of_mut!((*lv).calls),
            (addr_of_mut!(one.calls)) as *const Found,
            false,
        );
    }
}

// True when the member scope of `ent` is not fully known: the type or its
// companion object has a supertype outside the repository, directly or
// further up.
unsafe fn scope_unknown(ix: *const Index, ent: i32) -> bool {
    let comp: i32 = (*((*ix).ents).offset(ent as isize)).companion;
    return (open(ix, ent)) || ((comp != JR_NONE) && (open(ix, comp)));
}

// Why a Kotlin lookup passed an enclosing type that may have the member
// unseen kotlin_lookup.
const JR_STOP_NO: i32 = 0;
const JR_STOP_GIVEN: i32 = 1;
const JR_STOP_OPEN: i32 = 2;

// A simple name in a Kotlin file, level by level:
//   the documented definition's own parameters and type parameters, `this`
//   each enclosing type, innermost first: what it holds itself (with its
//     companion), its type parameters and constructor parameters, what it
//     inherits, what its companion inherits
//   explicit imports -> the file's package -> star imports -> default imports
// `members` false: types only (the head of a qualified name, a supertype).
// *gap: a type with members a parse error hides was passed without a hit; a
// member found after it may be shadowed by one of those.
// *stop (JR_STOP_*): an enclosing type was passed without a hit that may have
// a member of the name unseen (kotlin_simple); what the lookup returns
// then lies further out than that type.
unsafe fn kotlin_lookup(
    c: &ResolveContext,
    name: *const c_char,
    members: bool,
    out: *mut KotlinLevel,
    exact: *mut bool,
    gap: *mut bool,
    stop: *mut i32,
) -> LookupKind {
    let ix: *const Index = c.ix;
    let f: *const File = c.f;
    let mut lv: KotlinLevel = KotlinLevel::default();
    let mut invisible: bool = false;
    clear_bytes(out as *mut c_void, 0, size_of::<KotlinLevel>());
    *(exact) = false;
    *(gap) = false;
    *(stop) = JR_STOP_NO;
    if (!(c.member).is_null())
        && ((listed((*c.member).tparams, name))
            || ((members) && (listed((*c.member).pnames, name))))
    {
        return LK_LOCAL;
    }
    // a documented class: every parameter of its primary constructor is its
    // own, a `val` one included (seen from a member's doc that one is a
    // property of the class, further down)
    if (!(c.type_index).is_null())
        && ((listed((*c.type_index).tparams, name))
            || ((members) && (listed((*c.type_index).pnames, name))))
    {
        return LK_LOCAL;
    }
    if (members) && (strcmp(name, c"this".as_ptr()) == 0) {
        return LK_LOCAL;
    }
    for i in 0..c.nchain {
        let ent: i32 = (c.chain)[i as usize];
        let decl: *const TypeDecl = (c.chain_decl)[i as usize];
        clear_bytes(
            (addr_of_mut!(lv)) as *mut c_void,
            0,
            size_of::<KotlinLevel>(),
        );
        if strcmp((*((*ix).ents).offset(ent as isize)).name, name) == 0 {
            cand_type(c, addr_of_mut!(lv.types), ent);
        }
        kl_type_scope(c, addr_of_mut!(lv), ent, name, members);
        if kl_any((addr_of_mut!(lv)) as *const KotlinLevel) {
            *(out) = lv;
            return LK_FOUND;
        }
        invisible = (invisible)
            || (kl_invisible((addr_of_mut!(lv)) as *const KotlinLevel));
        if (listed((*decl).tparams, name))
            || ((members) && (listed((*decl).pnames, name)))
        {
            return LK_LOCAL;
        }
        if (members) && ((*((*ix).ents).offset(ent as isize)).incomplete) {
            *(gap) = true;
        }
        if !(c.inherit) {
            continue;
        }
        clear_bytes(
            (addr_of_mut!(lv)) as *mut c_void,
            0,
            size_of::<KotlinLevel>(),
        );
        kl_inherited(c, addr_of_mut!(lv), ent, name, true, members, gap);
        if kl_any((addr_of_mut!(lv)) as *const KotlinLevel) {
            *(out) = lv;
            return LK_FOUND;
        }
        invisible = (invisible)
            || (kl_invisible((addr_of_mut!(lv)) as *const KotlinLevel));
        if !(members) {
            continue;
        }
        // the companion's member scope is part of the class's, with what the
        // companion inherits
        let comp: i32 = (*((*ix).ents).offset(ent as isize)).companion;
        if comp != JR_NONE {
            clear_bytes(
                (addr_of_mut!(lv)) as *mut c_void,
                0,
                size_of::<KotlinLevel>(),
            );
            kl_inherited(c, addr_of_mut!(lv), comp, name, false, true, gap);
            if kl_any((addr_of_mut!(lv)) as *const KotlinLevel) {
                *(out) = lv;
                return LK_FOUND;
            }
            invisible = (invisible)
                || (kl_invisible((addr_of_mut!(lv)) as *const KotlinLevel));
        }
        if *(stop) == JR_STOP_NO {
            *(stop) = if given_member(c, ent, name) {
                JR_STOP_GIVEN
            } else {
                if scope_unknown(ix, ent) {
                    JR_STOP_OPEN
                } else {
                    JR_STOP_NO
                }
            };
        }
    }
    // explicit imports
    clear_bytes(
        (addr_of_mut!(lv)) as *mut c_void,
        0,
        size_of::<KotlinLevel>(),
    );
    {
        let mut imp: *const Import = import_next(f, name, ptr::null());
        while !(imp).is_null() {
            kl_import(c, addr_of_mut!(lv), (*imp).target, members);

            imp = import_next(f, name, imp);
        }
    }
    if kl_any((addr_of_mut!(lv)) as *const KotlinLevel) {
        *(out) = lv;
        *(exact) = true;
        return LK_FOUND;
    }
    invisible =
        (invisible) || (kl_invisible((addr_of_mut!(lv)) as *const KotlinLevel));
    // the file's package
    clear_bytes(
        (addr_of_mut!(lv)) as *mut c_void,
        0,
        size_of::<KotlinLevel>(),
    );
    cand_type(c, addr_of_mut!(lv.types), pkg_type(ix, (*f).package, name));
    if members {
        kl_top(c, addr_of_mut!(lv), (*f).package, name);
    }
    if kl_any((addr_of_mut!(lv)) as *const KotlinLevel) {
        *(out) = lv;
        return LK_FOUND;
    }
    invisible =
        (invisible) || (kl_invisible((addr_of_mut!(lv)) as *const KotlinLevel));
    // star imports: of a package, or of a type (its nested types and members)
    clear_bytes(
        (addr_of_mut!(lv)) as *mut c_void,
        0,
        size_of::<KotlinLevel>(),
    );
    for i in 0..(*f).nwild {
        let target: *const c_char = (*((*f).wild).offset(i as isize)).target;
        if is_package(ix, target) {
            cand_type(c, addr_of_mut!(lv.types), pkg_type(ix, target, name));
            if members {
                kl_top(c, addr_of_mut!(lv), target, name);
            }
        }
        let type_index: i32 = find_entity(ix, target);
        if type_index != JR_NONE {
            cand_type(
                c,
                addr_of_mut!(lv.types),
                kotlin_nested(ix, type_index, name),
            );
            if members {
                kl_members(c, addr_of_mut!(lv), type_index, name);
            }
        }
    }
    if kl_any((addr_of_mut!(lv)) as *const KotlinLevel) {
        *(out) = lv;
        return LK_FOUND;
    }
    invisible =
        (invisible) || (kl_invisible((addr_of_mut!(lv)) as *const KotlinLevel));
    // default imports: the repository's own declarations there first
    clear_bytes(
        (addr_of_mut!(lv)) as *mut c_void,
        0,
        size_of::<KotlinLevel>(),
    );
    {
        let mut i: usize = 0;
        while i < (size_of::<[*const c_char; 10]>() / size_of::<*const c_char>())
        {
            cand_type(
                c,
                addr_of_mut!(lv.types),
                pkg_type(ix, JR_KOTLIN_DEFAULT_PKGS[i], name),
            );
            if members {
                kl_top(c, addr_of_mut!(lv), JR_KOTLIN_DEFAULT_PKGS[i], name);
            }

            i += 1;
        }
    }
    if (!(kl_any((addr_of_mut!(lv)) as *const KotlinLevel)))
        && ((named(&JR_KOTLIN_DEFAULT, name)) || (named(&JR_JAVA_LANG, name)))
    {
        cand(addr_of_mut!(lv.types), b'X' as c_char, JR_NONE);
    }
    if kl_any((addr_of_mut!(lv)) as *const KotlinLevel) {
        *(out) = lv;
        return LK_FOUND;
    }
    invisible =
        (invisible) || (kl_invisible((addr_of_mut!(lv)) as *const KotlinLevel));
    return if invisible { LK_INVISIBLE } else { LK_NONE };
}

// What a level with candidates is: one type, one property or one function.
// A type and a function of one name at one level (the factory-function
// idiom, `class Json` beside `fun Json(..)`) are the TYPE. The order relied
// on: KDoc's link resolution asks a scope for the classifier of a name before
// it asks for functions and properties, and the documentation tool links the
// first answer. That is stated from knowledge of the Kotlin documentation
// tooling (the compiler's KDoc link resolver, which Dokka uses); it was not
// checked online. The type binds with the tier the lookup gave. A type whose
// node another declaration took (class and factory in one file share the
// file-based qualified name, and the later one owns it) is a graph gap: the
// function never stands in for it. However many overloads the function has
// makes no difference. `@sample` names a function, so that order says nothing
// about it: there a type and a function of one name stay ambiguous.
// Everything else with more than one candidate is ambiguous, never a pick: a
// property and a function, a type and a property, a second type, an overload
// group, members of two owners.
unsafe fn kl_result(
    c: &ResolveContext,
    lv: *const KotlinLevel,
    exact: bool,
) -> Resolution {
    let kinds: i32 = ((i32::from(((*lv).types).count > 0))
        + (i32::from(((*lv).vals).count > 0)))
        + (i32::from(((*lv).calls).count > 0));
    if (((((!(c.sample)) && (((*lv).types).count == SKIP_ONE))
        && (((((*lv).types).kind) as i32) == b'T' as i32))
        && (((*lv).calls).count > 0))
        && (((*lv).vals).count == 0))
        && (!((*lv).clash))
    {
        let type_index: Resolution = type_result(c, ((*lv).types).ent, exact);
        (*(addr_of_mut!((*(*c.ix).stats).type_and_function)))
            .fetch_add(1 as i64, Ordering::Relaxed);
        if type_index.st == JR_OK {
            (*(addr_of_mut!((*(*c.ix).stats).type_and_function_node)))
                .fetch_add(1 as i64, Ordering::Relaxed);
        }
        return type_index;
    }
    if ((kinds > SKIP_ONE) || (((*lv).types).count > SKIP_ONE)) || ((*lv).clash)
    {
        return unres(CBM_DOCLINK_REASON_AMBIGUOUS);
    }
    if ((*lv).types).count == SKIP_ONE {
        if ((((*lv).types).kind) as i32) == b'T' as i32 {
            return type_result(c, ((*lv).types).ent, exact);
        }
        return unres(kind_reason(((*lv).types).kind));
    }
    let m: *const Found = if ((*lv).vals).count > 0 {
        addr_of!((*lv).vals)
    } else {
        addr_of!((*lv).calls)
    };
    return if (*m).several {
        unres(CBM_DOCLINK_REASON_AMBIGUOUS)
    } else {
        found_result(m, exact)
    };
}

// A simple KDoc name.
// A function or a property further out than an enclosing type binds only when
// that type is known not to have a member of the name. KDoc resolves a name
// in the member scope of each enclosing class, inherited members included,
// before it looks at the class around it, the file's imports or its package.
// So when the lookup passed a type
//   - that has the name without declaring it (given_member: a member of
//     kotlin.Any, of an enum class), or
//   - whose supertypes, or whose companion's, leave the repository
//     (scope_unknown): one of them may declare the name,
// a function or a property found further out is not taken: the reference is
// `external`, and so is a name found nowhere. The order relied on is stated
// from knowledge of the Kotlin documentation tooling (the compiler's KDoc
// link resolver); it was not checked online.
// A TYPE found further out binds as before: the guard is about the member
// functions and properties a type has, not about nested types a supertype
// outside the repository might lend.
unsafe fn kotlin_simple(c: &ResolveContext, name: *const c_char) -> Resolution {
    let mut lv: KotlinLevel = KotlinLevel::default();
    let mut exact: bool = false;
    let mut gap: bool = false;
    let mut stop: i32 = JR_STOP_NO;
    let lk: LookupKind = kotlin_lookup(
        c,
        name,
        true,
        addr_of_mut!(lv),
        addr_of_mut!(exact),
        addr_of_mut!(gap),
        addr_of_mut!(stop),
    );
    if (lk as u32) == (LK_LOCAL as u32) {
        return Resolution {
            st: JR_LOCAL,
            ..Resolution::default()
        };
    }
    if (lk as u32) == (LK_INVISIBLE as u32) {
        return unres(CBM_DOCLINK_REASON_TEST_ONLY);
    }
    if (lk as u32) == (LK_FOUND as u32) {
        // a member found past a type with hidden members: one of those may
        // be the one. A type is safe: a type declared where the scan lost its
        // place is quarantined by name.
        if (gap) && ((lv.types).count == 0) {
            return unres(CBM_DOCLINK_REASON_GRAPH_GAP);
        }
        if (stop != JR_STOP_NO) && ((lv.types).count == 0) {
            let st: *mut Stats = (*c.ix).stats;
            (*(if stop == JR_STOP_GIVEN {
                addr_of_mut!((*st).kotlin_given)
            } else {
                addr_of_mut!((*st).kotlin_open)
            }))
            .fetch_add(1 as i64, Ordering::Relaxed);
            return unres(CBM_DOCLINK_REASON_EXTERNAL);
        }
        return kl_result(c, (addr_of_mut!(lv)) as *const KotlinLevel, exact);
    }
    if stop != JR_STOP_NO {
        return unres(CBM_DOCLINK_REASON_EXTERNAL);
        // the member that type may have
    }
    let res: Resolution = unfound_simple(c, name);
    return if (gap) && (res.reason == CBM_DOCLINK_REASON_MISSING) {
        unres(CBM_DOCLINK_REASON_GRAPH_GAP)
    } else {
        res
    };
}

// A (qualified) type name in a Kotlin file: the head through the scope or,
// when no level has it, from the root as packages and then a type; the rest
// as nested types.
pub(super) unsafe fn kotlin_type_path(
    c: &ResolveContext,
    segs: *const *mut c_char,
    n: i32,
) -> TypePath {
    let ix: *const Index = c.ix;
    let mut lv: KotlinLevel = KotlinLevel::default();
    let mut exact: bool = false;
    let mut gap: bool = false;
    let mut stop: i32 = JR_STOP_NO;
    // types only: nothing to hold back
    let lk: LookupKind = kotlin_lookup(
        c,
        (*segs.offset(0)) as *const c_char,
        false,
        addr_of_mut!(lv),
        addr_of_mut!(exact),
        addr_of_mut!(gap),
        addr_of_mut!(stop),
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
    let mut cur: i32 = JR_NONE;
    let mut from: i32 = SKIP_ONE;
    if (lk as u32) == (LK_FOUND as u32) {
        if (lv.types).count > SKIP_ONE {
            return tp_res(unres(CBM_DOCLINK_REASON_AMBIGUOUS));
        }
        if (((lv.types).kind) as i32) != b'T' as i32 {
            return tp_res(unres(kind_reason((lv.types).kind)));
        }
        cur = (lv.types).ent;
    }
    {
        let mut i: i32 = SKIP_ONE;
        'iteration_3_loop: while (cur == JR_NONE) && (i < n) {
            'iteration_3: {
                let mut pkg: [c_char; KEY_BYTES] = zeroed();
                if !(join(
                    segs,
                    0,
                    i,
                    addr_of_mut!(pkg).cast::<c_char>(),
                    size_of::<[c_char; KEY_BYTES]>(),
                )) {
                    break 'iteration_3_loop;
                }
                let ent: i32 = pkg_type(
                    ix,
                    (addr_of_mut!(pkg).cast::<c_char>()) as *const c_char,
                    (*segs.offset(i as isize)) as *const c_char,
                );
                if ent == JR_NONE {
                    break 'iteration_3;
                }
                if !(visible(c, ent)) {
                    return tp_res(unres(CBM_DOCLINK_REASON_TEST_ONLY));
                }
                cur = ent;
                from = i + SKIP_ONE;
                exact = true;
            }
            i += 1;
        }
    }
    if cur == JR_NONE {
        let mut tp: TypePath = tp_res(unfound_path(c, segs, n));
        tp.unfound = true;
        return tp;
    }
    for i in from..n {
        let next: i32 =
            kotlin_nested(ix, cur, (*segs.offset(i as isize)) as *const c_char);
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

pub(super) unsafe fn ext_key(
    recv: i32,
    name: *const c_char,
    out: *mut c_char,
    cap: usize,
) -> bool {
    let number;
    let receiver: &[u8] = if recv == JR_NONE {
        b"?"
    } else {
        number = decimal(i64::from(recv));
        name_bytes(number.as_ptr().cast())
    };
    write_key(out, cap, &[receiver, b"\x1f", name_bytes(name)])
}

// True when the extension `m` of `file` is in scope of the source under its
// simple name: declared in the source's package, imported, or a member of an
// enclosing type. KDoc finds an extension for `Type.name` among the callables
// in scope, nowhere else.
unsafe fn ext_in_scope(c: &ResolveContext, file: i32, m: *const Member) -> bool {
    let xf: *const File =
        (addr_of_mut!(*((*c.ix).files).offset(file as isize))) as *const File;
    if (*m).owner != JR_NONE {
        let owner: i32 = (*((*xf).types).offset(((*m).owner) as isize)).entity;
        for i in 0..c.nchain {
            if (c.chain)[i as usize] == owner {
                return true;
            }
        }
        return false;
    }
    if strcmp((*xf).package, (*c.f).package) == 0 {
        return true;
    }
    for i in 0..(*c.f).nwild {
        if strcmp((*((*c.f).wild).offset(i as isize)).target, (*xf).package) == 0
        {
            return true;
        }
    }
    let pl: usize = strlen((*xf).package);
    {
        let mut imp: *const Import = import_next(c.f, (*m).name, ptr::null());
        while !(imp).is_null() {
            let t: *const c_char = (*imp).target;
            if (if pl == 0 {
                i32::from(strcmp(t, (*m).name) == 0)
            } else {
                i32::from(
                    ((strncmp(t, (*xf).package, pl) == 0)
                        && (((*t.offset(pl as isize)) as i32) == b'.' as i32))
                        && (strcmp(
                            (t.offset(pl as isize)).offset(SKIP_ONE as isize),
                            (*m).name,
                        ) == 0),
                )
            }) != 0
            {
                return true;
            }

            imp = import_next(c.f, (*m).name, imp);
        }
    }
    return false;
}

// The extensions `name` in scope whose receiver is `ent`, else one of its
// supertypes (nearest first), else any type. false when there is none.
unsafe fn kotlin_extension(
    c: &ResolveContext,
    ent: i32,
    name: *const c_char,
    res: *mut Resolution,
    invisible: *mut bool,
) -> bool {
    let ix: *const Index = c.ix;
    let e: *const Entity =
        (addr_of_mut!(*((*ix).ents).offset(ent as isize))) as *const Entity;
    for k in -1..=(*e).nclosure {
        let recv: i32 = if k < 0 {
            ent
        } else {
            if k < (*e).nclosure {
                *((*e).closure).offset(k as isize)
            } else {
                JR_NONE
            }
        };
        let mut key: [c_char; KEY_BYTES] = zeroed();
        if !(ext_key(
            recv,
            name,
            addr_of_mut!(key).cast::<c_char>(),
            size_of::<[c_char; KEY_BYTES]>(),
        )) {
            return false;
        }
        let l: *const IntList = (cbm_ht_get(
            ((*ix).extensions) as *const HashTable,
            (addr_of_mut!(key).cast::<c_char>()) as *const c_char,
        )) as *const IntList;
        let mut found: Found = Found::default();
        let mut package: *const c_char = ptr::null();
        {
            let mut i: i32 = 0;

            while (!(l).is_null()) && ((i + SKIP_ONE) < (*l).count) {
                'iteration_4: {
                    let file: i32 = *((*l).items).offset(i as isize);
                    let m: *const Member = (addr_of_mut!(
                        *((*((*ix).files).offset(file as isize)).members)
                            .offset(
                                (*((*l).items).offset((i + SKIP_ONE) as isize))
                                    as isize
                            )
                    ))
                        as *const Member;
                    if !(ext_in_scope(c, file, m)) {
                        break 'iteration_4;
                    }
                    if (c.prod)
                        && ((*((*ix).files).offset(file as isize)).is_test)
                    {
                        *(invisible) = true;
                        break 'iteration_4;
                    }
                    // the same name and signature in two packages are two functions
                    if (!(package).is_null())
                        && (strcmp(
                            package,
                            (*((*ix).files).offset(file as isize)).package,
                        ) != 0)
                    {
                        found.several = true;
                    }
                    package = (*((*ix).files).offset(file as isize)).package;
                    found_take(c, addr_of_mut!(found), file, m);
                }
                i += PAIR_LEN;
            }
        }
        if found.count > 0 {
            *(res) = if found.several {
                unres(CBM_DOCLINK_REASON_AMBIGUOUS)
            } else {
                found_result((addr_of_mut!(found)) as *const Found, false)
            };
            return true;
        }
    }
    return false;
}

// `name` in the type `ent`, named explicitly: what the type holds itself
// (with its companion), then what it inherits and what its companion
// inherits, the members every type has, and the extensions in scope whose
// receiver it is.
// An extension is taken only when the type is known not to have a member of
// the name: a member comes before an extension. Where the supertypes of the
// type, or of its companion, leave the repository (scope_unknown), one of
// them may declare the name: `external`, never the extension. Stated from
// knowledge of the language and of the KDoc link resolver, which asks the
// type's member scope before the extensions in scope; not checked online.
unsafe fn kotlin_in_type(
    c: &ResolveContext,
    ent: i32,
    name: *const c_char,
    exact: bool,
) -> Resolution {
    let ix: *const Index = c.ix;
    let mut lv: KotlinLevel = KotlinLevel::default();
    let mut gap: bool = false;
    clear_bytes(
        (addr_of_mut!(lv)) as *mut c_void,
        0,
        size_of::<KotlinLevel>(),
    );
    kl_type_scope(c, addr_of_mut!(lv), ent, name, true);
    if kl_any((addr_of_mut!(lv)) as *const KotlinLevel) {
        return kl_result(c, (addr_of_mut!(lv)) as *const KotlinLevel, exact);
    }
    let mut invisible: bool =
        kl_invisible((addr_of_mut!(lv)) as *const KotlinLevel);
    if (*((*ix).ents).offset(ent as isize)).incomplete {
        return unres(CBM_DOCLINK_REASON_GRAPH_GAP);
    }
    clear_bytes(
        (addr_of_mut!(lv)) as *mut c_void,
        0,
        size_of::<KotlinLevel>(),
    );
    kl_inherited(
        c,
        addr_of_mut!(lv),
        ent,
        name,
        true,
        true,
        addr_of_mut!(gap),
    );
    if kl_any((addr_of_mut!(lv)) as *const KotlinLevel) {
        return kl_result(c, (addr_of_mut!(lv)) as *const KotlinLevel, exact);
    }
    invisible =
        (invisible) || (kl_invisible((addr_of_mut!(lv)) as *const KotlinLevel));
    let comp: i32 = (*((*ix).ents).offset(ent as isize)).companion;
    if (comp != JR_NONE) && (!(gap)) {
        clear_bytes(
            (addr_of_mut!(lv)) as *mut c_void,
            0,
            size_of::<KotlinLevel>(),
        );
        kl_inherited(
            c,
            addr_of_mut!(lv),
            comp,
            name,
            false,
            true,
            addr_of_mut!(gap),
        );
        if kl_any((addr_of_mut!(lv)) as *const KotlinLevel) {
            return kl_result(
                c,
                (addr_of_mut!(lv)) as *const KotlinLevel,
                exact,
            );
        }
        invisible = (invisible)
            || (kl_invisible((addr_of_mut!(lv)) as *const KotlinLevel));
    }
    if gap {
        return unres(CBM_DOCLINK_REASON_GRAPH_GAP);
    }
    if implicit_member(ix, ent, name) {
        return unres(CBM_DOCLINK_REASON_EXTERNAL);
    }
    let mut ext: Resolution = Resolution::default();
    let has_ext: bool = kotlin_extension(
        c,
        ent,
        name,
        addr_of_mut!(ext),
        addr_of_mut!(invisible),
    );
    if scope_unknown(ix, ent) {
        if has_ext {
            (*(addr_of_mut!((*(*ix).stats).kotlin_extension)))
                .fetch_add(1 as i64, Ordering::Relaxed);
            return unres(CBM_DOCLINK_REASON_EXTERNAL);
        }
        return unres(if invisible {
            CBM_DOCLINK_REASON_TEST_ONLY
        } else {
            CBM_DOCLINK_REASON_EXTERNAL
        });
    }
    if has_ext {
        return ext;
    }
    return unres(if invisible {
        CBM_DOCLINK_REASON_TEST_ONLY
    } else {
        CBM_DOCLINK_REASON_MISSING
    });
}

// True when `name` is a parameter or type parameter of the documented
// definition or of a type around it.
unsafe fn kotlin_local(c: &ResolveContext, name: *const c_char) -> bool {
    if (!(c.member).is_null())
        && ((listed((*c.member).pnames, name))
            || (listed((*c.member).tparams, name)))
    {
        return true;
    }
    for i in 0..c.nchain {
        if (listed((*(c.chain_decl)[i as usize]).pnames, name))
            || (listed((*(c.chain_decl)[i as usize]).tparams, name))
        {
            return true;
        }
    }
    return strcmp(name, c"this".as_ptr()) == 0;
}

// A dotted KDoc name. The qualifier is a type (found through the scope, or
// from the root) and the last name what that type holds; or the qualifier is
// a package and the last name one of its types or top-level declarations.
unsafe fn kotlin_qualified(
    c: &ResolveContext,
    r: *const Reference,
) -> Resolution {
    let ix: *const Index = c.ix;
    let n: i32 = (*r).nsegs;
    let last: *const c_char =
        (((*r).segs)[(n - SKIP_ONE) as usize]) as *const c_char;
    if kotlin_local(c, (((*r).segs)[0]) as *const c_char) {
        return Resolution {
            st: JR_LOCAL,
            ..Resolution::default()
        };
        // `param.member` names nothing elsewhere
    }
    let qual: TypePath = kotlin_type_path(
        c,
        addr_of!((*r).segs).cast::<*mut c_char>(),
        n - SKIP_ONE,
    );
    if ((qual.st) as u32) == (TP_OK as u32) {
        return kotlin_in_type(c, qual.ent, last, qual.exact);
    }
    if ((qual.st) as u32) == (TP_PARTIAL as u32) {
        return unres(CBM_DOCLINK_REASON_MISSING);
        // a type without that nested type
    }
    if !(qual.unfound) {
        return qual.res;
        // the qualifier is known, and is no type of the repository
    }
    let mut pkg: [c_char; KEY_BYTES] = zeroed();
    if !(join(
        addr_of!((*r).segs).cast::<*mut c_char>(),
        0,
        n - SKIP_ONE,
        addr_of_mut!(pkg).cast::<c_char>(),
        size_of::<[c_char; KEY_BYTES]>(),
    )) {
        return unres(CBM_DOCLINK_REASON_UNPARSEABLE);
    }
    let mut lv: KotlinLevel = KotlinLevel::default();
    clear_bytes(
        (addr_of_mut!(lv)) as *mut c_void,
        0,
        size_of::<KotlinLevel>(),
    );
    cand_type(
        c,
        addr_of_mut!(lv.types),
        pkg_type(
            ix,
            (addr_of_mut!(pkg).cast::<c_char>()) as *const c_char,
            last,
        ),
    );
    kl_top(
        c,
        addr_of_mut!(lv),
        (addr_of_mut!(pkg).cast::<c_char>()) as *const c_char,
        last,
    );
    if kl_any((addr_of_mut!(lv)) as *const KotlinLevel) {
        return kl_result(c, (addr_of_mut!(lv)) as *const KotlinLevel, true);
    }
    if kl_invisible((addr_of_mut!(lv)) as *const KotlinLevel) {
        return unres(CBM_DOCLINK_REASON_TEST_ONLY);
    }
    return unfound_path(c, addr_of!((*r).segs).cast::<*mut c_char>(), n);
}

pub(super) unsafe fn kotlin_resolve(
    c: &ResolveContext,
    r: *const Reference,
) -> Resolution {
    return if (*r).nsegs == SKIP_ONE {
        kotlin_simple(c, (((*r).segs)[0]) as *const c_char)
    } else {
        kotlin_qualified(c, r)
    };
}
