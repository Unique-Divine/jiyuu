//! Match a schema path against the globs in a gqlgen config.

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
    use super::{MAX_SEGMENTS, glob_matches};

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
