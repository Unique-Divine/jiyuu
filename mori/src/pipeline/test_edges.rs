//! Link tests to the code they exercise.
//!
//! TESTS edges run from a test function to each production function it
//! calls, read off the CALLS edges extraction already wrote. TESTS_FILE edges
//! run from a test file to the production file its name points at
//! (`foo_test.go` to `foo.go`). The test-path and test-name rules here are
//! also the ones the call resolvers, the env join and the importance score
//! use, through the C names they export.

/// Whether `path` looks like a test file, in any language.
pub(crate) fn is_test_path(path: &[u8]) -> bool {
    let base = match path.iter().rposition(|&b| b == b'/') {
        Some(slash) => &path[slash + 1..],
        None => path,
    };
    // Python test_*.py
    if base.starts_with(b"test_") {
        return true;
    }
    // _test.<ext>: Go, Python, Rust, C++, Lua
    const TEST_SUFFIXES: [&[u8]; 5] = [
        b"_test.go",
        b"_test.py",
        b"_test.rs",
        b"_test.cpp",
        b"_test.lua",
    ];
    if TEST_SUFFIXES.iter().any(|s| path.ends_with(s)) {
        return true;
    }
    // .test.<ext> and .spec.<ext>: JS, TS, TSX
    const JS_MARKS: [&[u8]; 6] = [
        b".test.ts",
        b".spec.ts",
        b".test.js",
        b".spec.js",
        b".test.tsx",
        b".spec.tsx",
    ];
    if JS_MARKS.iter().any(|m| contains(path, m)) {
        return true;
    }
    // Test or Spec before the extension: Java, Kotlin, C#, PHP, Scala
    const CLASS_SUFFIXES: [&[u8]; 5] = [
        b"Test.java",
        b"Test.kt",
        b"Test.cs",
        b"Test.php",
        b"Spec.scala",
    ];
    if CLASS_SUFFIXES.iter().any(|s| path.ends_with(s)) {
        return true;
    }
    // A test directory anywhere in the path, or at its start
    const DIRS: [&[u8]; 4] = [b"__tests__/", b"/tests/", b"/test/", b"/spec/"];
    if DIRS.iter().any(|d| contains(path, d)) {
        return true;
    }
    const LEADING_DIRS: [&[u8]; 4] =
        [b"tests/", b"test/", b"spec/", b"__tests__/"];
    if LEADING_DIRS.iter().any(|d| path.starts_with(d)) {
        return true;
    }
    // Ruby _spec.rb
    path.ends_with(b"_spec.rb")
}

/// Whether `name` looks like a test function, in any language.
pub(crate) fn is_test_func_name(name: &[u8]) -> bool {
    // Go: Test, Benchmark and Example need an uppercase letter or nothing
    // after the prefix, so `Testable` is not a test.
    for prefix in [&b"Test"[..], b"Benchmark", b"Example"] {
        if let Some(rest) = name.strip_prefix(prefix)
            && rest.first().is_none_or(u8::is_ascii_uppercase)
        {
            return true;
        }
    }
    // Python, Rust, C++, Lua: test_; Java, PHP, Scala, Kotlin: testCamel
    if name.starts_with(b"test_") {
        return true;
    }
    if let Some(rest) = name.strip_prefix(b"test")
        && rest.first().is_some_and(u8::is_ascii_uppercase)
    {
        return true;
    }
    // JS/TS test blocks and lifecycle hooks; Julia macros
    const EXACT: [&[u8]; 9] = [
        b"test",
        b"it",
        b"describe",
        b"beforeAll",
        b"afterAll",
        b"beforeEach",
        b"afterEach",
        b"@testset",
        b"@test",
    ];
    EXACT.contains(&name)
}

/// Whether a node's properties mark it as test code.
pub(crate) fn props_mark_test(properties: &[u8]) -> bool {
    contains(properties, br#""is_test":true"#)
}

/// The production file a test file's name points at, if any.
pub(crate) fn test_to_prod_path(test_path: &[u8]) -> Option<Vec<u8>> {
    let slash = test_path.iter().rposition(|&b| b == b'/');
    let dir = &test_path[..slash.unwrap_or(0)];
    let base = slash.map_or(test_path, |s| &test_path[s + 1..]);
    let join = |name: &[u8], ext: &[u8]| {
        let mut path =
            Vec::with_capacity(dir.len() + 1 + name.len() + ext.len());
        if !dir.is_empty() {
            path.extend_from_slice(dir);
            path.push(b'/');
        }
        path.extend_from_slice(name);
        path.extend_from_slice(ext);
        path
    };

    // Go: foo_test.go -> foo.go, when the first _test.go ends the name
    if let Some(at) = find(base, b"_test.go")
        && at + b"_test.go".len() == base.len()
    {
        return Some(join(&base[..at], b".go"));
    }
    // Python: test_foo.py -> foo.py, when the first .py ends the name
    if base.starts_with(b"test_")
        && let Some(at) = find(base, b".py")
        && at + b".py".len() == base.len()
    {
        return Some(join(&base[b"test_".len()..at], b".py"));
    }
    // JS/TS: foo.test.ts -> foo.ts, foo.spec.ts -> foo.ts
    for mark in [&b".test."[..], b".spec."] {
        if let Some(at) = find(base, mark) {
            return Some(join(&base[..at], &base[at + mark.len() - 1..]));
        }
    }
    None
}

/// The longest scoped name the enclosing-test walk accepts, exclusive.
const SCOPE_LIMIT: usize = 512;

/// The enclosing scopes of a qualified name, innermost first.
pub(crate) fn enclosing_scopes(qn: &[u8]) -> impl Iterator<Item = &[u8]> {
    let fits = qn.len() < SCOPE_LIMIT;
    let mut end = if fits { qn.len() } else { 0 };
    std::iter::from_fn(move || {
        let dot = qn[..end].iter().rposition(|&b| b == b'.')?;
        end = dot;
        Some(&qn[..dot])
    })
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    find(haystack, needle).is_some()
}

#[cfg(feature = "native-engine-abi")]
mod native {
    use super::{
        enclosing_scopes, is_test_func_name, is_test_path, props_mark_test,
        test_to_prod_path,
    };
    use crate::native_graph_buffer::{
        Edge, Node, cbm_gbuf_find_by_id, cbm_gbuf_find_by_label,
        cbm_gbuf_find_by_qn, cbm_gbuf_find_edges_by_type, cbm_gbuf_insert_edge,
    };
    use std::ffi::{CStr, CString, c_char, c_int, c_void};
    use std::ptr::null;
    use std::slice;

    /// CBM_LANG_PYTHON in internal/cbm/cbm.h.
    const LANG_PYTHON: c_int = 1;
    const LOG_INFO: c_int = 1;

    unsafe extern "C" {
        fn cbm_is_test_file(rel_path: *const c_char, lang: c_int) -> bool;
        fn cbm_registry_test_double_path(rel_path: *const c_char) -> bool;
        fn cbm_language_for_filename(filename: *const c_char) -> c_int;
        fn cbm_pipeline_fqn_compute(
            project: *const c_char,
            rel_path: *const c_char,
            name: *const c_char,
        ) -> *mut c_char;
        fn cbm_log(level: c_int, msg: *const c_char, ...);
    }

    // SAFETY: the caller keeps the native owner of ptr alive for the borrow.
    unsafe fn bytes<'a>(ptr: *const c_char) -> Option<&'a [u8]> {
        (!ptr.is_null()).then(|| unsafe { CStr::from_ptr(ptr) }.to_bytes())
    }

    // SAFETY: the caller passes null or a live graph node.
    unsafe fn node_is_test(node: *const Node) -> bool {
        let Some(node) = (unsafe { node.as_ref() }) else {
            return false;
        };
        unsafe { bytes(node.properties_json) }.is_some_and(props_mark_test)
    }

    // A node is test code when its properties say so or its file is a test file.
    unsafe fn marked_or_in_test_file(node: &Node) -> bool {
        unsafe {
            node_is_test(node) || bytes(node.file_path).is_some_and(is_test_path)
        }
    }

    fn is_function(node: &Node) -> bool {
        matches!(unsafe { bytes(node.label) }, Some(b"Function" | b"Method"))
    }

    /// A def declared inside a Python test function (a closure, a local
    /// helper, a method of a test-local class) is that test's own code. Its
    /// calls test the target on the test's behalf, so TESTS goes to the
    /// nearest enclosing test function of the same file. None when there is
    /// none: a helper nested in a non-test function gets no TESTS edge.
    unsafe fn enclosing_python_test(
        graph: *const c_void,
        src: &Node,
    ) -> Option<*const Node> {
        let qn = unsafe { bytes(src.qualified_name) }?;
        let file = unsafe { bytes(src.file_path) }?;
        if src.label.is_null() || !is_function(src) {
            return None;
        }
        let base = match file.iter().rposition(|&b| b == b'/') {
            Some(slash) => &file[slash + 1..],
            None => file,
        };
        let base = CString::new(base).ok()?;
        let lang = unsafe {
            cbm_language_for_filename(crate::ffi_output::c_str(&base))
        };
        if lang != LANG_PYTHON {
            return None;
        }
        for scope in enclosing_scopes(qn) {
            let scope = CString::new(scope).ok()?;
            let outer = unsafe {
                cbm_gbuf_find_by_qn(graph, crate::ffi_output::c_str(&scope))
            };
            let Some(outer_node) = (unsafe { outer.as_ref() }) else {
                continue;
            };
            if unsafe { bytes(outer_node.file_path) } != Some(file) {
                return None;
            }
            if is_function(outer_node)
                && unsafe { bytes(outer_node.name) }
                    .is_some_and(is_test_func_name)
            {
                return Some(outer);
            }
        }
        None
    }

    unsafe fn insert(graph: *mut c_void, source: i64, target: i64, kind: &CStr) {
        unsafe {
            cbm_gbuf_insert_edge(
                graph,
                source,
                target,
                kind.as_ptr(),
                c"{}".as_ptr(),
            )
        };
    }

    unsafe fn tests_edges(graph: *mut c_void) -> u64 {
        let mut edges: *const *const Edge = null();
        let mut count = 0;
        let rc = unsafe {
            cbm_gbuf_find_edges_by_type(
                graph,
                c"CALLS".as_ptr(),
                &mut edges,
                &mut count,
            )
        };
        if rc != 0 || count <= 0 {
            return 0;
        }
        // Copy the endpoints before any write to the graph.
        let calls: Vec<(i64, i64)> =
            unsafe { slice::from_raw_parts(edges, count as usize) }
                .iter()
                .map(|&e| unsafe { ((*e).source_id, (*e).target_id) })
                .collect();

        let mut written = 0;
        for (source_id, target_id) in calls {
            let src = unsafe { cbm_gbuf_find_by_id(graph, source_id) };
            let tgt = unsafe { cbm_gbuf_find_by_id(graph, target_id) };
            let (Some(src), Some(tgt)) =
                (unsafe { src.as_ref() }, unsafe { tgt.as_ref() })
            else {
                continue;
            };
            if !unsafe { marked_or_in_test_file(src) }
                || unsafe { marked_or_in_test_file(tgt) }
            {
                continue;
            }
            let test_id =
                if unsafe { bytes(src.name) }.is_some_and(is_test_func_name) {
                    src.id
                } else {
                    match unsafe { enclosing_python_test(graph, src) } {
                        Some(outer) => unsafe { (*outer).id },
                        None => continue,
                    }
                };
            unsafe { insert(graph, test_id, tgt.id, c"TESTS") };
            written += 1;
        }
        written
    }

    unsafe fn tests_file_edges(
        graph: *mut c_void,
        project: *const c_char,
    ) -> u64 {
        let mut nodes: *const *const Node = null();
        let mut count = 0;
        if unsafe {
            cbm_gbuf_find_by_label(
                graph,
                c"File".as_ptr(),
                &mut nodes,
                &mut count,
            )
        } != 0
            || count <= 0
        {
            return 0;
        }
        let files: Vec<*const Node> =
            unsafe { slice::from_raw_parts(nodes, count as usize) }.to_vec();

        let mut written = 0;
        for file in files {
            let file = unsafe { &*file };
            let Some(path) = (unsafe { bytes(file.file_path) }) else {
                continue;
            };
            if !is_test_path(path) {
                continue;
            }
            let Some(prod) = test_to_prod_path(path) else {
                continue;
            };
            let Ok(prod) = CString::new(prod) else {
                continue;
            };
            let qn = unsafe {
                cbm_pipeline_fqn_compute(
                    project,
                    crate::ffi_output::c_str(&prod),
                    c"__file__".as_ptr(),
                )
            };
            let prod_node = unsafe { cbm_gbuf_find_by_qn(graph, qn) };
            unsafe { libc::free(qn.cast()) };
            if let Some(prod_node) = unsafe { prod_node.as_ref() }
                && prod_node.id != file.id
            {
                unsafe { insert(graph, file.id, prod_node.id, c"TESTS_FILE") };
                written += 1;
            }
        }
        written
    }

    fn log_count(count: u64) -> CString {
        CString::new(count.to_string()).expect("digits have no NUL")
    }

    /// Run the TESTS pass over a native graph buffer. Returns 0.
    ///
    /// # Safety
    /// `graph` is a live graph buffer that this call may write, and
    /// `project` is null or a NUL-terminated project name.
    #[unsafe(no_mangle)]
    pub(crate) unsafe extern "C" fn mori_pipeline_pass_tests(
        graph: *mut c_void,
        project: *const c_char,
    ) -> c_int {
        unsafe {
            cbm_log(
                LOG_INFO,
                c"pass.start".as_ptr(),
                c"pass".as_ptr(),
                c"tests".as_ptr(),
                null::<c_char>(),
            )
        };
        let tests = unsafe { tests_edges(graph) };
        let tests_file = unsafe { tests_file_edges(graph, project) };
        let (tests, tests_file) = (log_count(tests), log_count(tests_file));
        unsafe {
            cbm_log(
                LOG_INFO,
                c"pass.done".as_ptr(),
                c"pass".as_ptr(),
                c"tests".as_ptr(),
                c"tests".as_ptr(),
                crate::ffi_output::c_str(&tests),
                c"tests_file".as_ptr(),
                crate::ffi_output::c_str(&tests_file),
                null::<c_char>(),
            )
        };
        0
    }

    /// # Safety
    /// `node` is null or a live graph node.
    #[unsafe(no_mangle)]
    unsafe extern "C" fn cbm_node_is_test(node: *const Node) -> bool {
        unsafe { node_is_test(node) }
    }

    /// Test or test-double code may use test code: the caller's node, the
    /// extractor's file rule or the test-double path rule says so.
    ///
    /// # Safety
    /// `source` is null or a live graph node; `rel` is null or NUL-terminated.
    #[unsafe(no_mangle)]
    unsafe extern "C" fn cbm_pipeline_caller_is_test(
        source: *const Node,
        rel: *const c_char,
        lang: c_int,
    ) -> bool {
        unsafe {
            node_is_test(source)
                || cbm_is_test_file(rel, lang)
                || cbm_registry_test_double_path(rel)
        }
    }

    /// # Safety
    /// `path` is null or NUL-terminated.
    #[unsafe(no_mangle)]
    unsafe extern "C" fn cbm_is_test_path(path: *const c_char) -> bool {
        unsafe { bytes(path) }.is_some_and(is_test_path)
    }

    /// # Safety
    /// `name` is null or NUL-terminated.
    #[unsafe(no_mangle)]
    unsafe extern "C" fn cbm_is_test_func_name(name: *const c_char) -> bool {
        unsafe { bytes(name) }.is_some_and(is_test_func_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_paths_by_language() {
        for path in [
            "foo_test.go",
            "test_handler.py",
            "handler.test.js",
            "handler.spec.ts",
            "Component.test.tsx",
            "OrderTest.java",
            "handler_test.rs",
            "handler_test.cpp",
            "OrderTest.cs",
            "OrderTest.php",
            "OrderSpec.scala",
            "OrderTest.kt",
            "handler_test.lua",
            "__tests__/Component.js",
            "tests/test_main.py",
            "spec/handler_spec.rb",
            "src/test/Thing.java",
            "pkg/tests/util.go",
            "app/spec/model.rb",
            "lib/x_spec.rb",
            "dir/test_x.go",
        ] {
            assert!(is_test_path(path.as_bytes()), "{path}");
        }
        for path in [
            "",
            "foo.go",
            "handler.py",
            "handler.ts",
            "Component.tsx",
            "Order.java",
            "Order.scala",
            "contest.go",
            "latest.py",
            "testdata/x.go",
            "src/tests",
            "dir/mytest_x.go",
        ] {
            assert!(!is_test_path(path.as_bytes()), "{path}");
        }
    }

    #[test]
    fn test_function_names() {
        for name in [
            "Test",
            "TestCreate",
            "TestHTTPHandler",
            "Benchmark",
            "BenchmarkSort",
            "Example",
            "ExampleUse",
            "test_create",
            "testCreate",
            "test",
            "it",
            "describe",
            "beforeAll",
            "afterAll",
            "beforeEach",
            "afterEach",
            "@testset",
            "@test",
        ] {
            assert!(is_test_func_name(name.as_bytes()), "{name}");
        }
        for name in [
            "",
            "Testable",
            "Benchmarking",
            "Examples",
            "testing",
            "tests",
            "create",
            "handleRequest",
            "its",
            "@tests",
        ] {
            assert!(!is_test_func_name(name.as_bytes()), "{name}");
        }
    }

    #[test]
    fn production_paths_from_test_names() {
        let prod = |p: &str| {
            test_to_prod_path(p.as_bytes())
                .map(|v| String::from_utf8(v).unwrap())
        };
        assert_eq!(prod("pkg/foo_test.go").as_deref(), Some("pkg/foo.go"));
        assert_eq!(prod("foo_test.go").as_deref(), Some("foo.go"));
        assert_eq!(prod("/foo_test.go").as_deref(), Some("foo.go"));
        assert_eq!(prod("a/test_foo.py").as_deref(), Some("a/foo.py"));
        assert_eq!(prod("test_.py").as_deref(), Some(".py"));
        assert_eq!(prod("src/foo.test.ts").as_deref(), Some("src/foo.ts"));
        assert_eq!(prod("foo.spec.tsx").as_deref(), Some("foo.tsx"));
        assert_eq!(prod("a.test.b.spec.js").as_deref(), Some("a.b.spec.js"));
        // The first marker must end the name.
        assert_eq!(prod("x_test.go_test.go"), None);
        assert_eq!(prod("test_a.py.py"), None);
        assert_eq!(prod("tests/util.go"), None);
        assert_eq!(prod("OrderTest.java"), None);
    }

    #[test]
    fn scopes_walk_outward_and_stop_at_the_limit() {
        let scopes: Vec<&[u8]> = enclosing_scopes(b"p.m.f.g").collect();
        assert_eq!(scopes, [&b"p.m.f"[..], b"p.m", b"p"]);
        assert_eq!(enclosing_scopes(b"plain").count(), 0);
        let long = b"a.".repeat(SCOPE_LIMIT / 2);
        assert_eq!(enclosing_scopes(&long).count(), 0);
        let fits = &long[..SCOPE_LIMIT - 1];
        assert!(enclosing_scopes(fits).count() > 0);
    }

    #[test]
    fn properties_mark_tests_only_with_the_exact_key() {
        assert!(props_mark_test(br#"{"is_test":true}"#));
        assert!(!props_mark_test(br#"{"is_test": true}"#));
        assert!(!props_mark_test(br#"{"is_test":false}"#));
    }
}
