//! File-local Perl package declarations and method lookup.

use std::collections::HashMap;
use std::ffi::{CStr, CString};

const LOOKUP_CAP: usize = 32;

#[derive(Default)]
struct Package {
    parents: Vec<CString>,
    subs: HashMap<CString, CString>,
}

#[derive(Default)]
struct Packages(HashMap<CString, Package>);

impl Packages {
    fn package_mut(&mut self, name: &CStr) -> &mut Package {
        self.0.entry(name.to_owned()).or_default()
    }

    fn add_parent(&mut self, package: &CStr, parent: &CStr) {
        if !package.is_empty() && !parent.is_empty() {
            self.package_mut(package).parents.push(parent.to_owned());
        }
    }

    fn add_sub(&mut self, package: &CStr, name: &CStr, module: Option<&CStr>) {
        if name.is_empty() {
            return;
        }
        self.package_mut(package)
            .subs
            .entry(name.to_owned())
            .or_insert_with(|| {
                let Some(module) = module else {
                    return name.to_owned();
                };
                // Perl's extractor gives subs file-qualified graph identities.
                let mut qn = Vec::with_capacity(
                    module.count_bytes() + name.count_bytes() + 2,
                );
                qn.extend_from_slice(module.to_bytes());
                qn.push(b'.');
                qn.extend_from_slice(name.to_bytes_with_nul());
                CString::from_vec_with_nul(qn)
                    .expect("CStr components contain no interior NUL")
            });
    }

    fn lookup<T>(
        &self,
        package: &CStr,
        method: &CStr,
        super_call: bool,
        mut exact: impl FnMut(&CStr) -> Option<T>,
        mut direct: impl FnMut(&CStr, &CStr) -> Option<T>,
    ) -> Option<T> {
        // SUPER starts at the first declared parent. Ordinary dispatch visits
        // own methods first, then follows the historical LIFO parent order.
        let start = if super_call {
            self.0.get(package)?.parents.first()?.as_c_str()
        } else {
            package
        };
        let mut frontier = [start; LOOKUP_CAP];
        let mut pending = 1;
        let mut visited = [start; LOOKUP_CAP];
        let mut seen = 0;
        while pending > 0 && seen < LOOKUP_CAP {
            pending -= 1;
            let package = frontier[pending];
            if visited[..seen].contains(&package) {
                continue;
            }
            visited[seen] = package;
            seen += 1;

            let entry = self.0.get(package);
            if let Some(target) = entry
                .and_then(|entry| entry.subs.get(method))
                .and_then(|qn| exact(qn))
                .or_else(|| direct(package, method))
            {
                return Some(target);
            }
            if let Some(entry) = entry {
                for parent in entry.parents.iter().take(LOOKUP_CAP - pending) {
                    frontier[pending] = parent;
                    pending += 1;
                }
            }
        }
        None
    }
}

mod ffi;

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve<'a>(
        packages: &Packages,
        package: &CStr,
        name: &CStr,
        super_call: bool,
        registered: &[&'a CStr],
    ) -> Option<&'a CStr> {
        packages.lookup(
            package,
            name,
            super_call,
            |qn| {
                registered
                    .iter()
                    .copied()
                    .find(|candidate| *candidate == qn)
            },
            |_, _| None,
        )
    }

    #[test]
    fn resolves_constructor_and_method_on_exact_package() {
        let mut index = Packages::default();
        index.add_sub(c"Acme::Widget", c"new", Some(c"test.main"));
        index.add_sub(c"Acme::Widget", c"render", Some(c"test.main"));
        let registered = [c"test.main.new", c"test.main.render"];
        for (name, target) in [c"new", c"render"].into_iter().zip(registered) {
            assert_eq!(
                resolve(&index, c"Acme::Widget", name, false, &registered),
                Some(target)
            );
            assert_eq!(
                resolve(&index, c"Widget", name, false, &registered),
                None
            );
            assert_eq!(
                resolve(&index, c"Unknown::Pkg", name, false, &registered),
                None
            );
        }
    }

    #[test]
    fn finds_inherited_method_through_parent_with_no_subs() {
        let mut index = Packages::default();
        index.add_parent(c"Derived", c"Intermediate");
        index.add_parent(c"Intermediate", c"Base");
        index.add_sub(c"Base", c"speak", Some(c"test.main"));
        assert_eq!(
            resolve(&index, c"Derived", c"speak", false, &[c"test.main.speak"]),
            Some(c"test.main.speak")
        );
        assert_eq!(resolve(&index, c"Derived", c"speak", false, &[]), None);
    }

    #[test]
    fn dispatch_order_preserves_precedence_and_first_parent_super() {
        let mut index = Packages::default();
        index.add_parent(c"Child", c"First");
        index.add_parent(c"Child", c"Last");
        let registered = [c"first.greet", c"last.greet", c"child.greet"];
        index.add_sub(c"First", c"greet", Some(c"first"));
        index.add_sub(c"Last", c"greet", Some(c"last"));
        assert_eq!(
            resolve(&index, c"Child", c"greet", false, &registered),
            Some(c"last.greet")
        );
        index.add_sub(c"Child", c"greet", Some(c"child"));
        assert_eq!(
            resolve(&index, c"Child", c"greet", false, &registered),
            Some(c"child.greet")
        );
        assert_eq!(
            resolve(&index, c"Child", c"greet", true, &registered),
            Some(c"first.greet")
        );
        assert_eq!(resolve(&index, c"First", c"greet", true, &registered), None);
    }

    #[test]
    fn shares_the_graph_qn_only_when_both_packages_declare_the_sub() {
        let mut index = Packages::default();
        index.add_sub(c"Base", c"greet", Some(c"test.main"));
        index.add_sub(c"Child", c"greet", Some(c"test.main"));
        index.add_parent(c"Child", c"Base");
        let registered = [c"test.main.greet"];
        assert_eq!(
            resolve(&index, c"Child", c"greet", true, &registered),
            Some(c"test.main.greet")
        );
        assert_eq!(
            resolve(&index, c"Orphan", c"greet", false, &registered),
            None
        );
    }

    #[test]
    fn supports_exact_native_method_fallback() {
        let index = Packages::default();
        let lookup = |package| {
            index.lookup(
                package,
                c"write",
                false,
                |_| None,
                |package, name| {
                    (package == c"IO::Handle" && name == c"write").then_some(42)
                },
            )
        };
        assert_eq!(lookup(c"IO::Handle"), Some(42));
        assert_eq!(lookup(c"Handle"), None);
    }

    #[test]
    fn stops_at_cycles_and_lookup_limit() {
        let mut index = Packages::default();
        index.add_parent(c"Child", c"Base");
        index.add_parent(c"Base", c"Child");
        let mut visits = 0;
        assert_eq!(
            index.lookup(
                c"Child",
                c"missing",
                false,
                |_| None,
                |_, _| {
                    visits += 1;
                    None::<()>
                }
            ),
            None
        );
        assert_eq!(visits, 2);

        let chain: Vec<_> = (0..=LOOKUP_CAP)
            .map(|number| CString::new(number.to_string()).unwrap())
            .collect();
        for link in chain.windows(2) {
            index.add_parent(&link[0], &link[1]);
        }
        index.add_sub(&chain[LOOKUP_CAP], c"only", None);
        assert_eq!(resolve(&index, &chain[0], c"only", false, &[c"only"]), None);
        assert_eq!(
            resolve(&index, &chain[1], c"only", false, &[c"only"]),
            Some(c"only")
        );
    }
}
