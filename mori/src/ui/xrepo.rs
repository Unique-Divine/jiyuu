//! Read-only cross-repo views for Repo Galaxy.

use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString, OsStr, c_char, c_int, c_void};
use std::fs;
use std::path::Path;
use std::ptr::NonNull;

use rusqlite::{Connection, Row, Transaction, ffi, params};
use serde::Serialize;

use crate::ui_http_transport::Reply;

const MAX_PROJECTS: usize = 512;
const MAX_ENTRIES: usize = 20_000;
const MAX_PROJECT_NAME: usize = 255;
const MAX_EDGE_TYPE: usize = 63;
const DEFAULT_PAGE: i32 = 100;
const MAX_PAGE: i32 = 1000;

const TYPE_RANGE: &str = "e.type >= 'CROSS_' AND e.type < 'CROSS`'";
const TARGET: &str =
    "coalesce(json_extract(e.properties, '$.target_project'), '')";
// Providers carry a callers object. Older provider copies can still be
// recognized by their HANDLES edge. For channels, count the emitter's copy.
const CALLER_SIDE: &str = "
    (CASE WHEN e.type = 'CROSS_CHANNEL' THEN EXISTS (
        SELECT 1 FROM edges x
        WHERE x.source_id = e.source_id AND x.target_id = e.target_id AND x.type = 'EMITS'
    ) ELSE json_type(e.properties, '$.callers') IS NULL AND NOT EXISTS (
        SELECT 1 FROM edges h
        WHERE h.source_id = e.source_id AND h.target_id = e.target_id AND h.type = 'HANDLES'
    ) END)";

unsafe extern "C" {
    fn cbm_validate_project_name(name: *const c_char) -> bool;
    fn cbm_is_project_index_db(name: *const c_char) -> bool;
    fn cbm_store_open_path_query(path: *const c_char) -> *mut c_void;
    fn cbm_store_get_db(store: *mut c_void) -> *mut ffi::sqlite3;
    fn cbm_store_close(store: *mut c_void);
}

struct QueryStore(NonNull<c_void>);

impl Drop for QueryStore {
    fn drop(&mut self) {
        // SAFETY: this guard owns the store and outlives the borrowed connection.
        unsafe { cbm_store_close(self.0.as_ptr()) };
    }
}

fn snapshot<T>(
    path: &Path,
    read: impl FnOnce(&Transaction<'_>) -> rusqlite::Result<T>,
) -> rusqlite::Result<T> {
    let path = CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| rusqlite::Error::InvalidPath(path.to_owned()))?;
    // SAFETY: the path is NUL terminated. The native store retains ownership
    // and uses the same WAL first-access lock and reader settings as other UI reads.
    let store = QueryStore(
        NonNull::new(unsafe { cbm_store_open_path_query(path.as_ptr()) })
            .ok_or(rusqlite::Error::InvalidQuery)?,
    );
    // SAFETY: this store is private to this request and lives until after
    // the transaction and the non-owning Connection have dropped.
    let db = unsafe { cbm_store_get_db(store.0.as_ptr()) };
    let mut connection = unsafe { Connection::from_handle(db) }?;
    let transaction = connection.transaction()?;
    let result = read(&transaction)?;
    transaction.commit()?;
    Ok(result)
}

fn valid_project(name: &CStr) -> bool {
    // SAFETY: the shared validator reads a NUL-terminated name.
    unsafe { cbm_validate_project_name(name.as_ptr()) }
}

fn is_index(filename: &CStr) -> bool {
    // SAFETY: the shared predicate reads a NUL-terminated filename.
    unsafe { cbm_is_project_index_db(filename.as_ptr()) }
}

#[derive(Default, Serialize)]
struct Repo {
    name: String,
    indexed: bool,
    nodes: i64,
    edges: i64,
    links_out: i64,
    links_in: i64,
}

#[derive(Serialize)]
struct Arc {
    from: String,
    to: String,
    r#type: String,
    count: i64,
}

#[derive(Serialize)]
struct Summary {
    repos: Vec<Repo>,
    links: Vec<Arc>,
    types: Vec<String>,
    total_links: i64,
    skipped: Vec<String>,
    truncated: bool,
    scope: &'static str,
}

fn repo<'a>(
    repos: &'a mut Vec<Repo>,
    indices: &mut HashMap<String, usize>,
    name: &str,
) -> &'a mut Repo {
    let index = *indices.entry(name.to_owned()).or_insert_with(|| {
        let index = repos.len();
        repos.push(Repo {
            name: name.to_owned(),
            ..Repo::default()
        });
        index
    });
    &mut repos[index]
}

fn project_files(cache: &Path) -> std::io::Result<(Vec<String>, bool)> {
    let mut projects = Vec::new();
    let mut truncated = false;
    for (index, entry) in fs::read_dir(cache)?.enumerate() {
        if index == MAX_ENTRIES {
            truncated = true;
            break;
        }
        let Ok(filename) = entry?.file_name().into_string() else {
            continue;
        };
        let Some(name) = filename.strip_suffix(".db") else {
            continue;
        };
        if name.len() <= MAX_PROJECT_NAME
            && is_index(
                &CString::new(filename.as_bytes())
                    .expect("file name has no NUL"),
            )
            && valid_project(&CString::new(name).expect("file name has no NUL"))
        {
            projects.push(name.to_owned());
        }
    }
    projects.sort_unstable();
    truncated |= projects.len() > MAX_PROJECTS;
    projects.truncate(MAX_PROJECTS);
    Ok((projects, truncated))
}

fn summary(cache: &Path) -> Result<Summary, HttpError> {
    let (projects, truncated) = project_files(cache).map_err(|_| CACHE_ERROR)?;
    let mut repos = Vec::new();
    let mut indices = HashMap::new();
    let mut links: Vec<Arc> = Vec::new();
    let mut skipped = Vec::new();
    let group_sql = format!(
        "SELECT e.type, CAST({TARGET} AS TEXT), count(*) FROM edges e
         WHERE e.project = ?1 AND {TYPE_RANGE} AND {CALLER_SIDE} GROUP BY e.type, {TARGET}"
    );
    for project in projects {
        let result = snapshot(&cache.join(format!("{project}.db")), |db| {
            // activity.db and other SQLite stores can share the cache.
            let has_tables = db.query_row(
                "SELECT count(*) = 3 FROM sqlite_master WHERE type = 'table'
                 AND name IN ('projects', 'nodes', 'edges')",
                [],
                |row| row.get::<_, bool>(0),
            )?;
            if !has_tables {
                return Ok(());
            }
            let (has_project, nodes, edges) = db.query_row(
                "SELECT EXISTS (SELECT 1 FROM projects WHERE name = ?1),
                        (SELECT count(*) FROM nodes WHERE project = ?1),
                        (SELECT count(*) FROM edges WHERE project = ?1)",
                [&project],
                |row| Ok((row.get::<_, bool>(0)?, row.get(1)?, row.get(2)?)),
            )?;
            if !has_project {
                return Ok(());
            }
            let indexed = repo(&mut repos, &mut indices, &project);
            indexed.indexed = true;
            indexed.nodes = nodes;
            indexed.edges = edges;

            let mut stmt = db.prepare(&group_sql)?;
            let mut rows = stmt.query([&project])?;
            while let Some(row) = rows.next()? {
                let target = text(row, 1)?;
                if target == project
                    || !CString::new(target.as_bytes())
                        .is_ok_and(|target| valid_project(&target))
                {
                    continue;
                }
                let edge_type = text(row, 0)?;
                links.push(Arc {
                    from: project.clone(),
                    to: target.chars().take(MAX_PROJECT_NAME).collect(),
                    r#type: edge_type.chars().take(MAX_EDGE_TYPE).collect(),
                    count: row.get(2)?,
                });
            }
            Ok(())
        });
        if result.is_err() {
            skipped.push(project);
        }
    }
    // Preserve project order first, followed by targets without their own index.
    for link in &links {
        repo(&mut repos, &mut indices, &link.from).links_out += link.count;
        repo(&mut repos, &mut indices, &link.to).links_in += link.count;
    }
    links.sort_by(|a, b| {
        b.count.cmp(&a.count).then_with(|| {
            (&a.from, &a.to, &a.r#type).cmp(&(&b.from, &b.to, &b.r#type))
        })
    });
    let mut seen = HashSet::new();
    let types = links
        .iter()
        .filter_map(|link| {
            seen.insert(link.r#type.as_str())
                .then_some(link.r#type.clone())
        })
        .collect();
    Ok(Summary {
        repos,
        total_links: links.iter().map(|link| link.count).sum(),
        links,
        types,
        skipped,
        truncated,
        scope: "caller-side CROSS_* edges stored by the cross-repo pass",
    })
}

#[derive(Serialize)]
struct Page<'a> {
    from: &'a str,
    to: &'a str,
    r#type: &'a str,
    limit: i32,
    offset: i32,
    total: i64,
    links: Vec<Link>,
}

#[derive(Serialize)]
struct Link {
    r#type: String,
    to: String,
    via: String,
    method: String,
    target_function: String,
    target_file: String,
    source: Source,
    local_target: Symbol,
}

#[derive(Serialize)]
struct Symbol {
    name: String,
    qualified_name: String,
    label: String,
}

#[derive(Serialize)]
struct Source {
    #[serde(flatten)]
    symbol: Symbol,
    file_path: String,
    line: i64,
}

fn text(row: &Row<'_>, column: usize) -> rusqlite::Result<String> {
    let value = row.get_ref(column)?;
    if value == rusqlite::types::ValueRef::Null {
        return Ok(String::new());
    }
    let bytes = value
        .as_bytes()
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    // The former SQLite text reader passed a C string to the JSON writer.
    Ok(String::from_utf8_lossy(
        bytes.split(|&byte| byte == 0).next().unwrap_or_default(),
    )
    .into_owned())
}

fn symbol(row: &Row<'_>, column: usize) -> rusqlite::Result<Symbol> {
    Ok(Symbol {
        name: text(row, column)?,
        qualified_name: text(row, column + 1)?,
        label: text(row, column + 2)?,
    })
}

fn link(row: &Row<'_>) -> rusqlite::Result<Link> {
    Ok(Link {
        r#type: text(row, 0)?,
        to: text(row, 1)?,
        via: text(row, 2)?,
        method: text(row, 3)?,
        target_function: text(row, 4)?,
        target_file: text(row, 5)?,
        source: Source {
            symbol: symbol(row, 6)?,
            file_path: text(row, 9)?,
            line: row.get::<_, Option<i64>>(10)?.unwrap_or_default(),
        },
        local_target: symbol(row, 11)?,
    })
}

fn page<'a>(
    cache: &Path,
    from: &'a str,
    to: Option<&'a str>,
    edge_type: Option<&'a str>,
    limit: i32,
    offset: i32,
) -> Result<Page<'a>, HttpError> {
    let filename =
        CString::new(format!("{from}.db")).expect("validated project");
    let path = cache.join(filename.to_str().expect("validated project"));
    if !is_index(&filename) || !path.exists() {
        return Err(HttpError(404, "project not found"));
    }
    let filter = format!(
        "WHERE e.project = ?1 AND {TYPE_RANGE} AND {CALLER_SIDE}
         AND (?2 IS NULL OR {TARGET} = ?2) AND (?3 IS NULL OR e.type = ?3)"
    );
    snapshot(&path, |db| {
        let total = db.query_row(
            &format!("SELECT count(*) FROM edges e {filter}"),
            params![from, to, edge_type],
            |row| row.get(0),
        )?;
        let mut stmt = db.prepare(&format!(
            "SELECT e.type, CAST({TARGET} AS TEXT),
                CAST(coalesce(json_extract(e.properties, '$.operation'),
                     json_extract(e.properties, '$.procedure'), json_extract(e.properties, '$.url_path'),
                     json_extract(e.properties, '$.channel_name'), json_extract(e.properties, '$.service'), '') AS TEXT),
                CAST(json_extract(e.properties, '$.method') AS TEXT),
                CAST(json_extract(e.properties, '$.target_function') AS TEXT),
                CAST(json_extract(e.properties, '$.target_file') AS TEXT),
                s.name, s.qualified_name, s.label, s.file_path, s.start_line,
                t.name, t.qualified_name, t.label
             FROM edges e JOIN nodes s ON s.id = e.source_id JOIN nodes t ON t.id = e.target_id
             {filter} ORDER BY e.type, {TARGET}, s.file_path, s.start_line, e.id LIMIT ?4 OFFSET ?5"
        ))?;
        let links = stmt.query_map(params![from, to, edge_type, limit, offset], link)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Page {
            from,
            to: to.unwrap_or_default(),
            r#type: edge_type.unwrap_or_default(),
            limit,
            offset,
            total,
            links,
        })
    }).map_err(|_| LINKS_ERROR)
}

struct HttpError(c_int, &'static str);

const CACHE_ERROR: HttpError = HttpError(500, "cannot read the project cache");
const LINKS_ERROR: HttpError = HttpError(500, "cannot read links");
const INVALID_FILTER: HttpError = HttpError(400, "invalid from, to or type");

fn query_values<'a>(
    query: &'a [u8],
    key: &'a [u8],
) -> impl Iterator<Item = &'a [u8]> {
    query.split(|&byte| byte == b'&').filter_map(move |pair| {
        let rest = pair.strip_prefix(key)?;
        if rest.is_empty() {
            return Some(b"".as_slice());
        }
        rest.strip_prefix(b"=")
    })
}

fn hex(byte: u8) -> Option<u8> {
    (byte as char).to_digit(16).map(|value| value as u8)
}

fn decode(mut value: &[u8], max: usize) -> Option<CString> {
    let mut decoded = Vec::new();
    while let [byte, rest @ ..] = value {
        if decoded.len() == max {
            return None;
        }
        let (decoded_byte, tail) = match (byte, rest) {
            (b'%', [high, low, tail @ ..]) => {
                (hex(*high)? * 16 + hex(*low)?, tail)
            }
            (b'%', _) => return None,
            (b'+', _) => (b' ', rest),
            _ => (*byte, rest),
        };
        decoded.push(decoded_byte);
        value = tail;
    }
    if decoded.is_empty() {
        return None;
    }
    CString::new(decoded).ok()
}

fn filter_value(
    query: &[u8],
    key: &[u8],
    max: usize,
) -> Result<Option<CString>, HttpError> {
    let mut values = query_values(query, key);
    let first = values.next().unwrap_or_default();
    let decoded = decode(first, max);
    if decoded.is_none()
        && (!first.is_empty() || values.any(|value| !value.is_empty()))
    {
        return Err(HttpError(400, "invalid to or type"));
    }
    Ok(decoded)
}

fn query_number(query: &[u8], key: &[u8]) -> i32 {
    query_values(query, key)
        .next()
        .and_then(|value| decode(value, 31))
        // SAFETY: the decoder returns a NUL-terminated value. Use the same
        // atoi conversion as the other UI routes, including decimal prefixes.
        .map_or(0, |value| unsafe { libc::atoi(value.as_ptr()) })
}

fn links_json(cache: &Path, query: &[u8]) -> Result<Vec<u8>, HttpError> {
    let from = query_values(query, b"from")
        .next()
        .and_then(|value| decode(value, MAX_PROJECT_NAME))
        .ok_or(HttpError(400, "missing from parameter"))?;
    let to = filter_value(query, b"to", MAX_PROJECT_NAME)?;
    let edge_type = filter_value(query, b"type", MAX_EDGE_TYPE)?;
    if !valid_project(&from)
        || to.as_ref().is_some_and(|to| !valid_project(to))
        || edge_type.as_ref().is_some_and(|edge_type| {
            !edge_type
                .to_bytes()
                .strip_prefix(b"CROSS_")
                .is_some_and(|suffix| {
                    !suffix.is_empty()
                        && suffix.iter().all(|&byte| {
                            byte.is_ascii_uppercase() || byte == b'_'
                        })
                })
        })
    {
        return Err(INVALID_FILTER);
    }
    let limit = query_number(query, b"limit");
    let limit = if limit <= 0 {
        DEFAULT_PAGE
    } else {
        limit.min(MAX_PAGE)
    };
    let offset = query_number(query, b"offset").max(0);
    let result = page(
        cache,
        from.to_str().expect("validated project"),
        to.as_deref()
            .map(|to| to.to_str().expect("validated project")),
        edge_type
            .as_deref()
            .map(|edge_type| edge_type.to_str().expect("validated type")),
        limit,
        offset,
    )?;
    serde_json::to_vec(&result).map_err(|_| LINKS_ERROR)
}

#[cfg(unix)]
fn cache_path(cache: &CStr) -> Option<&Path> {
    use std::os::unix::ffi::OsStrExt;
    Some(Path::new(OsStr::from_bytes(cache.to_bytes())))
}

#[cfg(not(unix))]
fn cache_path(cache: &CStr) -> Option<&Path> {
    cache.to_str().ok().map(|path| Path::new(OsStr::new(path)))
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_ui_xrepo_reply(
    cache: *const c_char,
    query: *const c_char,
    reply: Reply,
    context: *mut c_void,
    headers: *const c_char,
    deadline_ms: c_int,
) {
    // SAFETY: the dispatcher supplies the cache and, for the links route,
    // the query as live NUL-terminated strings through this call.
    let cache = cache_path(unsafe { CStr::from_ptr(cache) });
    let result = if query.is_null() {
        cache
            .ok_or(CACHE_ERROR)
            .and_then(summary)
            .and_then(|summary| {
                serde_json::to_vec(&summary).map_err(|_| CACHE_ERROR)
            })
    } else {
        let query = unsafe { CStr::from_ptr(query) };
        cache
            .ok_or(LINKS_ERROR)
            .and_then(|cache| links_json(cache, query.to_bytes()))
    };
    let (status, body) = match result {
        Ok(json) => (200, json),
        Err(HttpError(status, message)) => {
            (status, format!(r#"{{"error":"{message}"}}"#).into_bytes())
        }
    };
    // SAFETY: the transport consumes the response synchronously. The
    // dispatcher owns the headers and reply context until this returns.
    unsafe {
        reply(
            context,
            status,
            headers,
            body.as_ptr().cast(),
            body.len(),
            deadline_ms,
        )
    };
}
