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
            if (name == MIGRATION_HISTORY_TABLE || name == crate::derived::TABLE)
                && kind == "table"
            {
                let ddl = if name == MIGRATION_HISTORY_TABLE {
                    MIGRATION_HISTORY_DDL
                } else {
                    crate::derived::DDL
                };
                if sql.as_deref() != Some(ddl) {
                    return Err(Error::Drift(format!(
                        "unexpected schema for {name}"
                    )));
                }
                continue;
            }
            if crate::is_engine_table(&table) && kind == "index" && sql.is_none()
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
pub(crate) fn tokens(sql: &str) -> Result<Vec<String>, Error> {
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
        } else if byte.is_ascii_digit()
            || (byte == b'.'
                && bytes.get(at + 1).is_some_and(u8::is_ascii_digit))
        {
            at = numeric_end(bytes, at);
            result.push(sql[start..at].to_ascii_lowercase());
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

/// Follow the numeric token boundaries in SQLite 3.54.1 sqlite3GetToken,
/// cases CC_DOT and CC_DIGIT. Underscore placement and numeric validity remain
/// SQLite parser checks; no whitespace/comment can be absorbed into a token.
/// https://www.sqlite.org/lang_expr.html#literal_values_constants_
fn numeric_end(bytes: &[u8], start: usize) -> usize {
    let mut at = start;
    if bytes.get(start) == Some(&b'0')
        && matches!(bytes.get(start + 1), Some(b'x' | b'X'))
        && bytes.get(start + 2).is_some_and(u8::is_ascii_hexdigit)
    {
        at += 3;
        while bytes
            .get(at)
            .is_some_and(|byte| byte.is_ascii_hexdigit() || *byte == b'_')
        {
            at += 1;
        }
    } else {
        while bytes
            .get(at)
            .is_some_and(|byte| byte.is_ascii_digit() || *byte == b'_')
        {
            at += 1;
        }
        if bytes.get(at) == Some(&b'.') {
            at += 1;
            while bytes
                .get(at)
                .is_some_and(|byte| byte.is_ascii_digit() || *byte == b'_')
            {
                at += 1;
            }
        }
        if matches!(bytes.get(at), Some(b'e' | b'E')) {
            let digits = at
                + 1
                + usize::from(matches!(bytes.get(at + 1), Some(b'+' | b'-')));
            if bytes.get(digits).is_some_and(u8::is_ascii_digit) {
                at = digits + 1;
                while bytes
                    .get(at)
                    .is_some_and(|byte| byte.is_ascii_digit() || *byte == b'_')
                {
                    at += 1;
                }
            }
        }
    }
    // SQLite consumes trailing identifier bytes into an illegal numeric token.
    // Valid schema SQL has already passed SQLite's parser, but retain those
    // boundaries conservatively if a corrupted stored definition contains one.
    while bytes.get(at).is_some_and(|byte| {
        byte.is_ascii_alphanumeric()
            || matches!(*byte, b'_' | b'$')
            || *byte >= 128
    }) {
        at += 1;
    }
    at
}

#[cfg(test)]
mod tests {
    use super::tokens;
    use crate::CheckError;
    type TestResult = Result<(), CheckError>;

    /// Every supported numeric form is one complete token. Separating an
    /// exponent or decimal component must not collapse to the same tokens.
    #[test]
    fn numeric_literal_tokens_preserve_adjacency() -> TestResult {
        for literal in [
            ".5",
            "1.",
            "1.e2",
            "1e+2",
            "1.e-2",
            "0xAb",
            "1_000",
            "0xDE_AD",
            "1_2.3_4e+5_6",
        ] {
            assert_eq!(
                tokens(literal)?,
                [literal.to_ascii_lowercase()],
                "{literal}"
            );
        }
        for (contiguous, separated) in [
            ("1.e2", "1. e2"),
            ("1.e2", "1./**/e2"),
            (".5e2", ".5 e2"),
            (".5e2", ".5/**/e2"),
            ("1e+2", "1 e + 2"),
            ("1.e-2", "1. e -2"),
            ("0xAB", "0 xAB"),
            ("1_000", "1 _000"),
            ("0xDE_AD", "0xDE _AD"),
        ] {
            assert_ne!(
                tokens(contiguous)?,
                tokens(separated)?,
                "{contiguous} vs {separated}"
            );
        }
        Ok(())
    }

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
