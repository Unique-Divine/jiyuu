//! Compare application schema with a trusted, checked-in DDL reference.

use std::collections::BTreeMap;

use rusqlite::Connection;

use crate::{Error, MIGRATION_HISTORY_DDL, MIGRATION_HISTORY_TABLE};

/// Expected application schema produced by a checked-in SQL schema file.
///
/// Construct this from trusted DDL maintained with the application. Do not
/// derive it from the database being checked, which would hide that database's
/// drift. Foreign-key and integrity checks remain separate from this comparison.
/// Table contents, business defaults, and migration history rows are not schema.
#[derive(Clone, Debug)]
pub struct Schema {
    objects: BTreeMap<(String, String), Object>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Object {
    table: String,
    sql: Option<Vec<String>>,
}

impl Schema {
    /// Execute trusted reference DDL in an isolated in-memory database and
    /// capture the schema SQLite stores. No application file is opened.
    ///
    /// Include all expected tables, indexes, views, triggers, and virtual-table
    /// declarations. SQLite generates FTS shadow objects from those declarations;
    /// verification compares the shadow schema too. For adopted databases,
    /// explicitly append the validated legacy history's expected DDL to the
    /// reference. No blanket ignored-table list is supported.
    pub fn from_sql(expected_ddl: &str) -> Result<Self, Error> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(expected_ddl)?;
        Self::read(&conn)
    }

    /// Reject missing, extra, or changed schema objects without writing SQL.
    ///
    /// Use this in project callbacks on fresh, upgraded, rehearsed, and live
    /// databases. Compare application rows and original legacy history in
    /// separate checks. The engine's own history table is verified independently
    /// against its immutable DDL and excluded from the application inventory.
    ///
    /// SQL comparison ignores comments, whitespace between tokens, and ASCII
    /// case in unquoted words. It preserves token boundaries and exact bytes in
    /// string literals and quoted identifiers. This is conservative comparison,
    /// not a proof that differently written SQL has equivalent behavior.
    pub fn verify(&self, conn: &Connection) -> Result<(), Error> {
        let actual = Self::read(conn)?;
        for (key, expected) in &self.objects {
            match actual.objects.get(key) {
                None => {
                    return Err(Error::Drift(format!(
                        "missing {} {}",
                        key.0, key.1
                    )));
                }
                Some(object) if object != expected => {
                    return Err(Error::Drift(format!(
                        "changed schema for {} {}",
                        key.0, key.1
                    )));
                }
                Some(_) => {}
            }
        }
        if let Some(key) = actual
            .objects
            .keys()
            .find(|key| !self.objects.contains_key(*key))
        {
            return Err(Error::Drift(format!("unexpected {} {}", key.0, key.1)));
        }
        Ok(())
    }

    fn read(conn: &Connection) -> Result<Self, Error> {
        let mut statement = conn.prepare("SELECT type, name, tbl_name, sql FROM main.sqlite_schema ORDER BY type, name")?;
        let mut rows = statement.query([])?;
        let mut objects = BTreeMap::new();
        while let Some(row) = rows.next()? {
            let kind: String = row.get(0)?;
            let name: String = row.get(1)?;
            let table: String = row.get(2)?;
            let sql: Option<String> = row.get(3)?;
            if name == MIGRATION_HISTORY_TABLE && kind == "table" {
                if sql.as_deref() != Some(MIGRATION_HISTORY_DDL) {
                    return Err(Error::Drift(format!(
                        "unexpected schema for {MIGRATION_HISTORY_TABLE}"
                    )));
                }
                continue;
            }
            if table == MIGRATION_HISTORY_TABLE
                && kind == "index"
                && sql.is_none()
            {
                continue;
            }
            objects.insert(
                (kind, name),
                Object {
                    table,
                    sql: sql.as_deref().map(tokens).transpose()?,
                },
            );
        }
        Ok(Self { objects })
    }
}

/// SQLite's stored schema is already parsed SQL. Token comparison avoids
/// accidentally erasing meaningful spaces or case inside literals and names.
fn tokens(sql: &str) -> Result<Vec<String>, Error> {
    let bytes = sql.as_bytes();
    let mut result = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let start = at;
        let byte = bytes[at];
        if byte.is_ascii_whitespace() {
            at += 1;
        } else if bytes[at..].starts_with(b"--") {
            at += 2;
            while at < bytes.len() && bytes[at] != b'\n' {
                at += 1;
            }
        } else if bytes[at..].starts_with(b"/*") {
            at += 2;
            while at + 1 < bytes.len() && &bytes[at..at + 2] != b"*/" {
                at += 1;
            }
            if at + 1 >= bytes.len() {
                return Err(Error::InvalidSpec(
                    "unterminated schema comment".into(),
                ));
            }
            at += 2;
        } else if matches!(byte, b'x' | b'X')
            && bytes.get(at + 1) == Some(&b'\'')
        {
            // A contiguous X'...' is one blob literal. X '...' instead names
            // an identifier followed by a separate string, which SQLite may
            // accept in a view without resolving its columns yet.
            at += 2;
            while at < bytes.len() && bytes[at] != b'\'' {
                at += 1;
            }
            if at >= bytes.len() {
                return Err(Error::InvalidSpec(
                    "unterminated schema blob".into(),
                ));
            }
            at += 1;
            result.push(format!("x{}", &sql[start + 1..at]));
        } else if matches!(byte, b'\'' | b'"' | b'`' | b'[') {
            let close = if byte == b'[' { b']' } else { byte };
            at += 1;
            loop {
                if at >= bytes.len() {
                    return Err(Error::InvalidSpec(
                        "unterminated schema quote".into(),
                    ));
                }
                if bytes[at] == close {
                    at += 1;
                    if byte != b'[' && at < bytes.len() && bytes[at] == close {
                        at += 1;
                        continue;
                    }
                    break;
                }
                at += 1;
            }
            result.push(sql[start..at].to_owned());
        } else if byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 128 {
            at += 1;
            while at < bytes.len()
                && (bytes[at].is_ascii_alphanumeric()
                    || bytes[at] == b'_'
                    || bytes[at] == b'$'
                    || bytes[at] >= 128)
            {
                at += 1;
            }
            result.push(sql[start..at].to_ascii_lowercase());
        } else {
            at += 1;
            for operator in
                ["->>", "->", ">=", "<=", "!=", "==", "<>", "||", "<<", ">>"]
            {
                if sql[start..].starts_with(operator) {
                    at = start + operator.len();
                    break;
                }
            }
            result.push(sql[start..at].to_owned());
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::tokens;
    use crate::CheckError;
    type TestResult = Result<(), CheckError>;

    /// Formatting normalization must not merge tokens or change quoted values.
    #[test]
    fn normalization_preserves_literals_names_and_boundaries() -> TestResult {
        assert_eq!(
            tokens("CHECK (value >= 1) -- comment\n")?,
            tokens("check/* comment */(value>=1)")?
        );
        for (left, right) in [
            ("'A B'", "'AB'"),
            ("'A'", "'a'"),
            ("'it''s'", "'its'"),
            ("\"A B\"", "\"AB\""),
            ("`Name`", "`name`"),
            ("[Name]", "[name]"),
            ("a b", "ab"),
            ("a >= b", "a > = b"),
            ("'--'", "''"),
            ("'/* x */'", "'x'"),
            ("a ->> b", "a -> > b"),
            ("X'4142'", "X '4142'"),
            ("X'4142'", "X/**/'4142'"),
        ] {
            assert_ne!(tokens(left)?, tokens(right)?, "{left} vs {right}");
        }
        assert_eq!(tokens("X'4142'")?, tokens("x'4142'")?);
        Ok(())
    }
}
