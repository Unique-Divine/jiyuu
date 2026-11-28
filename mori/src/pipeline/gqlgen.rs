//! Map schema files and fields to their gqlgen server and resolver.

use std::collections::{HashMap, HashSet};

use crate::link::RoutePlan;

pub(crate) const ROOT_COUNT: usize = 3;

#[derive(Default)]
pub(crate) struct SchemaIndex {
    schemas: Vec<SchemaRootEvidence>,
}

struct SchemaRootEvidence {
    file: Vec<u8>,
    group: Option<usize>,
    roots: [Option<Vec<u8>>; ROOT_COUNT],
    defined: bool,
}

impl SchemaIndex {
    pub(crate) fn add(
        &mut self,
        file: &[u8],
        group: Option<usize>,
        roots: [Option<&[u8]>; ROOT_COUNT],
        defined: bool,
    ) {
        self.schemas.push(SchemaRootEvidence {
            file: file.to_vec(),
            group,
            roots: roots.map(|name| name.map(<[u8]>::to_vec)),
            defined,
        });
    }

    /// Match a type to the first operation it roots, in query, mutation,
    /// subscription order. An explicit schema suppresses unnamed defaults.
    pub(crate) fn root_operation(
        &self,
        file: &[u8],
        group: Option<usize>,
        name: &[u8],
    ) -> Option<usize> {
        const DEFAULTS: [&[u8]; ROOT_COUNT] =
            [b"Query", b"Mutation", b"Subscription"];
        let mut best = [None::<(&[u8], usize, &[u8])>; ROOT_COUNT];
        let mut defined = false;
        for schema in &self.schemas {
            if schema.group != group
                || (group.is_none()
                    && !file.starts_with(parent_directory(&schema.file)))
            {
                continue;
            }
            defined |= schema.defined;
            let common_directory_len = if group.is_none() {
                file.iter()
                    .zip(&schema.file)
                    .take_while(|(left, right)| left == right)
                    .enumerate()
                    .filter_map(|(index, (&byte, _))| {
                        (byte == b'/').then_some(index + 1)
                    })
                    .last()
                    .unwrap_or(0)
            } else {
                0
            };
            for (current, root) in best.iter_mut().zip(&schema.roots) {
                if let Some(root) = root
                    && current.is_none_or(|(old_file, old_len, _)| {
                        common_directory_len > old_len
                            || (common_directory_len == old_len
                                && schema.file.as_slice() < old_file)
                    })
                {
                    *current = Some((&schema.file, common_directory_len, root));
                }
            }
        }
        best.into_iter().zip(DEFAULTS).position(|(entry, default)| {
            let root = entry
                .map(|(_, _, root)| root)
                .or((!defined).then_some(default));
            root == Some(name)
        })
    }
}

#[derive(Default)]
pub(crate) struct ResolverIndex {
    configured: HashSet<Vec<u8>>,
    heuristic: HashSet<Vec<u8>>,
    methods: HashMap<Vec<u8>, Vec<Method>>,
}

struct Method {
    id: i64,
    directory: Vec<u8>,
}

pub(crate) struct SchemaField<'a> {
    pub owner: &'a [u8],
    pub name: &'a [u8],
    pub file: &'a [u8],
    pub scope: ResolverScope<'a>,
}

pub(crate) enum ResolverScope<'a> {
    Configured(Option<&'a [u8]>),
    Heuristic,
}

fn directory(path: &[u8]) -> &[u8] {
    path.iter()
        .rposition(|&byte| byte == b'/')
        .map_or(&[], |slash| &path[..=slash])
}

fn parent_directory(path: &[u8]) -> &[u8] {
    let dir = directory(path);
    directory(dir.strip_suffix(b"/").unwrap_or(dir))
}

fn resolver_type<'a>(qn: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    let parent = qn.strip_suffix(name)?.strip_suffix(b".")?;
    let receiver = parent.rsplit(|&byte| byte == b'.').next()?;
    receiver
        .strip_suffix(b"Resolver")
        .filter(|base| !base.is_empty())
        .map(|_| receiver)
}

fn field_key(owner: &[u8], field: &[u8]) -> Vec<u8> {
    let mut key =
        Vec::with_capacity(owner.len() + b"Resolver.".len() + field.len());
    key.extend_from_slice(owner);
    key.extend_from_slice(b"Resolver.");
    key[0].make_ascii_lowercase();

    // gqlgen folds initialisms and removes underscores, except between digits.
    let mut cursor = 0;
    while let Some(&byte) = field.get(cursor) {
        if byte != b'_' {
            key.push(byte.to_ascii_lowercase());
            cursor += 1;
            continue;
        }
        let run = cursor;
        while field.get(cursor) == Some(&b'_') {
            cursor += 1;
        }
        if run > 0
            && field[run - 1].is_ascii_digit()
            && field.get(cursor).is_some_and(u8::is_ascii_digit)
        {
            key.push(b'_');
        }
    }
    key
}

impl ResolverIndex {
    pub(crate) fn add_package(&mut self, directory: &[u8]) {
        self.configured.insert(directory.to_vec());
    }

    pub(crate) fn add_method(
        &mut self,
        file: &[u8],
        is_go: bool,
        name: Option<&[u8]>,
        qn: Option<&[u8]>,
        id: i64,
    ) {
        let dir = directory(file);
        let base = &file[dir.len()..];
        if base == b"resolver.go"
            || (file.len() > b".resolvers.go".len()
                && file.ends_with(b".resolvers.go"))
        {
            self.heuristic.insert(dir.to_vec());
        }
        if !is_go {
            return;
        }
        let Some((name, qn)) = name.zip(qn) else {
            return;
        };
        let Some(receiver) = resolver_type(qn, name) else {
            return;
        };
        let mut key = Vec::with_capacity(receiver.len() + 1 + name.len());
        key.extend_from_slice(receiver);
        key.push(b'.');
        key.extend(name.iter().map(u8::to_ascii_lowercase));
        self.methods.entry(key).or_default().push(Method {
            id,
            directory: dir.to_vec(),
        });
    }

    fn owns(&self, method: &Method, field: &SchemaField<'_>) -> bool {
        let dir = method.directory.as_slice();
        match field.scope {
            ResolverScope::Configured(package) => package == Some(dir),
            ResolverScope::Heuristic => {
                if self.configured.contains(dir) || !self.heuristic.contains(dir)
                {
                    return false;
                }
                let schema_dir = directory(field.file);
                if schema_dir.is_empty() {
                    // A root schema can use a resolver package one level down.
                    return !dir
                        .strip_suffix(b"/")
                        .unwrap_or(dir)
                        .contains(&b'/');
                }
                dir.starts_with(schema_dir)
                    || dir == parent_directory(field.file)
            }
        }
    }

    pub(crate) fn plan<'a>(
        &self,
        fields: usize,
        candidates: impl Iterator<Item = SchemaField<'a>>,
    ) -> RoutePlan<i64> {
        let mut plan = RoutePlan::new(fields);
        for (field_index, field) in candidates.enumerate() {
            let key = field_key(field.owner, field.name);
            for method in self.methods.get(&key).into_iter().flatten() {
                if self.owns(method, &field)
                    && !plan.consider(field_index, method.id)
                {
                    return plan;
                }
            }
        }
        plan
    }
}

// Match GQLGEN_MAX_GLOB_SEGMENTS in the C reference. Both paths ignore
// empty components and stop at this many nonempty segments.
const MAX_SEGMENTS: usize = 64;

fn split_segments<'a>(
    path: &'a [u8],
    segments: &mut [&'a [u8]; MAX_SEGMENTS],
) -> usize {
    let mut len = 0;
    for (segment, slot) in path
        .split(|&byte| byte == b'/')
        .filter(|part| !part.is_empty())
        .zip(segments)
    {
        *slot = segment;
        len += 1;
    }
    len
}

fn segment_matches(pattern: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut star = None;
    while let Some(&byte) = text.get(t) {
        match pattern.get(p) {
            Some(&ch) if ch == b'?' || ch == byte => {
                p += 1;
                t += 1;
            }
            Some(b'*') => {
                star = Some((p, t));
                p += 1;
            }
            _ => {
                let Some((star_pos, mark)) = star else {
                    return false;
                };
                p = star_pos + 1;
                t = mark + 1;
                star = Some((star_pos, t));
            }
        }
    }
    pattern[p..].iter().all(|&byte| byte == b'*')
}

pub(crate) fn glob_matches(pattern: &[u8], path: &[u8]) -> bool {
    let (mut patterns, mut paths) =
        ([&[][..]; MAX_SEGMENTS], [&[][..]; MAX_SEGMENTS]);
    let pn = split_segments(pattern, &mut patterns);
    let tn = split_segments(path, &mut paths);
    let (patterns, paths) = (&patterns[..pn], &paths[..tn]);

    let (mut p, mut t) = (0, 0);
    let mut globstar = None;
    while let Some(segment) = paths.get(t) {
        match patterns.get(p) {
            Some(&b"**") => {
                globstar = Some((p, t));
                p += 1;
            }
            Some(pattern) if segment_matches(pattern, segment) => {
                p += 1;
                t += 1;
            }
            _ => {
                let Some((star_pos, mark)) = globstar else {
                    return false;
                };
                p = star_pos + 1;
                t = mark + 1;
                globstar = Some((star_pos, t));
            }
        }
    }
    patterns[p..].iter().all(|&segment| segment == b"**")
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_SEGMENTS, ResolverIndex, ResolverScope, SchemaField, SchemaIndex,
        glob_matches,
    };

    #[test]
    fn explicit_roots_merge_in_config_order_and_suppress_defaults() {
        let mut schemas = SchemaIndex::default();
        schemas.add(
            b"z.graphql",
            Some(0),
            [Some(b"LaterRoot"), Some(b"Write"), None],
            false,
        );
        schemas.add(b"a.graphql", Some(0), [Some(b"Read"), None, None], true);
        schemas.add(
            b"b.graphql",
            Some(1),
            [Some(b"Unrelated"), None, None],
            true,
        );
        let root = |name| schemas.root_operation(b"a.graphql", Some(0), name);
        assert_eq!(root(b"Read"), Some(0));
        assert_eq!(root(b"Write"), Some(1));
        assert_eq!(root(b"LaterRoot"), None);
        assert_eq!(root(b"Unrelated"), None);
        assert_eq!(root(b"Subscription"), None);
    }

    #[test]
    fn heuristic_roots_use_nearest_schema_and_keep_extension_defaults() {
        let mut schemas = SchemaIndex::default();
        schemas.add(
            b"global/schema.graphql",
            None,
            [Some(b"Global"), None, None],
            false,
        );
        schemas.add(
            b"api/b/schema.graphql",
            None,
            [Some(b"ApiB"), None, None],
            false,
        );
        schemas.add(
            b"api/a/schema.graphql",
            None,
            [Some(b"ApiA"), None, None],
            false,
        );
        let root = |file, name| schemas.root_operation(file, None, name);
        assert_eq!(root(b"api/b/types.graphql", b"ApiB"), Some(0));
        assert_eq!(root(b"api/types.graphql", b"ApiA"), Some(0));
        assert_eq!(root(b"api/types.graphql", b"Mutation"), Some(1));
        assert_eq!(root(b"elsewhere/types.graphql", b"Global"), Some(0));
        assert_eq!(root(b"elsewhere/types.graphql", b"ApiA"), None);
    }

    fn query_score<'a>(
        file: &'a [u8],
        scope: ResolverScope<'a>,
    ) -> SchemaField<'a> {
        SchemaField {
            owner: b"Query",
            name: b"_score0__1_",
            file,
            scope,
        }
    }

    #[test]
    fn resolves_gqlgen_field_names_with_numeric_underscores() {
        let mut index = ResolverIndex::default();
        for (name, qn, id) in [
            (&b"Score0_1"[..], &b"p.queryResolver.Score0_1"[..], 1),
            (b"Score01", b"p.queryResolver.Score01", 2),
            (b"Score0_1", b"p.Resolver.Score0_1", 3),
            (b"Score0_1", b"p.queryResolver.Else", 4),
        ] {
            index.add_method(b"api/resolver.go", true, Some(name), Some(qn), id);
        }
        let field = query_score(b"api/schema.graphql", ResolverScope::Heuristic);
        assert_eq!(index.plan(1, [field].into_iter()).resolver, Some(1));
    }

    #[test]
    fn scopes_configured_packages_and_handles_duplicate_route_candidates() {
        let mut index = ResolverIndex::default();
        index.add_package(b"a/");
        index.add_package(b"b/");
        for (file, id) in
            [(&b"a/resolver.go"[..], 1), (b"b/schema.resolvers.go", 2)]
        {
            index.add_method(
                file,
                true,
                Some(b"Score0_1"),
                Some(b"p.queryResolver.Score0_1"),
                id,
            );
        }
        let heuristic =
            || query_score(b"schema.graphql", ResolverScope::Heuristic);
        let configured =
            |dir| query_score(b"schema.graphql", ResolverScope::Configured(dir));
        let plan = index.plan(
            3,
            [
                heuristic(),
                configured(Some(b"a/")),
                configured(Some(b"a/")),
            ]
            .into_iter(),
        );
        assert_eq!(plan.resolver, Some(1));
        assert_eq!(plan.field_index, 1);
        assert!(!plan.ambiguous);

        let plan = index.plan(
            2,
            [configured(Some(b"a/")), configured(Some(b"b/"))].into_iter(),
        );
        assert_eq!(plan.resolver, None);
        assert_eq!(plan.field_index, 0);
        assert!(plan.ambiguous);
        assert_eq!(index.plan(1, [configured(None)].into_iter()).resolver, None);
    }

    #[test]
    fn heuristic_package_reach_matches_schema_directory_rules() {
        let mut index = ResolverIndex::default();
        index.add_method(
            b"api/nested/schema.resolvers.go",
            true,
            Some(b"Score0_1"),
            Some(b"p.queryResolver.Score0_1"),
            1,
        );
        for (file, resolver) in [
            (&b"schema.graphql"[..], None),
            (b"api/schema.graphql", Some(1)),
            (b"api/nested/schema.graphql", Some(1)),
            (b"api/nested/sub/schema.graphql", Some(1)),
            (b"api/nested/sub/other/schema.graphql", None),
            (b"elsewhere/schema.graphql", None),
        ] {
            let field = query_score(file, ResolverScope::Heuristic);
            assert_eq!(index.plan(1, [field].into_iter()).resolver, resolver);
        }
    }

    #[test]
    fn matches_schema_paths() {
        for (pattern, path, expected) in [
            ("schema/*.graphqls", "/schema//query.graphqls/", true),
            ("schema/*.graphqls", "schema/sub/query.graphqls", false),
            ("schema/**/query?.graphqls", "schema/query1.graphqls", true),
            (
                "schema/**/query?.graphqls",
                "schema/a/b/query1.graphqls",
                true,
            ),
            (
                "schema/**/query?.graphqls",
                "schema/a/query10.graphqls",
                false,
            ),
            (
                "schema/**/query?.graphqls",
                "other/a/query1.graphqls",
                false,
            ),
            ("**/a/**/z", "x/a/x/a/z", true),
            ("schema/**", "schema", true),
            ("schema/*", "schema", false),
            ("**", "/", true),
            ("", "/", true),
            ("***", "schema/nested", false),
            ("?", "é", false),
            ("??", "é", true),
        ] {
            assert_eq!(
                glob_matches(pattern.as_bytes(), path.as_bytes()),
                expected,
                "{pattern:?} against {path:?}"
            );
        }
        assert!(glob_matches(b"schema/?.graphqls", b"schema/\xff.graphqls"));
    }

    #[test]
    fn keeps_c_segment_limit() {
        let prefix = "path/".repeat(MAX_SEGMENTS);
        let pattern = format!("{prefix}a.graphqls");
        let path = format!("{prefix}b.graphqls");
        assert!(glob_matches(pattern.as_bytes(), path.as_bytes()));
    }
}
