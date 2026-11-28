//! Resolve ordered Python module imports into a cross-file export table.

use std::collections::{BTreeMap, HashMap};
use std::ffi::CStr;

const MAX_DEPTH: usize = 64;

type Name<'a> = &'a [u8];
type Target<'a> = &'a CStr;

#[derive(Clone, Copy)]
struct Definition<'a> {
    module: Name<'a>,
    scope: Name<'a>,
    qualified: Target<'a>,
    name: Option<Name<'a>>,
    source: Option<Name<'a>>,
    member: Option<Name<'a>>,
    kind: Kind,
}

#[derive(Clone, Copy)]
enum Kind {
    Definition,
    Reexport,
    Star,
    All,
}

#[derive(Clone, Copy)]
struct Binding<'a> {
    target: Option<Target<'a>>,
    own: bool,
}

#[derive(Default)]
struct Module<'a> {
    records: Vec<Definition<'a>>,
    names: BTreeMap<Name<'a>, Binding<'a>>,
    all: Option<Name<'a>>,
    star_source: bool,
    visited: bool,
}

impl<'a> Module<'a> {
    fn bind_import(&mut self, name: Name<'a>, target: Option<Target<'a>>) {
        let slot = self.names.entry(name).or_insert(Binding {
            target: None,
            own: false,
        });
        if !slot.own {
            slot.target = target;
        }
    }

    fn star(&mut self, source: &Self) {
        let mut copy = |name, binding: &Binding<'a>| {
            if binding.target.is_some() {
                self.bind_import(name, binding.target);
            }
        };
        if let Some(all) = source.all {
            for name in all.split(|byte| *byte == b'|') {
                if let Some(binding) = source.names.get(name) {
                    copy(name, binding);
                }
            }
        } else {
            for (&name, binding) in &source.names {
                if name.first() != Some(&b'_') {
                    copy(name, binding);
                }
            }
        }
    }

    fn barrier(&mut self) {
        for binding in self.names.values_mut() {
            if !binding.own {
                binding.target = None;
            }
        }
    }

    fn owns(def: &Definition<'_>, name: &[u8]) -> bool {
        def.source.is_none()
            && def
                .qualified
                .to_bytes()
                .strip_prefix(def.scope)
                .and_then(|rest| rest.strip_prefix(b"."))
                == Some(name)
    }
}

struct Namespaces<'a> {
    scopes: Vec<Name<'a>>,
    modules: Vec<Module<'a>>,
    suffixes: HashMap<Name<'a>, Option<usize>>,
}

fn parent(name: &[u8]) -> Option<&[u8]> {
    let dot = name.iter().rposition(|&byte| byte == b'.')?;
    (dot > 0).then(|| &name[..dot])
}

fn qualified(scope: &[u8], name: &[u8]) -> Vec<u8> {
    [scope, b".", name].concat()
}

impl<'a> Namespaces<'a> {
    fn build(
        defs: &[Definition<'a>],
        exact: &impl Fn(&CStr) -> Option<Target<'a>>,
    ) -> Self {
        let mut table = Self::new(defs);
        table.prepare(defs, exact);
        for module in 0..table.modules.len() {
            table.compute(module, 0, exact);
        }
        table
    }

    fn new(defs: &[Definition<'a>]) -> Self {
        let mut scopes: Vec<_> = defs.iter().map(|def| def.scope).collect();
        scopes.sort_unstable();
        scopes.dedup();
        let mut suffixes = HashMap::new();
        for (index, &scope) in scopes.iter().enumerate() {
            for offset in std::iter::once(0).chain(
                scope
                    .iter()
                    .enumerate()
                    .filter_map(|(i, &byte)| (byte == b'.').then_some(i + 1)),
            ) {
                suffixes
                    .entry(&scope[offset..])
                    .and_modify(|module| *module = None)
                    .or_insert(Some(index));
            }
        }
        let modules = (0..scopes.len()).map(|_| Module::default()).collect();
        Self {
            scopes,
            modules,
            suffixes,
        }
    }

    fn index(&self, scope: &[u8]) -> Option<usize> {
        self.scopes.binary_search(&scope).ok()
    }

    fn source(&self, record: Definition<'a>) -> Option<usize> {
        let spec = record.source?;
        let level = spec.iter().take_while(|&&byte| byte == b'.').count();
        if level == 0 {
            return self.suffixes.get(spec).copied().flatten();
        }
        let mut package = if record.module == record.scope {
            parent(record.scope)?
        } else {
            record.scope
        };
        for _ in 1..level {
            package = parent(package)?;
        }
        let rest = &spec[level..];
        if rest.is_empty() {
            self.index(package)
        } else {
            self.index(&qualified(package, rest))
        }
    }

    fn prepare(
        &mut self,
        defs: &[Definition<'a>],
        exact: &impl Fn(&CStr) -> Option<Target<'a>>,
    ) {
        for &def in defs {
            let module = self.index(def.scope).expect("definition scope");
            match def.kind {
                Kind::Definition => {}
                Kind::All => {
                    self.modules[module].all = Some(def.member.unwrap_or(b""));
                }
                Kind::Reexport | Kind::Star => {
                    self.modules[module].records.push(def);
                    if matches!(def.kind, Kind::Star)
                        && let Some(source) = self.source(def)
                    {
                        self.modules[source].star_source = true;
                    }
                }
            }
        }

        for def in defs {
            let module = self.index(def.scope).expect("definition scope");
            if matches!(def.kind, Kind::Definition)
                && self.modules[module].star_source
                && let Some(name) = def.name
                && Module::owns(def, name)
                && let Some(target) = exact(def.qualified)
            {
                self.modules[module].names.entry(name).or_insert(Binding {
                    target: Some(target),
                    own: true,
                });
            }
        }
    }

    fn resolve(
        &mut self,
        source: usize,
        name: Name<'a>,
        depth: usize,
        exact: &impl Fn(&CStr) -> Option<Target<'a>>,
    ) -> Option<Target<'a>> {
        let mut qn = qualified(self.scopes[source], name);
        qn.push(0);
        let qn = CStr::from_bytes_with_nul(&qn).ok()?;
        if let Some(target) = exact(qn) {
            return Some(target);
        }
        // An exact registry entry is usable even at the traversal limit.
        if depth > MAX_DEPTH {
            return None;
        }
        let spelling = qn.to_bytes();
        let scope = parent(spelling)?;
        let source = self.index(scope)?;
        let name = &spelling[scope.len() + 1..];
        self.compute(source, depth + 1, exact);
        self.modules[source].names.get(name)?.target
    }

    fn compute(
        &mut self,
        module: usize,
        depth: usize,
        exact: &impl Fn(&CStr) -> Option<Target<'a>>,
    ) {
        if self.modules[module].visited || depth > MAX_DEPTH {
            return;
        }
        // A cycle sees the names bound so far, in module and source order.
        self.modules[module].visited = true;
        for index in 0..self.modules[module].records.len() {
            let record = self.modules[module].records[index];
            if record.source.is_none() {
                continue;
            }
            let source = self.source(record);
            if matches!(record.kind, Kind::Reexport) {
                let target =
                    source.zip(record.member).and_then(|(src, name)| {
                        self.resolve(src, name, depth + 1, exact)
                    });
                if let Some(name) = record.name {
                    self.modules[module].bind_import(name, target);
                }
                continue;
            }
            let Some(source) = source else {
                self.modules[module].barrier();
                continue;
            };
            if source != module {
                self.compute(source, depth + 1, exact);
                let (left, right) =
                    self.modules.split_at_mut(source.max(module));
                if source < module {
                    right[0].star(&left[source]);
                } else {
                    left[module].star(&right[0]);
                }
            }
        }
    }

    fn exports(&self) -> Vec<(Vec<u8>, Target<'a>)> {
        let mut rows = Vec::new();
        for (&scope, module) in self.scopes.iter().zip(&self.modules) {
            for (&name, binding) in &module.names {
                if !binding.own
                    && let Some(target) = binding.target
                {
                    rows.push((qualified(scope, name), target));
                }
            }
        }
        rows.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        rows
    }
}

#[cfg(feature = "native-engine-abi")]
mod abi {
    use super::*;
    use std::ffi::{c_char, c_void};

    #[repr(C)]
    struct Row {
        module: *const c_char,
        scope_len: usize,
        qualified: *const c_char,
        name: *const c_char,
        source: *const c_char,
        member: *const c_char,
        label: *const c_char,
    }

    #[repr(C)]
    struct Bytes {
        data: *const u8,
        len: usize,
    }

    impl From<&[u8]> for Bytes {
        fn from(bytes: &[u8]) -> Self {
            Self {
                data: bytes.as_ptr(),
                len: bytes.len(),
            }
        }
    }

    #[repr(C)]
    struct Export {
        key: Bytes,
        target: *const c_char,
    }

    #[repr(C)]
    struct Native {
        row: unsafe extern "C" fn(*mut c_void, usize) -> Row,
        lookup: unsafe extern "C" fn(
            *mut c_void,
            *const c_char,
            usize,
        ) -> *const c_char,
        publish: unsafe extern "C" fn(
            *mut c_void,
            *const Bytes,
            usize,
            *const Export,
            usize,
        ) -> bool,
    }

    struct Registry<'a> {
        context: *mut c_void,
        native: &'a Native,
    }

    impl Registry<'_> {
        fn row(&self, index: usize) -> Option<Definition<'_>> {
            // SAFETY: The callback borrows rows from the current registry build.
            let row = unsafe { (self.native.row)(self.context, index) };
            let name = |p: *const c_char| {
                (!p.is_null()).then(|| {
                    // SAFETY: All non-null fields are live, terminated build inputs.
                    unsafe { CStr::from_ptr(p) }
                })
            };
            let module = name(row.module)?.to_bytes();
            let kind = match name(row.label)?.to_bytes() {
                b"PyReexport" => Kind::Reexport,
                b"PyStarImport" => Kind::Star,
                b"PyAll" => Kind::All,
                _ => Kind::Definition,
            };
            Some(Definition {
                module,
                scope: module.get(..row.scope_len)?,
                qualified: name(row.qualified)?,
                name: name(row.name).map(CStr::to_bytes),
                source: name(row.source).map(CStr::to_bytes),
                member: name(row.member).map(CStr::to_bytes),
                kind,
            })
        }

        fn lookup(&self, qn: &CStr) -> Option<Target<'_>> {
            // SAFETY: The registry retains the returned definition QN until
            // after the namespace rows are published.
            let result = unsafe {
                (self.native.lookup)(
                    self.context,
                    qn.as_ptr(),
                    qn.to_bytes_with_nul().len(),
                )
            };
            (!result.is_null()).then(|| unsafe { CStr::from_ptr(result) })
        }

        fn publish(
            &self,
            scopes: &[Name<'_>],
            exports: &[(Vec<u8>, Target<'_>)],
        ) -> bool {
            let scopes: Vec<_> =
                scopes.iter().map(|&scope| Bytes::from(scope)).collect();
            let rows: Vec<_> = exports
                .iter()
                .map(|(key, target)| Export {
                    key: key.as_slice().into(),
                    target: target.as_ptr(),
                })
                .collect();
            // SAFETY: The callback copies these arrays and string slices before
            // return. The target QNs already live in the registry.
            unsafe {
                (self.native.publish)(
                    self.context,
                    scopes.as_ptr(),
                    scopes.len(),
                    rows.as_ptr(),
                    rows.len(),
                )
            }
        }
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn mori_py_build_namespaces(
        context: *mut c_void,
        native: *const Native,
        count: usize,
    ) -> bool {
        // SAFETY: The caller holds the input, registry, and callback table live
        // for the duration of this build.
        let registry = Registry {
            context,
            native: unsafe { &*native },
        };
        let defs: Vec<_> = (0..count).filter_map(|i| registry.row(i)).collect();
        if defs.is_empty() {
            return true;
        }
        let namespaces = Namespaces::build(&defs, &|qn| registry.lookup(qn));
        registry.publish(&namespaces.scopes, &namespaces.exports())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(
        module: &'static [u8],
        qualified: &'static CStr,
        name: &'static [u8],
    ) -> Definition<'static> {
        Definition {
            module,
            scope: if module.len() > b".__init__".len() {
                module.strip_suffix(b".__init__").unwrap_or(module)
            } else {
                module
            },
            qualified,
            name: Some(name),
            source: None,
            member: None,
            kind: Kind::Definition,
        }
    }

    fn import(
        module: &'static [u8],
        source: &'static [u8],
        name: &'static [u8],
        member: &'static [u8],
        kind: Kind,
    ) -> Definition<'static> {
        Definition {
            source: Some(source),
            member: Some(member),
            kind,
            ..def(module, c"synthetic", name)
        }
    }

    fn rows(
        defs: &[Definition<'static>],
        registered: &[&'static CStr],
    ) -> Vec<(Vec<u8>, &'static CStr)> {
        Namespaces::build(defs, &|qn| {
            registered.iter().copied().find(|&target| target == qn)
        })
        .exports()
    }

    #[test]
    fn stars_respect_literal_all_and_unknown_rebinding() {
        let defs = [
            def(b"p.impl", c"p.impl.First", b"First"),
            def(b"p.impl", c"p.impl.Second", b"Second"),
            def(b"p.impl", c"p.impl._private", b"_private"),
            import(b"p.impl", b"", b"", b"First|_private", Kind::All),
            import(b"p.public.__init__", b"..impl", b"", b"", Kind::Star),
            import(
                b"p.public.__init__",
                b"unregistered",
                b"First",
                b"missing",
                Kind::Reexport,
            ),
            import(
                b"p.public.__init__",
                b"..impl",
                b"Renamed",
                b"Second",
                Kind::Reexport,
            ),
        ];
        assert_eq!(
            rows(
                &defs,
                &[c"p.impl.First", c"p.impl.Second", c"p.impl._private"]
            ),
            vec![
                (b"p.public.Renamed".to_vec(), c"p.impl.Second"),
                (b"p.public._private".to_vec(), c"p.impl._private"),
            ]
        );
    }

    #[test]
    fn unresolved_star_preserves_own_definitions_and_clears_imports() {
        let defs = [
            def(b"p.impl", c"p.impl.Origin", b"Origin"),
            def(b"p.middle", c"p.middle.Own", b"Own"),
            import(
                b"p.middle",
                b".impl",
                b"Transient",
                b"Origin",
                Kind::Reexport,
            ),
            import(b"p.middle", b"external", b"", b"", Kind::Star),
            import(b"p.middle", b".impl", b"Own", b"Origin", Kind::Reexport),
            import(b"p.public", b".middle", b"", b"", Kind::Star),
        ];
        assert_eq!(
            rows(&defs, &[c"p.middle.Own", c"p.impl.Origin"]),
            vec![(b"p.public.Own".to_vec(), c"p.middle.Own")]
        );
    }

    #[test]
    fn shared_suffix_is_unknown_and_cycle_uses_partial_state() {
        let reexport = Kind::Reexport;
        let defs = [
            def(b"p.priv.impl", c"p.priv.impl.F", b"F"),
            def(b"p.other.impl", c"p.other.impl.F", b"F"),
            import(b"p.a", b".priv.impl", b"Seed", b"F", reexport),
            import(b"p.a", b".b", b"Through", b"Copy", reexport),
            import(b"p.b", b".a", b"Copy", b"Seed", reexport),
            import(b"p.b", b"impl", b"Unknown", b"F", reexport),
        ];
        let target = c"p.priv.impl.F";
        assert_eq!(
            rows(&defs, &[target, c"p.other.impl.F"]),
            vec![
                (b"p.a.Seed".to_vec(), target),
                (b"p.a.Through".to_vec(), target),
                (b"p.b.Copy".to_vec(), target),
            ]
        );
    }
}
