//! Rust startup for the linked C program.

use std::ffi::{CStr, CString, OsStr, c_char, c_int, c_void};
use std::ops::ControlFlow;
use std::os::unix::ffi::OsStrExt;
use std::{panic, ptr};

use crate::project_page::{ProjectPage, ProjectRecord, build_project_page};
use crate::stdio::c_transport;
use crate::store::{FileOutlineQuery, Store};
use crate::tools;
use rusqlite::{Connection, ffi::sqlite3};

type ReadOverride = unsafe extern "C" fn(
    tool: *const c_char,
    options: *const c_void,
    store: *mut c_void,
    project: *const c_char,
) -> *mut c_char;
type ReleasePayload = unsafe extern "C" fn(payload: *mut c_char);

#[repr(C)]
struct GraphSchemaOptions {
    limit: c_int,
    offset: c_int,
    include_properties: bool,
}

#[repr(C)]
struct OutlineRow {
    name: *const c_char,
    label: *const c_char,
    qualified_name: *const c_char,
    start_line: c_int,
    end_line: c_int,
}

type OutlineEmit = unsafe extern "C" fn(
    context: *mut c_void,
    row: *const OutlineRow,
    lengths: *const usize,
) -> c_int;
type OutlineReader = unsafe extern "C" fn(
    db: *mut sqlite3,
    project: *const c_char,
    file_path: *const c_char,
    labels: *const *const c_char,
    label_count: c_int,
    limit: c_int,
    offset: c_int,
    total: *mut c_int,
    emit: OutlineEmit,
    context: *mut c_void,
) -> c_int;

type ProjectIdentityEmit = unsafe extern "C" fn(
    context: *mut c_void,
    name: *const c_char,
    root_path: *const c_char,
) -> c_int;
type ProjectIdentityReader = unsafe extern "C" fn(
    store: *mut c_void,
    name_capacity: usize,
    emit: ProjectIdentityEmit,
    context: *mut c_void,
) -> c_int;
type ProjectStatsReader = unsafe extern "C" fn(
    store: *mut c_void,
    project: *const c_char,
    nodes: *mut c_int,
    edges: *mut c_int,
);
type ProjectMetadataEmit = unsafe extern "C" fn(
    context: *mut c_void,
    root_path: *const c_char,
    indexed_at: *const c_char,
);
type ProjectMetadataReader = unsafe extern "C" fn(
    store: *mut c_void,
    project: *const c_char,
    emit: ProjectMetadataEmit,
    context: *mut c_void,
) -> bool;

const STORE_OK: c_int = 0;
const STORE_ERR: c_int = -1;
const STORE_CANCELLED: c_int = -3;

unsafe extern "C" {
    /// Prepares the native runtime, then calls back into Rust to select the
    /// process role and subcommand. Its first statement binds the allocators.
    fn cbm_c_main(argc: c_int, argv: *mut *mut c_char) -> c_int;

    fn cbm_alloc_init();
    fn cbm_mcp_set_read_override(handle: ReadOverride, release: ReleasePayload);
    fn cbm_mcp_set_project_readers(
        identity: ProjectIdentityReader,
        stats: ProjectStatsReader,
        metadata: ProjectMetadataReader,
    );
    fn cbm_mcp_set_project_page_builder(
        builder: unsafe extern "C" fn(
            *mut ProjectRecord,
            c_int,
            *mut ProjectPage,
        ),
    );
    fn cbm_store_get_db(store: *mut c_void) -> *mut sqlite3;
    fn cbm_store_set_file_outline_reader(reader: OutlineReader);
    fn cbm_mcp_project_adr_presence(
        store: *mut c_void,
        project: *const c_char,
    ) -> c_int;
}

// The returned wrapper must be dropped before C releases its store.
unsafe fn borrow_c_store(store: *mut c_void) -> Option<Store> {
    // SAFETY: callers pass the live C store and use the wrapper within the
    // callback. from_handle does not close the C connection on drop.
    let db = unsafe { cbm_store_get_db(store) };
    if db.is_null() {
        return None;
    }
    Some(Store::from_connection(
        unsafe { Connection::from_handle(db) }.ok()?,
    ))
}

unsafe extern "C" fn read_project_identity(
    store: *mut c_void,
    name_capacity: usize,
    emit: ProjectIdentityEmit,
    context: *mut c_void,
) -> c_int {
    const SKIP: c_int = 0;
    const MISSING_ROOT: c_int = -1;
    panic::catch_unwind(|| {
        // SAFETY: C passes an open query-only store and keeps it live until
        // the callback returns.
        let Some(store) = (unsafe { borrow_c_store(store) }) else {
            return SKIP;
        };
        let Ok(Some(identity)) = store.project_identity(name_capacity) else {
            return SKIP;
        };
        let Some(root_path) = identity.root_path else {
            return MISSING_ROOT;
        };
        let (Ok(name), Ok(root_path)) =
            (CString::new(identity.name), CString::new(root_path))
        else {
            return SKIP;
        };
        // SAFETY: C copies both strings synchronously into its project record.
        unsafe { emit(context, name.as_ptr(), root_path.as_ptr()) }
    })
    .unwrap_or(SKIP)
}

unsafe extern "C" fn read_project_stats(
    store: *mut c_void,
    project: *const c_char,
    nodes: *mut c_int,
    edges: *mut c_int,
) {
    let result = panic::catch_unwind(|| {
        // SAFETY: C keeps the store and project string live until return.
        let store = unsafe { borrow_c_store(store) }?;
        let project = unsafe { CStr::from_ptr(project) }.to_bytes();
        Some(store.project_stats(project))
    })
    .ok()
    .flatten()
    .unwrap_or((STORE_ERR, STORE_ERR));
    // SAFETY: C supplies distinct writable result fields in its record.
    unsafe { (*nodes, *edges) = result };
}

unsafe extern "C" fn read_project_metadata(
    store: *mut c_void,
    project: *const c_char,
    emit: ProjectMetadataEmit,
    context: *mut c_void,
) -> bool {
    panic::catch_unwind(|| {
        // SAFETY: C keeps the store and project string live until return.
        let store = unsafe { borrow_c_store(store) }?;
        let project = unsafe { CStr::from_ptr(project) }.to_bytes();
        let metadata = store.project_metadata(project).ok()??;
        let root_path = metadata
            .root_path
            .and_then(|value| CString::new(value).ok());
        let indexed_at = metadata
            .indexed_at
            .and_then(|value| CString::new(value).ok());
        let root_path = root_path
            .as_ref()
            .map_or(ptr::null(), |value| value.as_ptr());
        let indexed_at = indexed_at
            .as_ref()
            .map_or(ptr::null(), |value| value.as_ptr());
        // SAFETY: the sink copies both nullable strings synchronously.
        unsafe { emit(context, root_path, indexed_at) };
        Some(())
    })
    .ok()
    .flatten()
    .is_some()
}

unsafe extern "C" fn read_file_outline(
    db: *mut sqlite3,
    project: *const c_char,
    file_path: *const c_char,
    labels: *const *const c_char,
    label_count: c_int,
    limit: c_int,
    offset: c_int,
    total: *mut c_int,
    emit: OutlineEmit,
    context: *mut c_void,
) -> c_int {
    let result = panic::catch_unwind(|| {
        // SAFETY: the C store validates the NUL-terminated arguments and
        // label array before the call. The SQLite handle remains owned by C.
        let (project, path, labels, connection) = unsafe {
            let labels = if label_count == 0 {
                &[]
            } else {
                std::slice::from_raw_parts(labels, label_count as usize)
            };
            let labels = labels
                .iter()
                .map(|&label| CStr::from_ptr(label).to_bytes())
                .collect::<Vec<_>>();
            (
                CStr::from_ptr(project).to_bytes(),
                CStr::from_ptr(file_path).to_bytes(),
                labels,
                Connection::from_handle(db)?,
            )
        };
        let query = FileOutlineQuery {
            project,
            path,
            labels: &labels,
            limit,
            offset,
        };
        Store::from_connection(connection).file_outline(query, |row| {
            let fields = [row.name, row.label, row.qualified_name];
            let pointers = fields.map(|bytes| {
                if bytes.is_empty() {
                    c"".as_ptr()
                } else {
                    bytes.as_ptr().cast()
                }
            });
            let lengths = fields.map(<[u8]>::len);
            let raw = OutlineRow {
                name: pointers[0],
                label: pointers[1],
                qualified_name: pointers[2],
                start_line: row.start_line,
                end_line: row.end_line,
            };
            // SAFETY: the C sink copies the row synchronously before SQLite
            // advances, using the exact byte lengths of the borrowed slices.
            let status = unsafe { emit(context, &raw, lengths.as_ptr()) };
            if status == STORE_OK {
                ControlFlow::Continue(())
            } else {
                ControlFlow::Break(status)
            }
        })
    });
    match result {
        Ok(Ok(ControlFlow::Continue(count))) => {
            // SAFETY: the C store validates the output pointer before calling.
            unsafe { *total = count };
            STORE_OK
        }
        Ok(Ok(ControlFlow::Break(status))) => status,
        Ok(Err(rusqlite::Error::SqliteFailure(error, _)))
            if error.code == rusqlite::ErrorCode::OperationInterrupted =>
        {
            STORE_CANCELLED
        }
        _ => STORE_ERR,
    }
}

unsafe extern "C" fn read_override(
    tool: *const c_char,
    options: *const c_void,
    store: *mut c_void,
    project: *const c_char,
) -> *mut c_char {
    if tool.is_null()
        || options.is_null()
        || store.is_null()
        || project.is_null()
    {
        return ptr::null_mut();
    }
    let result = panic::catch_unwind(|| {
        // SAFETY: the C dispatch borrows valid NUL-terminated strings and a
        // request-owned C store for the duration of this synchronous call.
        let tool = unsafe { CStr::from_ptr(tool) };
        if tool.to_bytes() != b"get_graph_schema" {
            return None;
        }
        // SAFETY: the C dispatcher attaches normalized schema settings to
        // this tool name and keeps them live until the callback returns.
        let options = unsafe { &*options.cast::<GraphSchemaOptions>() };
        let project_name = unsafe { CStr::from_ptr(project) }.to_str().ok()?;
        let adr_present =
            match unsafe { cbm_mcp_project_adr_presence(store, project) } {
                0 => Some(false),
                1 => Some(true),
                _ => None,
            };
        let store = unsafe { borrow_c_store(store) }?;
        let payload = tools::graph_schema(
            &store,
            project_name,
            options.limit as usize,
            options.offset as usize,
            options.include_properties,
            adr_present,
        )?;
        CString::new(payload).ok()
    });
    result
        .ok()
        .flatten()
        .map_or(ptr::null_mut(), CString::into_raw)
}

unsafe extern "C" fn release_payload(payload: *mut c_char) {
    // SAFETY: C calls this once for each non-null pointer read_override
    // returns via CString::into_raw, without modifying the allocation.
    drop(unsafe { CString::from_raw(payload) });
}

// Keep these discriminants in sync with cbm_daemon_process_role_t in
// daemon/bootstrap.h and main_subcommand_t in main.c, respectively.
#[repr(C)]
enum ProcessRole {
    Invalid = 0,
    Stateless = 1,
    Daemon = 2,
    Worker = 3,
    McpClient = 4,
    LocalCli = 5,
    HookClient = 6,
    DaemonCtl = 7,
}

#[repr(C)]
enum Subcommand {
    None = 0,
    Version = 1,
    Help = 2,
    AllowRoot = 3,
    Cli = 4,
    HookAugment = 5,
    TestImpact = 6,
    Install = 7,
    Uninstall = 8,
    Update = 9,
    Config = 10,
}

impl Subcommand {
    fn parse(arg: &[u8]) -> Self {
        match arg {
            b"--version" => Self::Version,
            b"--help" | b"-h" => Self::Help,
            b"allow-root" => Self::AllowRoot,
            b"cli" => Self::Cli,
            b"hook-augment" => Self::HookAugment,
            b"test-impact" => Self::TestImpact,
            b"install" => Self::Install,
            b"uninstall" => Self::Uninstall,
            b"update" => Self::Update,
            b"config" => Self::Config,
            _ => Self::None,
        }
    }
}

/// Borrow the original C strings, including non-UTF-8 bytes on POSIX.
///
/// The caller supplies a C main argument vector that outlives the iterator.
unsafe fn argv_bytes<'a>(
    argc: c_int,
    argv: *const *mut c_char,
) -> impl Iterator<Item = &'a [u8]> + Clone {
    let argv = if argc > 0 {
        // SAFETY: the caller guarantees argc initialized slots.
        unsafe { std::slice::from_raw_parts(argv, argc as usize) }
    } else {
        &[]
    };
    argv.iter().map(|&arg| {
        // SAFETY: C main's argv entries are valid NUL-terminated strings.
        unsafe { CStr::from_ptr(arg) }.to_bytes()
    })
}

fn positive_worker_budget(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .try_fold(0usize, |value, &byte| {
            if byte.is_ascii_digit() {
                value.checked_mul(10)?.checked_add(usize::from(byte - b'0'))
            } else {
                None
            }
        })
        .is_some_and(|value| value > 0)
}

// Syntactic gate for the supervisor's re-exec. C verifies the actual build
// identity and worker options again before admitting the worker.
fn worker_args_exact<'a>(mut args: impl Iterator<Item = &'a [u8]>) -> bool {
    let [
        Some(b"cli"),
        Some(b"--index-worker"),
        Some(b"--index-worker-build"),
        Some(fingerprint),
        Some(b"index_repository"),
        Some(payload),
        Some(b"--response-out"),
        Some(response),
    ] = std::array::from_fn(|_| args.next())
    else {
        return false;
    };
    if fingerprint.len() != 64
        || !fingerprint
            .iter()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        || payload.is_empty()
        || response.is_empty()
    {
        return false;
    }

    let mut flags = args.peekable();
    if flags
        .next_if_eq(&b"--index-worker-memory-budget-bytes".as_slice())
        .is_some()
        && !flags.next().is_some_and(positive_worker_budget)
    {
        return false;
    }
    flags.next_if_eq(&b"--index-worker-single-thread".as_slice());
    for option in [
        b"--index-worker-marker".as_slice(),
        b"--index-worker-quarantine",
        b"--index-worker-publish-fence",
    ] {
        if flags.next_if_eq(&option).is_some()
            && !flags.next().is_some_and(|value| !value.is_empty())
        {
            return false;
        }
    }
    flags.next().is_none()
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_main_role(
    argc: c_int,
    argv: *const *mut c_char,
) -> ProcessRole {
    // SAFETY: cbm_c_main passes its live, platform-prepared argument vector.
    let mut args = unsafe { argv_bytes(argc, argv) };
    if args.next().is_none_or(<[u8]>::is_empty) {
        return ProcessRole::Invalid;
    }

    const DAEMON_INTERNAL: &[u8] = b"--cbm-daemon-internal";
    if args.clone().any(|arg| arg == DAEMON_INTERNAL) {
        let exact = args.next() == Some(DAEMON_INTERNAL)
            && matches!(
                (args.next(), args.next()),
                (None, None) | (Some(b"--cbm-daemon-permanent"), None)
            );
        return if exact {
            ProcessRole::Daemon
        } else {
            ProcessRole::Invalid
        };
    }
    if args.clone().any(|arg| arg == b"--index-worker") {
        return if worker_args_exact(args) {
            ProcessRole::Worker
        } else {
            ProcessRole::Invalid
        };
    }

    // Stop at the first public mode. CLI arguments can contain command names
    // or --version. Help anywhere after cli, config, or daemon is the legacy
    // exception and bypasses daemon coordination.
    while let Some(arg) = args.next() {
        let role = match Subcommand::parse(arg) {
            Subcommand::Cli | Subcommand::Config => ProcessRole::LocalCli,
            Subcommand::HookAugment => return ProcessRole::HookClient,
            Subcommand::None if arg == b"daemon" => ProcessRole::DaemonCtl,
            Subcommand::None => continue,
            _ => return ProcessRole::Stateless,
        };
        return if args
            .any(|arg| matches!(Subcommand::parse(arg), Subcommand::Help))
        {
            ProcessRole::Stateless
        } else {
            role
        };
    }
    ProcessRole::McpClient
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_main_subcommand(
    argc: c_int,
    argv: *const *mut c_char,
    next_arg: *mut c_int,
) -> Subcommand {
    // SAFETY: cbm_c_main passes its live, platform-prepared argument vector.
    let args = unsafe { argv_bytes(argc, argv) };
    for (index, arg) in (1..argc).zip(args.skip(1)) {
        let command = Subcommand::parse(arg);
        if !matches!(command, Subcommand::None) {
            // SAFETY: C supplies a writable int slot for the argument offset.
            unsafe { next_arg.write(index + 1) };
            return command;
        }
    }
    // SAFETY: Same output slot as above.
    unsafe { next_arg.write(argc) };
    Subcommand::None
}

/// The C runtime calls this entry point. Rust handles developer export and
/// standalone UI here. Other commands enter C startup and use the Rust selectors.
/// Returning hands the exit code to the C runtime.
///
/// # Safety
///
/// Called only by the C runtime, with argc strings in argv followed by
/// NULL, as for any C `main`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn main(argc: c_int, argv: *mut *mut c_char) -> c_int {
    // SAFETY: the C runtime provides argc initialized, non-null argv slots.
    let mut args = unsafe { argv_bytes(argc, argv) }.skip(1);
    let command = args.next();
    if command == Some(b"dev") {
        // SAFETY: SQLite has not been used yet. Initialize the same
        // allocator as C main before the exporter opens the database.
        unsafe { cbm_alloc_init() };
        return crate::export::run(args.map(OsStr::from_bytes));
    }
    // SAFETY: C has not started any indexing threads and the function pointer
    // stays valid for the process lifetime, including each worker's own main.
    unsafe {
        jiyuu_mori::mori_register_graphql_callbacks();
        if std::env::var_os("MORI_IMPL_GRAPHQL_EXTRACT").as_deref()
            == Some(OsStr::new("rust"))
        {
            jiyuu_mori::mori_register_graphql_extractor();
        }
        if std::env::var_os("MORI_IMPL_GO_IMPORTS").as_deref()
            == Some(OsStr::new("rust"))
        {
            jiyuu_mori::mori_register_go_import_extractor();
        }
        if std::env::var_os("MORI_IMPL_MARKDOWN_EXTRACT").as_deref()
            == Some(OsStr::new("rust"))
        {
            jiyuu_mori::mori_register_markdown_extractor();
        }
        #[cfg(unix)]
        jiyuu_mori::mori_register_staged_graph_writer();
        #[cfg(unix)]
        jiyuu_mori::mori_register_discover_walk();
        c_transport::register();
        crate::repair_recovery::register();
        crate::named_watch::register();
        crate::job_recovery::register();
        crate::index_manifest::register();
        crate::index_route::register();
        crate::full_build_driver::register();
        if std::env::var_os("MORI_IMPL_SOURCE_PUBLISH")
            .is_none_or(|value| value == "rust")
        {
            jiyuu_mori::mori_register_source_publish();
        }
        crate::importance::register();
        crate::query_cli::c_args::register();
        crate::cli_output::register();
        crate::initialize::c_initialize::register();
        crate::mcp_dispatch::register();
        crate::cypher_ffi::register();
        crate::tool_list::c_tool_list::register();
        crate::search::register();
        crate::path_verdict::register();
        crate::trace_predecessor::register();
        crate::snippet_window::register();
        crate::ingest_trace::register();
    }

    // SAFETY: these process-wide callbacks are registered before C starts any work.
    unsafe {
        cbm_mcp_set_read_override(read_override, release_payload);
        cbm_store_set_file_outline_reader(read_file_outline);
        cbm_mcp_set_project_readers(
            read_project_identity,
            read_project_stats,
            read_project_metadata,
        );
        cbm_mcp_set_project_page_builder(build_project_page);
        crate::project_response::register_renderers(release_payload);
    }
    if command == Some(b"ui") {
        return crate::ui::run(&args.collect::<Vec<_>>());
    }
    // SAFETY: the original C runtime arguments meet cbm_c_main's contract.
    unsafe { cbm_c_main(argc, argv) }
}
