use super::*;

static TEST_TYPES: [u8; 2] = [1, 2];

fn ty(index: usize) -> *const FieldType {
    (&TEST_TYPES[index] as *const u8).cast()
}

fn row(name: &CStr) -> RecordRow {
    RecordRow {
        identity: name.as_ptr().cast(),
        qualified_name: name.as_ptr(),
        alias_of: null(),
        bases: null(),
        field_names: null(),
        field_types: null(),
    }
}

struct Rows<'a>(&'a [RecordRow]);

struct TestRegistry<'a> {
    rows: &'a Rows<'a>,
    module: &'a CStr,
}

unsafe extern "C" fn exact(
    data: *const c_void,
    name: *const c_char,
) -> RecordRow {
    let rows = unsafe { (*data.cast::<TestRegistry>()).rows };
    rows.0
        .iter()
        .copied()
        .find(|row| unsafe { c_str(row.qualified_name) == c_str(name) })
        .unwrap_or_else(|| row(c"").without_identity())
}

unsafe extern "C" fn exact_long_in_module(
    data: *const c_void,
    name: *const c_char,
) -> RecordRow {
    let module = unsafe { (*data.cast::<TestRegistry>()).module };
    let name = unsafe { CStr::from_ptr(name) };
    let mut prefixed = module.to_bytes().to_vec();
    prefixed.push(b'.');
    prefixed.extend_from_slice(name.to_bytes_with_nul());
    unsafe { exact(data, prefixed.as_ptr().cast()) }
}

impl RecordRow {
    fn without_identity(mut self) -> Self {
        self.identity = null();
        self
    }
}

unsafe extern "C" fn visit_short(
    data: *const c_void,
    _: *const c_char,
    visit: VisitRecord,
    state: *mut c_void,
) {
    // Like the linear native iterator, offer every row. Rust must check the leaf.
    let rows = unsafe { (*data.cast::<TestRegistry>()).rows };
    for &row in rows.0 {
        if !unsafe { visit(state, row) } {
            break;
        }
    }
}

fn registry(source: &TestRegistry<'_>, includes: &[*const c_char]) -> Registry {
    Registry {
        data: (source as *const TestRegistry<'_>).cast(),
        exact,
        exact_long_in_module,
        visit_short,
        module: source.module.as_ptr(),
        includes: includes.as_ptr(),
        include_count: includes.len(),
    }
}

fn owner(
    registry: &Registry,
    name: &CStr,
    field: &CStr,
) -> *const RegisteredType {
    unsafe {
        mori_c_field_lookup(
            registry,
            name.as_ptr(),
            field.as_ptr(),
            std::ptr::null_mut(),
        )
    }
}

fn typed(registry: &Registry, name: &CStr, field: &CStr) -> *const FieldType {
    let mut ty = null();
    unsafe {
        mori_c_field_lookup(registry, name.as_ptr(), field.as_ptr(), &mut ty)
    };
    ty
}

#[test]
fn exact_receiver_owns_the_same_named_field_in_its_own_record() {
    // pipeline_c_member_access_binds_field_by_object_type / clsp_fix2_struct_field_access_simple
    let fields = [c"count".as_ptr(), null()];
    let alpha_types = [ty(0)];
    let beta_types = [ty(1)];
    let alpha = RecordRow {
        field_names: fields.as_ptr(),
        field_types: alpha_types.as_ptr(),
        ..row(c"repo.use.Alpha")
    };
    let beta = RecordRow {
        field_names: fields.as_ptr(),
        field_types: beta_types.as_ptr(),
        ..row(c"repo.use.Beta")
    };
    let rows = Rows(&[alpha, beta]);
    let source = TestRegistry {
        rows: &rows,
        module: c"repo.use",
    };
    let registry = registry(&source, &[]);
    assert_eq!(owner(&registry, c"Alpha", c"count"), alpha.identity);
    assert_eq!(typed(&registry, c"Alpha", c"count"), ty(0));
    assert_eq!(owner(&registry, c"repo.use.Beta", c"count"), beta.identity);
    assert_eq!(typed(&registry, c"repo.use.Beta", c"count"), ty(1));
    assert!(owner(&registry, c"Opaque", c"count").is_null());
    assert!(owner(&registry, c"Alpha", c"miss").is_null());
}

#[test]
fn nested_pointer_field_uses_header_record_instead_of_parameter_stub() {
    // pipeline_c_member_access_binds_field_by_object_type: arrow_chain / Ops.shut.
    let dev_fields = [c"ops".as_ptr(), null()];
    let dev_types = [ty(0)];
    let ops_fields = [c"shut".as_ptr(), null()];
    let ops_types = [ty(1)];
    let dev = RecordRow {
        field_names: dev_fields.as_ptr(),
        field_types: dev_types.as_ptr(),
        ..row(c"repo.ops.Dev")
    };
    let ops = RecordRow {
        field_names: ops_fields.as_ptr(),
        field_types: ops_types.as_ptr(),
        ..row(c"repo.ops.Ops")
    };
    let rows = Rows(&[row(c"repo.use.Dev"), row(c"repo.use.Ops"), dev, ops]);
    let source = TestRegistry {
        rows: &rows,
        module: c"repo.use",
    };
    let registry = registry(&source, &[c"repo.ops".as_ptr()]);
    assert_eq!(typed(&registry, c"repo.use.Dev", c"ops"), ty(0));
    assert_eq!(owner(&registry, c"repo.use.Ops", c"shut"), ops.identity);
    assert_eq!(typed(&registry, c"repo.use.Ops", c"shut"), ty(1));
}

#[test]
fn include_evidence_requires_a_unique_record_and_a_module_boundary() {
    let fields = [c"open".as_ptr(), null()];
    let first = RecordRow {
        field_names: fields.as_ptr(),
        ..row(c"repo.ops.Ops")
    };
    let second = RecordRow {
        field_names: fields.as_ptr(),
        ..row(c"repo.ops_other.Ops")
    };
    let stub = row(c"repo.use.Ops");
    let rows = Rows(&[stub, first, second]);
    let source = TestRegistry {
        rows: &rows,
        module: c"repo.use",
    };
    for includes in
        [&[][..], &[c"repo.ops".as_ptr(), c"repo.ops_other".as_ptr()]]
    {
        let registry = registry(&source, includes);
        assert!(owner(&registry, c"repo.use.Ops", c"open").is_null());
        assert!(registry.resolve_record(c"repo.use.Ops").is_none());
    }
    let registry = registry(&source, &[c"repo.ops".as_ptr()]);
    assert_eq!(owner(&registry, c"repo.use.Ops", c"open"), first.identity);
    assert_eq!(
        owner(&registry, c"repo.ops_other.Ops", c"open"),
        second.identity
    );
}

#[test]
fn aliases_and_bases_preserve_owner_and_type_as_distinct_answers() {
    // clsp_c_typedef_chain plus the field-owner contract for unparsed types.
    let fields = [c"unparsed".as_ptr(), c"count".as_ptr(), null()];
    let child_types = [null(), ty(0)];
    let base_types = [ty(1), ty(1)];
    let bases = [c"repo.Base".as_ptr(), null()];
    let base = RecordRow {
        field_names: fields.as_ptr(),
        field_types: base_types.as_ptr(),
        ..row(c"repo.Base")
    };
    let child = RecordRow {
        bases: bases.as_ptr(),
        field_names: fields.as_ptr(),
        field_types: child_types.as_ptr(),
        ..row(c"repo.Child")
    };
    let alias = RecordRow {
        alias_of: c"repo.Child".as_ptr(),
        ..row(c"repo.Alias")
    };
    let cycle = RecordRow {
        alias_of: c"repo.Cycle".as_ptr(),
        ..row(c"repo.Cycle")
    };
    let rows = Rows(&[base, child, alias, cycle]);
    let source = TestRegistry {
        rows: &rows,
        module: c"repo",
    };
    let registry = registry(&source, &[]);
    assert_eq!(owner(&registry, c"Alias", c"unparsed"), child.identity);
    assert_eq!(typed(&registry, c"Alias", c"unparsed"), ty(1));
    assert_eq!(typed(&registry, c"Alias", c"count"), ty(0));
    assert!(owner(&registry, c"Cycle", c"count").is_null());
}

#[test]
fn module_qualified_exact_lookup_accepts_long_and_non_utf8_names() {
    use std::ffi::CString;

    let module = CString::new(vec![0xff; 1024]).unwrap();
    let mut qualified = module.as_bytes().to_vec();
    qualified.extend_from_slice(b".Ops");
    let qualified = CString::new(qualified).unwrap();
    let fields = [c"count".as_ptr(), null()];
    let record = RecordRow {
        field_names: fields.as_ptr(),
        ..row(&qualified)
    };
    let rows = Rows(&[record]);
    let source = TestRegistry {
        rows: &rows,
        module: &module,
    };
    let mut registry = registry(&source, &[]);
    unsafe extern "C" fn empty(
        _: *const c_void,
        _: *const c_char,
        _: VisitRecord,
        _: *mut c_void,
    ) {
    }
    registry.visit_short = empty;
    assert_eq!(owner(&registry, c"Ops", c"count"), record.identity);
}

// The cases below come from clsp_field_lookup_rust_matches_c, the parity test
// that compared this lookup with the C implementation it replaced.

fn resolve(registry: &Registry, name: &CStr) -> *const RegisteredType {
    unsafe {
        mori_c_field_lookup(
            registry,
            name.as_ptr(),
            null(),
            std::ptr::null_mut(),
        )
    }
}

#[test]
fn alias_walk_stops_past_five_steps() {
    let names: Vec<_> = (0..8)
        .map(|i| std::ffi::CString::new(format!("repo.use.L{i}")).unwrap())
        .collect();
    let fields = [c"count".as_ptr(), null()];
    let types = [ty(0)];
    let rows: Vec<_> = names
        .iter()
        .enumerate()
        .map(|(i, name)| match names.get(i + 1) {
            Some(next) => RecordRow {
                alias_of: next.as_ptr(),
                ..row(name)
            },
            None => RecordRow {
                field_names: fields.as_ptr(),
                field_types: types.as_ptr(),
                ..row(name)
            },
        })
        .collect();
    let rows = Rows(&rows);
    let source = TestRegistry {
        rows: &rows,
        module: c"repo.use",
    };
    let registry = registry(&source, &[]);
    assert!(owner(&registry, c"L0", c"count").is_null());
    assert!(owner(&registry, c"L1", c"count").is_null());
    assert_eq!(owner(&registry, c"L2", c"count"), rows.0[7].identity);
    assert_eq!(typed(&registry, c"L2", c"count"), ty(0));
}

#[test]
fn an_untyped_field_defers_to_a_later_typed_entry_of_the_same_name() {
    let fields = [c"u".as_ptr(), c"t".as_ptr(), c"u".as_ptr(), null()];
    let types = [null(), ty(0), ty(1)];
    let mixed = RecordRow {
        field_names: fields.as_ptr(),
        field_types: types.as_ptr(),
        ..row(c"repo.use.Mixed")
    };
    let untyped = RecordRow {
        field_names: fields.as_ptr(),
        ..row(c"repo.use.Untyped")
    };
    let rows = Rows(&[mixed, untyped]);
    let source = TestRegistry {
        rows: &rows,
        module: c"repo.use",
    };
    let registry = registry(&source, &[]);
    assert_eq!(typed(&registry, c"Mixed", c"u"), ty(1));
    assert_eq!(typed(&registry, c"Mixed", c"t"), ty(0));
    assert_eq!(owner(&registry, c"Untyped", c"u"), untyped.identity);
    assert!(typed(&registry, c"Untyped", c"u").is_null());
}

#[test]
fn memberless_stubs_resolve_only_when_unique() {
    let rows = Rows(&[row(c"stub.Lone"), row(c"x.Ghost"), row(c"y.Ghost")]);
    let source = TestRegistry {
        rows: &rows,
        module: c"repo.use",
    };
    let registry = registry(&source, &[]);
    assert_eq!(resolve(&registry, c"Lone"), rows.0[0].identity);
    assert!(resolve(&registry, c"Ghost").is_null());
    assert!(owner(&registry, c"Lone", c"count").is_null());
}

#[test]
fn local_module_wins_and_an_empty_parent_is_not_a_candidate() {
    let fields = [c"x".as_ptr(), null()];
    let local = RecordRow {
        field_names: fields.as_ptr(),
        ..row(c"repo.use.Local")
    };
    let other = RecordRow {
        field_names: fields.as_ptr(),
        ..row(c"z.Local")
    };
    let edge = RecordRow {
        field_names: fields.as_ptr(),
        ..row(c".Edge")
    };
    let rows = Rows(&[other, local, edge]);
    let local_source = TestRegistry {
        rows: &rows,
        module: c"repo.use",
    };
    let scoped = registry(&local_source, &[]);
    assert_eq!(owner(&scoped, c"Local", c"x"), local.identity);
    assert!(owner(&scoped, c"Edge", c"x").is_null());
    assert_eq!(owner(&scoped, c".Edge", c"x"), edge.identity);
    let empty_source = TestRegistry {
        rows: &rows,
        module: c"",
    };
    let unscoped = registry(&empty_source, &[]);
    assert!(owner(&unscoped, c"Local", c"x").is_null());
    assert_eq!(owner(&unscoped, c"Edge", c"x"), edge.identity);
    let parent_source = TestRegistry {
        rows: &rows,
        module: c"repo",
    };
    let parent = registry(&parent_source, &[]);
    assert_eq!(owner(&parent, c"Local", c"x"), local.identity);
}
