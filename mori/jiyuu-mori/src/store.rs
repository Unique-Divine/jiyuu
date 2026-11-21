//! Read a published Mori database through a single SQLite snapshot.

use std::collections::BTreeMap;
use std::path::Path;

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row};

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

pub(crate) fn quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
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
        .into_iter()
        .enumerate()
        .filter(|(_, name)| !skip.contains(name))
        .map(|(index, name)| Ok((name.to_owned(), Cell::read(row, index)?)))
        .collect()
}

impl Store {
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
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional().map(Option::unwrap_or_default)
    }

    pub(crate) fn nodes(&self) -> rusqlite::Result<Vec<Node>> {
        collect(&self.0, "SELECT * FROM nodes", |row| {
            Ok(Node {
                id: row.get("id")?,
                qualified_name: row.get("qualified_name")?,
                label: row.get("label")?,
                file_path: row
                    .get::<_, Option<String>>("file_path")?
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
                row.get::<_, Option<String>>("local_name_gen")?
                    .unwrap_or_default()
            } else {
                String::new()
            };
            Ok(Edge {
                source_id: row.get("source_id")?,
                target_id: row.get("target_id")?,
                kind: row.get("type")?,
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
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
    }
}
