//! Attribute search hits to nodes in the matched file on the live store.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::{panic, ptr};

use rusqlite::ffi::{self, sqlite3, sqlite3_stmt};

use super::Statement;

const NODE_QUERY: &CStr =
    c"SELECT id, label, name, qualified_name, file_path, start_line, end_line
    FROM nodes WHERE project = ?1 AND file_path = ?2";
const MAX_LINE_SPAN: i64 = 999_999;

struct Node {
    id: i64,
    label: CString,
    name: CString,
    qualified_name: CString,
    file_path: CString,
    start_line: c_int,
    end_line: c_int,
}

#[repr(C)]
struct NodeView {
    id: i64,
    label: *const c_char,
    name: *const c_char,
    qualified_name: *const c_char,
    file_path: *const c_char,
    start_line: c_int,
    end_line: c_int,
}

impl Node {
    fn view(&self) -> NodeView {
        NodeView {
            id: self.id,
            label: self.label.as_ptr(),
            name: self.name.as_ptr(),
            qualified_name: self.qualified_name.as_ptr(),
            file_path: self.file_path.as_ptr(),
            start_line: self.start_line,
            end_line: self.end_line,
        }
    }
}

struct Attribution {
    query: Statement,
    nodes: Vec<Node>,
}

impl Attribution {
    fn load(&mut self, project: &CStr, file: &CStr) -> bool {
        self.nodes.clear();
        let loaded = panic::catch_unwind(panic::AssertUnwindSafe(|| {
            self.read(project, file)
        }))
        .unwrap_or(false);
        // SAFETY: The request owns this statement. Its next use starts with
        // both parameters unbound and no cursor on the borrowed connection.
        unsafe {
            ffi::sqlite3_reset(self.query.0);
            ffi::sqlite3_clear_bindings(self.query.0);
        }
        if !loaded {
            self.nodes.clear();
        }
        loaded
    }

    fn read(&mut self, project: &CStr, file: &CStr) -> bool {
        // SAFETY: The statement is ready to run and bind_text copies both
        // NUL-terminated inputs.
        let bound = unsafe {
            self.query.bind_text(1, project.as_ptr()) == ffi::SQLITE_OK
                && self.query.bind_text(2, file.as_ptr()) == ffi::SQLITE_OK
        };
        if !bound {
            return false;
        }
        loop {
            match self.query.step() {
                ffi::SQLITE_DONE => return true,
                ffi::SQLITE_ROW => {
                    let row = self.query.0;
                    // SAFETY: The current row supplies all seven columns.
                    // Take the text bytes before stepping the statement again.
                    self.nodes.push(unsafe {
                        Node {
                            id: ffi::sqlite3_column_int64(row, 0),
                            label: column_text(row, 1),
                            name: column_text(row, 2),
                            qualified_name: column_text(row, 3),
                            file_path: column_text(row, 4),
                            start_line: ffi::sqlite3_column_int(row, 5),
                            end_line: ffi::sqlite3_column_int(row, 6),
                        }
                    });
                }
                _ => return false,
            }
        }
    }

    fn find(&self, line: c_int) -> Option<&Node> {
        self.nodes
            .iter()
            .filter(|node| node.start_line <= line && line <= node.end_line)
            .map(|node| {
                (i64::from(node.end_line) - i64::from(node.start_line), node)
            })
            .filter(|(span, _)| *span <= MAX_LINE_SPAN)
            .min_by_key(|(span, node)| {
                (*span, node.qualified_name.as_bytes(), node.id)
            })
            .map(|(_, node)| node)
    }
}

unsafe fn column_text(row: *mut sqlite3_stmt, index: c_int) -> CString {
    // SAFETY: The caller lends a live current row. Match the C store's
    // NUL-terminated copies, including for text that is invalid UTF-8.
    let text = unsafe { ffi::sqlite3_column_text(row, index) };
    if text.is_null() {
        c"".to_owned()
    } else {
        unsafe { CStr::from_ptr(text.cast()).to_owned() }
    }
}

unsafe extern "C" fn open(db: *mut sqlite3) -> *mut c_void {
    if db.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: The caller lends the db until close, which finalizes the query.
    let Some(query) = (unsafe { Statement::prepare(db, NODE_QUERY.as_ptr()) })
    else {
        return ptr::null_mut();
    };
    Box::into_raw(Box::new(Attribution {
        query,
        nodes: Vec::new(),
    }))
    .cast()
}

unsafe extern "C" fn load(
    reader: *mut c_void,
    project: *const c_char,
    file: *const c_char,
) -> bool {
    // SAFETY: The caller owns one live reader per request and lends both
    // C strings for this call.
    unsafe { &mut *reader.cast::<Attribution>() }
        .load(unsafe { CStr::from_ptr(project) }, unsafe {
            CStr::from_ptr(file)
        })
}

unsafe extern "C" fn find(
    reader: *const c_void,
    line: c_int,
    output: *mut NodeView,
) -> bool {
    // SAFETY: The caller lends a live reader and writable output. The view
    // borrows the reader's strings until the next load or close.
    let Some(node) = (unsafe { &*reader.cast::<Attribution>() }).find(line)
    else {
        return false;
    };
    unsafe { output.write(node.view()) };
    true
}

unsafe extern "C" fn close(reader: *mut c_void) {
    // SAFETY: The caller consumes a live reader returned by open exactly once.
    drop(unsafe { Box::from_raw(reader.cast::<Attribution>()) });
}

#[repr(C)]
struct Callbacks {
    open: unsafe extern "C" fn(*mut sqlite3) -> *mut c_void,
    load:
        unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> bool,
    find: unsafe extern "C" fn(*const c_void, c_int, *mut NodeView) -> bool,
    close: unsafe extern "C" fn(*mut c_void),
}

static CALLBACKS: Callbacks = Callbacks {
    open,
    load,
    find,
    close,
};

#[cfg(not(test))]
unsafe extern "C" {
    fn cbm_mcp_set_search_code_attribution(callbacks: *const Callbacks);
}

#[cfg(not(test))]
pub(super) unsafe fn register() {
    // SAFETY: C retains a pointer to a static table and registers at startup.
    unsafe { cbm_mcp_set_search_code_attribution(&CALLBACKS) };
}

#[cfg(test)]
mod tests {
    use std::mem::MaybeUninit;

    use rusqlite::Connection;

    use super::*;

    #[test]
    fn attributes_inclusive_ranges_and_breaks_ties_by_name_then_id() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE nodes(id, project, label, name, qualified_name, file_path, start_line, end_line);
             INSERT INTO nodes VALUES
               (1,'p','Class','outer','outer','a.py',1,20),
               (2,'p','Method','z','z','a.py',4,6),
               (4,'p','Method','a','a','a.py',4,6),
               (3,'p','Method','a','a','a.py',4,6),
               (5,'p','Method','b','b','a.py',8,8),
               (6,'p','File','wide','wide','a.py',21,1000021),
               (7,'p','File','edge','edge','a.py',22,1000021),
               (8,'other','Function','other','other','a.py',7,7),
               (9,'p','Function','other','other','b.py',7,7);",
        )
        .unwrap();

        // SAFETY: The fixture keeps the DB, reader, input strings and result
        // storage alive through each call.
        unsafe {
            let reader = (CALLBACKS.open)(db.handle());
            assert!(!reader.is_null());
            let id_at = |line| {
                let mut node = MaybeUninit::<NodeView>::uninit();
                (CALLBACKS.find)(reader, line, node.as_mut_ptr())
                    .then(|| node.assume_init().id)
            };
            assert!((CALLBACKS.load)(reader, c"p".as_ptr(), c"a.py".as_ptr()));
            for (line, expected) in [
                (0, None),
                (1, Some(1)),
                (4, Some(3)),
                (6, Some(3)),
                (7, Some(1)),
                (8, Some(5)),
                (21, None),
                (22, Some(7)),
                (1000021, Some(7)),
            ] {
                assert_eq!(id_at(line), expected, "line {line}");
            }
            assert!((CALLBACKS.load)(reader, c"p".as_ptr(), c"b.py".as_ptr()));
            assert_eq!(id_at(7), Some(9));
            assert!((CALLBACKS.load)(reader, c"p".as_ptr(), c"empty".as_ptr()));
            assert_eq!(id_at(7), None);
            db.execute_batch("DROP TABLE nodes").unwrap();
            assert!(!(CALLBACKS.load)(reader, c"p".as_ptr(), c"a.py".as_ptr()));
            assert_eq!(id_at(7), None);
            (CALLBACKS.close)(reader);
            assert!(db.execute_batch("SELECT 1").is_ok());
        }
    }
}
