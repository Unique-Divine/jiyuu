//! Checksum-tracked views and FTS maintenance triggers, separate from frozen SQL.

use std::collections::BTreeSet;

use rusqlite::Connection;
use sha2::{Digest, Sha256};

use crate::{Error, SchemaObject};

pub(crate) const TABLE: &str = "evolve_sqlite_derived";
pub(crate) const DDL: &str = "CREATE TABLE evolve_sqlite_derived (
                    name TEXT PRIMARY KEY NOT NULL,
                    sha256 TEXT NOT NULL,
                    applied_at INTEGER NOT NULL
                ) STRICT";

/// A replaceable definition for application views or FTS maintenance triggers.
///
/// SQL runs after versioned migrations when its checksum changes. Use guarded
/// DROP/CREATE statements so the definition can be reapplied. SQL cannot change
/// table rows or table structure. Business seeds belong in explicit create-once
/// or versioned application operations, not in these definitions.
#[derive(Clone, Copy, Debug)]
pub struct DerivedSql<'a> {
    /// Stable, unique ASCII identifier. Keep the declaration when removing an
    /// object; change its SQL to an explicit DROP rather than forgetting history.
    pub name: &'a str,
    /// Trusted view/trigger DDL. Exact bytes contribute to the checksum.
    pub sql: &'a str,
    /// Existing FTS5 tables to rebuild after this definition changes. Changing
    /// this list also changes the checksum. FTS table creation stays versioned.
    pub fts_rebuilds: &'a [&'a str],
}

/// A changed or not-yet-recorded derived definition shown by a reviewed plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedDerived {
    pub name: String,
    pub sha256: String,
    pub fts_rebuilds: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Definition {
    pub planned: PlannedDerived,
    pub sql: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct History {
    name: String,
    sha256: String,
    applied_at: i64,
}

pub(crate) fn identifier(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().enumerate().all(|(at, byte)| {
            byte == b'_'
                || byte.is_ascii_alphabetic()
                || (at > 0 && byte.is_ascii_digit())
        })
        && !name.to_ascii_lowercase().starts_with("sqlite_")
        && !name.to_ascii_lowercase().starts_with("evolve_sqlite_")
}

pub(crate) fn validate(
    definitions: &[DerivedSql<'_>],
) -> Result<Vec<Definition>, Error> {
    let mut seen = BTreeSet::new();
    definitions
        .iter()
        .map(|definition| {
            if !identifier(definition.name)
                || !seen.insert(definition.name.to_ascii_lowercase())
                || definition.sql.contains('\0')
            {
                return Err(Error::InvalidSpec(format!(
                    "invalid or duplicate derived name {:?}",
                    definition.name
                )));
            }
            let mut targets = BTreeSet::new();
            for target in definition.fts_rebuilds {
                if !identifier(target)
                    || !targets.insert(target.to_ascii_lowercase())
                {
                    return Err(Error::InvalidSpec(format!(
                        "invalid or duplicate FTS target {target:?}"
                    )));
                }
            }
            let mut checksum = Sha256::new();
            checksum.update(b"evolve-derived-v1\0");
            checksum.update((definition.sql.len() as u64).to_be_bytes());
            checksum.update(definition.sql.as_bytes());
            for target in definition.fts_rebuilds {
                checksum.update((target.len() as u64).to_be_bytes());
                checksum.update(target.as_bytes());
            }
            Ok(Definition {
                planned: PlannedDerived {
                    name: definition.name.into(),
                    sha256: format!("{:x}", checksum.finalize()),
                    fts_rebuilds: definition
                        .fts_rebuilds
                        .iter()
                        .map(|target| (*target).into())
                        .collect(),
                },
                sql: definition.sql.into(),
            })
        })
        .collect()
}

pub(crate) fn read_history(
    conn: &Connection,
    schema: &[SchemaObject],
    definitions: &[Definition],
) -> Result<Vec<History>, Error> {
    let Some(table) = schema
        .iter()
        .find(|object| object.name.eq_ignore_ascii_case(TABLE))
    else {
        return Ok(Vec::new());
    };
    if table.kind != "table" || table.sql.as_deref() != Some(DDL) {
        return Err(Error::Drift(format!("unexpected schema for {TABLE}")));
    }
    if schema.iter().any(|object| {
        object.kind == "trigger" && object.table.eq_ignore_ascii_case(TABLE)
    }) {
        return Err(Error::Drift(format!("unexpected trigger on {TABLE}")));
    }
    let mut statement = conn.prepare("SELECT name, sha256, applied_at FROM evolve_sqlite_derived ORDER BY name")?;
    let history = statement
        .query_map([], |row| {
            Ok(History {
                name: row.get(0)?,
                sha256: row.get(1)?,
                applied_at: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for entry in &history {
        if !definitions
            .iter()
            .any(|definition| definition.planned.name == entry.name)
        {
            return Err(Error::Ahead(format!(
                "derived definition {}",
                entry.name
            )));
        }
        if entry.applied_at < 0
            || entry.sha256.len() != 64
            || !entry.sha256.bytes().all(|byte| {
                byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
            })
        {
            return Err(Error::Drift(format!(
                "invalid derived history for {}",
                entry.name
            )));
        }
    }
    Ok(history)
}

pub(crate) fn pending(definition: &Definition, history: &[History]) -> bool {
    !history.iter().any(|entry| {
        entry.name == definition.planned.name
            && entry.sha256 == definition.planned.sha256
    })
}

/// A declaration must name an actual FTS5 virtual table, not an ordinary table
/// with an identically named column that could receive business data writes.
pub(crate) fn validate_fts(conn: &Connection, table: &str) -> Result<(), Error> {
    let sql: Option<String> = conn.query_row("SELECT sql FROM sqlite_schema WHERE type='table' AND name=?1 COLLATE NOCASE", [table], |row| row.get(0)).optional()?;
    let tokens = sql
        .as_deref()
        .map(crate::schema::tokens)
        .transpose()?
        .unwrap_or_default();
    if tokens.first().map(String::as_str) != Some("create")
        || tokens.get(1).map(String::as_str) != Some("virtual")
        || tokens.get(2).map(String::as_str) != Some("table")
        || tokens.get(4).map(String::as_str) != Some("using")
        || tokens.get(5).map(String::as_str) != Some("fts5")
    {
        return Err(Error::InvalidSpec(format!(
            "FTS rebuild target {table:?} is not an FTS5 virtual table"
        )));
    }
    Ok(())
}

use rusqlite::OptionalExtension;

/// Actual table classes observed after versioned DDL. Names alone do not prove
/// shadow ownership: external-content and columnsize=0 omit some shadow tables.
#[derive(Clone, Default)]
pub(crate) struct TableInventory {
    types: std::collections::BTreeMap<String, String>,
    pub fts_writes: BTreeSet<String>,
}

impl TableInventory {
    pub fn read(conn: &Connection) -> Result<Self, Error> {
        let mut statement = conn.prepare(
            "SELECT name, type FROM pragma_table_list WHERE schema='main'",
        )?;
        let types = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?.to_ascii_lowercase(), row.get(1)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Self {
            types,
            fts_writes: BTreeSet::new(),
        })
    }

    pub fn ordinary_source(&self, name: &str) -> bool {
        !crate::is_engine_table(name)
            && self
                .types
                .get(&name.to_ascii_lowercase())
                .is_some_and(|kind| kind == "table")
            && !name.to_ascii_lowercase().starts_with("sqlite_")
    }

    pub fn for_fts(
        &self,
        conn: &Connection,
        target: &str,
    ) -> Result<Self, Error> {
        validate_fts(conn, target)?;
        let sql: String = conn.query_row(
            "SELECT sql FROM sqlite_schema WHERE type='table' AND name=?1 COLLATE NOCASE",
            [target], |row| row.get(0),
        )?;
        let mut constructor = Connection::open_in_memory()?;
        let authorizer = crate::SqlAuthorizer::install(
            &mut constructor,
            &crate::RebuildPolicy::default(),
        )?;
        {
            let _restricted =
                authorizer.restrict_to(crate::SqlPermissionMode::Migration);
            // Connection::execute rejects additional statements in rusqlite.
            // Run only the validated constructor, under permissions that deny
            // attachments, transaction control, pragma writes, and extension loading.
            // No source rows or triggers are copied into this private database.
            constructor.execute(&sql, [])?;
        }
        let created = Self::read(&constructor)?;
        let mut inventory = self.clone();
        inventory.fts_writes.clear();
        let target = target.to_ascii_lowercase();
        if self.types.get(&target).map(String::as_str) != Some("virtual") {
            return Err(Error::InvalidSpec(format!(
                "FTS target {target:?} is not virtual"
            )));
        }
        inventory.fts_writes.insert(target.clone());
        for suffix in ["data", "idx", "config", "docsize", "content"] {
            let name = format!("{target}_{suffix}");
            // table_list itself uses module suffix recognition and can label
            // an ordinary lookalike as shadow even when FTS never creates it.
            // Require that this exact constructor actually creates the table.
            if self.types.get(&name).map(String::as_str) == Some("shadow")
                && created.types.get(&name).map(String::as_str) == Some("shadow")
            {
                inventory.fts_writes.insert(name);
            }
        }
        Ok(inventory)
    }

    /// Applications maintain FTS through triggers on ordinary source tables.
    /// Shadow/virtual-table triggers are unsupported and must not execute as
    /// side effects of the engine's rebuild command.
    pub fn reject_index_triggers(&self, conn: &Connection) -> Result<(), Error> {
        let mut statement = conn.prepare(
            "SELECT name, tbl_name FROM sqlite_schema WHERE type='trigger'",
        )?;
        let triggers = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for trigger in triggers {
            let (name, table) = trigger?;
            if self.fts_writes.contains(&table.to_ascii_lowercase()) {
                return Err(Error::InvalidSpec(format!(
                    "unsupported trigger {name:?} on FTS index table {table:?}"
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::TableInventory;
    use crate::CheckError;
    use rusqlite::Connection;
    type TestResult = Result<(), CheckError>;

    /// External-content FTS omits its content shadow, and columnsize=0 omits
    /// docsize. Ordinary tables with those suffixes are not index-owned rows.
    #[test]
    fn actual_shadow_classes_exclude_ordinary_same_suffix_tables() -> TestResult
    {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("CREATE TABLE note(id INTEGER PRIMARY KEY, body TEXT);
            CREATE VIRTUAL TABLE ft USING fts5(body,content='note',content_rowid='id',columnsize=0);
            CREATE TABLE ft_content(value TEXT);
            CREATE TABLE ft_docsize(value TEXT);
            CREATE VIRTUAL TABLE ft_other USING fts5(body);")?;
        let tables = TableInventory::read(&conn)?;
        let owned = tables.for_fts(&conn, "ft")?;
        assert!(owned.fts_writes.contains("ft"));
        assert!(owned.fts_writes.contains("ft_data"));
        assert!(!owned.fts_writes.contains("ft_content"));
        assert!(!owned.fts_writes.contains("ft_docsize"));
        assert!(!owned.fts_writes.contains("ft_other_data"));
        assert!(tables.ordinary_source("note"));
        // SQLite labels this ordinary lookalike shadow. Trigger-source
        // acceptance remains conservative; rebuild writes still exclude it.
        assert!(!tables.ordinary_source("ft_content"));
        assert!(!tables.ordinary_source("ft_data"));
        assert!(!tables.ordinary_source("ft"));
        Ok(())
    }
}
