use super::*;

const MAX_DEPTH: u32 = 16;

fn declare(ancestry: &mut Ancestry, name: &CStr, parent: Option<&CStr>) {
    ancestry.declare(name, b"", name, parent);
}

fn method(
    ancestry: &Ancestry,
    owner: &CStr,
    side: Side,
    search: Search,
    targets: &[(&CStr, &'static CStr)],
) -> Lookup<&'static CStr> {
    ancestry
        .lookup(owner, side, search, MAX_DEPTH, |key| {
            targets
                .iter()
                .find(|(qn, _)| *qn == key)
                .map(|(_, hit)| *hit)
        })
        .0
}

#[test]
fn instance_order_and_super() {
    let mut ancestry = Ancestry::default();
    declare(&mut ancestry, c"Widget", Some(c"Base"));
    for name in [c"Base", c"Included", c"First", c"Last"] {
        declare(&mut ancestry, name, None);
    }
    ancestry.mixin(c"Widget", MixinKind::Include, b"", c"Included");
    ancestry.mixin(c"Widget", MixinKind::Prepend, b"", c"First");
    ancestry.mixin(c"Widget", MixinKind::Prepend, b"", c"Last");
    ancestry.finalize(|_| None);
    let picks = [
        (c"Last", c"main.Last.save"),
        (c"First", c"main.First.save"),
        (c"Widget", c"main.Widget.save"),
        (c"Included", c"main.Included.save"),
        (c"Base", c"main.Base.save"),
    ];
    for start in 0..picks.len() {
        assert_eq!(
            method(
                &ancestry,
                c"Widget",
                Side::Instance,
                Search::Receiver,
                &picks[start..]
            ),
            Lookup::Hit(picks[start].1),
        );
    }
    assert_eq!(
        method(&ancestry, c"Widget", Side::Instance, Search::Super, &picks),
        Lookup::Hit(c"main.Included.save"),
    );
    assert_eq!(
        method(
            &ancestry,
            c"Widget",
            Side::Instance,
            Search::Super,
            &picks[4..]
        ),
        Lookup::Hit(c"main.Base.save"),
    );
}

#[test]
fn singleton_order_and_super() {
    let mut ancestry = Ancestry::default();
    declare(&mut ancestry, c"Reader", Some(c"Base"));
    for name in [c"Base", c"Lookup"] {
        declare(&mut ancestry, name, None);
    }
    ancestry.mixin(c"Reader", MixinKind::Extend, b"", c"Lookup");
    ancestry.finalize(|_| None);
    let picks = [
        (c"Reader.self", c"main.Reader.fetch"),
        (c"Lookup", c"main.Lookup.fetch"),
        (c"Base.self", c"main.Base.fetch"),
        (c"Reader", c"other.Reader.fetch"),
    ];
    for start in 0..3 {
        assert_eq!(
            method(
                &ancestry,
                c"Reader",
                Side::Singleton,
                Search::Receiver,
                &picks[start..]
            ),
            Lookup::Hit(picks[start].1),
        );
    }
    assert_eq!(
        method(&ancestry, c"Reader", Side::Singleton, Search::Super, &picks),
        Lookup::Hit(c"main.Lookup.fetch"),
    );
    assert_eq!(
        method(
            &ancestry,
            c"Reader",
            Side::Instance,
            Search::Receiver,
            &picks
        ),
        Lookup::Hit(c"other.Reader.fetch"),
    );
}

#[test]
fn unresolved_ancestor_blocks_only_later_probes_and_fallbacks() {
    let mut ancestry = Ancestry::default();
    declare(&mut ancestry, c"Incomplete", Some(c"UnknownParent"));
    declare(&mut ancestry, c"Visible", None);
    declare(&mut ancestry, c"Plain", None);
    declare(&mut ancestry, c"Prepended", None);
    ancestry.mixin(c"Prepended", MixinKind::Prepend, b"", c"UnknownPrepend");
    ancestry.mixin(c"Incomplete", MixinKind::Include, b"", c"UnknownInclude");
    ancestry.mixin(c"Incomplete", MixinKind::Include, b"", c"Visible");
    ancestry.mixin(c"Incomplete", MixinKind::Extend, b"", c"UnknownExtend");
    ancestry.finalize(|_| None);
    let picks = [
        (c"Visible", c"main.Visible.run"),
        (c"Object", c"Object.run"),
        (c"Prepended", c"main.Prepended.run"),
    ];
    assert_eq!(
        method(
            &ancestry,
            c"Prepended",
            Side::Instance,
            Search::Receiver,
            &picks
        ),
        Lookup::Blocked,
    );
    assert_eq!(
        method(
            &ancestry,
            c"Incomplete",
            Side::Instance,
            Search::Receiver,
            &picks
        ),
        Lookup::Hit(c"main.Visible.run"),
    );
    for search in [Search::Receiver, Search::SelfDispatch, Search::Super] {
        assert_eq!(
            method(
                &ancestry,
                c"Incomplete",
                Side::Instance,
                search,
                &picks[1..]
            ),
            Lookup::Blocked,
        );
        assert_eq!(
            method(&ancestry, c"Incomplete", Side::Singleton, search, &picks),
            Lookup::Blocked,
        );
        let expected = if search == Search::Receiver {
            Lookup::Hit(c"Object.run")
        } else {
            Lookup::Miss
        };
        assert_eq!(
            method(&ancestry, c"Plain", Side::Instance, search, &picks),
            expected
        );
    }
}

#[test]
fn compact_names_keep_extractor_qn_and_do_not_open_outer_scope() {
    let mut ancestry = Ancestry::default();
    ancestry.declare(c"main.M.A::B", b"M", c"A.B", Some(c"Base"));
    ancestry.declare(c"main.M.A.B", b"M.A", c"B", None);
    ancestry.declare(c"main.M.Base", b"M", c"Base", None);
    ancestry.declare(c"main.M.A.Hidden", b"M.A", c"Hidden", None);
    ancestry.declare(c"main.M.A::B.Inner", b"M.A::B", c"Inner", None);
    ancestry.finalize(|_| None);
    assert_eq!(
        constant_at(&ancestry.constants, b"M", c"A.B"),
        Some(c"main.M.A::B")
    );
    assert_eq!(
        constant_at(&ancestry.constants, b"M.A::B", c"Base"),
        Some(c"main.M.Base")
    );
    assert_eq!(
        constant_at(&ancestry.constants, b"M.A::B", c"Inner"),
        Some(c"main.M.A::B.Inner")
    );
    assert_eq!(constant_at(&ancestry.constants, b"M.A::B", c"Hidden"), None);
    assert_eq!(
        method(
            &ancestry,
            c"main.M.A::B",
            Side::Instance,
            Search::Receiver,
            &[(c"main.M.Base", c"main.M.Base.run")]
        ),
        Lookup::Hit(c"main.M.Base.run"),
    );
}

#[test]
fn builtin_ruby_and_rails_ancestry() {
    let mut ancestry = Ancestry::default();
    declare(&mut ancestry, c"User", Some(c"ApplicationRecord"));
    ancestry
        .finalize(|path| (path == c"ApplicationRecord").then(|| path.into()));
    assert!(ancestry.is_ar_model(c"User", MAX_DEPTH));
    assert!(!ancestry.is_ar_model(c"Other", MAX_DEPTH));
    assert_eq!(
        method(
            &ancestry,
            c"User",
            Side::Singleton,
            Search::Receiver,
            &[(c"ActiveRecord.Base.self", c"ActiveRecord.Base.find")]
        ),
        Lookup::Hit(c"ActiveRecord.Base.find"),
    );
}

#[test]
fn duplicate_mixin_paths_probe_each_module_once() {
    for side in [Side::Instance, Side::Singleton] {
        for count in [6, 12] {
            let mut ancestry = Ancestry::default();
            let names: Vec<_> = (0..count)
                .map(|i| CString::new(format!("M{i}")).unwrap())
                .collect();
            declare(&mut ancestry, c"Probe", None);
            for name in &names {
                declare(&mut ancestry, name, None);
            }
            let first = match side {
                Side::Instance => MixinKind::Include,
                Side::Singleton => MixinKind::Extend,
            };
            ancestry.mixin(c"Probe", first, b"", &names[0]);
            for pair in names.windows(2) {
                ancestry.mixin(&pair[0], MixinKind::Prepend, b"", &pair[1]);
                ancestry.mixin(&pair[0], MixinKind::Include, b"", &pair[1]);
            }
            // Also exercise a cycle through the first module.
            ancestry.mixin(
                &names[count - 1],
                MixinKind::Include,
                b"",
                &names[0],
            );
            ancestry.finalize(|_| None);
            let mut probes = 0;
            let (result, steps) = ancestry.lookup(
                c"Probe",
                side,
                Search::SelfDispatch,
                MAX_DEPTH,
                |_| {
                    probes += 1;
                    None::<()>
                },
            );
            assert_eq!(result, Lookup::Miss);
            assert_eq!(probes, count + 1);
            assert_eq!(steps, 2 * count as u64 + 1);
        }
    }
}
