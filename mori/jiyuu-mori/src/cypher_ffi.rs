//! Adapter for the native query handler's read snapshot and response owner.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::panic;

use rusqlite::{Connection, ffi::sqlite3};

use crate::cypher_linear::{self, ExecutionBudget, ExecutionError, ResultRows};

#[repr(C)]
struct Cell {
    data: *const c_char,
    len: usize,
}

impl From<&[u8]> for Cell {
    fn from(value: &[u8]) -> Self {
        Self {
            data: value.as_ptr().cast(),
            len: value.len(),
        }
    }
}

#[repr(C)]
struct Sink {
    context: *mut c_void,
    begin: unsafe extern "C" fn(
        *mut c_void,
        *const Cell,
        usize,
        usize,
        bool,
        Cell,
    ) -> bool,
    row: unsafe extern "C" fn(*mut c_void, *const Cell) -> bool,
    storage_error: unsafe extern "C" fn(*mut c_void, Cell),
}

#[repr(i32)]
enum Status {
    Ok,
    Unsupported,
    Deadline,
    Cancelled,
    RowCeiling,
    Error,
}

type BudgetStatus = unsafe extern "C" fn(*mut c_void) -> c_int;
type Evaluate = unsafe extern "C" fn(
    *mut sqlite3,
    *const c_char,
    *const c_char,
    BudgetStatus,
    *mut c_void,
    *const Sink,
) -> c_int;

unsafe extern "C" {
    fn cbm_cypher_budget_remaining_ms(budget: *const c_void) -> u64;
    fn cbm_cypher_max_depth() -> c_int;
    fn cbm_mcp_set_cypher_evaluator(evaluate: Option<Evaluate>);
}

#[cfg(not(test))]
pub(crate) unsafe fn register() {
    // SAFETY: startup runs before C starts request threads.
    unsafe { cbm_mcp_set_cypher_evaluator(Some(evaluate)) };
}

unsafe extern "C" fn evaluate(
    db: *mut sqlite3,
    query: *const c_char,
    project: *const c_char,
    budget_status: BudgetStatus,
    budget_context: *mut c_void,
    sink: *const Sink,
) -> c_int {
    panic::catch_unwind(|| {
        // SAFETY: C provides the live read-batched handle, NUL-terminated
        // strings, and callbacks with contexts valid for this invocation.
        let check = || match unsafe { budget_status(budget_context) } {
            status if status == Status::Deadline as c_int => {
                Err(ExecutionError::Deadline)
            }
            status if status == Status::Cancelled as c_int => {
                Err(ExecutionError::Cancelled)
            }
            _ => Ok(()),
        };
        // SAFETY: The native query handler lends its request budget here.
        let remaining_ms = || {
            let remaining = unsafe {
                cbm_cypher_budget_remaining_ms(budget_context.cast_const())
            };
            #[cfg(all(test, debug_assertions))]
            tests::after_remaining_read(remaining);
            remaining
        };
        let mut budget = ExecutionBudget::shared(&check, &remaining_ms);
        #[cfg(debug_assertions)]
        if let Some(limit) = std::env::var("MORI_TEST_ORDERED_COUNT_GROUP_LIMIT")
            .ok()
            .and_then(|value| value.parse().ok())
        {
            budget.set_ordered_count_group_limit_for_tests(limit);
        }
        #[cfg(all(test, debug_assertions))]
        tests::configure_budget(&mut budget);
        let query = unsafe { CStr::from_ptr(query) };
        let project = unsafe { CStr::from_ptr(project) };
        let Ok(source) = query.to_str() else {
            return Status::Unsupported;
        };
        let Ok(project) = project.to_str() else {
            return Status::Unsupported;
        };
        let sink = unsafe { &*sink };
        let query = cypher_linear::classify(source);
        if let Err(error) = budget.check() {
            return match error {
                ExecutionError::Deadline => Status::Deadline,
                ExecutionError::Cancelled => Status::Cancelled,
                _ => Status::Error,
            };
        }
        let query = match query {
            Ok(query) => query,
            Err(error) => {
                return if let Some(detail) = error.diagnostic() {
                    sink.error(&detail)
                } else {
                    Status::Unsupported
                };
            }
        };
        // SAFETY: the native limit reader returns a positive integer.
        budget.path_depth_cap = unsafe { cbm_cypher_max_depth() } as u32;
        // from_handle borrows the native handle without taking ownership.
        let result = unsafe { Connection::from_handle(db) }
            .map_err(ExecutionError::Storage)
            .and_then(|connection| {
                query.execute_with_budget(&connection, project, &mut budget)
            });
        match result {
            Ok(rows) => sink.emit(&rows),
            Err(ExecutionError::Unsupported) => Status::Unsupported,
            Err(ExecutionError::Deadline) => Status::Deadline,
            Err(ExecutionError::Cancelled) => Status::Cancelled,
            Err(ExecutionError::RowCeiling) => Status::RowCeiling,
            Err(ExecutionError::Storage(error)) => {
                let detail =
                    format!("query aborted: reading the graph failed ({error})");
                sink.error(&detail)
            }
        }
    })
    .unwrap_or(Status::Error) as c_int
}

impl Sink {
    fn error(&self, detail: &str) -> Status {
        // SAFETY: C copies the detail during the call.
        unsafe { (self.storage_error)(self.context, detail.as_bytes().into()) };
        Status::Error
    }

    fn emit(&self, result: &ResultRows) -> Status {
        let mut cells: Vec<_> = result
            .columns
            .iter()
            .map(|text| text.as_bytes().into())
            .collect();
        // SAFETY: callbacks copy cells synchronously and keep no Rust
        // pointers. The header determines the width of each following row.
        if !unsafe {
            (self.begin)(
                self.context,
                cells.as_ptr(),
                cells.len(),
                result.rows.len(),
                result.truncated,
                result
                    .warning()
                    .as_deref()
                    .unwrap_or_default()
                    .as_bytes()
                    .into(),
            )
        } {
            return Status::Error;
        }
        for row in &result.rows {
            cells.clear();
            cells.extend(row.iter().map(|bytes| Cell::from(bytes.as_slice())));
            if !unsafe { (self.row)(self.context, cells.as_ptr()) } {
                return Status::Error;
            }
        }
        Status::Ok
    }
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use std::{cell::RefCell, ptr};

    use super::*;

    const START_MS: u64 = 100;
    const RUST_CUTOFF_MS: u64 = START_MS + 500;
    const DEADLINE_MS: u64 = START_MS + 30_000;

    struct Clock {
        now_ms: u64,
        remaining_reads: Vec<u64>,
        rust_status: Option<c_int>,
    }

    thread_local! {
        static CLOCK: RefCell<Option<Clock>> = const { RefCell::new(None) };
    }

    unsafe extern "C" {
        fn cbm_cypher_test_set_deadline_clock(
            clock: Option<extern "C" fn() -> u64>,
        );
        fn cbm_mcp_server_new(path: *const c_char) -> *mut c_void;
        fn cbm_mcp_server_free(server: *mut c_void);
        fn cbm_mcp_server_store(server: *mut c_void) -> *mut c_void;
        fn cbm_mcp_server_set_project(
            server: *mut c_void,
            project: *const c_char,
        );
        fn cbm_store_get_db(store: *mut c_void) -> *mut sqlite3;
        fn cbm_mcp_handle_tool(
            server: *mut c_void,
            tool: *const c_char,
            args: *const c_char,
        ) -> *mut c_char;
    }

    pub(super) fn configure_budget(budget: &mut ExecutionBudget<'_>) {
        if CLOCK.with(|clock| clock.borrow().is_some()) {
            budget.set_ordered_count_group_limit_for_tests(2);
        }
    }

    pub(super) fn after_remaining_read(remaining: u64) {
        CLOCK.with(|clock| {
            if let Some(clock) = clock.borrow_mut().as_mut() {
                clock.remaining_reads.push(remaining);
                // The first observation sets the Rust cutoff. Move the
                // clock to that cutoff before the next budget check.
                clock.now_ms = RUST_CUTOFF_MS;
            }
        });
    }

    extern "C" fn clock_now() -> u64 {
        CLOCK.with(|clock| {
            clock.borrow().as_ref().map_or(0, |clock| clock.now_ms)
        })
    }

    unsafe extern "C" fn evaluate_then_expire(
        db: *mut sqlite3,
        query: *const c_char,
        project: *const c_char,
        budget_status: BudgetStatus,
        budget_context: *mut c_void,
        sink: *const Sink,
    ) -> c_int {
        let status = unsafe {
            evaluate(db, query, project, budget_status, budget_context, sink)
        };
        CLOCK.with(|clock| {
            if let Some(clock) = clock.borrow_mut().as_mut() {
                clock.rust_status = Some(status);
                clock.now_ms = DEADLINE_MS;
            }
        });
        status
    }

    struct Server(*mut c_void);

    impl Drop for Server {
        fn drop(&mut self) {
            unsafe {
                cbm_mcp_set_cypher_evaluator(None);
                cbm_cypher_test_set_deadline_clock(None);
                cbm_mcp_server_free(self.0);
            }
            CLOCK.with(|clock| *clock.borrow_mut() = None);
        }
    }

    #[test]
    fn ordered_count_replay_hands_off_at_the_original_deadline() {
        let server = Server(unsafe { cbm_mcp_server_new(ptr::null()) });
        assert!(!server.0.is_null());
        unsafe { cbm_mcp_server_set_project(server.0, c"p".as_ptr()) };
        let db = unsafe {
            Connection::from_handle(cbm_store_get_db(cbm_mcp_server_store(
                server.0,
            )))
        }
        .unwrap();
        db.execute_batch(
            "INSERT INTO projects VALUES ('p', 'now', '/tmp');
             INSERT INTO nodes(project, label, name, qualified_name) VALUES
                 ('p', 'Probe', 'alpha', 'a'),
                 ('p', 'Probe', 'beta', 'b'),
                 ('p', 'Probe', 'gamma', 'c');",
        )
        .unwrap();
        drop(db);

        CLOCK.with(|clock| {
            *clock.borrow_mut() = Some(Clock {
                now_ms: START_MS,
                remaining_reads: Vec::new(),
                rust_status: None,
            });
        });
        let reply = unsafe {
            cbm_cypher_test_set_deadline_clock(Some(clock_now));
            cbm_mcp_set_cypher_evaluator(Some(evaluate_then_expire));
            cbm_mcp_handle_tool(
                server.0,
                c"query_graph".as_ptr(),
                cr#"{"project":"p","query":"MATCH (n:Probe) RETURN n.name, count(*) AS total ORDER BY total DESC LIMIT 1"}"#.as_ptr(),
            )
        };
        assert!(!reply.is_null());
        let bytes = unsafe { CStr::from_ptr(reply) }.to_bytes().to_vec();
        unsafe { libc::free(reply.cast()) };
        let output: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        CLOCK.with(|clock| {
            let clock = clock.borrow();
            let clock = clock.as_ref().unwrap();
            assert_eq!(clock.remaining_reads, [30_000, 29_500]);
            assert_eq!(clock.rust_status, Some(Status::Unsupported as c_int));
        });
        assert_eq!(output["isError"], true, "{output}");
        let message = output["content"][0]["text"].as_str().unwrap();
        assert!(
            message.contains("exceeded the execution time limit"),
            "{output}"
        );
    }
}
