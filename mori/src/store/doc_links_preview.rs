//! Read bounded doc-link samples on the caller's SQLite connection.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::mem::{forget, size_of};
use std::ptr::{self, NonNull};

use rusqlite::ffi::sqlite3;
use rusqlite::types::{ToSqlOutput, ValueRef};
use rusqlite::{Connection, Row, params};

// Match CBM_DOC_LINK_PREVIEW_* in store.h.
const ROW_LIMIT: usize = 50;
const LOOKAHEAD: usize = 3;
const LONG_CAP: usize = 1024 + LOOKAHEAD;
const SHORT_CAP: usize = 128 + LOOKAHEAD;

// Match the C store ABI.
const STORE_CLASS: c_int = 10;
const OK: c_int = 0;
const ERR: c_int = -1;

const TABLE_SQL: &str = "SELECT 1 FROM sqlite_master WHERE type = 'table' \
                         AND name = 'doc_link_unresolved';";
// Sort and limit the original rows. The BLOB projection measures bytes even
// when the stored text has embedded NULs or invalid UTF-8.
const PREVIEW_SQL: &str = "
    SELECT substr(CAST(d.rel_path AS BLOB),1,?2),length(CAST(d.rel_path AS BLOB)),
           CAST(d.line AS INTEGER),
           substr(CAST(d.syntax AS BLOB),1,?3),length(CAST(d.syntax AS BLOB)),
           substr(CAST(d.raw AS BLOB),1,?2),length(CAST(d.raw AS BLOB)),
           substr(CAST(d.reason AS BLOB),1,?3),length(CAST(d.reason AS BLOB))
    FROM doc_link_unresolved AS d WHERE d.project=?1
    ORDER BY d.reason,d.rel_path,d.line,d.raw LIMIT ?4;";

type CopyBytes = unsafe extern "C" fn(*const c_void, usize) -> *mut c_char;
type Report = unsafe extern "C" fn(*mut c_void, *const c_char, bool);

#[cfg(not(all(test, not(feature = "c-backend"))))]
unsafe extern "C" {
    fn cbm_calloc(class: c_int, bytes: usize) -> *mut c_void;
    fn cbm_free(class: c_int, ptr: *mut c_void);
}

// Unlinked unit tests use the system allocator for the same ownership checks.
#[cfg(all(test, not(feature = "c-backend")))]
unsafe fn cbm_calloc(_: c_int, bytes: usize) -> *mut c_void {
    unsafe { libc::calloc(1, bytes) }
}

#[cfg(all(test, not(feature = "c-backend")))]
unsafe fn cbm_free(_: c_int, ptr: *mut c_void) {
    unsafe { libc::free(ptr) }
}

#[derive(Clone, Copy)]
struct ReadError {
    message: &'static CStr,
    sqlite: bool,
}

const INVALID_FIELD: ReadError = ReadError {
    message: c"doc_links preview copy failed",
    sqlite: false,
};

impl ReadError {
    fn sqlite(message: &'static CStr) -> Self {
        Self {
            message,
            sqlite: true,
        }
    }
}

#[repr(C)]
#[derive(Default)]
struct PreviewText {
    text: *const c_char,
    length: usize,
    original_bytes: u64,
}

#[repr(C)]
#[derive(Default)]
struct PreviewRow {
    rel_path: PreviewText,
    line: c_int,
    syntax: PreviewText,
    raw: PreviewText,
    reason: PreviewText,
}

struct Page {
    rows: NonNull<PreviewRow>,
    len: usize,
}

impl Page {
    fn new() -> Option<Self> {
        let rows = unsafe {
            cbm_calloc(STORE_CLASS, ROW_LIMIT * size_of::<PreviewRow>())
        };
        Some(Self {
            rows: NonNull::new(rows.cast())?,
            len: 0,
        })
    }

    fn push(&mut self, row: &Row<'_>, copy: CopyBytes) -> Result<(), ReadError> {
        if self.len == ROW_LIMIT {
            return Err(ReadError {
                message: c"doc_links preview row limit",
                sqlite: false,
            });
        }

        let slot = unsafe {
            let ptr = self.rows.as_ptr().add(self.len);
            ptr.write(PreviewRow::default());
            self.len += 1;
            &mut *ptr
        };
        // sqlite3_column_int coerced NULL to zero and kept the low 32 bits.
        slot.line = row
            .get::<_, Option<i64>>(2)
            .map_err(|_| INVALID_FIELD)?
            .unwrap_or_default() as c_int;
        slot.rel_path = field(row, 0, LONG_CAP, copy).ok_or(INVALID_FIELD)?;
        slot.syntax = field(row, 3, SHORT_CAP, copy).ok_or(INVALID_FIELD)?;
        slot.raw = field(row, 5, LONG_CAP, copy).ok_or(INVALID_FIELD)?;
        slot.reason = field(row, 7, SHORT_CAP, copy).ok_or(INVALID_FIELD)?;
        Ok(())
    }

    fn into_raw(self) -> (*mut PreviewRow, c_int) {
        let output = (self.rows.as_ptr(), self.len as c_int);
        forget(self);
        output
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        // Partially initialized rows have null pointers for uncopied fields.
        unsafe {
            cbm_store_free_doc_link_previews(
                self.rows.as_ptr(),
                self.len as c_int,
            )
        };
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_store_free_doc_link_previews(
    rows: *mut PreviewRow,
    count: c_int,
) {
    if rows.is_null() {
        return;
    }
    for i in 0..count {
        let row = unsafe { &*rows.add(i as usize) };
        for field in [&row.rel_path, &row.syntax, &row.raw, &row.reason] {
            unsafe { cbm_free(STORE_CLASS, field.text.cast_mut().cast()) };
        }
    }
    unsafe { cbm_free(STORE_CLASS, rows.cast()) };
}

fn field(
    row: &Row<'_>,
    column: usize,
    cap: usize,
    copy: CopyBytes,
) -> Option<PreviewText> {
    let original_bytes =
        u64::try_from(row.get::<_, i64>(column + 1).ok()?).ok()?;
    let bytes = match row.get_ref(column).ok()? {
        ValueRef::Blob(bytes) => bytes,
        // SQLite can return NULL for substr of a zero-byte BLOB.
        ValueRef::Null if original_bytes == 0 => &[],
        _ => return None,
    };
    if bytes.len() as u64 != original_bytes.min(cap as u64) {
        return None;
    }
    // Use the same tracked copy as the legacy summary sample getter.
    let text = unsafe { copy(bytes.as_ptr().cast(), bytes.len()) };
    (!text.is_null()).then_some(PreviewText {
        text,
        length: bytes.len(),
        original_bytes,
    })
}

fn table_exists(db: &Connection) -> Result<bool, ReadError> {
    let mut stmt = db
        .prepare(TABLE_SQL)
        .map_err(|_| ReadError::sqlite(c"doc_links probe prepare"))?;
    let mut rows = stmt
        .query([])
        .map_err(|_| ReadError::sqlite(c"doc_links probe"))?;
    rows.next()
        .map(|row| row.is_some())
        .map_err(|_| ReadError::sqlite(c"doc_links probe"))
}

fn read_preview(
    db: &Connection,
    project: &CStr,
    copy: CopyBytes,
) -> Result<Page, Option<ReadError>> {
    let mut stmt = db
        .prepare(PREVIEW_SQL)
        .map_err(|_| Some(ReadError::sqlite(c"doc_links preview prepare")))?;
    let mut rows = stmt
        .query(params![
            ToSqlOutput::Borrowed(ValueRef::Text(project.to_bytes())),
            LONG_CAP as i64,
            SHORT_CAP as i64,
            ROW_LIMIT as i64,
        ])
        .map_err(|_| Some(ReadError::sqlite(c"doc_links preview bind")))?;
    let mut page = Page::new().ok_or(None)?;
    while let Some(row) = rows
        .next()
        .map_err(|_| Some(ReadError::sqlite(c"doc_links preview scan")))?
    {
        page.push(row, copy).map_err(Some)?;
    }
    Ok(page)
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_doc_links_table_state(
    db: *mut sqlite3,
    context: *mut c_void,
    report: Report,
) -> c_int {
    let Ok(db) = (unsafe { Connection::from_handle(db) }) else {
        return ERR;
    };
    match table_exists(&db) {
        Ok(present) => c_int::from(present),
        Err(error) => {
            unsafe { report(context, error.message.as_ptr(), error.sqlite) };
            ERR
        }
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_doc_links_preview(
    db: *mut sqlite3,
    project: *const c_char,
    out: *mut *mut PreviewRow,
    count: *mut c_int,
    table_present: *mut bool,
    copy: CopyBytes,
    context: *mut c_void,
    report: Report,
) -> c_int {
    if out.is_null() || count.is_null() {
        return ERR;
    }
    unsafe {
        out.write(ptr::null_mut());
        count.write(0);
        if !table_present.is_null() {
            table_present.write(false);
        }
    }
    if db.is_null() || project.is_null() {
        return ERR;
    }
    // This handle stays owned by the store. Keep its transaction and progress
    // handler in place and let SQLite interrupt the scan if requested.
    let Ok(db) = (unsafe { Connection::from_handle(db) }) else {
        return ERR;
    };
    match table_exists(&db) {
        Ok(false) => return OK,
        Ok(true) => {
            if !table_present.is_null() {
                unsafe { table_present.write(true) };
            }
        }
        Err(error) => {
            unsafe { report(context, error.message.as_ptr(), error.sqlite) };
            return ERR;
        }
    }

    match read_preview(&db, unsafe { CStr::from_ptr(project) }, copy) {
        Ok(page) => {
            let (rows, len) = page.into_raw();
            unsafe {
                out.write(rows);
                count.write(len);
            }
            OK
        }
        Err(error) => {
            if let Some(error) = error {
                unsafe { report(context, error.message.as_ptr(), error.sqlite) };
            }
            ERR
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::ffi as sql;
    use std::cell::Cell;
    use std::slice;

    thread_local! {
        static COPIES: Cell<usize> = const { Cell::new(0) };
        static COPIED_BYTES: Cell<usize> = const { Cell::new(0) };
        static FAIL_AFTER: Cell<usize> = const { Cell::new(usize::MAX) };
    }

    unsafe extern "C" fn copy(
        source: *const c_void,
        length: usize,
    ) -> *mut c_char {
        if COPIES.get() == FAIL_AFTER.get() {
            FAIL_AFTER.set(usize::MAX);
            return ptr::null_mut();
        }
        COPIES.set(COPIES.get() + 1);
        COPIED_BYTES.set(COPIED_BYTES.get() + length + 1);
        let bytes = unsafe { cbm_calloc(STORE_CLASS, length + 1) }.cast::<u8>();
        assert!(!bytes.is_null());
        unsafe { ptr::copy_nonoverlapping(source.cast(), bytes, length) };
        bytes.cast()
    }

    unsafe extern "C" fn report(
        context: *mut c_void,
        message: *const c_char,
        _: bool,
    ) {
        unsafe { context.cast::<*const c_char>().write(message) };
    }

    fn create_table(db: &Connection) {
        db.execute_batch(
            "CREATE TABLE doc_link_unresolved(project TEXT, rel_path TEXT, line INTEGER,
                                              syntax TEXT, raw TEXT, reason TEXT);",
        )
        .unwrap();
    }

    fn preview(
        db: &Connection,
        project: &CStr,
    ) -> (c_int, Option<Page>, bool, Option<&'static CStr>) {
        let mut out = ptr::null_mut();
        let mut count = -1;
        let mut present = false;
        let mut error: *const c_char = ptr::null();
        let status = unsafe {
            mori_doc_links_preview(
                db.handle(),
                project.as_ptr(),
                &mut out,
                &mut count,
                &mut present,
                copy,
                ptr::from_mut(&mut error).cast(),
                report,
            )
        };
        if out.is_null() {
            assert_eq!(count, 0);
        }
        let page = NonNull::new(out).map(|rows| Page {
            rows,
            len: count as usize,
        });
        let error = (!error.is_null()).then(|| unsafe { CStr::from_ptr(error) });
        (status, page, present, error)
    }

    fn rows(page: &Page) -> &[PreviewRow] {
        unsafe { slice::from_raw_parts(page.rows.as_ptr(), page.len) }
    }

    fn bytes(text: &PreviewText) -> &[u8] {
        let with_nul =
            unsafe { slice::from_raw_parts(text.text.cast(), text.length + 1) };
        assert_eq!(with_nul[text.length], 0);
        &with_nul[..text.length]
    }

    #[test]
    fn samples_keep_the_native_order_and_copy_budget() {
        let db = Connection::open_in_memory().unwrap();
        create_table(&db);
        db.execute_batch(
            "INSERT INTO doc_link_unresolved VALUES ('p','','',x'','','error'),
                                                   ('other','','','','','ambiguous');",
        )
        .unwrap();
        let raw = vec![b'x'; LONG_CAP + 10];
        let syntax = vec![b's'; SHORT_CAP + 10];
        for i in (0..51).rev() {
            db.execute(
                "INSERT INTO doc_link_unresolved VALUES ('p',?1,5,?2,?3,'missing');",
                params![format!("src/Case{i:03}.cs"), syntax, raw],
            )
            .unwrap();
        }
        COPIES.set(0);
        COPIED_BYTES.set(0);
        let (status, page, present, error) = preview(&db, c"p");
        assert_eq!((status, present, error), (OK, true, None));
        let page = page.unwrap();
        let samples = rows(&page);
        assert_eq!(samples.len(), 50);
        assert_eq!(bytes(&samples[0].rel_path), b"");
        assert_eq!(bytes(&samples[0].syntax), b"");
        assert_eq!(samples[0].line, 0);
        for (i, row) in samples[1..].iter().enumerate() {
            assert_eq!(
                bytes(&row.rel_path),
                format!("src/Case{i:03}.cs").as_bytes()
            );
            assert_eq!(row.line, 5);
            assert_eq!(bytes(&row.syntax), &syntax[..131]);
            assert_eq!(row.syntax.original_bytes, syntax.len() as u64);
            assert_eq!(bytes(&row.raw), &raw[..1027]);
            assert_eq!(row.raw.original_bytes, raw.len() as u64);
        }
        assert_eq!(COPIES.get(), 200);
        assert!(COPIED_BYTES.get() <= 116_000);
    }

    #[test]
    fn raw_sort_uses_the_full_source_and_retains_non_utf8_bytes() {
        let db = Connection::open_in_memory().unwrap();
        create_table(&db);
        let project = ToSqlOutput::Borrowed(ValueRef::Text(b"p\xff"));
        let mut first = vec![b't'; LONG_CAP];
        first.extend_from_slice(b"aa");
        let mut second = vec![b't'; LONG_CAP];
        second.push(b'z');
        for (line, raw, reason) in [
            (2, &second, "missing"),
            (2, &first, "missing"),
            (1, &second, "missing"),
            (4_294_967_299_i64, &first, "ambiguous"),
        ] {
            db.execute(
                "INSERT INTO doc_link_unresolved VALUES (?1,'src/Tie.cs',?2,x'4100ff',?3,?4);",
                params![project, line, raw, reason],
            )
            .unwrap();
        }

        let (status, page, present, _) = preview(&db, c"p\xff");
        assert_eq!((status, present), (OK, true));
        let page = page.unwrap();
        let samples = rows(&page);
        assert_eq!(samples.len(), 4);
        for (row, (line, original_bytes)) in
            samples
                .iter()
                .zip([(3, 1029), (1, 1028), (2, 1029), (2, 1028)])
        {
            assert_eq!(row.line, line);
            assert_eq!(bytes(&row.syntax), b"A\0\xff");
            assert_eq!(row.raw.original_bytes, original_bytes);
            assert_eq!(bytes(&row.raw), &first[..LONG_CAP]);
        }
    }

    #[test]
    fn missing_tables_and_partial_copy_failures_keep_the_connection_reusable() {
        let db = Connection::open_in_memory().unwrap();
        let (status, page, present, error) = preview(&db, c"p");
        assert_eq!((status, present, error), (OK, false, None));
        assert!(page.is_none());

        create_table(&db);
        db.execute_batch(
            "BEGIN;
             INSERT INTO doc_link_unresolved VALUES ('p','FailureA',3,'see','first','missing'),
                                                    ('p','FailureB',4,'see','second','missing');",
        )
        .unwrap();

        for successful_copies in [0, 4] {
            COPIES.set(0);
            FAIL_AFTER.set(successful_copies);
            let (status, page, present, error) = preview(&db, c"p");
            assert_eq!(
                (status, present, error),
                (ERR, true, Some(c"doc_links preview copy failed"))
            );
            assert!(page.is_none());
            assert_eq!(COPIES.get(), successful_copies);
            assert!(!db.is_autocommit());

            let (status, page, present, _) = preview(&db, c"p");
            assert_eq!((status, present), (OK, true));
            assert_eq!(rows(&page.unwrap()).len(), 2);
        }
        db.execute_batch("ROLLBACK;").unwrap();
        let (_, page, present, _) = preview(&db, c"p");
        assert!(present);
        assert!(rows(&page.unwrap()).is_empty());
    }

    #[test]
    fn borrow_preserves_the_callers_progress_handler() {
        unsafe extern "C" fn interrupt(context: *mut c_void) -> c_int {
            let called = unsafe { &mut *context.cast::<usize>() };
            *called += 1;
            1
        }

        let db = Connection::open_in_memory().unwrap();
        create_table(&db);
        db.execute_batch("BEGIN;").unwrap();

        // Exercise both the borrowed entry point and the scan itself with an
        // existing handler. Neither call owns the progress handler.
        assert!(table_exists(&db).unwrap_or(false));
        let mut called = 0usize;
        unsafe {
            sql::sqlite3_progress_handler(
                db.handle(),
                1,
                Some(interrupt),
                ptr::from_mut(&mut called).cast(),
            )
        };
        let (status, page, present, error) = preview(&db, c"p");
        assert_eq!((status, present), (ERR, false));
        // SQLite can run the progress handler while preparing or stepping SQL.
        assert!(
            [Some(c"doc_links probe prepare"), Some(c"doc_links probe")]
                .contains(&error),
            "unexpected table probe error: {error:?}"
        );
        assert!(page.is_none());
        let error = read_preview(&db, c"p", copy).err().flatten().unwrap();
        assert!(error.sqlite);
        assert_eq!(
            unsafe { sql::sqlite3_errcode(db.handle()) },
            sql::SQLITE_INTERRUPT
        );
        assert!(called > 0);
        assert!(
            db.query_row("SELECT 1;", [], |row| row.get::<_, i32>(0))
                .is_err()
        );
        assert!(!db.is_autocommit());

        unsafe {
            sql::sqlite3_progress_handler(db.handle(), 0, None, ptr::null_mut())
        };
        assert_eq!(preview(&db, c"p").0, OK);
        db.execute_batch("ROLLBACK;").unwrap();
    }
}
