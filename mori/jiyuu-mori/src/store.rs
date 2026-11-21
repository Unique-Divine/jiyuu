//! Read a published Mori database through a single SQLite snapshot.

use std::collections::BTreeMap;
use std::ops::ControlFlow;
use std::path::Path;

use rusqlite::types::{
    FromSql, FromSqlError, FromSqlResult, ToSqlOutput, ValueRef,
};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Row, params_from_iter,
};

pub(crate) type Cells = BTreeMap<String, Cell>;

pub(crate) enum Cell {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl Cell {
    fn read(row: &Row<'_>, index: usize) -> rusqlite::Result<Self> {
        Ok(match row.get_ref(index)? {
            ValueRef::Null => Self::Null,
            ValueRef::Integer(value) => Self::Integer(value),
            ValueRef::Real(value) => Self::Real(value),
            ValueRef::Text(value) => Self::Text(decode_text(value)),
            ValueRef::Blob(value) => Self::Blob(value.to_vec()),
        })
    }
}

// SQLite accepts invalid UTF-8 in TEXT. Preserve each invalid byte as Python's
// SQL reference exporter does, including docstrings truncated mid-character.
fn decode_text(mut bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut result = String::new();
    while let Err(error) = std::str::from_utf8(bytes) {
        let remaining = &bytes[error.valid_up_to()..];
        result.push_str(&String::from_utf8_lossy(&bytes[..error.valid_up_to()]));
        let invalid_len = error.error_len().unwrap_or(remaining.len());
        for byte in &remaining[..invalid_len] {
            let _ = write!(result, "\\x{byte:02x}");
        }
        bytes = &remaining[invalid_len..];
    }
    result.push_str(&String::from_utf8_lossy(bytes));
    result
}

// Text in the published graph can originate in filesystem paths and source
// bytes. Decode it for export while keeping SQL identifiers and CLI arguments
// on their existing paths.
struct GraphText(String);

impl FromSql for GraphText {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value {
            ValueRef::Text(bytes) => Ok(Self(decode_text(bytes))),
            _ => Err(FromSqlError::InvalidType),
        }
    }
}

pub(crate) struct Node {
    pub(crate) id: i64,
    pub(crate) qualified_name: String,
    pub(crate) label: String,
    pub(crate) file_path: String,
    pub(crate) properties: Cells,
}

pub(crate) struct Edge {
    pub(crate) source_id: i64,
    pub(crate) target_id: i64,
    pub(crate) kind: String,
    pub(crate) local_name: String,
    pub(crate) properties: Cells,
}

pub(crate) struct Column {
    pub(crate) name: String,
    pub(crate) pk: i64,
}

pub(crate) struct Store(Connection);

pub(crate) struct ProjectIdentity {
    pub(crate) name: Vec<u8>,
    pub(crate) root_path: Option<Vec<u8>>,
}

pub(crate) struct FileOutlineQuery<'a> {
    pub(crate) project: &'a [u8],
    pub(crate) path: &'a [u8],
    pub(crate) labels: &'a [&'a [u8]],
    pub(crate) limit: i32,
    pub(crate) offset: i32,
}

pub(crate) struct FileOutlineRow<'a> {
    pub(crate) name: &'a [u8],
    pub(crate) label: &'a [u8],
    pub(crate) qualified_name: &'a [u8],
    pub(crate) start_line: i32,
    pub(crate) end_line: i32,
}

#[derive(Clone, Copy)]
pub(crate) enum SchemaGroup {
    NodeLabel,
    EdgeType,
}

impl SchemaGroup {
    fn table_and_column(self) -> (&'static str, &'static str) {
        match self {
            Self::NodeLabel => ("nodes", "label"),
            Self::EdgeType => ("edges", "type"),
        }
    }
}

pub(crate) fn quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn c_string_bytes(bytes: &[u8]) -> &[u8] {
    bytes.split(|&byte| byte == 0).next().unwrap_or_default()
}

fn collect<T>(
    connection: &Connection,
    sql: &str,
    decode: impl FnMut(&Row<'_>) -> rusqlite::Result<T>,
) -> rusqlite::Result<Vec<T>> {
    connection.prepare(sql)?.query_map([], decode)?.collect()
}

fn properties(row: &Row<'_>, skip: &[&str]) -> rusqlite::Result<Cells> {
    row.as_ref()
        .column_names()
        .enumerate()
        .filter(|(_, name)| !skip.contains(name))
        .map(|(index, name)| Ok((name.to_owned(), Cell::read(row, index)?)))
        .collect()
}

impl Store {
    pub(crate) fn from_connection(connection: Connection) -> Self {
        Self(connection)
    }

    pub(crate) fn project_identity(
        &self,
        name_capacity: usize,
    ) -> rusqlite::Result<Option<ProjectIdentity>> {
        let mut statement = self.0.prepare(
            "SELECT CAST(name AS TEXT), indexed_at, root_path FROM projects ORDER BY name",
        )?;
        let mut rows = statement.query([])?;
        let mut primary = None;
        while let Some(row) = rows.next()? {
            let ValueRef::Text(name) = row.get_ref(0)? else {
                continue;
            };
            let name = c_string_bytes(name);
            if name.is_empty() || name.windows(2).any(|pair| pair == b"::") {
                continue;
            }
            if primary.is_some() {
                return Ok(None);
            }
            // Keep the legacy list_projects name buffer limit. The value
            // used for the root lookup and counts is the same display name.
            primary = Some(
                name[..name.len().min(name_capacity.saturating_sub(1))].to_vec(),
            );
        }
        let Some(name) = primary else { return Ok(None) };
        drop(rows);

        let root_path = self
            .0
            .query_row(
                "SELECT CAST(root_path AS TEXT) FROM projects WHERE name = ?1",
                [ToSqlOutput::Borrowed(ValueRef::Text(&name))],
                |row| {
                    Ok(match row.get_ref(0)? {
                        ValueRef::Text(path) => {
                            Some(c_string_bytes(path).to_vec())
                        }
                        _ => None,
                    })
                },
            )
            .optional();
        let root_path = match root_path {
            Ok(Some(value)) => value,
            _ => Some(Vec::new()),
        };
        Ok(Some(ProjectIdentity { name, root_path }))
    }

    pub(crate) fn project_stats(&self, project: &[u8]) -> (i32, i32) {
        let count = |sql| {
            self.0
                .query_row(
                    sql,
                    [ToSqlOutput::Borrowed(ValueRef::Text(project))],
                    |row| Ok(row.get::<_, i64>(0)? as i32),
                )
                .unwrap_or(-1)
        };
        (
            count("SELECT COUNT(*) FROM nodes WHERE project = ?1"),
            count("SELECT COUNT(*) FROM edges WHERE project = ?1"),
        )
    }

    pub(crate) fn file_outline(
        &self,
        query: FileOutlineQuery<'_>,
        mut emit: impl FnMut(FileOutlineRow<'_>) -> ControlFlow<i32>,
    ) -> rusqlite::Result<ControlFlow<i32, i32>> {
        let mut filter = String::from(
            " FROM nodes WHERE project = ? AND file_path = ?
              AND label NOT IN ('Module','Package','File','Folder','Project')",
        );
        if !query.labels.is_empty() {
            let labels = vec!["?"; query.labels.len()].join(",");
            filter.push_str(&format!(" AND label IN ({labels})"));
        }
        // The C handler passes normalized paths, which can contain bytes
        // outside UTF-8. Bind them as SQLite TEXT without transcoding.
        let mut binds: Vec<_> = [query.project, query.path]
            .into_iter()
            .chain(query.labels.iter().copied())
            .map(|bytes| ToSqlOutput::Borrowed(ValueRef::Text(bytes)))
            .collect();
        let total: i64 = self.0.query_row(
            &format!("SELECT COUNT(*){filter}"),
            params_from_iter(&binds),
            |row| row.get(0),
        )?;
        let sql = format!(
            "SELECT CAST(name AS TEXT), CAST(label AS TEXT), CAST(qualified_name AS TEXT),
                    COALESCE(CAST(start_line AS INTEGER), 0), COALESCE(CAST(end_line AS INTEGER), 0)
             {filter}
             ORDER BY start_line, end_line, label COLLATE BINARY,
                      name COLLATE BINARY, qualified_name COLLATE BINARY, id
             LIMIT ? OFFSET ?"
        );
        binds.push(ToSqlOutput::Borrowed(ValueRef::Integer(query.limit.into())));
        binds.push(ToSqlOutput::Borrowed(ValueRef::Integer(
            query.offset.into(),
        )));
        let mut statement = self.0.prepare(&sql)?;
        let mut rows = statement.query(params_from_iter(&binds))?;
        while let Some(row) = rows.next()? {
            let field = |index| -> rusqlite::Result<&[u8]> {
                let value = row.get_ref(index)?;
                Ok(match value {
                    ValueRef::Text(bytes) => bytes,
                    _ => &[],
                })
            };
            let result = emit(FileOutlineRow {
                name: field(0)?,
                label: field(1)?,
                qualified_name: field(2)?,
                start_line: row.get::<_, i64>(3)? as i32,
                end_line: row.get::<_, i64>(4)? as i32,
            });
            if let ControlFlow::Break(status) = result {
                return Ok(ControlFlow::Break(status));
            }
        }
        Ok(ControlFlow::Continue(total as i32))
    }

    pub(crate) fn schema_counts(
        &self,
        project: &str,
        group: SchemaGroup,
    ) -> rusqlite::Result<Vec<(String, i32)>> {
        let (table, column) = group.table_and_column();
        // Seek to each distinct value and count its index range. GROUP BY
        // scans every index entry before finding the next distinct value.
        let sql = format!(
            "WITH RECURSIVE v(x) AS (
                SELECT MIN({column}) FROM {table} WHERE project = ?1
                UNION ALL
                SELECT (SELECT MIN({column}) FROM {table} WHERE project = ?1 AND {column} > v.x)
                FROM v WHERE v.x IS NOT NULL
            )
            SELECT x, (SELECT COUNT(*) FROM {table} WHERE project = ?1 AND {column} = v.x) AS count
            FROM v WHERE x IS NOT NULL ORDER BY count DESC, x"
        );
        self.0
            .prepare(&sql)?
            .query_map([project], |row| {
                Ok((row.get(0)?, row.get::<_, i64>(1)? as i32))
            })?
            .collect()
    }

    pub(crate) fn schema_properties(
        &self,
        project: &str,
        group: SchemaGroup,
        name: &str,
    ) -> rusqlite::Result<Vec<String>> {
        let (table, column) = group.table_and_column();
        let base: &[&str] = match group {
            SchemaGroup::NodeLabel => &[
                "name",
                "qualified_name",
                "file_path",
                "start_line",
                "end_line",
            ],
            SchemaGroup::EdgeType => &["source_id", "target_id"],
        };
        let sql = format!(
            "SELECT CAST(key AS TEXT) FROM (
                SELECT DISTINCT property.key FROM {table}, json_each({table}.properties) AS property
                WHERE {table}.project = ?1 AND {table}.{column} = ?2 AND {table}.properties != '{{}}'
                ORDER BY property.key LIMIT 50
            )"
        );
        let mut properties =
            base.iter().map(|key| (*key).to_owned()).collect::<Vec<_>>();
        let mut query = self.0.prepare(&sql)?;
        let keys =
            query.query_map([project, name], |row| row.get::<_, String>(0))?;
        for key in keys {
            let key = key?;
            if matches!(group, SchemaGroup::NodeLabel)
                && matches!(key.as_str(), "fp" | "sp" | "bt")
            {
                continue;
            }
            properties.push(key);
        }
        Ok(properties)
    }

    pub(crate) fn open(path: &Path) -> rusqlite::Result<Self> {
        let connection =
            Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        connection.execute_batch("BEGIN")?;
        Ok(Self(connection))
    }

    pub(crate) fn tables(&self) -> rusqlite::Result<Vec<String>> {
        collect(
            &self.0,
            "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name",
            |row| row.get(0),
        )
    }

    pub(crate) fn schema(
        &self,
    ) -> rusqlite::Result<Vec<(String, String, String)>> {
        collect(
            &self.0,
            "SELECT type, name, coalesce(sql, '') FROM sqlite_master ORDER BY type, name",
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
    }

    pub(crate) fn identity(&self) -> rusqlite::Result<(String, String)> {
        if !self
            .columns("projects")?
            .iter()
            .any(|column| column.name == "root_path")
        {
            return Ok((String::new(), String::new()));
        }
        self.0.query_row(
            "SELECT name, root_path FROM projects ORDER BY length(name), name LIMIT 1",
            [],
            |row| Ok((row.get::<_, GraphText>(0)?.0, row.get::<_, GraphText>(1)?.0)),
        ).optional().map(Option::unwrap_or_default)
    }

    pub(crate) fn nodes(&self) -> rusqlite::Result<Vec<Node>> {
        collect(&self.0, "SELECT * FROM nodes", |row| {
            Ok(Node {
                id: row.get("id")?,
                qualified_name: row.get::<_, GraphText>("qualified_name")?.0,
                label: row.get::<_, GraphText>("label")?.0,
                file_path: row
                    .get::<_, Option<GraphText>>("file_path")?
                    .map(|value| value.0)
                    .unwrap_or_default(),
                properties: properties(
                    row,
                    &["id", "qualified_name", "label", "file_path", "project"],
                )?,
            })
        })
    }

    pub(crate) fn edges(&self) -> rusqlite::Result<Vec<Edge>> {
        let has_local_name = self
            .columns("edges")?
            .iter()
            .any(|column| column.name == "local_name_gen");
        collect(&self.0, "SELECT * FROM edges", |row| {
            let local_name = if has_local_name {
                row.get::<_, Option<GraphText>>("local_name_gen")?
                    .map(|value| value.0)
                    .unwrap_or_default()
            } else {
                String::new()
            };
            Ok(Edge {
                source_id: row.get("source_id")?,
                target_id: row.get("target_id")?,
                kind: row.get::<_, GraphText>("type")?.0,
                local_name,
                properties: properties(
                    row,
                    &[
                        "id",
                        "source_id",
                        "target_id",
                        "type",
                        "local_name_gen",
                        "project",
                    ],
                )?,
            })
        })
    }

    pub(crate) fn columns(&self, table: &str) -> rusqlite::Result<Vec<Column>> {
        collect(
            &self.0,
            &format!("PRAGMA table_xinfo({})", quote_identifier(table)),
            |row| {
                Ok(Column {
                    name: row.get("name")?,
                    pk: row.get("pk")?,
                })
            },
        )
    }

    pub(crate) fn count(&self, table: &str) -> rusqlite::Result<i64> {
        self.0.query_row(
            &format!("SELECT count(*) FROM {}", quote_identifier(table)),
            [],
            |row| row.get(0),
        )
    }

    pub(crate) fn rows(&self, table: &str) -> rusqlite::Result<Vec<Cells>> {
        let columns = self.columns(table)?;
        let quoted = columns
            .iter()
            .map(|column| quote_identifier(&column.name))
            .collect::<Vec<_>>()
            .join(", ");
        collect(
            &self.0,
            &format!("SELECT {quoted} FROM {}", quote_identifier(table)),
            |row| properties(row, &[]),
        )
    }

    pub(crate) fn fts_vocabulary(
        &self,
    ) -> rusqlite::Result<Vec<(String, i64, i64)>> {
        self.0.execute_batch("CREATE VIRTUAL TABLE temp.mori_export_vocab                               USING fts5vocab(main, nodes_fts, 'row')")?;
        collect(
            &self.0,
            "SELECT term, doc, cnt FROM temp.mori_export_vocab ORDER BY term",
            |row| Ok((row.get::<_, GraphText>(0)?.0, row.get(1)?, row.get(2)?)),
        )
    }
}
