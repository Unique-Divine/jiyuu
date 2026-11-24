//! Graph digest exports for the native store and test-impact engine.

mod content;
mod graph;

use std::ffi::{CStr, c_int};
use std::ptr::{NonNull, null_mut};
use std::slice;

use rusqlite::ffi as sql;

const STORE_OK: c_int = 0;
const STORE_ERR: c_int = -1;
const STORE_CANCELLED: c_int = -3;

#[repr(C)]
struct Store {
    _opaque: [u8; 0],
}

#[repr(C)]
struct ReadScope {
    _opaque: [u8; 0],
}

unsafe extern "C" {
    fn cbm_store_get_db(store: *mut Store) -> *mut sql::sqlite3;
    fn cbm_store_read_scope_store(scope: *const ReadScope) -> *mut Store;
    fn cbm_store_read_scope_check(scope: *mut ReadScope) -> c_int;
    fn cbm_store_read_scope_fail(scope: *mut ReadScope, status: c_int) -> c_int;
}

struct Statement {
    db: NonNull<sql::sqlite3>,
    ptr: Option<NonNull<sql::sqlite3_stmt>>,
}

impl Statement {
    /// The connection must stay open until the statement is finalized.
    unsafe fn prepare(
        db: NonNull<sql::sqlite3>,
        query: &CStr,
    ) -> Result<Self, c_int> {
        let mut ptr = null_mut();
        // SAFETY: The caller borrows a live connection and query is NUL-terminated.
        let code = unsafe {
            sql::sqlite3_prepare_v2(
                db.as_ptr(),
                query.as_ptr(),
                -1,
                &mut ptr,
                null_mut(),
            )
        };
        if code != sql::SQLITE_OK {
            return Err(code);
        }
        Ok(Self {
            db,
            ptr: Some(NonNull::new(ptr).ok_or(sql::SQLITE_ERROR)?),
        })
    }

    fn raw(&self) -> *mut sql::sqlite3_stmt {
        self.ptr.expect("statement remains open").as_ptr()
    }

    fn bind_text(&mut self, name: &CStr) -> c_int {
        // SAFETY: SQLite copies the caller's NUL-terminated name.
        unsafe {
            sql::sqlite3_bind_text(
                self.raw(),
                1,
                name.as_ptr(),
                -1,
                sql::SQLITE_TRANSIENT(),
            )
        }
    }

    fn bind_static_text(&mut self, name: &'static CStr) -> c_int {
        // SAFETY: name outlives the statement.
        unsafe { sql::sqlite3_bind_text(self.raw(), 1, name.as_ptr(), -1, None) }
    }

    /// bytes must remain live until the statement is finalized.
    unsafe fn bind_blob(&mut self, bytes: &[u8]) -> c_int {
        let Ok(length) = c_int::try_from(bytes.len()) else {
            return sql::SQLITE_RANGE;
        };
        // SAFETY: The caller retains the slice until SQLite releases the binding.
        unsafe {
            sql::sqlite3_bind_blob(
                self.raw(),
                1,
                bytes.as_ptr().cast(),
                length,
                None,
            )
        }
    }

    fn step(&mut self) -> c_int {
        // SAFETY: No borrowed column value can outlive this mutable borrow.
        unsafe { sql::sqlite3_step(self.raw()) }
    }

    fn columns(&self) -> c_int {
        // SAFETY: The statement is open.
        unsafe { sql::sqlite3_column_count(self.raw()) }
    }

    fn kind(&self, column: c_int) -> c_int {
        // SAFETY: Digest calls use in-range columns from the fixed queries.
        unsafe { sql::sqlite3_column_type(self.raw(), column) }
    }

    fn integer(&self, column: c_int) -> i64 {
        // SAFETY: The caller confirmed SQLITE_INTEGER at this column.
        unsafe { sql::sqlite3_column_int64(self.raw(), column) }
    }

    fn real(&self, column: c_int) -> f64 {
        // SAFETY: The caller confirmed SQLITE_FLOAT at this column.
        unsafe { sql::sqlite3_column_double(self.raw(), column) }
    }

    fn text(&self, column: c_int) -> Option<&[u8]> {
        // SAFETY: The caller confirmed SQLITE_TEXT, so SQLite neither converts
        // the column type nor invalidates an earlier value from another column.
        let ptr = unsafe { sql::sqlite3_column_text(self.raw(), column) };
        self.column_bytes(column, NonNull::new(ptr.cast_mut())?.as_ptr())
    }

    fn blob(&self, column: c_int) -> Option<&[u8]> {
        // SAFETY: The caller confirmed SQLITE_BLOB at this column.
        let ptr = unsafe { sql::sqlite3_column_blob(self.raw(), column) };
        self.column_bytes(column, ptr.cast())
    }

    fn column_bytes(&self, column: c_int, ptr: *const u8) -> Option<&[u8]> {
        // SAFETY: The column previously returned text or blob data.
        let length = unsafe { sql::sqlite3_column_bytes(self.raw(), column) };
        // SAFETY: The statement retains its live connection.
        let error = unsafe { sql::sqlite3_errcode(self.db.as_ptr()) };
        let length = usize::try_from(length).ok()?;
        if error == sql::SQLITE_NOMEM {
            return None;
        }
        if length == 0 {
            return Some(&[]);
        }
        let ptr = NonNull::new(ptr.cast_mut())?;
        // SAFETY: SQLite retains these bytes until next step or finalization,
        // which both require a mutable borrow of this statement.
        Some(unsafe { slice::from_raw_parts(ptr.as_ptr(), length) })
    }

    fn finalize(&mut self) -> c_int {
        self.ptr.take().map_or(sql::SQLITE_OK, |ptr| {
            // SAFETY: This statement owns ptr and finalizes it exactly once.
            unsafe { sql::sqlite3_finalize(ptr.as_ptr()) }
        })
    }
}

impl Drop for Statement {
    fn drop(&mut self) {
        self.finalize();
    }
}
