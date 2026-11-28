//! Read and materialize one outline page on the native store's connection.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::{mem, panic, ptr, slice};

mod request;

use rusqlite::ffi::{sqlite3, sqlite3_free, sqlite3_malloc64};
use rusqlite::types::{ToSqlOutput, ValueRef};
use rusqlite::{Connection, Error, ErrorCode, params_from_iter};

// Match CBM_STORE_* in store.h.
const OK: c_int = 0;
const ERR: c_int = -1;
const CANCELLED: c_int = -3;
const SCAN_LIMIT: c_int = -4;

#[repr(C)]
pub(crate) struct OutlineRow {
    name: *const c_char,
    label: *const c_char,
    qualified_name: *const c_char,
    start_line: c_int,
    end_line: c_int,
}

type Cancel = unsafe extern "C" fn(*mut c_void) -> bool;

struct Page {
    rows: *mut OutlineRow,
    len: usize,
    limit: usize,
    text_remaining: usize,
}

impl Page {
    fn new(limit: usize, text_budget: usize) -> Result<Self, c_int> {
        // SAFETY: The C store bounds limit. SQLite supplies aligned memory
        // and the C caller releases it through the same SQLite allocator.
        let rows = unsafe {
            sqlite3_malloc64((limit * mem::size_of::<OutlineRow>()) as u64)
        }
        .cast::<OutlineRow>();
        if rows.is_null() {
            return Err(ERR);
        }
        Ok(Self {
            rows,
            len: 0,
            limit,
            text_remaining: text_budget,
        })
    }

    fn push(
        &mut self,
        text: [&[u8]; 3],
        start_line: c_int,
        end_line: c_int,
    ) -> Result<(), c_int> {
        // Charge the full SQLite field lengths, even when the C renderer
        // stops at an embedded NUL.
        let text_bytes: usize = text.iter().map(|bytes| bytes.len() + 1).sum();
        if self.len == self.limit || text_bytes > self.text_remaining {
            return Err(SCAN_LIMIT);
        }
        self.text_remaining -= text_bytes;
        // SAFETY: This slot is within the allocation. Initialize its pointers
        // before increasing len so Drop can clean up a partially copied row.
        let row = unsafe {
            let slot = self.rows.add(self.len);
            slot.write(OutlineRow {
                name: ptr::null(),
                label: ptr::null(),
                qualified_name: ptr::null(),
                start_line,
                end_line,
            });
            self.len += 1;
            &mut *slot
        };
        row.name = copy_text(text[0])?;
        row.label = copy_text(text[1])?;
        row.qualified_name = copy_text(text[2])?;
        // SAFETY: All row fields now contain initialized values.
        unsafe { crate::ffi_output::written(row, 1) };
        Ok(())
    }

    fn into_raw(self) -> (*mut OutlineRow, c_int) {
        let parts = (self.rows, self.len as c_int);
        mem::forget(self);
        parts
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        // SAFETY: Only initialized slots are visited. SQLite accepts NULL
        // in free, and all other pointers came from its allocator.
        unsafe {
            for row in slice::from_raw_parts(self.rows, self.len) {
                sqlite3_free(row.name.cast_mut().cast());
                sqlite3_free(row.label.cast_mut().cast());
                sqlite3_free(row.qualified_name.cast_mut().cast());
            }
            sqlite3_free(self.rows.cast());
        }
    }
}

fn copy_text(bytes: &[u8]) -> Result<*const c_char, c_int> {
    let len = bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len());
    // SAFETY: On success the allocation has room for len bytes and a NUL.
    unsafe {
        let copy = sqlite3_malloc64((len + 1) as u64).cast::<u8>();
        if copy.is_null() {
            return Err(ERR);
        }
        ptr::copy_nonoverlapping(bytes.as_ptr(), copy, len);
        copy.add(len).write(0);
        crate::ffi_output::written(copy, len + 1);
        Ok(copy.cast())
    }
}

fn sqlite_status(error: Error) -> c_int {
    match error {
        Error::SqliteFailure(error, _)
            if error.code == ErrorCode::OperationInterrupted =>
        {
            CANCELLED
        }
        _ => ERR,
    }
}

fn read_page(
    db: &Connection,
    project: &[u8],
    path: &[u8],
    labels: &[&[u8]],
    offset: c_int,
    mut page: Page,
    mut cancelled: impl FnMut() -> bool,
) -> Result<(Page, c_int), c_int> {
    let mut filter = String::from(
        " FROM nodes WHERE project = ? AND file_path = ?
          AND label NOT IN ('Module','Package','File','Folder','Project')",
    );
    if !labels.is_empty() {
        let parameters = vec!["?"; labels.len()].join(",");
        filter.push_str(&format!(" AND label IN ({parameters})"));
    }
    // Paths and labels arrive as C bytes. Bind them as TEXT without recoding.
    let mut binds: Vec<_> = [project, path]
        .into_iter()
        .chain(labels.iter().copied())
        .map(|bytes| ToSqlOutput::Borrowed(ValueRef::Text(bytes)))
        .collect();

    let mut count_statement = db
        .prepare(&format!("SELECT COUNT(*){filter}"))
        .map_err(sqlite_status)?;
    let mut count_rows = count_statement
        .query(params_from_iter(&binds))
        .map_err(sqlite_status)?;
    let total = count_rows
        .next()
        .map_err(sqlite_status)?
        .ok_or(ERR)?
        .get::<_, i64>(0)
        .map_err(sqlite_status)? as c_int;
    // Keep COUNT stepped on its row until the page finishes. This pins the
    // same SQLite snapshot even if the caller has no explicit read batch.
    if cancelled() {
        return Err(CANCELLED);
    }
    let sql = format!(
        "SELECT CAST(name AS TEXT), CAST(label AS TEXT), CAST(qualified_name AS TEXT),
                COALESCE(CAST(start_line AS INTEGER), 0), COALESCE(CAST(end_line AS INTEGER), 0)
         {filter}
         ORDER BY start_line, end_line, label COLLATE BINARY,
                  name COLLATE BINARY, qualified_name COLLATE BINARY, id
         LIMIT ? OFFSET ?"
    );
    binds.push(ToSqlOutput::Borrowed(ValueRef::Integer(page.limit as i64)));
    binds.push(ToSqlOutput::Borrowed(ValueRef::Integer(offset.into())));
    let mut statement = db.prepare(&sql).map_err(sqlite_status)?;
    let mut rows = statement
        .query(params_from_iter(&binds))
        .map_err(sqlite_status)?;
    loop {
        if cancelled() {
            return Err(CANCELLED);
        }
        let Some(row) = rows.next().map_err(sqlite_status)? else {
            break;
        };
        let field = |index| match row.get_ref(index) {
            Ok(ValueRef::Text(bytes)) => Ok(bytes),
            Ok(_) => Ok(&[][..]),
            Err(error) => Err(sqlite_status(error)),
        };
        page.push(
            [field(0)?, field(1)?, field(2)?],
            row.get::<_, i64>(3).map_err(sqlite_status)? as c_int,
            row.get::<_, i64>(4).map_err(sqlite_status)? as c_int,
        )?;
    }
    Ok((page, total))
}

/// C validates all inputs and owns the progress handler on this connection.
/// On success the C store owns the row allocation and its strings.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mori_read_file_outline(
    db: *mut sqlite3,
    project: *const c_char,
    file_path: *const c_char,
    labels: *const *const c_char,
    label_count: c_int,
    limit: c_int,
    offset: c_int,
    text_budget: usize,
    cancel: Cancel,
    context: *mut c_void,
    out: *mut *mut OutlineRow,
    count: *mut c_int,
    total: *mut c_int,
) -> c_int {
    let result = panic::catch_unwind(|| {
        // SAFETY: The store lends its live connection, NUL-terminated
        // strings and callback context until this synchronous call returns.
        unsafe {
            let db = Connection::from_handle(db).map_err(sqlite_status)?;
            let labels = if label_count == 0 {
                &[]
            } else {
                slice::from_raw_parts(labels, label_count as usize)
            };
            let labels: Vec<_> = labels
                .iter()
                .map(|&label| CStr::from_ptr(label).to_bytes())
                .collect();
            read_page(
                &db,
                CStr::from_ptr(project).to_bytes(),
                CStr::from_ptr(file_path).to_bytes(),
                &labels,
                offset,
                Page::new(limit as usize, text_budget)?,
                || cancel(context),
            )
        }
    });
    match result {
        Ok(Ok((page, filtered_total))) => {
            // SAFETY: The store supplies distinct writable output fields.
            unsafe {
                let (rows, len) = page.into_raw();
                crate::ffi_output::write(out, rows);
                crate::ffi_output::write(count, len);
                crate::ffi_output::write(total, filtered_total);
            }
            OK
        }
        Ok(Err(status)) => status,
        Err(_) => ERR,
    }
}
