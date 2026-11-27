//! UNION and RETURN row semantics from the native Cypher evaluator.

#[cfg(test)]
use std::path::Path;
#[cfg(test)]
use std::time::Instant;

use super::{
    Binding, Connection, DistinctRows, ENGINE_LIMIT, ExecutionBudget,
    ExecutionError, HashMap, MatchQuery, NATIVE_IDENT, Node, ParseError, Parser,
    Projection, ProjectionExpression, ResultRows, Token, Tokens, Unsupported,
    c_string_bytes, native_token_id,
};

#[derive(Debug)]
pub(crate) struct Query<'a> {
    branches: Vec<MatchQuery<'a>>,
    distinct: bool,
}

pub(super) fn parse(source: &str) -> Result<Query<'_>, ParseError> {
    if source.contains('\0') {
        return Err(Unsupported.into());
    }
    let mut parser = Parser {
        tokens: Tokens::new(source)?,
        names: HashMap::new(),
        projected_fields: HashMap::new(),
        missing_projection: None,
        after_with: false,
        row_syntax: false,
    };
    let mut optional = parser.keyword("OPTIONAL");
    if !parser.keyword("MATCH") {
        return Err(Unsupported.into());
    }

    let mut branches = Vec::new();
    let mut distinct = false;
    let mut scope_error = None;
    loop {
        let (branch, error) = super::match_query(&mut parser, optional)?;
        scope_error = scope_error.or(error);
        branches.push(branch);
        if !parser.keyword("UNION") {
            break;
        }
        parser.row_syntax = true;
        let all = parser.keyword("ALL");
        if branches.len() == 1 {
            // The native evaluator uses the first operator for the chain.
            distinct = !all;
        }
        optional = parser.keyword("OPTIONAL");
        if !parser.keyword("MATCH") {
            // These clauses have their own leading syntax or diagnostic in C.
            const NATIVE_ONLY: &[&str] = &[
                "OPTIONAL",
                "UNWIND",
                "CREATE",
                "DELETE",
                "DETACH",
                "SET",
                "REMOVE",
                "MERGE",
                "YIELD",
                "CALL",
                "FOREACH",
                "MANDATORY",
                "DROP",
                "CONSTRAINT",
            ];
            if matches!(parser.tokens.peek(), Some(Token::Word(word))
                if NATIVE_ONLY.iter().any(|keyword| word.eq_ignore_ascii_case(keyword)))
            {
                return Err(Unsupported.into());
            }
            return Err(parser.expected_pattern_token(0));
        }
        parser.names.clear();
        parser.projected_fields.clear();
        parser.missing_projection = None;
        parser.after_with = false;
    }
    if parser.tokens.peek().is_some() {
        let error = if parser.row_syntax {
            trailing_input(&mut parser, source)
        } else {
            parser.second_with().or_else(|| parser.trailing_delimiter())
        };
        return Err(error.unwrap_or(ParseError::Unsupported));
    }
    if let Some(error) = scope_error {
        return Err(error);
    }
    Ok(Query { branches, distinct })
}

fn trailing_input(parser: &mut Parser<'_>, source: &str) -> Option<ParseError> {
    let mut offset = parser.tokens.offset();
    let word = match parser.tokens.next()? {
        Token::Word(word) | Token::Number(word) => word.to_owned(),
        Token::Symbol(symbol) => char::from(symbol).to_string(),
        Token::Literal(bytes) => {
            offset += 1;
            String::from_utf8_lossy(c_string_bytes(&bytes)).into_owned()
        }
        Token::DotDot
        | Token::Regex
        | Token::LessOrEqual
        | Token::GreaterOrEqual
        | Token::NotEqual => source[offset..offset + 2].to_owned(),
    };
    let mut with_hint = word.eq_ignore_ascii_case("WITH");
    let mut starts = word.eq_ignore_ascii_case("STARTS");
    while !with_hint {
        let Some(token) = parser.tokens.next() else {
            break;
        };
        with_hint = matches!(token, Token::Word(word) if word.eq_ignore_ascii_case("WITH"))
            && !starts;
        starts = matches!(token, Token::Word(word) if word.eq_ignore_ascii_case("STARTS"));
    }
    Some(ParseError::TrailingInput {
        offset,
        word,
        with_hint,
    })
}

impl Query<'_> {
    #[cfg(test)]
    pub(super) fn first_branch(&self) -> &MatchQuery<'_> {
        &self.branches[0]
    }

    #[cfg(test)]
    pub(crate) fn execute(
        &self,
        db: &Path,
        project: &str,
    ) -> Result<ResultRows, ExecutionError> {
        let connection = Connection::open_with_flags(
            db,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        self.execute_on(&connection, project)
    }

    #[cfg(test)]
    pub(crate) fn execute_on(
        &self,
        connection: &Connection,
        project: &str,
    ) -> Result<ResultRows, ExecutionError> {
        self.execute_with_budget(
            connection,
            project,
            &mut ExecutionBudget::new(&Instant::now, &|| false),
        )
    }

    /// Read every branch from the caller's snapshot under one query budget.
    pub(crate) fn execute_with_budget(
        &self,
        connection: &Connection,
        project: &str,
        budget: &mut ExecutionBudget<'_>,
    ) -> Result<ResultRows, ExecutionError> {
        let standalone = self.branches.len() == 1;
        let mut branches = self.branches.iter();
        let mut result = branches
            .next()
            .unwrap()
            .execute(connection, project, standalone, budget)?;
        if !standalone {
            let mut seen = DistinctRows {
                cap: ENGINE_LIMIT,
                ..DistinctRows::default()
            };
            if self.distinct {
                for row in std::mem::take(&mut result.rows) {
                    budget.step()?;
                    seen.admit(row, &mut result);
                }
            }
            let width = result.columns.len();
            for branch in branches {
                let branch =
                    branch.execute(connection, project, false, budget)?;
                result.truncated |= branch.truncated;
                result.warnings.merge(branch.warnings);
                // Native copies later rows positionally at the first width.
                // A narrower nonempty branch has no defined native result.
                if branch.columns.len() < width && !branch.rows.is_empty() {
                    return Err(ExecutionError::Unsupported);
                }
                for mut row in branch.rows {
                    budget.step()?;
                    if result.rows.len() == ENGINE_LIMIT {
                        break;
                    }
                    row.truncate(width);
                    if self.distinct {
                        seen.admit(row, &mut result);
                    } else {
                        result.rows.push(row);
                    }
                }
            }
            budget.check()?;
        }
        if result.rows.len() >= ENGINE_LIMIT {
            return Err(ExecutionError::RowCeiling);
        }
        Ok(result)
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum StarField {
    Name,
    QualifiedName,
    Label,
    FilePath,
}

impl StarField {
    pub(super) fn value(self, node: &Node) -> &[u8] {
        match self {
            Self::Name => &node.name,
            Self::QualifiedName => &node.qualified_name,
            Self::Label => &node.label,
            Self::FilePath => &node.file_path,
        }
    }
}

pub(super) fn star_projections<'a>(
    names: &HashMap<&'a str, Binding>,
) -> Vec<Projection<'a>> {
    let mut declared: Vec<_> = names.iter().collect();
    declared.sort_unstable_by_key(|(_, binding)| match binding {
        Binding::Node(index) => (0, *index),
        Binding::Edge(index) => (1, *index),
        Binding::Unbound => unreachable!("only declared bindings have names"),
    });
    declared
        .into_iter()
        .flat_map(|(name, &binding)| {
            [
                ("name", StarField::Name),
                ("qualified_name", StarField::QualifiedName),
                ("label", StarField::Label),
                ("file_path", StarField::FilePath),
            ]
            .into_iter()
            .map(move |(property, field)| {
                let expression = match binding {
                    Binding::Node(index) => {
                        ProjectionExpression::StarField(index, field)
                    }
                    Binding::Edge(_) if matches!(field, StarField::Name) => {
                        ProjectionExpression::Field(binding, Some("type"))
                    }
                    _ => ProjectionExpression::Constant(b""),
                };
                let name = format!("{name}.{property}");
                Projection {
                    expression,
                    source_name: name.clone(),
                    name,
                    scope_name: None,
                }
            })
        })
        .collect()
}

impl<'a> Parser<'a> {
    pub(super) fn collect_projection(
        &mut self,
    ) -> Result<Projection<'a>, ParseError> {
        self.row_syntax = true;
        // Native tolerates missing parentheses, but requires an argument.
        self.consume(b'(');
        self.keyword("DISTINCT"); // Native applies inner DISTINCT only to COUNT.
        let (name, binding, property) = if self.consume(b'*') {
            ("*", Binding::Unbound, None)
        } else {
            let name = self.pattern_name()?;
            let binding = self.projection_binding(name);
            let property = if self.consume(b'.')
                && native_token_id(self.tokens.peek()) == NATIVE_IDENT
            {
                Some(self.name()?)
            } else {
                None
            };
            (name, binding, property)
        };
        self.consume(b')');
        Ok(Projection {
            expression: ProjectionExpression::Collect(binding, property),
            name: format!("COLLECT({name})"),
            source_name: super::projection_name(name, property),
            scope_name: None,
        })
    }
}

#[derive(Default)]
pub(super) struct Collected {
    count: usize,
    prefix: Vec<u8>,
}

impl Collected {
    // C formats a quoted list, then copies it into a 64-byte output buffer.
    // Keep only the bytes that can survive that copy, even for large groups.
    const MAX_BYTES: usize = 63;

    pub(super) fn push(&mut self, value: &[u8]) {
        let delimiter: &[u8] = if self.count == 0 { b"[" } else { b"," };
        self.count += 1;
        let available = Self::MAX_BYTES - self.prefix.len();
        self.prefix.extend(
            delimiter
                .iter()
                .chain(b"\"")
                .chain(c_string_bytes(value))
                .chain(b"\"")
                .take(available)
                .copied(),
        );
    }

    pub(super) fn bytes(mut self, with: bool) -> Vec<u8> {
        // WITH formats COLLECT through its generic aggregate count path.
        if with {
            return self.count.to_string().into_bytes();
        }
        if self.count == 0 {
            return b"[]".to_vec();
        }
        if self.prefix.len() < Self::MAX_BYTES {
            self.prefix.push(b']');
        }
        self.prefix
    }
}

#[cfg(test)]
mod tests {
    use super::{Connection, ExecutionBudget, ExecutionError, Instant, parse};
    use std::cell::Cell;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    const NODES: &str = "CREATE TABLE nodes (
        id INTEGER PRIMARY KEY, project TEXT, label TEXT, name TEXT,
        qualified_name TEXT, file_path TEXT, start_line INTEGER, end_line INTEGER,
        properties TEXT
    );";

    #[test]
    fn union_checks_the_ceiling_after_duplicate_elimination() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(NODES).unwrap();
        db.execute_batch(
            "WITH RECURSIVE seq(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM seq WHERE n<100001)
             INSERT INTO nodes(id, project, label, name) SELECT n, 'test', 'Probe', 'same' FROM seq;"
        ).unwrap();
        let result = parse(
            "MATCH (a:Probe) RETURN a.name UNION MATCH (b:Probe) RETURN b.name",
        )
        .unwrap()
        .execute_on(&db, "test")
        .unwrap();
        assert_eq!(result.rows, vec![vec![b"same".to_vec()]]);
        assert!(result.truncated);
        let query = parse(
            "MATCH (a:Probe) RETURN a.name LIMIT 50000 UNION ALL MATCH (b:Probe) RETURN b.name LIMIT 50000",
        ).unwrap();
        assert!(matches!(
            query.execute_on(&db, "test"),
            Err(ExecutionError::RowCeiling)
        ));
    }

    #[test]
    fn union_reads_every_branch_from_the_callers_snapshot() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "mori-cypher-snapshot-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        let db = path.join("fixture.db");
        let writer = Connection::open(&db).unwrap();
        writer.execute_batch("PRAGMA journal_mode = WAL;").unwrap();
        writer.execute_batch(NODES).unwrap();
        writer.execute_batch("INSERT INTO nodes (project, label, name) VALUES ('test', 'Probe', 'old');").unwrap();
        let mut reader = Connection::open(&db).unwrap();
        let snapshot = reader.transaction().unwrap();
        let checks = Cell::new(0);
        let update = || {
            checks.set(checks.get() + 1);
            // Commit between the first and second branch scans.
            if checks.get() == 4 {
                writer
                    .execute_batch("UPDATE nodes SET name = 'new';")
                    .unwrap();
            }
            false
        };
        let mut budget = ExecutionBudget::new(&Instant::now, &update);
        let query = parse(
            "MATCH (a:Probe) RETURN a.name UNION ALL MATCH (b:Probe) RETURN b.name",
        ).unwrap();
        let result = query
            .execute_with_budget(&snapshot, "test", &mut budget)
            .unwrap();
        assert_eq!(result.rows, vec![vec![b"old".to_vec()]; 2]);
        snapshot.commit().unwrap();
        let result = query.execute_on(&reader, "test").unwrap();
        assert_eq!(result.rows, vec![vec![b"new".to_vec()]; 2]);
        drop(reader);
        drop(writer);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn row_syntax_reports_native_diagnostics_in_parse_order() {
        // Pinned C parses the entire UNION before it checks any branch's scope.
        for (query, expected) in [
            (
                "MATCH (a:Caller) RETURN a.name UNION",
                "expected token type 0, got 88 at pos 36",
            ),
            (
                "MATCH (a:Caller) RETURN a.name UNION ALL RETURN a",
                "expected token type 0, got 2 at pos 41",
            ),
            (
                "MATCH (a:Caller) RETURN a.name UNION MATCH (b:Target) RETURN a.name",
                "variable 'a' is not in scope for RETURN — no pattern in this query names it",
            ),
            (
                "MATCH (a:Caller) RETURN absent.name UNION MATCH (b:Target) RETURN collect()",
                "expected token type 85, got 67 at pos 74",
            ),
            (
                "MATCH (a:Caller) RETURN absent.name UNION MATCH (b:Target) RETURN * AS bad",
                "unexpected input at pos 68 ('AS') — the query was not fully parsed.",
            ),
            (
                "MATCH (a:Caller) RETURN COLLECT(a.name) ORDER BY COLLECT(a.name)",
                "unexpected input at pos 58 ('.') — the query was not fully parsed.",
            ),
            (
                "MATCH (a:Caller) RETURN * LIMIT",
                "expected token type 87, got 88 at pos 31",
            ),
        ] {
            assert_eq!(
                parse(query).unwrap_err().diagnostic().as_deref(),
                Some(expected),
                "{query}",
            );
        }
    }
}
