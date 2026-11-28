//! PHP type and method lookup over the C type registry.
//!
//! `php_lsp.c` calls these through the exports in `native.rs`. The registry
//! rows stay in C, including rows that class and trait refinement add.

use std::ffi::CStr;

trait Registry<'a> {
    type Type: Copy;
    type Method;

    fn exact_type(&self, qn: &CStr) -> Option<Self::Type>;
    /// Look up `prefix.qn`. `None` when the joined key cannot be built.
    fn joined_type(&self, prefix: &[u8], qn: &CStr) -> Option<Self::Type>;
    fn method(&self, receiver: &CStr, name: &CStr) -> Option<Self::Method>;
    fn function(&self, qn: &CStr) -> Option<Self::Method>;
    /// Visit the registry's own rows with this short name in registration
    /// order.
    fn visit_short_types(&self, name: &CStr, visit: impl FnMut(Self::Type));
    fn type_name(&self, ty: Self::Type) -> &'a CStr;
    fn parents(&self, ty: Self::Type) -> impl Iterator<Item = &'a CStr>;
    /// Listed method names with their function keys. A missing key is `None`.
    fn listed_methods(
        &self,
        ty: Self::Type,
    ) -> impl Iterator<Item = (&'a CStr, Option<&'a CStr>)>;
}

struct Resolver<'a, R, E> {
    registry: R,
    module: Option<&'a CStr>,
    external_uses: E,
}

impl<'a, R: Registry<'a>, E: Iterator<Item = &'a CStr> + Clone>
    Resolver<'a, R, E>
{
    /// True when `qn` is, or lies under, the target of a class `use` the
    /// pipeline left unresolved: a type outside the project.
    fn is_external(&self, qn: &CStr) -> bool {
        self.external_uses.clone().any(|external| {
            matches!(
                qn.to_bytes().strip_prefix(external.to_bytes()),
                Some([] | [b'.', ..])
            )
        })
    }

    /// Find a registered type for a namespaced QN. Lookup order:
    ///
    /// 1. the exact QN;
    /// 2. `module.qn`, then `prefix.qn` for each shorter module prefix;
    /// 3. the registry's types with the same short name, preferring the
    ///    first one whose QN shares the most dot-separated module segments.
    ///
    /// A QN named through a vendor `use` stops after step 1.
    fn lookup_type(&self, qn: &CStr) -> Option<R::Type> {
        if let Some(ty) = self.registry.exact_type(qn) {
            return Some(ty);
        }
        if self.is_external(qn) {
            return None;
        }

        let module = self.module.map_or(&[][..], CStr::to_bytes);
        if !module.is_empty() {
            let prefixes = std::iter::once(module).chain(
                module.iter().enumerate().rev().filter_map(|(index, byte)| {
                    (*byte == b'.').then_some(&module[..index])
                }),
            );
            for prefix in prefixes {
                if let Some(ty) = self.registry.joined_type(prefix, qn) {
                    return Some(ty);
                }
            }
        }

        // Graph keys are file-derived, while `use App\Models\User` yields
        // App.Models.User, so try the leaf after all exact probes.
        let start = qn
            .to_bytes()
            .iter()
            .rposition(|b| *b == b'.')
            .map_or(0, |i| i + 1);
        let leaf = CStr::from_bytes_with_nul(&qn.to_bytes_with_nul()[start..])
            .expect("C name suffix");
        if leaf.is_empty() {
            return None;
        }

        let mut best: Option<(R::Type, usize)> = None;
        self.registry.visit_short_types(leaf, |ty| {
            let score = module
                .iter()
                .zip(self.registry.type_name(ty).to_bytes())
                .take_while(|(a, b)| a == b)
                .filter(|(byte, _)| **byte == b'.')
                .count();
            if best.is_none_or(|(_, best_score)| score > best_score) {
                best = Some((ty, score));
            }
        });
        best.map(|(ty, _)| ty)
    }

    /// Look up a method on a class, then walk its extends, implements and
    /// flattened-trait ancestors.
    fn lookup_method(&self, class: &CStr, name: &CStr) -> Option<R::Method> {
        if let Some(method) = self.registry.method(class, name) {
            return Some(method);
        }
        let ty = self.lookup_type(class)?;
        let canonical = self.registry.type_name(ty);
        if canonical != class
            && let Some(method) = self.registry.method(canonical, name)
        {
            return Some(method);
        }

        // Pop the most recently pushed parent first. Bound both the pending
        // and the visited names.
        const LIMIT: usize = 32;
        let mut frontier = [c""; LIMIT];
        let mut visited = [c""; LIMIT];
        let mut pending = 0;
        let mut seen = 0;
        let push_parents =
            |frontier: &mut [&'a CStr], pending: &mut usize, ty| {
                for (slot, parent) in frontier[*pending..]
                    .iter_mut()
                    .zip(self.registry.parents(ty))
                {
                    *slot = parent;
                    *pending += 1;
                }
            };
        push_parents(&mut frontier, &mut pending, ty);

        while pending > 0 && seen < LIMIT {
            pending -= 1;
            let parent = frontier[pending];
            if visited[..seen].contains(&parent) {
                continue;
            }
            visited[seen] = parent;
            seen += 1;
            if let Some(method) = self.registry.method(parent, name) {
                return Some(method);
            }
            let Some(ty) = self.lookup_type(parent) else {
                continue;
            };
            let canonical = self.registry.type_name(ty);
            if canonical != parent
                && let Some(method) = self.registry.method(canonical, name)
            {
                return Some(method);
            }
            if let Some(method) =
                self.registry.listed_methods(ty).find_map(|(leaf, qn)| {
                    (leaf == name)
                        .then(|| qn.and_then(|qn| self.registry.function(qn)))
                        .flatten()
                })
            {
                return Some(method);
            }
            push_parents(&mut frontier, &mut pending, ty);
        }
        None
    }
}

#[cfg(feature = "native-engine-abi")]
mod native;

#[cfg(test)]
mod tests;
