//! Bind a version-2 graph digest to an existing native read snapshot.

use super::{
    ReadScope, STORE_CANCELLED, STORE_ERR, STORE_OK, Statement,
    cbm_store_get_db, cbm_store_read_scope_check, cbm_store_read_scope_fail,
    cbm_store_read_scope_store, sql,
};
use sha2::{Digest, Sha256};
use std::ffi::{CStr, c_int};
use std::ptr::{NonNull, addr_of_mut, write_bytes};
use std::{slice, str};

const VERSION: u32 = 2;
const CHUNK: usize = 65_536;
const MAX_COLUMNS: usize = 10;
const COVERAGE_META: usize = 7;
const STORE_META_INDEX: usize = 8;
const UNRESOLVED: usize = 9;

const NULL: u8 = 0;
const INTEGER: u8 = 1;
const TEXT: u8 = 2;
const BLOB: u8 = 3;

const N: u8 = 1 << NULL;
const I: u8 = 1 << INTEGER;
const T: u8 = 1 << TEXT;
const B: u8 = 1 << BLOB;

struct Column {
    name: &'static [u8],
    types: u8,
    kind: u8,
    primary_key: i64,
}

const fn column(
    name: &'static [u8],
    types: u8,
    kind: u8,
    primary_key: i64,
) -> Column {
    Column {
        name,
        types,
        kind,
        primary_key,
    }
}

struct Table {
    name: &'static CStr,
    columns: &'static [Column],
    schema: &'static CStr,
    rows: &'static CStr,
}

const PROJECTS: &[Column] = &[
    column(b"name", T, 0, 1),
    column(b"indexed_at", T, 0, 0),
    column(b"root_path", T, 0, 0),
];
const FILE_HASHES: &[Column] = &[
    column(b"project", T, 0, 1),
    column(b"rel_path", T, 0, 2),
    column(b"sha256", T, 0, 0),
    column(b"mtime_ns", I, 0, 0),
    column(b"size", I, 0, 0),
];
const NODES: &[Column] = &[
    column(b"id", I, 0, 1),
    column(b"project", T, 0, 0),
    column(b"label", T, 0, 0),
    column(b"name", T, 0, 0),
    column(b"qualified_name", T, 0, 0),
    column(b"file_path", T | N, 0, 0),
    column(b"start_line", I | N, 0, 0),
    column(b"end_line", I | N, 0, 0),
    column(b"properties", T | N, 0, 0),
];
const EDGES: &[Column] = &[
    column(b"id", I, 0, 1),
    column(b"project", T, 0, 0),
    column(b"source_id", I, 0, 0),
    column(b"target_id", I, 0, 0),
    column(b"type", T, 0, 0),
    column(b"properties", T | N, 0, 0),
    column(b"url_path_gen", T | N, 2, 0),
    column(b"local_name_gen", T | N, 2, 0),
];
const PROJECT_SUMMARIES: &[Column] = &[
    column(b"project", T, 0, 1),
    column(b"summary", T, 0, 0),
    column(b"source_hash", T, 0, 0),
    column(b"created_at", T, 0, 0),
    column(b"updated_at", T, 0, 0),
];
const LSP_SURFACE: &[Column] = &[
    column(b"project", T, 0, 1),
    column(b"rel_path", T, 0, 2),
    column(b"surface_sha", T, 0, 0),
    column(b"defs_json", T, 0, 0),
    column(b"ref_bloom", B | N, 0, 0),
    column(b"config_ctx", T, 0, 0),
];
const INDEX_COVERAGE: &[Column] = &[
    column(b"project", T, 0, 1),
    column(b"rel_path", T, 0, 2),
    column(b"kind", T, 0, 3),
    column(b"detail", T | N, 0, 0),
];
const INDEX_COVERAGE_META: &[Column] = &[
    column(b"project", T, 0, 1),
    column(b"generation", T, 0, 0),
    column(b"index_mode", T, 0, 0),
    column(b"recorded_at", T, 0, 0),
    column(b"recording_status", T, 0, 0),
    column(b"ignored_files_stored", I, 0, 0),
    column(b"ignored_files_total", I, 0, 0),
    column(b"coverage_version", I, 0, 0),
    column(b"hash_records_complete", I, 0, 0),
    column(b"unresolved_calls_complete", I, 0, 0),
];
const STORE_META: &[Column] = &[column(b"k", T, 0, 1), column(b"v", T, 0, 0)];
const UNRESOLVED_CANDIDATES: &[Column] = &[
    column(b"project", T, 0, 1),
    column(b"candidate", T, 0, 2),
    column(b"rel_path", T, 0, 3),
    column(b"kind", T, 0, 0),
];

// Descriptor and column order are part of the canonical stream.
const TABLES: [Table; 10] = [
    Table {
        name: c"projects",
        columns: PROJECTS,
        schema: c"PRAGMA main.table_xinfo('projects');",
        rows: c"SELECT name,indexed_at,root_path FROM main.projects ORDER BY CAST(name AS BLOB) ASC;",
    },
    Table {
        name: c"file_hashes",
        columns: FILE_HASHES,
        schema: c"PRAGMA main.table_xinfo('file_hashes');",
        rows: c"SELECT project,rel_path,sha256,mtime_ns,size FROM main.file_hashes ORDER BY CAST(project AS \
            BLOB) ASC,CAST(rel_path AS BLOB) ASC;",
    },
    Table {
        name: c"nodes",
        columns: NODES,
        schema: c"PRAGMA main.table_xinfo('nodes');",
        rows: c"SELECT id,project,label,name,qualified_name,file_path,start_line,end_line,properties FROM \
            main.nodes ORDER BY id ASC;",
    },
    Table {
        name: c"edges",
        columns: EDGES,
        schema: c"PRAGMA main.table_xinfo('edges');",
        rows: c"SELECT id,project,source_id,target_id,type,properties,url_path_gen,local_name_gen FROM \
            main.edges ORDER BY id ASC;",
    },
    Table {
        name: c"project_summaries",
        columns: PROJECT_SUMMARIES,
        schema: c"PRAGMA main.table_xinfo('project_summaries');",
        rows: c"SELECT project,summary,source_hash,created_at,updated_at FROM main.project_summaries ORDER \
            BY CAST(project AS BLOB) ASC;",
    },
    Table {
        name: c"lsp_surface",
        columns: LSP_SURFACE,
        schema: c"PRAGMA main.table_xinfo('lsp_surface');",
        rows: c"SELECT project,rel_path,surface_sha,defs_json,ref_bloom,config_ctx FROM main.lsp_surface \
            ORDER BY CAST(project AS BLOB) ASC,CAST(rel_path AS BLOB) ASC;",
    },
    Table {
        name: c"index_coverage",
        columns: INDEX_COVERAGE,
        schema: c"PRAGMA main.table_xinfo('index_coverage');",
        rows: c"SELECT project,rel_path,kind,detail FROM main.index_coverage ORDER BY CAST(project AS BLOB) \
            ASC,CAST(rel_path AS BLOB) ASC,CAST(kind AS BLOB) ASC;",
    },
    Table {
        name: c"index_coverage_meta",
        columns: INDEX_COVERAGE_META,
        schema: c"PRAGMA main.table_xinfo('index_coverage_meta');",
        rows: c"SELECT \
            project,generation,index_mode,recorded_at,recording_status,ignored_files_stored,ignored_files_total,coverage_version,hash_records_complete \
            FROM main.index_coverage_meta ORDER BY CAST(project AS BLOB) ASC;",
    },
    Table {
        name: c"store_meta",
        columns: STORE_META,
        schema: c"PRAGMA main.table_xinfo('store_meta');",
        rows: c"SELECT k,v FROM main.store_meta ORDER BY CAST(k AS BLOB) ASC;",
    },
    Table {
        name: c"index_unresolved_candidates",
        columns: UNRESOLVED_CANDIDATES,
        schema: c"PRAGMA main.table_xinfo('index_unresolved_candidates');",
        rows: c"SELECT project,candidate,rel_path,kind FROM main.index_unresolved_candidates ORDER BY \
            CAST(project AS BLOB) ASC,CAST(candidate AS BLOB) ASC,CAST(rel_path AS BLOB) ASC,CAST(kind \
            AS BLOB) ASC;",
    },
];

const COVERAGE_ROWS_CURRENT: &CStr = c"SELECT \
    project,generation,index_mode,recorded_at,recording_status,ignored_files_stored,ignored_files_total,coverage_version,hash_records_complete,unresolved_calls_complete \
    FROM main.index_coverage_meta ORDER BY CAST(project AS BLOB) ASC;";

#[derive(Clone, Copy)]
#[repr(i32)]
enum Status {
    Invalid = 1,
    Schema,
    ProjectMissing,
    Limit,
    Cancelled,
    Error,
}

type DigestResult<T> = Result<T, Status>;

#[derive(Clone, Copy)]
#[repr(C)]
struct Limits {
    max_rows: u64,
    max_framed_bytes: u64,
}

#[repr(C)]
struct DigestOutput {
    version: u32,
    sha256: [u8; 32],
    rows: u64,
    framed_bytes: u64,
}

struct Scope {
    ptr: NonNull<ReadScope>,
    error: Option<Status>,
}

impl Scope {
    fn latch(&mut self, error: Status) -> Status {
        *self.error.get_or_insert_with(|| {
            let code = match error {
                Status::Cancelled => STORE_CANCELLED,
                _ => STORE_ERR,
            };
            // SAFETY: The caller keeps the original read scope live.
            unsafe { cbm_store_read_scope_fail(self.ptr.as_ptr(), code) };
            error
        })
    }

    fn check(&mut self) -> DigestResult<()> {
        // SAFETY: The caller retains this exclusive read scope.
        let code = unsafe { cbm_store_read_scope_check(self.ptr.as_ptr()) };
        match code {
            STORE_OK => self.error.map_or(Ok(()), Err),
            STORE_CANCELLED => Err(self.latch(Status::Cancelled)),
            _ => Err(self.latch(Status::Error)),
        }
    }

    fn sqlite_failure(&mut self, code: c_int) -> Status {
        if code == sql::SQLITE_INTERRUPT {
            // Only an interruption latched by the scope means cancellation.
            let _ = self.check();
        }
        self.latch(Status::Error)
    }
}

struct Graph<'a> {
    scope: &'a mut Scope,
    db: NonNull<sql::sqlite3>,
    limits: Limits,
    sha: Sha256,
    rows: u64,
    bytes: u64,
    has_unresolved_completeness: bool,
    has_db_uid: bool,
    has_mutation_gen: bool,
}

fn text(stmt: &Statement, col: c_int) -> DigestResult<&[u8]> {
    if stmt.kind(col) != sql::SQLITE_TEXT {
        return Err(Status::Schema);
    }
    stmt.text(col).ok_or(Status::Error)
}

fn table_index(name: &[u8]) -> Option<usize> {
    TABLES
        .iter()
        .position(|table| name.eq_ignore_ascii_case(table.name.to_bytes()))
}

impl Graph<'_> {
    fn with_statement<T>(
        &mut self,
        query: &CStr,
        run: impl FnOnce(&mut Self, &mut Statement) -> DigestResult<T>,
    ) -> DigestResult<T> {
        self.scope.check()?;
        // SAFETY: The scope keeps the connection live for the entire scan.
        let mut stmt = match unsafe { Statement::prepare(self.db, query) } {
            Ok(stmt) => stmt,
            Err(code) => {
                let error = self.scope.sqlite_failure(code);
                let _ = self.scope.check();
                return Err(error);
            }
        };
        let result = self
            .scope
            .check()
            .and_then(|()| run(self, &mut stmt))
            .map_err(|error| self.scope.latch(error));

        // Unconditionally release the statement, including on a failed scope.
        let _ = self.scope.check();
        let code = stmt.finalize();
        if code != sql::SQLITE_OK {
            self.scope.sqlite_failure(code);
        }
        let finalized = self.scope.check();
        result.and_then(|value| finalized.map(|()| value))
    }

    /// Return whether the checked statement advanced to another row.
    fn step(&mut self, stmt: &mut Statement) -> DigestResult<bool> {
        self.scope.check()?;
        let code = stmt.step();
        if code != sql::SQLITE_ROW && code != sql::SQLITE_DONE {
            self.scope.sqlite_failure(code);
        }
        self.scope.check()?;
        Ok(code == sql::SQLITE_ROW)
    }

    fn check_binding(&mut self, code: c_int) -> DigestResult<()> {
        if code == sql::SQLITE_OK {
            Ok(())
        } else {
            Err(self.scope.sqlite_failure(code))
        }
    }

    fn validate_schema(
        &mut self,
        project: &[u8],
    ) -> DigestResult<[bool; TABLES.len()]> {
        self.with_statement(c"PRAGMA main.encoding;", |scan, stmt| {
            if stmt.columns() != 1
                || !scan.step(stmt)?
                || text(stmt, 0)? != b"UTF-8"
                || scan.step(stmt)?
            {
                return Err(Status::Schema);
            }
            Ok(())
        })?;
        let present = self.check_tables()?;
        for (index, present) in present.iter().enumerate() {
            if *present {
                self.check_columns(index)?;
            }
        }
        self.with_statement(
            c"SELECT 1 FROM main.projects WHERE CAST(name AS BLOB)=?1 LIMIT 2;",
            |scan, stmt| {
                // SAFETY: The input project lives for the whole call, including finalization.
                scan.check_binding(unsafe { stmt.bind_blob(project) })?;
                if !scan.step(stmt)? {
                    return Err(Status::ProjectMissing);
                }
                if scan.step(stmt)? {
                    return Err(Status::Schema);
                }
                Ok(())
            },
        )?;
        Ok(present)
    }

    fn check_tables(&mut self) -> DigestResult<[bool; TABLES.len()]> {
        let present =
            self.with_statement(c"PRAGMA main.table_list;", |scan, stmt| {
                // An unrecognized pragma can return no columns and no error.
                if stmt.columns() < 3 {
                    return Err(Status::Schema);
                }
                let mut present = [false; TABLES.len()];
                while scan.step(stmt)? {
                    let schema = text(stmt, 0)?;
                    let name = text(stmt, 1)?;
                    let kind = text(stmt, 2)?;
                    if schema != b"main" {
                        return Err(Status::Schema);
                    }
                    if let Some(index) = table_index(name) {
                        if present[index]
                            || name != TABLES[index].name.to_bytes()
                            || kind != b"table"
                        {
                            return Err(Status::Schema);
                        }
                        present[index] = true;
                    }
                }
                Ok(present)
            })?;
        if present[..STORE_META_INDEX].contains(&false) {
            return Err(Status::Schema);
        }
        self.with_statement(
            c"SELECT name FROM temp.sqlite_schema WHERE type IN ('table','view');",
            |scan, stmt| {
                while scan.step(stmt)? {
                    if table_index(text(stmt, 0)?).is_some() {
                        return Err(Status::Schema);
                    }
                }
                Ok(())
            },
        )?;
        Ok(present)
    }

    fn check_columns(&mut self, index: usize) -> DigestResult<()> {
        let table = &TABLES[index];
        let legacy_coverage = index == COVERAGE_META;
        let required = table.columns.len() - usize::from(legacy_coverage);
        let seen = self.with_statement(table.schema, |scan, stmt| {
            let mut seen = [false; MAX_COLUMNS];
            if stmt.columns() != 7 {
                return Err(Status::Schema);
            }
            while scan.step(stmt)? {
                let name = text(stmt, 1)?;
                let Some((column, expected)) = table
                    .columns
                    .iter()
                    .enumerate()
                    .find(|(_, column)| column.name == name)
                else {
                    return Err(Status::Schema);
                };
                if seen[column]
                    || stmt.kind(5) != sql::SQLITE_INTEGER
                    || stmt.kind(6) != sql::SQLITE_INTEGER
                    || stmt.integer(5) != expected.primary_key
                    || stmt.integer(6) != i64::from(expected.kind)
                {
                    return Err(Status::Schema);
                }
                if expected.primary_key != 0
                    && expected.types == I
                    && !text(stmt, 2)?.eq_ignore_ascii_case(b"INTEGER")
                {
                    return Err(Status::Schema);
                }
                seen[column] = true;
            }
            if seen[..required].contains(&false) {
                return Err(Status::Schema);
            }
            Ok(seen)
        })?;
        if legacy_coverage {
            self.has_unresolved_completeness = seen[required];
        }
        Ok(())
    }

    fn bytes(&mut self, bytes: &[u8]) -> DigestResult<()> {
        self.scope.check()?;
        let length = u64::try_from(bytes.len()).map_err(|_| Status::Limit)?;
        if length > self.limits.max_framed_bytes - self.bytes {
            return Err(Status::Limit);
        }
        for chunk in bytes.chunks(CHUNK) {
            self.scope.check()?;
            self.sha.update(chunk);
            self.bytes += chunk.len() as u64;
            self.scope.check()?;
        }
        Ok(())
    }

    fn value(&mut self, kind: u8, bytes: &[u8]) -> DigestResult<()> {
        let length = u64::try_from(bytes.len()).map_err(|_| Status::Limit)?;
        self.bytes(&[kind])?;
        self.bytes(&length.to_be_bytes())?;
        self.bytes(bytes)
    }

    fn table_header(
        &mut self,
        index: usize,
        table: &Table,
        columns: &[Column],
        present: bool,
    ) -> DigestResult<()> {
        self.bytes(&[0x20])?;
        self.bytes(&(index as u32).to_be_bytes())?;
        self.value(TEXT, table.name.to_bytes())?;
        self.bytes(&(columns.len() as u32).to_be_bytes())?;
        for column in columns {
            self.value(TEXT, column.name)?;
            self.bytes(&[column.types])?;
            self.bytes(&[column.kind])?;
        }
        self.bytes(&[u8::from(present)])?;
        if !present {
            return self.value(NULL, &[]);
        }

        self.with_statement(
            c"SELECT name,type,sql FROM main.sqlite_schema WHERE name=?1 COLLATE NOCASE AND type IN \
                ('table','view');",
            |scan, stmt| {
                scan.check_binding(stmt.bind_static_text(table.name))?;
                if !scan.step(stmt)? {
                    return Err(Status::Schema);
                }
                let name = text(stmt, 0)?;
                let kind = text(stmt, 1)?;
                let sql = text(stmt, 2)?;
                if name != table.name.to_bytes() || kind != b"table" {
                    return Err(Status::Schema);
                }
                scan.value(TEXT, sql)?;
                if scan.step(stmt)? {
                    return Err(Status::Schema);
                }
                Ok(())
            },
        )
    }

    fn check_generation(&mut self, stmt: &Statement) -> DigestResult<()> {
        let key = text(stmt, 0)?;
        let value = text(stmt, 1)?;
        if key == b"db_uid" {
            if self.has_db_uid
                || value.len() != 16
                || !value.iter().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
            {
                return Err(Status::Schema);
            }
            self.has_db_uid = true;
        } else if key == b"mutation_gen" {
            if self.has_mutation_gen
                || value.is_empty()
                || value.len() > 20
                || (value.len() > 1 && value[0] == b'0')
                || !value.iter().all(u8::is_ascii_digit)
                || str::from_utf8(value)
                    .ok()
                    .and_then(|n| n.parse::<u64>().ok())
                    .is_none()
            {
                return Err(Status::Schema);
            }
            self.has_mutation_gen = true;
        }
        Ok(())
    }

    fn column_value(
        &mut self,
        stmt: &Statement,
        index: c_int,
        column: &Column,
    ) -> DigestResult<()> {
        let kind = match stmt.kind(index) {
            sql::SQLITE_NULL => NULL,
            sql::SQLITE_INTEGER => INTEGER,
            sql::SQLITE_TEXT => TEXT,
            sql::SQLITE_BLOB => BLOB,
            _ => return Err(Status::Schema),
        };
        if column.types & (1 << kind) == 0 {
            return Err(Status::Schema);
        }
        match kind {
            NULL => self.value(kind, &[]),
            INTEGER => self.value(kind, &stmt.integer(index).to_be_bytes()),
            TEXT => self.value(kind, text(stmt, index)?),
            BLOB => self.value(kind, stmt.blob(index).ok_or(Status::Error)?),
            _ => unreachable!("kind mapped from supported SQLite types"),
        }
    }

    fn table_rows(
        &mut self,
        index: usize,
        query: &CStr,
        columns: &[Column],
        present: bool,
    ) -> DigestResult<()> {
        let mut rows = 0u64;
        if present {
            self.with_statement(query, |scan, stmt| {
                if stmt.columns() != columns.len() as c_int {
                    return Err(Status::Schema);
                }
                while scan.step(stmt)? {
                    scan.scope.check()?;
                    if scan.rows == scan.limits.max_rows {
                        return Err(Status::Limit);
                    }
                    if index == STORE_META_INDEX {
                        scan.check_generation(stmt)?;
                    }
                    scan.bytes(&[0x21])?;
                    scan.bytes(&(columns.len() as u32).to_be_bytes())?;
                    for (index, column) in columns.iter().enumerate() {
                        scan.column_value(stmt, index as c_int, column)?;
                    }
                    rows += 1;
                    scan.rows += 1;
                }
                Ok(())
            })?;
            if index == STORE_META_INDEX
                && (!self.has_db_uid || !self.has_mutation_gen)
            {
                return Err(Status::Schema);
            }
        }
        self.bytes(&[0x22])?;
        self.bytes(&rows.to_be_bytes())
    }

    fn run(&mut self, project: &[u8]) -> DigestResult<()> {
        let tables = self.validate_schema(project)?;
        self.bytes(b"cbm-store-graph-digest\0")?;
        self.bytes(&VERSION.to_be_bytes())?;
        self.bytes(&[0x10])?;
        self.value(TEXT, project)?;

        for (index, (table, present)) in TABLES.iter().zip(tables).enumerate() {
            if index == UNRESOLVED && !present {
                continue;
            }
            let (columns, query) = if index != COVERAGE_META {
                (table.columns, table.rows)
            } else if self.has_unresolved_completeness {
                (table.columns, COVERAGE_ROWS_CURRENT)
            } else {
                (&table.columns[..table.columns.len() - 1], table.rows)
            };
            self.table_header(index, table, columns, present)?;
            self.table_rows(index, query, columns, present)?;
        }

        self.bytes(&[0x7f])?;
        self.bytes(&self.rows.to_be_bytes())?;
        self.scope.check()
    }
}

/// The caller owns an exclusive native read scope throughout this call and
/// closes it before using a digest as a binding. Input pointers follow the
/// length, lifetime, and exclusive output requirements in store_graph_digest.h.
#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_store_graph_digest(
    scope: *mut ReadScope,
    project: *const u8,
    project_len: usize,
    limits: *const Limits,
    out: *mut DigestOutput,
) -> c_int {
    if !out.is_null() {
        // SAFETY: The caller provides writable output. C expects padding zeroed too.
        unsafe { write_bytes(out, 0, 1) };
    }
    let Some(ptr) = NonNull::new(scope) else {
        return Status::Invalid as c_int;
    };
    let mut scope = Scope { ptr, error: None };
    if let Err(error) = scope.check() {
        return error as c_int;
    }

    // SAFETY: When non-null, limits points at a live C input value.
    let Some(&limits) = (unsafe { limits.as_ref() }) else {
        return scope.latch(Status::Invalid) as c_int;
    };
    if project.is_null()
        || project_len == 0
        || project_len > c_int::MAX as usize
        || out.is_null()
        || limits.max_rows == 0
        || limits.max_framed_bytes == 0
        || limits.max_framed_bytes > u64::MAX / 8
    {
        return scope.latch(Status::Invalid) as c_int;
    }
    // SAFETY: The caller lends project_len initialized bytes.
    let project = unsafe { slice::from_raw_parts(project, project_len) };
    for chunk in project.chunks(CHUNK) {
        if let Err(error) = scope.check() {
            return error as c_int;
        }
        if chunk.contains(&0) {
            return scope.latch(Status::Invalid) as c_int;
        }
        if let Err(error) = scope.check() {
            return error as c_int;
        }
    }
    // SAFETY: The live scope owns this borrowed store and its pinned SQLite handle.
    let db = unsafe {
        NonNull::new(cbm_store_read_scope_store(ptr.as_ptr()))
            .and_then(|store| NonNull::new(cbm_store_get_db(store.as_ptr())))
    };
    let Some(db) = db else {
        return scope.latch(Status::Error) as c_int;
    };
    let mut graph = Graph {
        scope: &mut scope,
        db,
        limits,
        sha: Sha256::new(),
        rows: 0,
        bytes: 0,
        has_unresolved_completeness: false,
        has_db_uid: false,
        has_mutation_gen: false,
    };
    if let Err(error) = graph.run(project) {
        return graph.scope.latch(error) as c_int;
    }
    let digest = graph.sha.finalize();
    if let Err(error) = graph.scope.check().and_then(|()| graph.scope.check()) {
        return error as c_int;
    }

    // SAFETY: out was checked non-null and the caller owns it exclusively.
    // Write fields individually so that the pre-zeroed padding stays intact.
    unsafe {
        addr_of_mut!((*out).version).write(VERSION);
        addr_of_mut!((*out).sha256).write(digest.into());
        addr_of_mut!((*out).rows).write(graph.rows);
        addr_of_mut!((*out).framed_bytes).write(graph.bytes);
    }
    STORE_OK
}
