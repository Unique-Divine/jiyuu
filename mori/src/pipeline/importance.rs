//! Score each symbol's importance from its weighted degree.
//!
//! ```text
//! importance = sqrt(num_refs) * private * generic * distinctive * test
//! ```
//!
//! `num_refs` counts incoming CALLS and USAGE edges. A private name (leading
//! underscore) takes 0.1, a name defined in five or more distinct files 0.1,
//! a snake_case or camelCase name of eight or more bytes 10, and test
//! scaffolding (a test file, or the target of a TESTS edge) 0.1.
//!
//! The score is a numeric `importance` key in the node's existing properties,
//! so older indexes simply lack it. Two routes write it with the same rule
//! and the same property writer: the graph-buffer pass, run last among the
//! passes so every edge it counts exists, and the SQL recompute of the
//! closure-delta route, which rescores the whole staging store because the
//! distinct-file count is project-wide.
//!
//! The distinct-file count is memoized per distinct name, so a same-name
//! group of size k is scanned once rather than k times. The complexity suite
//! gates `g_importance_name_visits` on staying linear.

use std::ffi::c_int;
use std::sync::atomic::AtomicU64;

/// Symbols the graph-buffer pass scored.
#[unsafe(no_mangle)]
#[allow(non_upper_case_globals)]
static g_importance_nodes: AtomicU64 = AtomicU64::new(0);
/// Same-name group members visited while counting distinct files.
#[unsafe(no_mangle)]
#[allow(non_upper_case_globals)]
static g_importance_name_visits: AtomicU64 = AtomicU64::new(0);
/// Rows the SQL recompute rescored.
#[unsafe(no_mangle)]
#[allow(non_upper_case_globals)]
static g_importance_store_rows: AtomicU64 = AtomicU64::new(0);

const GENERIC_MIN_FILES: c_int = 5;
const DISTINCT_MIN_LEN: usize = 8;
/// The formatted score's buffer size in the C writer, NUL included.
const VALUE_LIMIT: usize = 32;

/// The scoring rule.
pub(crate) fn score(
    num_refs: c_int,
    name_distinct_files: c_int,
    private_name: bool,
    distinctive_name: bool,
    test_scaffolding: bool,
) -> f64 {
    let mut score = f64::from(num_refs.max(0)).sqrt();
    if private_name {
        score *= 0.1;
    }
    if name_distinct_files >= GENERIC_MIN_FILES {
        score *= 0.1;
    }
    if distinctive_name {
        score *= 10.0;
    }
    if test_scaffolding {
        score *= 0.1;
    }
    score
}

/// The facts gathered for one symbol.
pub(crate) struct Inputs<'a> {
    pub(crate) name: Option<&'a [u8]>,
    pub(crate) file_path: Option<&'a [u8]>,
    pub(crate) num_refs: c_int,
    pub(crate) name_distinct_files: c_int,
    pub(crate) tests_target: bool,
}

impl Inputs<'_> {
    pub(crate) fn score(&self) -> f64 {
        let name = self.name.unwrap_or_default();
        let test_scaffolding =
            self.file_path.is_some_and(crate::test_edges::is_test_path)
                || self.tests_target;
        score(
            self.num_refs,
            self.name_distinct_files,
            name.first() == Some(&b'_'),
            is_distinctive(name),
            test_scaffolding,
        )
    }
}

/// snake_case (an embedded `_`) or camelCase (a lower-to-upper hump), at
/// least eight bytes long. A long plain lowercase word is not distinctive.
fn is_distinctive(name: &[u8]) -> bool {
    name.len() >= DISTINCT_MIN_LEN
        && (name.contains(&b'_')
            || name
                .windows(2)
                .any(|w| w[0].is_ascii_lowercase() && w[1].is_ascii_uppercase()))
}

/// isspace in the C locale.
fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// printf's `%.6f`, or None when it would not fit the writer's buffer.
fn format_score(score: f64) -> Option<Vec<u8>> {
    let text = if score.is_nan() {
        if score.is_sign_negative() {
            "-nan"
        } else {
            "nan"
        }
        .to_owned()
    } else if score.is_infinite() {
        if score < 0.0 { "-inf" } else { "inf" }.to_owned()
    } else {
        format!("{score:.6}")
    };
    (text.len() < VALUE_LIMIT).then(|| text.into_bytes())
}

/// The end of the JSON value starting at `start`, or None if unterminated.
/// Nested values are skipped whole, so an overwrite cannot truncate them.
fn value_end(json: &[u8], start: usize) -> Option<usize> {
    let mut p = start;
    match json.get(p) {
        Some(b'"') => {
            p += 1;
            while p < json.len() {
                if json[p] == b'\\' && p + 1 < json.len() {
                    p += 2;
                    continue;
                }
                if json[p] == b'"' {
                    return Some(p + 1);
                }
                p += 1;
            }
            None
        }
        Some(b'{' | b'[') => {
            let mut depth = 0;
            let mut in_string = false;
            while p < json.len() {
                let b = json[p];
                if in_string {
                    if b == b'\\' && p + 1 < json.len() {
                        p += 2;
                        continue;
                    }
                    if b == b'"' {
                        in_string = false;
                    }
                } else if b == b'"' {
                    in_string = true;
                } else if b == b'{' || b == b'[' {
                    depth += 1;
                } else if b == b'}' || b == b']' {
                    depth -= 1;
                    if depth == 0 {
                        return Some(p + 1);
                    }
                }
                p += 1;
            }
            None
        }
        _ => {
            while p < json.len() && !matches!(json[p], b',' | b'}' | b']') {
                p += 1;
            }
            Some(p)
        }
    }
}

/// The `"importance"` key in key position: after `{` or `,` and before `:`,
/// whitespace aside. Text inside a string value does not count.
fn find_key(json: &[u8]) -> Option<usize> {
    const KEY: &[u8] = b"\"importance\"";
    let mut from = 0;
    while let Some(found) = json[from..]
        .windows(KEY.len())
        .position(|w| w == KEY)
        .map(|at| from + at)
    {
        let before = json[..found]
            .iter()
            .rposition(|&b| !is_space(b))
            .map(|at| json[at]);
        let after = json[found + KEY.len()..]
            .iter()
            .find(|&&b| !is_space(b))
            .copied();
        if matches!(before, Some(b'{' | b',')) && after == Some(b':') {
            return Some(found);
        }
        from = found + KEY.len();
    }
    None
}

/// `json` with its `importance` key set to `score`. An existing key is
/// overwritten in place, never appended twice: rescoring routes see nodes
/// that already carry it. None when `json` is not an object or the value
/// does not fit; callers then leave the properties alone.
pub(crate) fn set_prop(json: Option<&[u8]>, score: f64) -> Option<Vec<u8>> {
    let old = json.unwrap_or(b"{}");
    if old.len() < 2 || old.last() != Some(&b'}') {
        return None;
    }
    let value = format_score(score)?;
    if let Some(key) = find_key(old) {
        let colon = key + old[key..].iter().position(|&b| b == b':')?;
        let mut start = colon + 1;
        while start < old.len() && is_space(old[start]) {
            start += 1;
        }
        let end = value_end(old, start)?;
        return Some([&old[..start], &value, &old[end..]].concat());
    }
    let comma: &[u8] = if old.len() == 2 { b"" } else { b"," };
    Some(
        [
            &old[..old.len() - 1],
            comma,
            b"\"importance\":",
            &value,
            b"}",
        ]
        .concat(),
    )
}

#[cfg(feature = "native-engine-abi")]
mod native {
    use super::{
        Inputs, g_importance_name_visits, g_importance_nodes,
        g_importance_store_rows, set_prop,
    };
    use crate::native_graph_buffer::{
        Edge, Node, cbm_gbuf_find_by_label, cbm_gbuf_find_by_name,
        cbm_gbuf_find_edges_by_target_type, cbm_gbuf_node_set_properties_json,
    };
    use rusqlite::ffi;
    use std::collections::{HashMap, HashSet};
    use std::ffi::{CStr, CString, c_char, c_double, c_int, c_void};
    use std::ptr::{null, null_mut};
    use std::slice;
    use std::sync::atomic::Ordering::Relaxed;

    const LOG_INFO: c_int = 1;
    const LOG_ERROR: c_int = 3;
    const STORE_OK: c_int = 0;
    const LABELS: [&CStr; 3] = [c"Function", c"Method", c"Class"];

    unsafe extern "C" {
        fn cbm_log(level: c_int, msg: *const c_char, ...);
        fn cbm_store_get_db(store: *mut c_void) -> *mut ffi::sqlite3;
        fn cbm_store_begin(store: *mut c_void) -> c_int;
        fn cbm_store_commit(store: *mut c_void) -> c_int;
        fn cbm_store_rollback(store: *mut c_void) -> c_int;
    }

    // SAFETY: the caller keeps the native owner of ptr alive for the borrow.
    unsafe fn bytes<'a>(ptr: *const c_char) -> Option<&'a [u8]> {
        (!ptr.is_null()).then(|| unsafe { CStr::from_ptr(ptr) }.to_bytes())
    }

    unsafe fn log_done(event: &CStr, count: u64) {
        let count = CString::new(count.to_string()).expect("digits have no NUL");
        // SAFETY: every argument is a live C string and the list ends in NULL.
        unsafe {
            cbm_log(
                LOG_INFO,
                event.as_ptr(),
                c"symbols".as_ptr(),
                crate::ffi_output::c_str(&count),
                c"implementation".as_ptr(),
                c"rust".as_ptr(),
                null::<c_char>(),
            )
        };
    }

    /// Hand C a malloc'd copy that its caller releases with free().
    fn malloc_copy(value: &[u8]) -> *mut c_char {
        // SAFETY: the allocation holds the bytes and their terminator.
        unsafe {
            let out = libc::malloc(value.len() + 1).cast::<u8>();
            if out.is_null() {
                return out.cast();
            }
            out.copy_from_nonoverlapping(value.as_ptr(), value.len());
            *out.add(value.len()) = 0;
            crate::ffi_output::written(out, value.len() + 1);
            out.cast()
        }
    }

    // Keep this layout aligned with cbm_importance_inputs_t.
    #[repr(C)]
    struct NativeInputs {
        name: *const c_char,
        file_path: *const c_char,
        num_refs: c_int,
        name_distinct_files: c_int,
        tests_target: bool,
    }

    /// # Safety
    /// `inputs` is null or points at inputs whose strings are null or
    /// NUL-terminated.
    #[unsafe(no_mangle)]
    unsafe extern "C" fn cbm_pipeline_importance_score(
        inputs: *const NativeInputs,
    ) -> c_double {
        let Some(inputs) = (unsafe { inputs.as_ref() }) else {
            return 0.0;
        };
        Inputs {
            name: unsafe { bytes(inputs.name) },
            file_path: unsafe { bytes(inputs.file_path) },
            num_refs: inputs.num_refs,
            name_distinct_files: inputs.name_distinct_files,
            tests_target: inputs.tests_target,
        }
        .score()
    }

    /// Returns a malloc'd copy of `json` with `importance` set, or null.
    ///
    /// # Safety
    /// `json` is null or NUL-terminated.
    #[unsafe(no_mangle)]
    unsafe extern "C" fn cbm_pipeline_importance_set_prop(
        json: *const c_char,
        score: c_double,
    ) -> *mut c_char {
        match set_prop(unsafe { bytes(json) }, score) {
            Some(props) => malloc_copy(&props),
            None => null_mut(),
        }
    }

    /// # Safety
    /// `node` is null or a live node of a graph this call may write.
    #[unsafe(no_mangle)]
    unsafe extern "C" fn cbm_pipeline_importance_append_prop(
        node: *mut Node,
        score: c_double,
    ) {
        let Some(current) = (unsafe { node.as_ref() }) else {
            return;
        };
        let Some(props) =
            set_prop(unsafe { bytes(current.properties_json) }, score)
        else {
            return;
        };
        let props = CString::new(props).expect("properties come from C strings");
        // The setter copies the string into the graph's allocator.
        unsafe {
            cbm_gbuf_node_set_properties_json(
                node,
                crate::ffi_output::c_str(&props),
            )
        };
    }

    struct Graph<'a> {
        graph: *const c_void,
        /// Distinct files per name. Keys borrow node names, which outlive
        /// the pass.
        files_per_name: HashMap<&'a [u8], c_int>,
    }

    impl<'a> Graph<'a> {
        fn incoming(&self, id: i64, kind: &CStr) -> c_int {
            let mut edges: *const *const Edge = null();
            let mut count = 0;
            // SAFETY: the pass holds the live graph for this call.
            let rc = unsafe {
                cbm_gbuf_find_edges_by_target_type(
                    self.graph,
                    id,
                    kind.as_ptr(),
                    &mut edges,
                    &mut count,
                )
            };
            if rc == 0 { count } else { 0 }
        }

        /// Distinct files `name` is defined in. Two definitions in one file
        /// count once. Computed once per distinct name.
        fn distinct_files(&mut self, name: Option<&'a [u8]>) -> c_int {
            let Some(name) = name.filter(|n| !n.is_empty()) else {
                return 0;
            };
            if let Some(&files) = self.files_per_name.get(name) {
                return files;
            }
            let mut nodes: *const *const Node = null();
            let mut count = 0;
            let mut files = 0;
            let key = CString::new(name).expect("names come from C strings");
            // SAFETY: the pass holds the live graph; the array is read
            // before any write.
            if unsafe {
                cbm_gbuf_find_by_name(
                    self.graph,
                    crate::ffi_output::c_str(&key),
                    &mut nodes,
                    &mut count,
                )
            } == 0
                && count > 0
            {
                g_importance_name_visits.fetch_add(count as u64, Relaxed);
                let nodes =
                    unsafe { slice::from_raw_parts(nodes, count as usize) };
                let paths: HashSet<&[u8]> = nodes
                    .iter()
                    .filter_map(|&n| unsafe { bytes((*n).file_path) })
                    .collect();
                files = paths.len() as c_int;
            }
            self.files_per_name.insert(name, files);
            files
        }
    }

    /// Score every Function, Method and Class in a native graph buffer.
    ///
    /// # Safety
    /// `graph` is null or a live graph buffer that this call may write.
    #[unsafe(no_mangle)]
    unsafe extern "C" fn mori_pipeline_pass_importance(graph: *mut c_void) {
        if graph.is_null() {
            return;
        }
        let mut index = Graph {
            graph,
            files_per_name: HashMap::new(),
        };
        let mut updated = 0;
        for label in LABELS {
            let mut nodes: *const *const Node = null();
            let mut count = 0;
            if unsafe {
                cbm_gbuf_find_by_label(
                    graph,
                    label.as_ptr(),
                    &mut nodes,
                    &mut count,
                )
            } != 0
                || count <= 0
            {
                continue;
            }
            let nodes: Vec<*const Node> =
                unsafe { slice::from_raw_parts(nodes, count as usize) }.to_vec();
            for node in nodes {
                let n = unsafe { &*node };
                let name = unsafe { bytes(n.name) };
                let inputs = Inputs {
                    name,
                    file_path: unsafe { bytes(n.file_path) },
                    num_refs: index.incoming(n.id, c"CALLS")
                        + index.incoming(n.id, c"USAGE"),
                    name_distinct_files: index.distinct_files(name),
                    tests_target: index.incoming(n.id, c"TESTS") > 0,
                };
                let score = inputs.score();
                unsafe {
                    cbm_pipeline_importance_append_prop(node.cast_mut(), score)
                };
                updated += 1;
            }
        }
        g_importance_nodes.fetch_add(updated, Relaxed);
        unsafe { log_done(c"pass.importance", updated) };
    }

    // SAFETY: SQLite passes argc live values for the duration of the call.
    unsafe fn args<'a>(
        argc: c_int,
        argv: *mut *mut ffi::sqlite3_value,
    ) -> &'a [*mut ffi::sqlite3_value] {
        unsafe { slice::from_raw_parts(argv, argc.max(0) as usize) }
    }

    // SAFETY: the value is live for the UDF call.
    unsafe fn value_text<'a>(
        value: *mut ffi::sqlite3_value,
    ) -> Option<&'a [u8]> {
        unsafe { bytes(ffi::sqlite3_value_text(value).cast()) }
    }

    /// cbm_imp_score(name, file_path, num_refs, name_files, tests_target)
    unsafe extern "C" fn sql_score(
        context: *mut ffi::sqlite3_context,
        argc: c_int,
        argv: *mut *mut ffi::sqlite3_value,
    ) {
        if argc != 5 {
            unsafe {
                ffi::sqlite3_result_error(
                    context,
                    c"cbm_imp_score arity".as_ptr(),
                    -1,
                )
            };
            return;
        }
        let argv = unsafe { args(argc, argv) };
        let inputs = unsafe {
            Inputs {
                name: value_text(argv[0]),
                file_path: value_text(argv[1]),
                num_refs: ffi::sqlite3_value_int(argv[2]),
                name_distinct_files: ffi::sqlite3_value_int(argv[3]),
                tests_target: ffi::sqlite3_value_int(argv[4]) != 0,
            }
        };
        unsafe { ffi::sqlite3_result_double(context, inputs.score()) };
    }

    /// cbm_imp_set(properties, score): NULL for a non-object blob, which the
    /// UPDATE's COALESCE turns into "leave the row alone".
    unsafe extern "C" fn sql_set(
        context: *mut ffi::sqlite3_context,
        argc: c_int,
        argv: *mut *mut ffi::sqlite3_value,
    ) {
        if argc != 2 {
            unsafe {
                ffi::sqlite3_result_error(
                    context,
                    c"cbm_imp_set arity".as_ptr(),
                    -1,
                )
            };
            return;
        }
        let argv = unsafe { args(argc, argv) };
        let props = unsafe { value_text(argv[0]) };
        let score = unsafe { ffi::sqlite3_value_double(argv[1]) };
        match set_prop(props, score) {
            Some(props) => {
                crate::ffi_output::bytes(&props);
                unsafe {
                    ffi::sqlite3_result_text(
                        context,
                        props.as_ptr().cast(),
                        props.len() as c_int,
                        ffi::SQLITE_TRANSIENT(),
                    )
                }
            }
            None => unsafe { ffi::sqlite3_result_null(context) },
        }
    }

    /// The aggregate CTE is MATERIALIZED on purpose: it reads `nodes` while
    /// the UPDATE writes `nodes`, and materializing settles that order.
    const RECOMPUTE_SQL: &str = concat!(
        "WITH agg AS MATERIALIZED (",
        "  SELECT n.id AS id,",
        "         cbm_imp_score(n.name, n.file_path, COALESCE(d.refs, 0),",
        "                       COALESCE(f.files, 0), COALESCE(d.tests, 0)) AS score",
        "  FROM nodes n",
        "  LEFT JOIN (SELECT target_id AS id,",
        "                    SUM(CASE WHEN type IN ('CALLS','USAGE') THEN 1 ELSE 0 END) AS refs,",
        "                    SUM(CASE WHEN type = 'TESTS' THEN 1 ELSE 0 END) AS tests",
        "             FROM edges WHERE project = ?1 GROUP BY target_id) d ON d.id = n.id",
        "  LEFT JOIN (SELECT name, COUNT(DISTINCT file_path) AS files",
        "             FROM nodes WHERE project = ?1 GROUP BY name) f ON f.name = n.name",
        "  WHERE n.project = ?1 AND n.label IN ('Function','Method','Class')",
        ")",
        "UPDATE nodes SET properties = COALESCE(cbm_imp_set(properties, agg.score), properties)",
        " FROM agg WHERE nodes.id = agg.id",
    );

    unsafe fn log_sql_error(event: &CStr, db: *mut ffi::sqlite3) {
        // SAFETY: errmsg stays valid until the next call on db.
        unsafe {
            cbm_log(
                LOG_ERROR,
                event.as_ptr(),
                c"err".as_ptr(),
                ffi::sqlite3_errmsg(db),
                null::<c_char>(),
            )
        };
    }

    unsafe fn register_functions(db: *mut ffi::sqlite3) -> bool {
        type Udf = unsafe extern "C" fn(
            *mut ffi::sqlite3_context,
            c_int,
            *mut *mut ffi::sqlite3_value,
        );
        let functions: [(&CStr, c_int, Udf); 2] = [
            (c"cbm_imp_score", 5, sql_score),
            (c"cbm_imp_set", 2, sql_set),
        ];
        functions.into_iter().all(|(name, arity, udf)| {
            // SAFETY: db is live; the functions have process lifetime.
            unsafe {
                ffi::sqlite3_create_function_v2(
                    db,
                    name.as_ptr(),
                    arity,
                    ffi::SQLITE_UTF8 | ffi::SQLITE_DETERMINISTIC,
                    null_mut(),
                    Some(udf),
                    None,
                    None,
                    None,
                ) == ffi::SQLITE_OK
            }
        })
    }

    /// Rescore a whole project inside a store, in SQL, for the closure-delta
    /// route: its in-memory graph holds proxy nodes without project-wide
    /// edges. Run on the staging store after the delta patch has merged
    /// nodes and relinked inbound edges. Returns 0 on success.
    ///
    /// # Safety
    /// `store` is null or a live store with no other user during the call;
    /// `project` is null or NUL-terminated.
    #[unsafe(no_mangle)]
    unsafe extern "C" fn cbm_pipeline_importance_recompute_store(
        store: *mut c_void,
        project: *const c_char,
    ) -> c_int {
        if store.is_null() || project.is_null() {
            return -1;
        }
        let db = unsafe { cbm_store_get_db(store) };
        if db.is_null() {
            return -1;
        }
        if !unsafe { register_functions(db) } {
            unsafe { log_sql_error(c"importance.store_udf_failed", db) };
            return -1;
        }
        if unsafe { cbm_store_begin(store) } != STORE_OK {
            return -1;
        }
        let mut statement = null_mut();
        if unsafe {
            ffi::sqlite3_prepare_v2(
                db,
                RECOMPUTE_SQL.as_ptr().cast(),
                RECOMPUTE_SQL.len() as c_int,
                &mut statement,
                null_mut(),
            )
        } != ffi::SQLITE_OK
        {
            unsafe {
                log_sql_error(c"importance.store_prepare_failed", db);
                cbm_store_rollback(store);
            }
            return -1;
        }
        // SAFETY: the statement is live until finalized here.
        let step = unsafe {
            ffi::sqlite3_bind_text(
                statement,
                1,
                project,
                -1,
                ffi::SQLITE_TRANSIENT(),
            );
            let step = ffi::sqlite3_step(statement);
            ffi::sqlite3_finalize(statement);
            step
        };
        if step != ffi::SQLITE_DONE {
            unsafe {
                log_sql_error(c"importance.store_step_failed", db);
                cbm_store_rollback(store);
            }
            return -1;
        }
        let changed = unsafe { ffi::sqlite3_changes(db) };
        if unsafe { cbm_store_commit(store) } != STORE_OK {
            return -1;
        }
        let rows = changed.max(0) as u64;
        g_importance_store_rows.fetch_add(rows, Relaxed);
        unsafe { log_done(c"pass.importance_store", rows) };
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores_with_the_native_limits_and_multipliers() {
        assert_eq!(score(c_int::MIN, 5, true, true, true).to_bits(), 0);
        assert_eq!(score(9, 4, false, false, false), 3.0);
        assert_eq!(score(9, 5, false, false, false), 3.0 * 0.1);
        assert_eq!(score(9, 4, true, false, false), 3.0 * 0.1);
        assert_eq!(score(9, 4, false, true, false), 30.0);
        assert_eq!(score(9, 4, false, false, true), 3.0 * 0.1);
        assert_eq!(
            score(9, 5, true, true, true),
            (((3.0 * 0.1) * 0.1) * 10.0) * 0.1
        );
    }

    #[test]
    fn names_are_private_and_distinctive_by_shape() {
        assert!(!is_distinctive(b""));
        assert!(!is_distinctive(b"getUser"));
        assert!(is_distinctive(b"getURLId"));
        assert!(!is_distinctive(b"longword"));
        assert!(is_distinctive(b"get_name"));
        assert!(is_distinctive(b"_long_key"));
        let scored = |name: &'static [u8], path: Option<&'static [u8]>| {
            Inputs {
                name: Some(name),
                file_path: path,
                num_refs: 4,
                name_distinct_files: 1,
                tests_target: false,
            }
            .score()
        };
        assert_eq!(scored(b"_x", None), 2.0 * 0.1);
        assert_eq!(scored(b"x", Some(b"pkg/thing_test.go")), 2.0 * 0.1);
        assert_eq!(scored(b"x", Some(b"pkg/testutil_helpers.go")), 2.0);
    }

    fn set(json: Option<&str>, score: f64) -> Option<String> {
        set_prop(json.map(str::as_bytes), score)
            .map(|v| String::from_utf8(v).unwrap())
    }

    #[test]
    fn property_writer_appends_or_overwrites_once() {
        assert_eq!(
            set(None, 1.5).as_deref(),
            Some(r#"{"importance":1.500000}"#)
        );
        assert_eq!(
            set(Some(r#"{"a":1}"#), 2.0).as_deref(),
            Some(r#"{"a":1,"importance":2.000000}"#)
        );
        assert_eq!(
            set(Some(r#"{"importance":1.0,"b":2}"#), 3.0).as_deref(),
            Some(r#"{"importance":3.000000,"b":2}"#)
        );
        assert_eq!(
            set(Some(r#"{ "importance" :  {"x":[1,"}"]}, "b":2}"#), 0.5)
                .as_deref(),
            Some(r#"{ "importance" :  0.500000, "b":2}"#)
        );
        // A key-shaped string value is not the key.
        assert_eq!(
            set(Some(r#"{"doc":"\"importance\":1"}"#), 1.0).as_deref(),
            Some(r#"{"doc":"\"importance\":1","importance":1.000000}"#)
        );
        assert_eq!(set(Some("[]"), 1.0), None);
        assert_eq!(set(Some("{"), 1.0), None);
        assert_eq!(set(Some(r#"{"importance":"open}"#), 1.0), None);
        assert_eq!(set(Some("{}"), 1e40), None);
        // printf rounds an exact tie to even.
        assert_eq!(
            set(Some("{}"), 0.0078125).as_deref(),
            Some(r#"{"importance":0.007812}"#)
        );
        assert_eq!(
            set(Some("{}"), f64::INFINITY).as_deref(),
            Some(r#"{"importance":inf}"#)
        );
    }
}
