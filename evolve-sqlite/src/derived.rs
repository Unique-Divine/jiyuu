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
