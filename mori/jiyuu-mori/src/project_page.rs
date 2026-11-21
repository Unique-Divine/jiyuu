use std::ffi::{CStr, c_char, c_int};
use std::slice;

// Mirrors cbm_mcp_project_record_t. Moving a row keeps its C-owned strings
// and statistics together. The C handler still frees each record.
#[repr(C)]
pub(crate) struct ProjectRecord {
    name: *const c_char,
    root_path: *const c_char,
    db_file: *const c_char,
    branch: *const c_char,
    nodes: c_int,
    edges: c_int,
    size_bytes: i64,
    populated: bool,
}

impl ProjectRecord {
    // SAFETY: Non-null keys must be live, NUL-terminated strings.
    unsafe fn sort_key(&self) -> (&[u8], &[u8], &[u8]) {
        let bytes = |key: *const c_char| {
            if key.is_null() {
                &[][..]
            } else {
                // Compare bytes like strcmp, including paths that aren't UTF-8.
                unsafe { CStr::from_ptr(key) }.to_bytes()
            }
        };
        (bytes(self.name), bytes(self.root_path), bytes(self.db_file))
    }
}

// Mirrors cbm_mcp_project_page_t.
#[repr(C)]
pub(crate) struct ProjectPage {
    limit: c_int,
    offset: c_int,
    start: c_int,
    end: c_int,
}

/// Sorts the discovered records before the C handler populates the page.
///
/// # Safety
/// `count` is nonnegative. If positive, `records` points to that many
/// initialized, writable records with live C string keys (or null keys).
/// `page` points to a distinct, initialized, writable page.
pub(crate) unsafe extern "C" fn build_project_page(
    records: *mut ProjectRecord,
    count: c_int,
    page: *mut ProjectPage,
) {
    if count > 1 {
        // SAFETY: C lends the entire array exclusively for this callback.
        let records =
            unsafe { slice::from_raw_parts_mut(records, count as usize) };
        // SAFETY: the C-owned strings remain live as their records move.
        records
            .sort_unstable_by(|a, b| unsafe { a.sort_key().cmp(&b.sort_key()) });
    }

    // SAFETY: C passes a separate page object with the parsed input bounds.
    let page = unsafe { &mut *page };
    page.limit = page.limit.clamp(1, 500);
    page.offset = page.offset.max(0);
    page.start = page.offset.min(count);
    page.end = page.start + page.limit.min(count - page.start);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;

    #[test]
    fn orders_name_root_and_db_file_before_slicing() {
        let row = |name: &CStr, root: &CStr, db: &CStr| ProjectRecord {
            name: name.as_ptr(),
            root_path: root.as_ptr(),
            db_file: db.as_ptr(),
            branch: ptr::null(),
            nodes: 0,
            edges: 0,
            size_bytes: 0,
            populated: false,
        };
        let mut records = [
            row(c"zeta", c"/a", c"a.db"),
            row(c"alpha", c"/a", c"\xFF.db"),
            row(c"alpha", c"/b", c"a.db"),
            row(c"alpha", c"/a", c"z.db"),
        ];
        let mut page = ProjectPage {
            limit: 2,
            offset: 1,
            start: 0,
            end: 0,
        };
        unsafe {
            build_project_page(
                records.as_mut_ptr(),
                records.len() as c_int,
                &mut page,
            )
        };
        assert_eq!((page.start, page.end), (1, 3));
        assert_eq!(
            records.each_ref().map(|row| unsafe { row.sort_key() }),
            [
                (&b"alpha"[..], &b"/a"[..], &b"z.db"[..]),
                (b"alpha", b"/a", b"\xFF.db"),
                (b"alpha", b"/b", b"a.db"),
                (b"zeta", b"/a", b"a.db"),
            ]
        );
    }

    #[test]
    fn clamps_arguments_and_accepts_an_empty_cache() {
        for (limit, offset, expected_limit, expected_offset) in
            [(0, -1, 1, 0), (c_int::MAX, c_int::MAX, 500, c_int::MAX)]
        {
            let mut page = ProjectPage {
                limit,
                offset,
                start: -1,
                end: -1,
            };
            unsafe { build_project_page(ptr::null_mut(), 0, &mut page) };
            assert_eq!(
                (page.limit, page.offset),
                (expected_limit, expected_offset)
            );
            assert_eq!((page.start, page.end), (0, 0));
        }
    }
}
