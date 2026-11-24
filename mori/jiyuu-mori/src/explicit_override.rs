//! Resolve a method against the nearest explicit ancestor declarations.

use std::collections::{BTreeMap, HashMap, HashSet};

#[cfg(feature = "native-engine-abi")]
mod native;

#[derive(Default)]
struct Hierarchy {
    bases: BTreeMap<i64, Vec<i64>>,
    methods: HashMap<i64, Vec<Method>>,
}

struct Method {
    id: i64,
    name: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
struct OverrideEdge {
    source: i64,
    target: i64,
}

impl Hierarchy {
    fn edges(
        &self,
        eligible: impl Fn(i64) -> bool,
        max_levels: usize,
    ) -> Vec<OverrideEdge> {
        let mut edges = Vec::new();
        let mut visited = HashSet::new();
        let mut level = Vec::new();
        let mut next = Vec::new();
        let mut pending = Vec::new();
        for (&class, bases) in &self.bases {
            let Some(methods) = self.methods.get(&class) else {
                continue;
            };
            if !eligible(class) {
                continue;
            }
            visited.clear();
            visited.insert(class);
            level.clear();
            level.extend(bases.iter().copied());
            pending.clear();
            pending.extend(methods.iter().map(|method| (method, false)));

            for _ in 0..max_levels {
                level.retain(|id| visited.insert(*id));
                if level.is_empty() || pending.is_empty() {
                    break;
                }
                level.sort_unstable();
                next.clear();
                for &ancestor in &level {
                    if let Some(declarations) = self.methods.get(&ancestor) {
                        for (method, bound) in &mut pending {
                            // The reference uses the first declaration of this
                            // name within each ancestor's DEFINES_METHOD order.
                            let target = declarations.iter().find(|base| {
                                base.id != method.id && base.name == method.name
                            });
                            if let Some(target) = target {
                                edges.push(OverrideEdge {
                                    source: method.id,
                                    target: target.id,
                                });
                                *bound = true;
                            }
                        }
                    }
                    if let Some(bases) = self.bases.get(&ancestor) {
                        next.extend(bases);
                    }
                }
                // Finish the whole level before removing matched methods.
                pending.retain(|(_, bound)| !bound);
                std::mem::swap(&mut level, &mut next);
            }
        }
        edges
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binds_by_name_at_the_nearest_declaring_level() {
        let mut hierarchy = Hierarchy::default();
        // Leaf implements two interfaces through empty intermediates. Both
        // interfaces declare process. Only their shared base declares close.
        hierarchy.bases.extend([
            (10, vec![31, 30, 10]),
            (30, vec![50, 50]),
            (31, vec![51]),
            (50, vec![90]),
            (51, vec![90]),
            (90, vec![30]),
            (11, vec![90]),
        ]);
        for (class, method, name) in [
            (10, 100, &b"process"[..]),
            (10, 101, b"close"),
            (10, 102, b"unknown"),
            (11, 110, b"process"),
            // Skip a shared method ID, then pick the first declaration
            // rather than sorting by the declaring method ID.
            (50, 100, b"process"),
            (50, 502, b"process"),
            (50, 501, b"process"),
            (51, 511, b"process"),
            (90, 900, b"process"),
            (90, 901, b"close"),
        ] {
            hierarchy.methods.entry(class).or_default().push(Method {
                id: method,
                name: name.to_vec(),
            });
        }
        assert_eq!(
            hierarchy.edges(|class| class == 10, 7),
            [
                OverrideEdge {
                    source: 100,
                    target: 502
                },
                OverrideEdge {
                    source: 100,
                    target: 511
                },
                OverrideEdge {
                    source: 101,
                    target: 901
                },
            ]
        );
        assert!(hierarchy.edges(|class| class == 10, 1).is_empty());
        assert_eq!(hierarchy.edges(|class| class == 10, 2).len(), 2);
    }

    #[test]
    fn a_cyclic_hierarchy_without_a_declaration_emits_nothing() {
        let mut hierarchy = Hierarchy::default();
        hierarchy.bases.extend([(1, vec![2]), (2, vec![1, 2])]);
        hierarchy.methods.insert(
            1,
            vec![Method {
                id: 3,
                name: vec![0xff],
            }],
        );
        assert!(hierarchy.edges(|_| true, 3).is_empty());
    }
}
