//! Digest graph identity independently of row IDs and generation metadata.

use super::{STORE_ERR, STORE_OK, Statement, Store, cbm_store_get_db, sql};
use sha2::{Digest, Sha256};
use std::ffi::{CStr, c_char, c_int};
use std::io::Write;
use std::ptr::{NonNull, copy_nonoverlapping, null_mut};

struct Query {
    name: &'static [u8],
    sql: &'static CStr,
}

const CONTENT_QUERIES: &[Query] = &[
    Query { name: b"nodes", sql: c"SELECT label,name,qualified_name,file_path,start_line,end_line,properties FROM main.nodes WHERE \
        project=?1 ORDER BY 3,1,4,5,6,2,7;" },
    Query { name: b"edges", sql: c"SELECT \
        s.label,s.qualified_name,s.file_path,s.start_line,e.type,t.label,t.qualified_name,t.file_path,t.start_line,e.properties,e.url_path_gen,e.local_name_gen \
        FROM main.edges e JOIN main.nodes s ON s.id=e.source_id JOIN main.nodes t ON t.id=e.target_id \
        WHERE e.project=?1 ORDER BY 2,1,3,4,5,7,6,8,9,10,11,12;" },
    Query { name: b"dangling_edges", sql: c"SELECT COUNT(*) FROM main.edges e WHERE e.project=?1 AND (NOT EXISTS (SELECT 1 FROM main.nodes n \
        WHERE n.id=e.source_id) OR NOT EXISTS (SELECT 1 FROM main.nodes n WHERE n.id=e.target_id));" },
    Query { name: b"file_hashes", sql: c"SELECT rel_path,sha256,size FROM main.file_hashes WHERE project=?1 ORDER BY 1,2,3;" },
    Query { name: b"lsp_surface", sql: c"SELECT rel_path,surface_sha,defs_json,ref_bloom,config_ctx FROM main.lsp_surface WHERE \
        project=?1 ORDER BY 1,2,3,4,5;" },
    Query { name: b"index_coverage", sql: c"SELECT rel_path,kind,detail FROM main.index_coverage WHERE project=?1 ORDER BY 1,2,3;" },
    Query { name: b"index_coverage_meta", sql: c"SELECT \
        index_mode,recording_status,ignored_files_stored,ignored_files_total,coverage_version,hash_records_complete,unresolved_calls_complete \
        FROM main.index_coverage_meta WHERE project=?1;" },
    Query { name: b"project_summaries", sql: c"SELECT summary,source_hash FROM main.project_summaries WHERE project=?1;" },
    Query { name: b"index_unresolved_candidates", sql: c"SELECT candidate,rel_path,kind FROM main.index_unresolved_candidates WHERE project=?1 ORDER BY \
        1,2,3;" },
];

const TOPOLOGY_QUERIES: &[Query] = &[
    Query { name: b"nodes", sql: c"SELECT label,name,qualified_name,file_path,start_line,end_line FROM main.nodes WHERE project=?1 \
        ORDER BY 3,1,4,5,6,2;" },
    Query { name: b"edges", sql: c"SELECT \
        s.label,s.qualified_name,s.file_path,s.start_line,e.type,t.label,t.qualified_name,t.file_path,t.start_line,e.url_path_gen,e.local_name_gen \
        FROM main.edges e JOIN main.nodes s ON s.id=e.source_id JOIN main.nodes t ON t.id=e.target_id \
        WHERE e.project=?1 AND e.type NOT IN ('SIMILAR_TO','SEMANTICALLY_RELATED') ORDER BY \
        2,1,3,4,5,7,6,8,9,10,11;" },
    Query { name: b"file_hashes", sql: c"SELECT rel_path,sha256,size FROM main.file_hashes WHERE project=?1 ORDER BY 1,2,3;" },
    Query { name: b"index_coverage", sql: c"SELECT rel_path,kind,detail FROM main.index_coverage WHERE project=?1 ORDER BY 1,2,3;" },
];

const LEGACY_COVERAGE: &CStr = c"SELECT \
    index_mode,recording_status,ignored_files_stored,ignored_files_total,coverage_version,hash_records_complete \
    FROM main.index_coverage_meta WHERE project=?1;";
const UNRESOLVED_SCHEMA: &CStr = c"SELECT 1 FROM main.sqlite_schema WHERE name='index_unresolved_candidates' COLLATE NOCASE LIMIT 1;";

unsafe extern "C" {
    fn snprintf(
        output: *mut c_char,
        size: usize,
        format: *const c_char,
        ...
    ) -> c_int;
}

fn frame(sha: &mut Sha256, tag: u8, bytes: &[u8]) {
    sha.update([tag]);
    sha.update((bytes.len() as u64).to_be_bytes());
    sha.update(bytes);
}

fn table(
    db: NonNull<sql::sqlite3>,
    query: &Query,
    project: &CStr,
    sha: &mut Sha256,
) -> Result<(), ()> {
    // SAFETY: The caller holds the live connection through this table scan.
    let mut stmt = unsafe {
        match Statement::prepare(db, query.sql) {
            Ok(stmt) => stmt,
            Err(_) if query.name == b"index_unresolved_candidates" => {
                // An absent legacy table contributes no frame. Errors reading
                // an existing table invalidate the digest.
                let mut schema =
                    Statement::prepare(db, UNRESOLVED_SCHEMA).map_err(|_| ())?;
                let absent = schema.step() == sql::SQLITE_DONE;
                return (schema.finalize() == sql::SQLITE_OK && absent)
                    .then_some(())
                    .ok_or(());
            }
            Err(_) if query.name == b"index_coverage_meta" => {
                Statement::prepare(db, LEGACY_COVERAGE).map_err(|_| ())?
            }
            Err(_) => return Err(()),
        }
    };
    if stmt.bind_text(project) != sql::SQLITE_OK {
        return Err(());
    }
    frame(sha, b'T', query.name);
    let mut rows = 0u64;
    loop {
        match stmt.step() {
            sql::SQLITE_DONE => break,
            sql::SQLITE_ROW => (),
            _ => return Err(()),
        }
        let columns = stmt.columns();
        frame(sha, b'R', &columns.to_be_bytes());
        for column in 0..columns {
            match stmt.kind(column) {
                sql::SQLITE_NULL => frame(sha, b'N', &[]),
                sql::SQLITE_INTEGER => {
                    let mut text = [0; 20];
                    let mut rest = text.as_mut_slice();
                    write!(&mut rest, "{}", stmt.integer(column))
                        .map_err(|_| ())?;
                    let remaining = rest.len();
                    let length = text.len() - remaining;
                    frame(sha, b'I', &text[..length]);
                }
                sql::SQLITE_FLOAT => {
                    // The v1 format uses C's exact %.17g representation.
                    let mut text = [0u8; 64];
                    // SAFETY: text is writable and snprintf receives a double.
                    let length = unsafe {
                        snprintf(
                            text.as_mut_ptr().cast(),
                            text.len(),
                            c"%.17g".as_ptr(),
                            stmt.real(column),
                        )
                    };
                    let bytes = text
                        .get(..usize::try_from(length).map_err(|_| ())?)
                        .ok_or(())?;
                    frame(sha, b'F', bytes);
                }
                sql::SQLITE_BLOB => {
                    frame(sha, b'B', stmt.blob(column).ok_or(())?)
                }
                sql::SQLITE_TEXT => {
                    frame(sha, b'S', stmt.text(column).ok_or(())?)
                }
                _ => return Err(()),
            }
        }
        rows += 1;
    }
    frame(sha, b'C', &rows.to_be_bytes());
    Ok(())
}

unsafe fn exec(db: NonNull<sql::sqlite3>, query: &CStr) -> bool {
    // SAFETY: The caller borrows the live connection; SQLite needs no callbacks.
    unsafe {
        sql::sqlite3_exec(
            db.as_ptr(),
            query.as_ptr(),
            None,
            null_mut(),
            null_mut(),
        ) == sql::SQLITE_OK
    }
}

unsafe fn digest(
    store: *mut Store,
    project: *const c_char,
    out: *mut u8,
    domain: &[u8],
    queries: &[Query],
) -> c_int {
    if store.is_null() || project.is_null() || out.is_null() {
        return STORE_ERR;
    }
    // SAFETY: The C caller lends a live store and terminated project name,
    // and provides at least 32 writable bytes for a successful result.
    unsafe {
        let Some(db) = NonNull::new(cbm_store_get_db(store)) else {
            return STORE_ERR;
        };
        let project = CStr::from_ptr(project);
        let mut sha = Sha256::new();
        frame(&mut sha, b'D', domain);

        // Borrow a caller transaction or open our own stable read snapshot.
        let own = sql::sqlite3_get_autocommit(db.as_ptr()) != 0;
        if own && !exec(db, c"BEGIN") {
            return STORE_ERR;
        }
        let ok = queries
            .iter()
            .all(|query| table(db, query, project, &mut sha).is_ok());
        if own && !exec(db, if ok { c"COMMIT" } else { c"ROLLBACK" }) {
            return STORE_ERR;
        }
        if !ok {
            return STORE_ERR;
        }
        let hash = sha.finalize();
        copy_nonoverlapping(hash.as_ptr(), out, hash.len());
        STORE_OK
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_store_graph_content_digest(
    store: *mut Store,
    project: *const c_char,
    out: *mut u8,
) -> c_int {
    // SAFETY: The caller follows the store.h ABI.
    unsafe {
        digest(
            store,
            project,
            out,
            b"cbm.graph.content.v1",
            CONTENT_QUERIES,
        )
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_store_graph_topology_digest(
    store: *mut Store,
    project: *const c_char,
    out: *mut u8,
) -> c_int {
    // SAFETY: The caller follows the store.h ABI.
    unsafe {
        digest(
            store,
            project,
            out,
            b"cbm.graph.topology.v1",
            TOPOLOGY_QUERIES,
        )
    }
}
