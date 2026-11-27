use super::*;
use crate::ffi_output;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::{ptr, slice};

const CANDIDATES_NOTE: &[u8] =
    b"doc<->code candidates by shared terms or the doc's folder, not links: p is the judged probability the section is about the target; verify";

#[repr(C)]
struct Node {
    id: i64,
    project: *const c_char,
    label: *const c_char,
    name: *const c_char,
    qualified_name: *const c_char,
    file_path: *const c_char,
    start_line: c_int,
    end_line: c_int,
    properties_json: *const c_char,
}

#[repr(C)]
struct CoverageRow {
    path: *const c_char,
    kind: *const c_char,
    detail: *const c_char,
}

// C retains all strings and arrays for the synchronous call.
#[repr(C)]
pub(super) struct Input {
    node: *const Node,
    window: *const Window,
    store: *mut c_void,
    display_path: *const c_char,
    generation: *const c_char,
    source: *const c_char,
    freshness: *const c_char,
    match_method: *const c_char,
    members: *const Node,
    coverage: *const CoverageRow,
    caller_names: *const *const c_char,
    callee_names: *const *const c_char,
    member_count: c_int,
    coverage_count: c_int,
    caller_count: c_int,
    callee_count: c_int,
    member_offset: c_int,
    member_limit: c_int,
    callers: c_int,
    callees: c_int,
    content_verified: bool,
    source_drift: bool,
    json_format: bool,
}

#[repr(C)]
struct Candidate {
    node: *const Node,
    probability: f64,
    kind: *const c_char,
    position: *const c_char,
    shared_terms: c_int,
    name_tokens: c_int,
    has_shared_terms: bool,
    has_name_tokens: bool,
}

type CandidateSink = unsafe extern "C" fn(*mut c_void, *const Candidate);

unsafe extern "C" {
    fn cbm_json_to_tree(json: *const c_char) -> *mut c_char;
    fn cbm_mcp_snippet_free_tree(tree: *mut c_char);
    fn cbm_mcp_text_result(text: *const c_char, is_error: bool) -> *mut c_char;
    fn cbm_mcp_snippet_visit_candidates(
        store: *mut c_void,
        node: *const Node,
        by_section: bool,
        emit: CandidateSink,
        context: *mut c_void,
    );
}

// SAFETY: every caller has a C-owned string live for the requested borrow.
unsafe fn optional_bytes<'a>(text: *const c_char) -> Option<&'a [u8]> {
    // SAFETY: Non-null strings at this boundary are NUL-terminated.
    (!text.is_null()).then(|| unsafe { CStr::from_ptr(text) }.to_bytes())
}

unsafe fn bytes<'a>(text: *const c_char) -> &'a [u8] {
    // SAFETY: The caller lends the string as specified by optional_bytes.
    unsafe { optional_bytes(text) }.unwrap_or_default()
}

unsafe fn array<'a, T>(ptr: *const T, count: c_int) -> &'a [T] {
    if count <= 0 {
        return &[];
    }
    // SAFETY: A positive native count describes that many live entries.
    unsafe { slice::from_raw_parts(ptr, count as usize) }
}

fn tree_payload(json: Vec<u8>) -> Option<Vec<u8>> {
    let json = CString::new(json).expect("serialized JSON contains no NUL");
    // SAFETY: The shared compact renderer borrows the JSON during this call.
    let tree = unsafe { cbm_json_to_tree(ffi_output::c_str(&json)) };
    // SAFETY: The renderer returns a live C string or null. Use the C
    // destructor for its allocation, including on Windows.
    unsafe {
        let output = optional_bytes(tree).map(<[u8]>::to_vec);
        cbm_mcp_snippet_free_tree(tree);
        output
    }
}

fn wrap(payload: Option<Vec<u8>>) -> *mut c_char {
    // SAFETY: The shared MCP wrapper copies a live C string and returns a
    // C-owned result, freed by the caller using its usual allocator.
    unsafe {
        if let Some(payload) = payload {
            let text =
                CString::new(payload).expect("rendered payload contains no NUL");
            cbm_mcp_text_result(ffi_output::c_str(&text), false)
        } else {
            cbm_mcp_text_result(ffi_output::c_str(c"out of memory"), true)
        }
    }
}

fn coverage_note(path: &[u8], rows: &[CoverageRow]) -> Option<Vec<u8>> {
    for row in rows {
        // SAFETY: The coverage query owns these strings during the call.
        let note = unsafe {
            if optional_bytes(row.path) != Some(path) {
                continue;
            }
            match bytes(row.kind) {
                b"parse_unusable" => "The parse of this file failed across nearly the whole of it, so most constructs are missing from the graph and naming line ranges would not help. Read the source directly — the source above is ground truth. (best-effort signal)".as_bytes().to_vec(),
                b"parse_partial" => {
                    let detail = optional_bytes(row.detail).filter(|text| !text.is_empty()).unwrap_or(b"?");
                    [
                        "This file was only PARTIALLY indexed — line range(s) ".as_bytes(),
                        detail,
                        b" could not be parsed, so constructs there may be missing from the graph (callers/callees and search results can under-report this file). The source above is ground truth. (best-effort signal)",
                    ].concat()
                }
                _ => continue,
            }
        };
        let mut note = note;
        note.truncate(1023); // legacy snprintf buffer including its NUL
        return Some(note);
    }
    None
}

struct Candidates {
    by_section: bool,
    rows: Vec<Object<'static>>,
}

unsafe extern "C" fn collect_candidate(
    context: *mut c_void,
    candidate: *const Candidate,
) {
    // SAFETY: The native visitor lends the candidate and its resolved node
    // for this callback. The context is the Rust accumulator for this call.
    let (out, candidate, node) = unsafe {
        let candidate = &*candidate;
        (
            &mut *context.cast::<Candidates>(),
            candidate,
            &*candidate.node,
        )
    };
    let mut row = Object::default();
    // SAFETY: Copy borrowed fields before the visitor releases the native row.
    unsafe {
        row.text(
            if out.by_section {
                "qualified_name"
            } else {
                "section"
            },
            bytes(node.qualified_name).to_vec(),
        );
        if out.by_section {
            row.text("label", bytes(node.label).to_vec());
        }
        row.text("file_path", bytes(node.file_path).to_vec());
        row.int("start_line", node.start_line);
        if out.by_section {
            row.int("end_line", node.end_line);
        }
        row.0.push((
            "p",
            Field::Real(
                f64::from((candidate.probability * 1000.0 + 0.5) as i32)
                    / 1000.0,
            ),
        ));
        for (key, text) in
            [("kind", candidate.kind), ("position", candidate.position)]
        {
            if let Some(text) = optional_bytes(text) {
                row.text(key, text.to_vec());
            }
        }
    }
    if out.by_section {
        if candidate.has_shared_terms {
            row.int("shared_terms", candidate.shared_terms);
        }
        if candidate.has_name_tokens {
            row.int("name_tokens", candidate.name_tokens);
        }
    }
    out.rows.push(row);
}

fn append_candidates(result: &mut Object<'_>, store: *mut c_void, node: &Node) {
    if store.is_null() || node.qualified_name.is_null() || node.project.is_null()
    {
        return;
    }
    // SAFETY: C lends the node and its strings for response assembly.
    let by_section = match unsafe { bytes(node.label) } {
        b"Section" => true,
        b"Function" | b"Method" => false,
        _ => return,
    };
    let mut candidates = Candidates {
        by_section,
        rows: Vec::new(),
    };
    // SAFETY: The native query uses the request's store and borrows the
    // accumulator only while invoking the synchronous callback.
    unsafe {
        cbm_mcp_snippet_visit_candidates(
            store,
            node,
            by_section,
            collect_candidate,
            ptr::from_mut(&mut candidates).cast(),
        );
    }
    if !candidates.rows.is_empty() {
        let key = if by_section {
            "possibly_about"
        } else {
            "possibly_described_in"
        };
        result.0.push((key, Field::Rows(candidates.rows)));
        result.text("candidates_note", CANDIDATES_NOTE);
    }
}

fn append_outline<'a>(
    result: &mut Object<'a>,
    candidates: &'a [Node],
    node_id: i64,
    window: &Window,
    limit: i32,
    offset: i32,
) -> Outline {
    let mut members: Vec<_> = candidates
        .iter()
        .filter(|node| {
            node.id != node_id
                && node.start_line >= window.original_start
                && node.end_line <= window.original_end
        })
        .collect();
    // SAFETY: Candidate strings stay live until response assembly finishes.
    members.sort_unstable_by_key(|&node| {
        (
            node.start_line,
            node.end_line,
            unsafe { bytes(node.qualified_name) },
            node.id,
        )
    });
    let outline = Outline {
        offset: offset.max(0),
        total: members.len() as i32,
    };
    let rows: Vec<_> = members
        .into_iter()
        .skip(outline.offset as usize)
        .take(limit.clamp(1, 500) as usize)
        .map(|node| {
            let mut row = Object::default();
            // SAFETY: Both native strings are borrowed from the live array.
            unsafe {
                row.text("qualified_name", bytes(node.qualified_name));
                row.text("label", bytes(node.label));
            }
            row.int("start_line", node.start_line);
            row.int("end_line", node.end_line);
            row
        })
        .collect();
    let returned = rows.len() as i32;
    result.text("source_mode", b"outline".as_slice());
    result.0.push(("members", Field::Rows(rows)));
    result.int("members_total", outline.total);
    outline.append_paging(result, returned);
    result.boolean("full_source_available", true);
    outline
}

/// # Safety
/// Input, window, node and all non-null fields must remain live for this call.
#[unsafe(no_mangle)]
unsafe extern "C" fn mori_snippet_response(input: *const Input) -> *mut c_char {
    // SAFETY: C supplies the borrowed input and its initialized records.
    let (input, node, window) = unsafe {
        let input = &*input;
        (input, &*input.node, &*input.window)
    };
    let mut result = Object::default();
    // SAFETY: All strings are borrowed from C or from the live source buffer.
    let source = unsafe {
        result.text("name", bytes(node.name));
        result.text("qualified_name", bytes(node.qualified_name));
        result.text("label", bytes(node.label));
        result.text("file_path", bytes(input.display_path));
        result.int("start_line", window.start);
        result.int("end_line", window.end);
        result.text("store_generation", bytes(input.generation));
        result.boolean("content_verified", input.content_verified);
        if input.source_drift {
            result.boolean("source_drift", true);
            result.text("freshness", bytes(input.freshness));
            result.text("recommended_action", b"reindex_then_retry".as_slice());
        }
        optional_bytes(input.source)
    };
    if window.clipped {
        result.boolean("source_truncated", true);
        result.boolean("source_clipped", true);
        result.int("next_start_line", window.end + 1);
        result.int("original_end_line", window.original_end);
    }
    let outline = if window.outline {
        // SAFETY: The file query's borrowed array has member_count records.
        let members = unsafe { array(input.members, input.member_count) };
        Some(append_outline(
            &mut result,
            members,
            node.id,
            window,
            input.member_limit,
            input.member_offset,
        ))
    } else {
        result.text("source_mode", b"full".as_slice());
        result.text("source", source.unwrap_or(b"(source not available)"));
        None
    };
    // SAFETY: Strings and arrays below are borrowed until response assembly finishes.
    unsafe {
        if let Some(method) = optional_bytes(input.match_method) {
            result.text("match_method", method);
        }
        result.int("callers", input.callers);
        result.int("callees", input.callees);
        if let Some(note) = coverage_note(
            bytes(node.file_path),
            array(input.coverage, input.coverage_count),
        ) {
            result.text("coverage_note", note);
        }
        for (key, ptr, count) in [
            ("caller_names", input.caller_names, input.caller_count),
            ("callee_names", input.callee_names, input.callee_count),
        ] {
            if count > 0 {
                let names = array(ptr, count)
                    .iter()
                    .filter_map(|name| optional_bytes(*name))
                    .map(|name| Text(Cow::Borrowed(name)))
                    .collect();
                result.0.push((key, Field::Names(names)));
            }
        }
    }
    append_candidates(&mut result, input.store, node);
    wrap(render_bounded(
        result,
        window,
        source,
        outline.as_ref(),
        |result| {
            let json = result.json();
            if input.json_format {
                Some(json)
            } else {
                tree_payload(json)
            }
        },
    ))
}

/// # Safety
/// C supplies a terminated input and count live nodes for this call.
#[unsafe(no_mangle)]
unsafe extern "C" fn mori_snippet_suggestions(
    input: *const c_char,
    nodes: *const Node,
    count: c_int,
) -> *mut c_char {
    let mut result = Object::default();
    result.text("status", b"ambiguous".as_slice());
    let mut message = format!("{count} matches for \"").into_bytes();
    // SAFETY: The caller keeps the input string and all node fields alive.
    unsafe {
        message.extend_from_slice(bytes(input));
        message.extend_from_slice(b"\". Pick a qualified_name from suggestions below, or use search_graph(name_pattern=\"...\") to narrow results.");
        message.truncate(511); // legacy snprintf buffer including its NUL
        result.text("message", message);
        let rows = array(nodes, count)
            .iter()
            .map(|node| {
                let mut row = Object::default();
                row.text("qualified_name", bytes(node.qualified_name));
                row.text("name", bytes(node.name));
                row.text("label", bytes(node.label));
                row.text("file_path", bytes(node.file_path));
                row
            })
            .collect();
        result.0.push(("suggestions", Field::Rows(rows)));
    }
    wrap(Some(result.json()))
}
