use super::*;
use std::ffi::CString;

struct Type<'a> {
    key: &'a CStr,
    parents: &'a [&'a CStr],
    listed: &'a [(&'a CStr, Option<&'a CStr>)],
}

fn ty(key: &CStr) -> Type<'_> {
    Type {
        key,
        parents: &[],
        listed: &[],
    }
}

struct Fixtures<'a> {
    types: &'a [Type<'a>],
    methods: &'a [(&'a CStr, &'a CStr, &'a CStr)],
}

impl<'a> Registry<'a> for Fixtures<'a> {
    type Type = &'a Type<'a>;
    type Method = &'a CStr;

    fn exact_type(&self, qn: &CStr) -> Option<Self::Type> {
        self.types.iter().find(|ty| ty.key == qn)
    }

    fn joined_type(&self, prefix: &[u8], qn: &CStr) -> Option<Self::Type> {
        let mut key = prefix.to_vec();
        key.push(b'.');
        key.extend_from_slice(qn.to_bytes());
        self.exact_type(&CString::new(key).unwrap())
    }

    fn method(&self, receiver: &CStr, name: &CStr) -> Option<Self::Method> {
        self.methods.iter().find_map(|&(class, leaf, qn)| {
            (class == receiver && leaf == name).then_some(qn)
        })
    }

    fn function(&self, qn: &CStr) -> Option<Self::Method> {
        self.methods
            .iter()
            .find_map(|&(_, _, key)| (key == qn).then_some(key))
    }

    fn visit_short_types(&self, name: &CStr, visit: impl FnMut(Self::Type)) {
        self.types
            .iter()
            .filter(|ty| {
                ty.key.to_bytes().rsplit(|b| *b == b'.').next()
                    == Some(name.to_bytes())
            })
            .for_each(visit);
    }

    fn type_name(&self, ty: Self::Type) -> &'a CStr {
        ty.key
    }

    fn parents(&self, ty: Self::Type) -> impl Iterator<Item = &'a CStr> {
        ty.parents.iter().copied()
    }

    fn listed_methods(
        &self,
        ty: Self::Type,
    ) -> impl Iterator<Item = (&'a CStr, Option<&'a CStr>)> {
        ty.listed.iter().copied()
    }
}

fn resolver<'a>(
    types: &'a [Type<'a>],
    methods: &'a [(&'a CStr, &'a CStr, &'a CStr)],
    external_uses: &'a [&'a CStr],
) -> Resolver<'a, Fixtures<'a>, impl Iterator<Item = &'a CStr> + Clone> {
    Resolver {
        registry: Fixtures { types, methods },
        module: Some(c"p.app.Main"),
        external_uses: external_uses.iter().copied(),
    }
}

#[test]
fn same_leaf_methods_keep_distinct_typed_targets() {
    // phplsp_ordinary_same_leaf_calls_join_by_exact_site also exercises the
    // C AST walk and span join. This checks its two lookup results.
    let types = [ty(c"p.app.Main.Alpha"), ty(c"p.app.Main.Beta")];
    let methods = [
        (c"p.app.Main.Alpha", c"render", c"p.app.Main.Alpha.render"),
        (c"p.app.Main.Beta", c"render", c"p.app.Main.Beta.render"),
    ];
    let r = resolver(&types, &methods, &[]);
    assert_eq!(
        r.lookup_method(c"p.app.Main.Alpha", c"render"),
        Some(c"p.app.Main.Alpha.render")
    );
    assert_eq!(
        r.lookup_method(c"p.app.Main.Beta", c"render"),
        Some(c"p.app.Main.Beta.render")
    );
}

#[test]
fn child_override_and_interface_inheritance() {
    // phplsp_child_overrides_parent_method, phplsp_interface_inheritance.
    let types = [
        ty(c"p.app.Main.Base"),
        Type {
            parents: &[c"Base"],
            ..ty(c"p.app.Main.Ext")
        },
        Type {
            parents: &[c"Ext"],
            ..ty(c"p.app.Main.Child")
        },
    ];
    let methods = [
        (c"p.app.Main.Base", c"tag", c"p.app.Main.Base.tag"),
        (c"p.app.Main.Child", c"tag", c"p.app.Main.Child.tag"),
    ];
    let r = resolver(&types, &methods, &[]);
    assert_eq!(
        r.lookup_method(c"Ext", c"tag"),
        Some(c"p.app.Main.Base.tag")
    );
    assert_eq!(
        r.lookup_method(c"Child", c"tag"),
        Some(c"p.app.Main.Child.tag")
    );
}

#[test]
fn external_namespace_blocks_project_guess_but_keeps_seeded_types() {
    // phplsp_vendor_complete_empty_is_distinct_from_unknown.
    let types = [
        ty(c"p.lib.Request.Request"),
        ty(c"Vendor.Http.Request.Seeded"),
        ty(c"p.lib.RequestOther"),
    ];
    let methods = [(
        c"p.lib.Request.Request",
        c"validated",
        c"p.lib.Request.Request.validated",
    )];
    let external = [c"Vendor.Http.Request"];
    let r = resolver(&types, &methods, &external);
    assert!(
        r.lookup_method(c"Vendor.Http.Request", c"validated")
            .is_none()
    );
    assert!(r.is_external(c"Vendor.Http.Request"));
    assert!(r.is_external(c"Vendor.Http.Request.Child"));
    assert!(r.lookup_type(c"Vendor.Http.Request.Seeded").is_some());
    assert!(!r.is_external(c"Vendor.Http.RequestOther"));
    assert!(r.lookup_type(c"Vendor.Http.RequestOther").is_some());
    let legacy = resolver(&types, &methods, &[]);
    assert_eq!(
        legacy.lookup_method(c"Vendor.Http.Request", c"validated"),
        Some(c"p.lib.Request.Request.validated")
    );
}

#[test]
fn module_prefixes_precede_the_short_name_scan() {
    let types = [
        ty(c"p.lib.One.Request"),
        ty(c"p.app.Main.Request"),
        ty(c"p.Lib.Models.Request"),
        ty(c"p.app.Main.Nested.Request"),
    ];
    let r = resolver(&types, &[], &[]);
    assert_eq!(
        r.lookup_type(c"Lib.Models.Request").unwrap().key,
        c"p.Lib.Models.Request"
    );
    assert_eq!(
        r.lookup_type(c"Nested.Request").unwrap().key,
        c"p.app.Main.Nested.Request"
    );
    assert_eq!(
        r.lookup_type(c"Request").unwrap().key,
        c"p.app.Main.Request"
    );
}

#[test]
fn the_first_closest_short_name_match_wins() {
    let mut types = [ty(c"p.lib.One.Request"), ty(c"p.lib.Two.Request")];
    for _ in 0..2 {
        types.reverse();
        let first = types[0].key;
        let r = resolver(&types, &[], &[]);
        assert_eq!(r.lookup_type(c"Unknown.Request").unwrap().key, first);
    }
    let types = [
        ty(c"p.lib.One.Request"),
        ty(c"p.app.Near.Request"),
        ty(c"p.app.Main.Deep.Request"),
    ];
    let r = resolver(&types, &[], &[]);
    // Shared segments stop at the end of the module, so Deep ties with Near.
    assert_eq!(
        r.lookup_type(c"Unknown.Request").unwrap().key,
        c"p.app.Near.Request"
    );
    assert!(r.lookup_type(c"Unknown.").is_none());
    assert!(r.lookup_type(c"Unknown.Absent").is_none());
}

#[test]
fn cycles_end_and_other_ancestors_can_supply_a_listed_method() {
    let types = [
        Type {
            parents: &[c"Mixin", c"Cycle"],
            ..ty(c"p.app.Main.Child")
        },
        Type {
            parents: &[c"Child"],
            ..ty(c"p.app.Main.Cycle")
        },
        Type {
            listed: &[(c"tap", None), (c"tap", Some(c"lib.Provenance.tap"))],
            ..ty(c"p.app.Main.Mixin")
        },
    ];
    let methods = [(c"lib.Provenance", c"tap", c"lib.Provenance.tap")];
    let r = resolver(&types, &methods, &[]);
    assert_eq!(
        r.lookup_method(c"Child", c"tap"),
        Some(c"lib.Provenance.tap")
    );
    assert!(r.lookup_method(c"Child", c"absent").is_none());
}
