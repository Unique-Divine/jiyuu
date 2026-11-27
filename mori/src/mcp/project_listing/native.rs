use std::borrow::Cow;
use std::ffi::{CStr, CString, c_char, c_int, c_longlong, c_void};
use std::{panic, ptr};

use serde::Serialize;

use super::{
    ListError, ListOptions, ProjectCatalog, ProjectRecord, collect_projects,
};
use crate::{ffi_output, project_response::encode_text};

const EMPTY_HINT: &CStr =
    c"No projects indexed. Call index_repository(repo_path=...) first.";

const END: c_int = 0;
const RECORD: c_int = 1;
const CANCELLED: c_int = 3;

// Mirrors cbm_mcp_project_record_t. Moving the record does not move its
// C-owned strings. The C destructor frees them using their original allocator.
#[derive(Default)]
#[repr(C)]
struct NativeRecord {
    name: *const c_char,
    root_path: *const c_char,
    db_file: *const c_char,
    branch: *const c_char,
    nodes: c_int,
    edges: c_int,
    size_bytes: i64,
    populated: bool,
}

impl NativeRecord {
    fn identity(&self) -> (&CStr, &CStr, &CStr) {
        // SAFETY: successful native reads populate all three identity strings.
        unsafe {
            (
                CStr::from_ptr(self.name),
                CStr::from_ptr(self.root_path),
                CStr::from_ptr(self.db_file),
            )
        }
    }

    fn branch(&self) -> Option<&CStr> {
        // SAFETY: hydration supplies an owned C string or leaves a null pointer.
        (!self.branch.is_null()).then(|| unsafe { CStr::from_ptr(self.branch) })
    }
}

impl ProjectRecord for NativeRecord {
    fn sort_key(&self) -> (&[u8], &[u8], &[u8]) {
        let (name, root, file) = self.identity();
        (name.to_bytes(), root.to_bytes(), file.to_bytes())
    }
}

impl Drop for NativeRecord {
    fn drop(&mut self) {
        // SAFETY: this record owns the pointers populated by C. Fields left
        // unfilled have null pointers and C accepts them.
        unsafe {
            ffi_output::written(self, 1);
            cbm_mcp_project_catalog_release_record(self);
        }
    }
}

struct NativeCatalog(ptr::NonNull<c_void>);

impl NativeCatalog {
    fn open(server: *mut c_void) -> Result<Self, *mut c_char> {
        let mut error = ptr::null_mut();
        // SAFETY: the entry point borrows the native server for this request.
        // The error out parameter is writable Rust stack storage.
        let catalog =
            unsafe { cbm_mcp_project_catalog_open(server, &mut error) };
        ptr::NonNull::new(catalog).map(Self).ok_or(error)
    }
}

impl ProjectCatalog for NativeCatalog {
    type Record = NativeRecord;

    fn next_record(&mut self) -> Result<Option<NativeRecord>, ListError> {
        let mut record = NativeRecord::default();
        // SAFETY: C borrows the live catalog and fills a previously empty row.
        let result = unsafe {
            cbm_mcp_project_catalog_next(self.0.as_ptr(), &mut record)
        };
        match result {
            END => Ok(None),
            RECORD => Ok(Some(record)),
            CANCELLED => Err(ListError::Cancelled),
            _ => Err(ListError::Collect),
        }
    }

    fn hydrate(
        &mut self,
        record: &mut NativeRecord,
        include_stats: bool,
    ) -> Result<(), ListError> {
        // SAFETY: both handles are live and C borrows a unique row to hydrate it.
        let status = unsafe {
            ffi_output::written(record, 1);
            cbm_mcp_project_catalog_hydrate(
                self.0.as_ptr(),
                record,
                include_stats,
            )
        };
        match status {
            RECORD => Ok(()),
            CANCELLED => Err(ListError::Cancelled),
            _ => Err(ListError::Populate),
        }
    }
}

impl Drop for NativeCatalog {
    fn drop(&mut self) {
        // SAFETY: the live native catalog is uniquely owned by this guard.
        unsafe { cbm_mcp_project_catalog_close(self.0.as_ptr()) };
    }
}

// Mirrors cbm_sb_t. C owns the buffer and manages its capacity.
#[repr(C)]
#[derive(Default)]
struct TreeBuffer {
    buf: *mut c_char,
    len: usize,
    cap: usize,
    oom: bool,
}

impl Drop for TreeBuffer {
    fn drop(&mut self) {
        // SAFETY: C allocated this buffer through the compact tree API.
        unsafe { cbm_sb_free(self) }
    }
}

unsafe extern "C" {
    fn cbm_mcp_project_catalog_open(
        server: *mut c_void,
        error: *mut *mut c_char,
    ) -> *mut c_void;
    fn cbm_mcp_project_catalog_next(
        catalog: *mut c_void,
        record: *mut NativeRecord,
    ) -> c_int;
    fn cbm_mcp_project_catalog_hydrate(
        catalog: *mut c_void,
        record: *mut NativeRecord,
        include_stats: bool,
    ) -> c_int;
    fn cbm_mcp_project_catalog_release_record(record: *mut NativeRecord);
    fn cbm_mcp_project_catalog_close(catalog: *mut c_void);
    fn cbm_mcp_text_result(text: *const c_char, error: bool) -> *mut c_char;
    fn cbm_mcp_render_project_table(
        sb: *mut TreeBuffer,
        records: *const NativeRecord,
        count: c_int,
        metadata_only: bool,
        include_stats: bool,
    );
    fn cbm_sb_free(sb: *mut TreeBuffer);
    fn cbm_tree_scalar_int(
        sb: *mut TreeBuffer,
        key: *const c_char,
        value: c_longlong,
    );
    fn cbm_tree_scalar_bool(
        sb: *mut TreeBuffer,
        key: *const c_char,
        value: bool,
    );
    fn cbm_tree_scalar_str(
        sb: *mut TreeBuffer,
        key: *const c_char,
        value: *const c_char,
    );
}

#[derive(Serialize)]
struct ProjectRow<'a> {
    name: Cow<'a, str>,
    root_path: Cow<'a, str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch: Option<Cow<'a, str>>,
    #[serde(flatten)]
    stats: Option<ProjectStats>,
}

#[derive(Serialize)]
struct ProjectStats {
    nodes: c_int,
    edges: c_int,
    size_bytes: i64,
}

#[derive(Serialize)]
struct ProjectList<'a> {
    projects: Vec<ProjectRow<'a>>,
    total: usize,
    offset: c_int,
    limit: c_int,
    returned: usize,
    has_more: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_offset: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<&'a str>,
}

fn project_list_json(
    records: &[NativeRecord],
    total: usize,
    end: usize,
    options: &ListOptions,
) -> Option<CString> {
    let projects = records
        .iter()
        .map(|row| {
            let (name, root_path, _) = row.identity();
            let branch = match row.branch() {
                Some(branch) => Some(encode_text(branch)?),
                None => None,
            };
            Some(ProjectRow {
                name: encode_text(name)?,
                root_path: encode_text(root_path)?,
                branch,
                stats: options.include_stats.then_some(ProjectStats {
                    nodes: row.nodes,
                    edges: row.edges,
                    size_bytes: row.size_bytes,
                }),
            })
        })
        .collect::<Option<Vec<_>>>()?;
    let has_more = end < total;
    let response = ProjectList {
        projects,
        total,
        offset: options.offset,
        limit: options.limit,
        returned: records.len(),
        has_more,
        next_offset: has_more.then_some(end),
        hint: (total == 0).then(|| EMPTY_HINT.to_str().expect("ASCII hint")),
    };
    CString::new(serde_json::to_vec(&response).ok()?).ok()
}

fn project_list_tree(
    records: &[NativeRecord],
    total: usize,
    end: usize,
    options: &ListOptions,
) -> Option<CString> {
    let mut sb = TreeBuffer::default();
    let returned = records.len();
    let has_more = end < total;
    // SAFETY: C borrows the selected records and an initialized compact buffer.
    // The page has at most 500 rows and total fits in c_int.
    unsafe {
        ffi_output::written(&sb, 1);
        let row_ptr = if records.is_empty() {
            ptr::null()
        } else {
            ffi_output::written(records.as_ptr(), returned);
            records.as_ptr()
        };
        cbm_mcp_render_project_table(
            &mut sb,
            row_ptr,
            returned as c_int,
            options.metadata_only,
            options.include_stats,
        );
        cbm_tree_scalar_int(&mut sb, c"total".as_ptr(), total as c_longlong);
        cbm_tree_scalar_int(
            &mut sb,
            c"returned".as_ptr(),
            returned as c_longlong,
        );
        cbm_tree_scalar_bool(&mut sb, c"has_more".as_ptr(), has_more);
        if has_more {
            cbm_tree_scalar_int(
                &mut sb,
                c"next_offset".as_ptr(),
                end as c_longlong,
            );
        }
        if total == 0 {
            cbm_tree_scalar_str(&mut sb, c"hint".as_ptr(), EMPTY_HINT.as_ptr());
        }
        if sb.oom || sb.buf.is_null() {
            return None;
        }
        Some(CStr::from_ptr(sb.buf).to_owned())
    }
}

fn result(payload: &CStr, error: bool) -> *mut c_char {
    // SAFETY: C reads live, initialized bytes and returns a C-owned copy.
    unsafe { cbm_mcp_text_result(ffi_output::c_str(payload), error) }
}

fn list_projects(server: *mut c_void, arguments: &[u8]) -> *mut c_char {
    let options = ListOptions::decode(arguments);
    let mut catalog = match NativeCatalog::open(server) {
        Ok(catalog) => catalog,
        Err(native_error) => return native_error,
    };
    let records = match collect_projects(&mut catalog, &options) {
        Ok(records) => records,
        Err(error) => {
            let message = match error {
                ListError::Cancelled => {
                    c"list_projects cancelled for this request"
                }
                ListError::Collect => c"out of memory while listing projects",
                ListError::Populate => c"failed to populate project page",
            };
            return result(message, true);
        }
    };

    let total = records.len();
    let page = options.page(total);
    let end = page.end;
    let selected = &records[page];
    let payload = if options.wants_json {
        project_list_json(selected, total, end, &options)
    } else {
        project_list_tree(selected, total, end, &options)
    };
    match payload {
        Some(payload) => result(&payload, false),
        None => result(c"out of memory", true),
    }
}

// SAFETY: C supplies a live request context (or null without cancellation)
// and lends a NUL-terminated argument buffer for the duration of the request.
#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_mcp_list_projects(
    server: *mut c_void,
    args_json: *const c_char,
) -> *mut c_char {
    panic::catch_unwind(|| {
        let arguments = if args_json.is_null() {
            b"{}"
        } else {
            unsafe { CStr::from_ptr(args_json) }.to_bytes()
        };
        list_projects(server, arguments)
    })
    .unwrap_or_else(|_| result(c"out of memory", true))
}
