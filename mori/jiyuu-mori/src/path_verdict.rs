//! Exact-path coverage policy for the MCP tool check_index_coverage.

use std::ffi::{CStr, c_char};

#[repr(C)]
#[derive(Clone, Copy)]
struct CoverageRow {
    rel_path: *const c_char,
    kind: *const c_char,
    detail: *const c_char,
}

#[repr(C)]
struct PathRequest {
    rows: *const CoverageRow,
    row_count: usize,
    requested_path: *const c_char,
    recording_status: *const c_char,
    outside: bool,
    lookup_ok: bool,
    generation_matches: bool,
    exact_path_verified: bool,
    content_current: bool,
}

#[repr(C)]
struct PathVerdict {
    status: *const c_char,
    recommended_action: *const c_char,
}

#[derive(Clone, Copy)]
enum Status {
    OutsideProject,
    Unavailable,
    Unusable,
    Partial,
    Excluded,
    Skipped,
    UnresolvedCalls,
    NoRecordedIssue,
}

impl Status {
    fn text(self) -> &'static CStr {
        match self {
            Self::OutsideProject => c"outside_project",
            Self::Unavailable => c"coverage_unavailable",
            Self::Unusable => c"unusable",
            Self::Partial => c"partial",
            Self::Excluded => c"excluded",
            Self::Skipped => c"skipped",
            Self::UnresolvedCalls => c"unresolved_calls",
            Self::NoRecordedIssue => c"no_recorded_issue",
        }
    }

    fn recommendation(self, content_current: bool) -> &'static CStr {
        if !content_current {
            return c"read_source_and_reindex";
        }
        match self {
            Self::Partial => c"read_ranges_and_verify_scope",
            Self::UnresolvedCalls => c"read_source_and_verify_calls",
            Self::Unusable | Self::Skipped => c"read_source_directly",
            Self::Excluded => c"read_source_or_change_ignore_rules",
            Self::NoRecordedIssue => c"use_graph_with_best_effort_caveat",
            Self::OutsideProject | Self::Unavailable => {
                c"read_source_and_reindex"
            }
        }
    }
}

fn row_status(kind: &[u8]) -> Option<(u8, Status)> {
    match kind {
        b"parse_unusable" => Some((0, Status::Unusable)),
        b"parse_partial" => Some((0, Status::Partial)),
        kind if kind.starts_with(b"not_indexed") => Some((1, Status::Excluded)),
        b"unresolved_calls" => Some((3, Status::UnresolvedCalls)),
        b"" => None,
        _ => Some((2, Status::Skipped)),
    }
}

/// Null stands for an absent value in the borrowed C request.
unsafe fn bytes<'a>(value: *const c_char) -> &'a [u8] {
    if value.is_null() {
        &[]
    } else {
        // SAFETY: The request owns the live NUL-terminated value.
        unsafe { CStr::from_ptr(value) }.to_bytes()
    }
}

unsafe extern "C" fn decide_path(request: *const PathRequest) -> PathVerdict {
    // SAFETY: C supplies one live request, with row storage and all strings
    // borrowed for the call. A zero row count permits a null row pointer.
    let request = unsafe { &*request };
    let rows = if request.row_count == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(request.rows, request.row_count) }
    };
    let requested_path = unsafe { bytes(request.requested_path) };
    let recording_status = unsafe { bytes(request.recording_status) };
    let status = if request.outside {
        Status::OutsideProject
    } else if !request.lookup_ok {
        Status::Unavailable
    } else {
        let is_exact = |row: &CoverageRow| {
            !row.rel_path.is_null()
                && unsafe { bytes(row.rel_path) } == requested_path
        };
        let has_exact = rows.iter().any(is_exact);
        // Prefer exact rows to ancestor rows. Among the rows under
        // consideration, min_by_key keeps the first row in each priority.
        let best_row = rows
            .iter()
            .filter(|row| !has_exact || is_exact(row))
            .filter_map(|row| row_status(unsafe { bytes(row.kind) }))
            .min_by_key(|(priority, _)| *priority);
        if let Some((_, status)) = best_row {
            status
        } else if request.generation_matches
            && (recording_status == b"complete"
                || (request.exact_path_verified
                    && recording_status == b"truncated"))
        {
            Status::NoRecordedIssue
        } else {
            Status::Unavailable
        }
    };
    PathVerdict {
        status: status.text().as_ptr(),
        recommended_action: status
            .recommendation(request.content_current)
            .as_ptr(),
    }
}

#[cfg(not(test))]
unsafe extern "C" {
    fn cbm_mcp_set_coverage_verdict_reader(
        reader: unsafe extern "C" fn(*const PathRequest) -> PathVerdict,
    );
}

/// Register before C starts request threads.
#[cfg(not(test))]
pub(crate) unsafe fn register() {
    // SAFETY: The registered function lives for the process lifetime.
    unsafe { cbm_mcp_set_coverage_verdict_reader(decide_path) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;

    fn request(rows: &[CoverageRow]) -> PathRequest {
        PathRequest {
            rows: rows.as_ptr(),
            row_count: rows.len(),
            requested_path: c"src/file.rs".as_ptr(),
            recording_status: c"complete".as_ptr(),
            outside: false,
            lookup_ok: true,
            generation_matches: true,
            exact_path_verified: true,
            content_current: true,
        }
    }

    fn row(path: &CStr, kind: &CStr) -> CoverageRow {
        CoverageRow {
            rel_path: path.as_ptr(),
            kind: kind.as_ptr(),
            detail: ptr::null(),
        }
    }

    fn check(
        request: &PathRequest,
        expected_status: &CStr,
        expected_action: &CStr,
    ) {
        // SAFETY: Test requests supply live strings and row arrays, and
        // returned strings have static lifetime.
        let verdict = unsafe { decide_path(request) };
        assert_eq!(unsafe { CStr::from_ptr(verdict.status) }, expected_status);
        assert_eq!(
            unsafe { CStr::from_ptr(verdict.recommended_action) },
            expected_action
        );
    }

    #[test]
    fn exact_rows_hide_ancestors_and_keep_their_order_within_a_priority() {
        let exact = c"src/file.rs";
        let rows = [
            row(c"src", c"not_indexed_dir"),
            row(exact, c"unresolved_calls"),
        ];
        check(
            &request(&rows),
            c"unresolved_calls",
            c"read_source_and_verify_calls",
        );

        let rows = [
            row(exact, c"not_indexed_file"),
            row(exact, c"parse_partial"),
            row(exact, c"parse_unusable"),
        ];
        check(&request(&rows), c"partial", c"read_ranges_and_verify_scope");
        check(
            &request(&[rows[2], rows[1]]),
            c"unusable",
            c"read_source_directly",
        );
    }

    #[test]
    fn a_truncated_recording_requires_exact_verified_content() {
        let mut path = request(&[]);
        path.rows = ptr::null();
        check(
            &path,
            c"no_recorded_issue",
            c"use_graph_with_best_effort_caveat",
        );

        path.recording_status = c"truncated".as_ptr();
        check(
            &path,
            c"no_recorded_issue",
            c"use_graph_with_best_effort_caveat",
        );
        path.exact_path_verified = false;
        check(&path, c"coverage_unavailable", c"read_source_and_reindex");

        path.recording_status = c"unknown".as_ptr();
        path.exact_path_verified = true;
        check(&path, c"coverage_unavailable", c"read_source_and_reindex");

        path.recording_status = c"complete".as_ptr();
        path.generation_matches = false;
        check(&path, c"coverage_unavailable", c"read_source_and_reindex");
    }

    #[test]
    fn existing_rows_survive_stale_metadata_but_freshness_changes_the_action() {
        let rows = [row(c"src", c"not_indexed_dir")];
        let mut path = request(&rows);
        path.generation_matches = false;
        check(&path, c"excluded", c"read_source_or_change_ignore_rules");

        path.content_current = false;
        check(&path, c"excluded", c"read_source_and_reindex");
        path.lookup_ok = false;
        check(&path, c"coverage_unavailable", c"read_source_and_reindex");
        path.outside = true;
        check(&path, c"outside_project", c"read_source_and_reindex");
    }
}
