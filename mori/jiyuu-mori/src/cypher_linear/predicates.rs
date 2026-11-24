use std::borrow::Cow;
use std::ffi::{CString, c_char, c_double, c_long};

use super::{
    Binding, Edge, ExpressionContext, Node, PROPERTY_LIMIT, ParseError, Parser,
    Token, Unsupported, c_string_bytes,
};

const MAX_DEPTH: usize = 256;

#[derive(Debug)]
pub(super) enum Expr<'a> {
    Condition(Condition<'a>),
    Not(Box<Self>),
    Chain(Logic, Vec<Self>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Logic {
    Or,
    Xor,
    And,
}

#[derive(Debug)]
pub(super) struct Condition<'a> {
    lhs: Subject<'a>,
    test: Test<'a>,
}

#[derive(Debug)]
enum Subject<'a> {
    Field(Field<'a>),
    Coalesce(Coalesce<'a>),
    Substring(Substring<'a>),
}

#[derive(Clone, Copy, Debug)]
struct Field<'a> {
    name: &'a str,
    binding: Binding,
    property: Option<&'a str>,
}

#[derive(Debug)]
pub(super) struct Coalesce<'a>(Vec<Operand<'a>>);

impl<'a> Coalesce<'a> {
    pub(super) fn first_reference(&self) -> Option<(&'a str, Option<&'a str>)> {
        self.first_field().map(|field| (field.name, field.property))
    }

    fn first_field(&self) -> Option<Field<'a>> {
        match self.0.first()? {
            Operand::Field(field) => Some(*field),
            Operand::Literal(_) => None,
        }
    }

    pub(super) fn value<'b>(
        &'b self,
        mut read: impl FnMut(Binding, Option<&str>) -> Option<Cow<'b, [u8]>>,
    ) -> Option<Cow<'b, [u8]>> {
        let mut first = Cow::Borrowed(&b""[..]);
        for arg in &self.0 {
            let value = match arg {
                Operand::Literal(bytes) => Cow::Borrowed(bytes.as_ref()),
                Operand::Field(field) if field.binding == Binding::Unbound => {
                    continue;
                }
                Operand::Field(field) => read(field.binding, field.property)?,
            };
            // Wait for every alias, even when an earlier argument has a value.
            if first.is_empty() {
                first = value;
            }
        }
        Some(first)
    }
}

#[derive(Debug)]
pub(super) struct Substring<'a>(Vec<Operand<'a>>);

impl<'a> Substring<'a> {
    pub(super) fn first_reference(&self) -> Option<(&'a str, Option<&'a str>)> {
        self.first_field().map(|field| (field.name, field.property))
    }

    fn first_field(&self) -> Option<Field<'a>> {
        match self.0.first()? {
            Operand::Field(field) => Some(*field),
            Operand::Literal(_) => None,
        }
    }

    pub(super) fn value<'b>(
        &'b self,
        mut read: impl FnMut(Binding, Option<&str>) -> Option<Cow<'b, [u8]>>,
    ) -> Option<Vec<u8>> {
        let mut args = [
            Cow::Borrowed(&b""[..]),
            Cow::Borrowed(&b""[..]),
            Cow::Borrowed(&b""[..]),
        ];
        for (index, arg) in self.0.iter().enumerate() {
            let value = match arg {
                Operand::Literal(bytes) => Cow::Borrowed(bytes.as_ref()),
                Operand::Field(field) if field.binding == Binding::Unbound => {
                    Cow::Borrowed(&b""[..])
                }
                Operand::Field(field) => read(field.binding, field.property)?,
            };
            // Native uses at most three arguments. Predicates still wait for
            // every referenced alias before evaluating at the seed.
            if let Some(slot) = args.get_mut(index) {
                *slot = value;
            }
        }
        if self.0.len() < 2 {
            return Some(Vec::new());
        }
        let text = c_string_bytes(&args[0]);
        let start = usize::try_from(native_long(&args[1])).ok();
        let Some(tail) = start
            .filter(|&start| start < text.len())
            .map(|start| &text[start..])
        else {
            return Some(Vec::new());
        };
        let take = if self.0.len() >= 3 {
            native_long(&args[2]).try_into().unwrap_or(0)
        } else {
            tail.len()
        };
        Some(tail[..tail.len().min(take).min(PROPERTY_LIMIT)].to_vec())
    }
}

fn native_long(bytes: &[u8]) -> c_long {
    unsafe extern "C" {
        fn strtol(
            input: *const c_char,
            end: *mut *mut c_char,
            base: std::ffi::c_int,
        ) -> c_long;
    }
    let bytes =
        CString::new(c_string_bytes(bytes)).expect("NUL-trimmed integer");
    // SAFETY: The input is NUL-terminated and we do not request an end pointer.
    unsafe { strtol(bytes.as_ptr(), std::ptr::null_mut(), 10) }
}

#[derive(Debug)]
enum Test<'a> {
    Label(&'a str),
    Null { negated: bool },
    In(Vec<Cow<'a, [u8]>>),
    Compare(Operator, Operand<'a>),
}

#[derive(Debug)]
enum Operand<'a> {
    Literal(Cow<'a, [u8]>),
    Field(Field<'a>),
}

#[derive(Clone, Copy, Debug)]
enum Operator {
    Equal,
    NotEqual,
    Regex,
    Contains,
    StartsWith,
    EndsWith,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
}

#[derive(Clone, Copy, Default)]
pub(super) struct Bindings<'a> {
    pub(super) nodes: [Option<&'a Node>; super::paths::NODE_SLOTS],
    pub(super) edges: [Option<&'a Edge>; super::paths::EDGE_SLOTS],
    pub(super) complete_path: bool,
    pub(super) projected: Option<&'a [Node]>,
}

impl<'a> Bindings<'a> {
    fn node(&self, index: usize) -> Option<&'a Node> {
        if let Some(projected) = self.projected {
            projected.get(index)
        } else {
            self.nodes[index]
        }
    }
}

impl<'a> Parser<'a> {
    pub(super) fn where_expr(&mut self) -> Result<Expr<'a>, ParseError> {
        self.logic_expr(0, Logic::Or)
    }

    fn logic_expr(
        &mut self,
        depth: usize,
        logic: Logic,
    ) -> Result<Expr<'a>, ParseError> {
        let (keyword, next) = match logic {
            Logic::Or => ("OR", Some(Logic::Xor)),
            Logic::Xor => ("XOR", Some(Logic::And)),
            Logic::And => ("AND", None),
        };
        let operand = |parser: &mut Self| match next {
            Some(next) => parser.logic_expr(depth, next),
            None => parser.where_atom(depth),
        };
        let first = operand(self)?;
        if !self.keyword(keyword) {
            return Ok(first);
        }
        let mut operands = vec![first];
        loop {
            operands.push(operand(self)?);
            if !self.keyword(keyword) {
                return Ok(Expr::Chain(logic, operands));
            }
        }
    }

    fn where_atom(&mut self, depth: usize) -> Result<Expr<'a>, ParseError> {
        if self.keyword("NOT") {
            if depth == MAX_DEPTH {
                return Err(ParseError::Depth);
            }
            return Ok(Expr::Not(Box::new(self.where_atom(depth + 1)?)));
        }
        if self.consume(b'(') {
            if depth == MAX_DEPTH {
                return Err(ParseError::Depth);
            }
            let expr = self.logic_expr(depth + 1, Logic::Or)?;
            self.symbol(b')')?;
            return Ok(expr);
        }
        Ok(Expr::Condition(self.condition()?))
    }

    fn condition(&mut self) -> Result<Condition<'a>, ParseError> {
        let name = self.name()?;
        let lhs = if self.consume(b'(') {
            if name.eq_ignore_ascii_case("coalesce") {
                Subject::Coalesce(self.coalesce()?)
            } else if name.eq_ignore_ascii_case("substring") {
                Subject::Substring(self.substring(false)?)
            } else if ["replace", "left", "right"]
                .iter()
                .any(|function| name.eq_ignore_ascii_case(function))
            {
                return Err(Unsupported.into());
            } else {
                return Err(ParseError::UnsupportedWhereFunction(
                    name.to_owned(),
                ));
            }
        } else {
            Subject::Field(self.field(name)?)
        };
        let test = if matches!(lhs, Subject::Field(Field { property: None, .. }))
            && self.consume(b':')
        {
            Test::Label(self.name()?)
        } else if self.keyword("IS") {
            let negated = self.keyword("NOT");
            if !self.keyword("NULL") {
                return Err(Unsupported.into());
            }
            Test::Null { negated }
        } else if self.keyword("IN") {
            self.symbol(b'[')?;
            let mut values = Vec::new();
            while !self.consume(b']') {
                // Native accepts adjacent values and a trailing comma.
                if !values.is_empty() && self.consume(b',') && self.consume(b']')
                {
                    break;
                }
                values.push(self.where_literal()?);
            }
            Test::In(values)
        } else {
            let op = self.where_operator()?;
            let value = if self.keyword("TRUE") {
                Operand::Literal(Cow::Borrowed(b"true"))
            } else if self.keyword("FALSE") {
                Operand::Literal(Cow::Borrowed(b"false"))
            } else if matches!(self.tokens.peek(), Some(Token::Word(_))) {
                let name = self.name()?;
                Operand::Field(self.field(name)?)
            } else {
                Operand::Literal(self.where_literal()?)
            };
            Test::Compare(op, value)
        };
        Ok(Condition { lhs, test })
    }

    fn field(&mut self, name: &'a str) -> Result<Field<'a>, Unsupported> {
        let binding = self.binding(name).or_else(|err| {
            self.after_with.then_some(Binding::Unbound).ok_or(err)
        })?;
        let property = self.consume(b'.').then(|| self.name()).transpose()?;
        Ok(Field {
            name,
            binding,
            property,
        })
    }

    pub(super) fn coalesce(&mut self) -> Result<Coalesce<'a>, Unsupported> {
        self.scalar_arguments(|parser, name, _| parser.field(name))
            .map(Coalesce)
    }

    pub(super) fn substring(
        &mut self,
        projection: bool,
    ) -> Result<Substring<'a>, Unsupported> {
        self.scalar_arguments(|parser, name, first| {
            let mut field = if projection {
                // Native scope validation checks only the first argument.
                let binding = if first {
                    parser.projection_binding(name)
                } else {
                    parser.binding(name).unwrap_or(Binding::Unbound)
                };
                let property =
                    parser.consume(b'.').then(|| parser.name()).transpose()?;
                Field {
                    name,
                    binding,
                    property,
                }
            } else {
                parser.field(name)?
            };
            // The native scalar reader tries the full virtual name first,
            // including an unaliased WITH field such as n.pos.
            if let Some(prop) = field.property
                && !parser.projected_fields.is_empty()
                && let Some(&index) =
                    parser.projected_fields.get(&format!("{name}.{prop}"))
            {
                field.binding = Binding::Node(index);
                field.property = None;
            }
            Ok(field)
        })
        .map(Substring)
    }

    fn scalar_arguments(
        &mut self,
        mut field: impl FnMut(
            &mut Self,
            &'a str,
            bool,
        ) -> Result<Field<'a>, Unsupported>,
    ) -> Result<Vec<Operand<'a>>, Unsupported> {
        let mut args = Vec::new();
        if !self.consume(b')') {
            loop {
                let arg = if matches!(self.tokens.peek(), Some(Token::Word(_))) {
                    let name = self.name()?;
                    Operand::Field(field(self, name, args.is_empty())?)
                } else {
                    Operand::Literal(self.where_literal()?)
                };
                args.push(arg);
                if !self.consume(b',') {
                    self.symbol(b')')?;
                    break;
                }
            }
        }
        Ok(args)
    }

    fn where_literal(&mut self) -> Result<Cow<'a, [u8]>, Unsupported> {
        match self.tokens.next() {
            Some(Token::Literal(bytes)) => Ok(bytes),
            Some(Token::Number(number)) => Ok(Cow::Borrowed(number.as_bytes())),
            _ => Err(Unsupported),
        }
    }

    fn where_operator(&mut self) -> Result<Operator, ParseError> {
        let offset = self.tokens.offset();
        Ok(match self.tokens.next() {
            Some(Token::Symbol(b'=')) => Operator::Equal,
            Some(Token::Regex) => Operator::Regex,
            Some(Token::NotEqual) => Operator::NotEqual,
            Some(Token::Symbol(b'<')) => Operator::Less,
            Some(Token::LessOrEqual) => Operator::LessOrEqual,
            Some(Token::Symbol(b'>')) => Operator::Greater,
            Some(Token::GreaterOrEqual) => Operator::GreaterOrEqual,
            Some(Token::Word(word)) if word.eq_ignore_ascii_case("CONTAINS") => {
                Operator::Contains
            }
            Some(Token::Word(word)) if word.eq_ignore_ascii_case("STARTS") => {
                if !self.keyword("WITH") {
                    return Err(Unsupported.into());
                }
                Operator::StartsWith
            }
            Some(Token::Word(word)) if word.eq_ignore_ascii_case("ENDS") => {
                if !self.keyword("WITH") {
                    return Err(Unsupported.into());
                }
                Operator::EndsWith
            }
            Some(Token::Word(_)) | None => {
                return Err(ParseError::UnexpectedOperator(offset));
            }
            _ => return Err(Unsupported.into()),
        })
    }
}

impl Expr<'_> {
    pub(super) fn has_regex(&self) -> bool {
        match self {
            Self::Condition(condition) => {
                matches!(condition.test, Test::Compare(Operator::Regex, _))
            }
            Self::Not(expr) => expr.has_regex(),
            Self::Chain(_, operands) => operands.iter().any(Self::has_regex),
        }
    }

    /// A predicate has no value until all aliases it reads are bound.
    pub(super) fn evaluate(
        &self,
        bindings: &Bindings<'_>,
        context: &ExpressionContext<'_>,
    ) -> Option<bool> {
        match self {
            Self::Condition(condition) => condition.evaluate(bindings, context),
            Self::Not(expr) => {
                expr.evaluate(bindings, context).map(|value| !value)
            }
            Self::Chain(logic, operands) => {
                let mut parity = false;
                let mut pending = false;
                for operand in operands {
                    match operand.evaluate(bindings, context) {
                        Some(true) if *logic == Logic::Or => return Some(true),
                        Some(false) if *logic == Logic::And => {
                            return Some(false);
                        }
                        Some(value) => parity ^= value,
                        None => pending = true,
                    }
                }
                (!pending).then_some(match logic {
                    Logic::Or => false,
                    Logic::Xor => parity,
                    Logic::And => true,
                })
            }
        }
    }

    pub(super) fn selectivity(&self, index: usize) -> u8 {
        match self {
            Self::Condition(condition) => condition.selectivity(index),
            Self::Chain(Logic::And, operands) => operands
                .iter()
                .map(|expr| expr.selectivity(index))
                .max()
                .unwrap_or(0),
            _ => 0,
        }
    }
}

impl Condition<'_> {
    fn selectivity(&self, index: usize) -> u8 {
        if !matches!(
            self.test,
            Test::Compare(Operator::Equal, Operand::Literal(_))
        ) {
            return 0;
        }
        let field = match &self.lhs {
            Subject::Field(field) => Some(*field),
            Subject::Coalesce(call) => call.first_field(),
            Subject::Substring(call) => call.first_field(),
        };
        field
            .filter(|field| field.binding == Binding::Node(index))
            .and_then(|field| field.property)
            .map_or(0, super::property_selectivity)
    }

    fn evaluate(
        &self,
        bindings: &Bindings<'_>,
        context: &ExpressionContext<'_>,
    ) -> Option<bool> {
        if let Test::Label(label) = self.test {
            let Subject::Field(field) = self.lhs else {
                unreachable!("label test requires a variable");
            };
            return match field.binding {
                Binding::Node(index) => bindings
                    .node(index)
                    .map(|node| node.label == label.as_bytes())
                    .or(bindings.complete_path.then_some(false)),
                Binding::Edge(index) => (bindings.edges[index].is_some()
                    || bindings.complete_path)
                    .then_some(false),
                Binding::Unbound => Some(false),
            };
        }
        if matches!(
            self.lhs,
            Subject::Field(Field {
                binding: Binding::Unbound,
                ..
            })
        ) || matches!(
            self.test,
            Test::Compare(
                _,
                Operand::Field(Field {
                    binding: Binding::Unbound,
                    ..
                })
            )
        ) {
            // A post-WITH predicate that reads an absent alias keeps the row.
            // Native applies NOT to the leaf result, so an outer NOT reverses it.
            return Some(true);
        }
        let read_argument = |binding, property: Option<&str>| {
            if let Binding::Edge(index) = binding
                && property.is_none()
            {
                return Some(Cow::Borrowed(
                    bindings.edges[index]?.properties.as_slice(),
                ));
            }
            Field {
                name: "",
                binding,
                property,
            }
            .value(bindings, context)
        };
        let actual = match &self.lhs {
            Subject::Field(field) => match field.value(bindings, context) {
                Some(value) => value,
                None => return bindings.complete_path.then_some(true),
            },
            Subject::Coalesce(call) => call.value(|binding, property| {
                read_argument(binding, property)
                    .or(bindings.complete_path.then_some(Cow::Borrowed(b"")))
            })?,
            Subject::Substring(call) => Cow::Owned(call.value(read_argument)?),
        };
        Some(match &self.test {
            Test::Label(_) => unreachable!("label does not read a property"),
            Test::Null { negated } => actual.is_empty() != *negated,
            Test::In(values) => values.iter().any(|value| value == &actual),
            Test::Compare(op, Operand::Literal(expected)) => {
                op.matches(&actual, expected, context)
            }
            Test::Compare(op, Operand::Field(field)) => {
                let Some(expected) = field.value(bindings, context) else {
                    return bindings.complete_path.then_some(true);
                };
                !actual.is_empty()
                    && !expected.is_empty()
                    && op.matches(&actual, &expected, context)
            }
        })
    }
}

impl Field<'_> {
    fn value<'a>(
        self,
        bindings: &Bindings<'a>,
        context: &ExpressionContext<'_>,
    ) -> Option<Cow<'a, [u8]>> {
        Some(match self.binding {
            Binding::Node(index) => {
                let node = bindings.node(index)?;
                self.property.map_or(Cow::Borrowed(&node.name), |property| {
                    node.property_bytes(property, context)
                })
            }
            Binding::Edge(index) => {
                bindings.edges[index]?.property_bytes(self.property)
            }
            Binding::Unbound => return None,
        })
    }
}

impl Operator {
    fn matches(
        self,
        actual: &[u8],
        expected: &[u8],
        context: &ExpressionContext<'_>,
    ) -> bool {
        match self {
            Self::Equal => actual == expected,
            Self::NotEqual => actual != expected,
            Self::Regex => context.regex_matches(actual, expected, false),
            Self::Contains => {
                expected.is_empty()
                    || actual
                        .windows(expected.len())
                        .any(|part| part == expected)
            }
            Self::StartsWith => actual.starts_with(expected),
            Self::EndsWith => actual.ends_with(expected),
            Self::Less => numeric_prefix(actual) < numeric_prefix(expected),
            Self::LessOrEqual => {
                numeric_prefix(actual) <= numeric_prefix(expected)
            }
            Self::Greater => numeric_prefix(actual) > numeric_prefix(expected),
            Self::GreaterOrEqual => {
                numeric_prefix(actual) >= numeric_prefix(expected)
            }
        }
    }
}

fn numeric_prefix(bytes: &[u8]) -> f64 {
    parse_number(bytes).unwrap_or(0.0)
}

pub(super) fn parse_number(bytes: &[u8]) -> Option<f64> {
    unsafe extern "C" {
        fn strtod(input: *const c_char, end: *mut *mut c_char) -> c_double;
    }
    // Native compares numeric prefixes, including hex, infinities and NaN.
    let text = CString::new(bytes).expect("Cypher cells contain no NUL bytes");
    let mut end = std::ptr::null_mut();
    // SAFETY: text is NUL-terminated and end points to writable storage.
    let value = unsafe { strtod(text.as_ptr(), &mut end) };
    (!std::ptr::eq(text.as_ptr(), end)).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cypher_linear::{classify, tokens};

    use rusqlite::Connection;

    fn fixture() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            r#"
            CREATE TABLE nodes (
                id INTEGER PRIMARY KEY, project TEXT, label TEXT, name TEXT,
                qualified_name TEXT, file_path TEXT, start_line INTEGER,
                end_line INTEGER, properties TEXT
            );
            CREATE TABLE edges (
                id INTEGER PRIMARY KEY, project TEXT, source_id INTEGER,
                target_id INTEGER, type TEXT, properties TEXT
            );
            INSERT INTO nodes VALUES
                (1, 'test', 'Source', 'one', 'one', 'a.py', 1, 10, '{"score":1.25,"ready":true}'),
                (2, 'test', 'Target', 'two', 'two', 'a.py', 2, 20, '{"score":2}'),
                (3, 'test', 'Source', 'three', 'three', '', 3, 30, '{}'),
                (4, 'test', 'Target', 'four', 'four', '', 4, 40, '{"score":"2suffix"}'),
                (5, 'test', 'Target', 'five', 'five', 'b.py', 5, 50, '{"score":0}');
            INSERT INTO edges VALUES
                (1, 'test', 1, 2, 'CALLS', '{"score":2.5}'),
                (2, 'test', 1, 4, 'CALLS', '{"score":3}'),
                (3, 'test', 3, 4, 'CALLS', '{}'),
                (4, 'test', 3, 5, 'CALLS', '{"score":4}'),
                (5, 'test', 2, 5, 'CALLS', '{}');
            "#,
        )
        .unwrap();
        db
    }

    fn assert_rows(db: &Connection, source: &str, expected: &[&str]) {
        let query = classify(source).unwrap();
        let result = query.execute_on(db, "test").unwrap();
        let actual: Vec<_> = result
            .rows
            .iter()
            .map(|row| std::str::from_utf8(&row[0]).unwrap())
            .collect();
        assert_eq!(actual, expected, "{source}");
    }

    #[test]
    fn where_precedence_membership_null_labels_and_native_numbers() {
        let db = fixture();
        let cases: &[(&str, &[&str])] = &[
            (
                "n:Source OR n.score > 1.25 AND n.file_path IS NULL XOR n.name = 'two'",
                &["one", "two", "three", "four"],
            ),
            (
                "NOT (n.file_path IS NOT NULL AND NOT n.score IN [2 '1.25',])",
                &["one", "two", "three", "four"],
            ),
            ("n.ready = TRUE", &["one"]),
            ("n.score = 1.25 OR n.score = 0", &["one", "five"]),
            ("n.score >= 2 AND n.score <> 2", &["four"]),
            ("n.score >= '0x1p1' AND n.score < 'Inf'", &["two", "four"]),
            ("n.score <= 'NaN'", &[]),
            ("n.name IN []", &[]),
            ("n.start_line <= 1.9", &["one"]),
        ];
        for (predicate, expected) in cases {
            assert_rows(
                &db,
                &format!(
                    "MATCH (n) WHERE {predicate} RETURN n.name, n.start_line ORDER BY n.start_line"
                ),
                expected,
            );
        }
    }

    #[test]
    fn where_waits_for_both_operands_and_reads_edges() {
        let db = fixture();
        let cases: &[(&str, &[&str])] = &[
            ("a.file_path = b.file_path", &["two"]),
            ("a.file_path <> b.file_path", &["five"]),
            (
                "NOT (a.file_path = b.file_path)",
                &["five", "five", "four", "four"],
            ),
            (
                "a.name = 'three' OR b.name = 'two'",
                &["five", "four", "two"],
            ),
            ("a.name = 'three' XOR b.name = 'four'", &["five", "four"]),
            (
                "r.type = 'CALLS' AND r.score > b.score",
                &["five", "four", "two"],
            ),
            ("r.score IS NULL AND a:Source", &["four"]),
        ];
        for (predicate, expected) in cases {
            assert_rows(
                &db,
                &format!(
                    "MATCH (a)-[r:CALLS]->(b) WHERE {predicate} RETURN b.name ORDER BY b.name"
                ),
                expected,
            );
        }
        assert_rows(
            &db,
            "MATCH (a)-[:CALLS]->(b)-[r:CALLS]->(c) WHERE NOT (b.name = 'four' OR r.score IS NOT NULL) AND a.name <> c.name RETURN c.name",
            &["five"],
        );
    }

    #[test]
    fn where_equalities_only_seed_through_and() {
        let selectivity = |predicate| {
            let source = format!(
                "MATCH (a)-[:CALLS]->(b) WHERE {predicate} RETURN a.name"
            );
            let query = classify(&source).unwrap();
            let branch = query.first_branch();
            [branch.selectivity(0), branch.selectivity(1)]
        };
        assert_eq!(
            selectivity("a.file_path = 'a' AND (b.name = 'b' AND a:Source)"),
            [2, 3]
        );
        assert_eq!(
            selectivity("a.file_path = 'a' AND (b.name = 'b' OR b.name = 'c')"),
            [2, 0]
        );
        assert_eq!(selectivity("NOT a.name = 'a' AND b.name = a.name"), [0, 0]);
    }

    #[test]
    fn where_scans_long_chains_and_bounds_nesting() {
        let clause = "n.name = 'one' OR ".repeat(4096);
        assert_rows(
            &fixture(),
            &format!("MATCH (n) WHERE {clause} n.name = 'five' RETURN n.name"),
            &["one", "five"],
        );
        for depth in [MAX_DEPTH, MAX_DEPTH + 1] {
            for (open, close) in [("(", ")"), ("NOT ", "")] {
                let source = format!(
                    "MATCH (n) WHERE {}n.name = 'one'{} RETURN n.name",
                    open.repeat(depth),
                    close.repeat(depth),
                );
                if depth == MAX_DEPTH {
                    assert!(classify(&source).is_ok());
                } else {
                    assert_eq!(
                        classify(&source).unwrap_err(),
                        ParseError::Depth
                    );
                }
            }
        }
    }

    #[test]
    fn where_distinguishes_syntax_errors_from_fallback() {
        let source = "MATCH (n) WHERE n.name BOGUS 'x' RETURN n.name";
        let error = classify(source).unwrap_err();
        assert_eq!(error, ParseError::UnexpectedOperator(23));
        assert_eq!(
            error.diagnostic().as_deref(),
            Some("unexpected operator at pos 23")
        );
        let source =
            "MATCH (a)-[r]->(b) WHERE type(r) STARTS WITH 'CALL' RETURN b.name";
        assert_eq!(
            classify(source).unwrap_err().diagnostic().as_deref(),
            Some(
                "unsupported function 'type' in WHERE (supported: coalesce, substring, replace, left, right)"
            ),
        );
        for source in [
            "MATCH (n) WHERE n.name =~ RETURN n.name",
            "MATCH (n) RETURN substring(n.name, -1)",
            "MATCH (n) WHERE n.name STARTS 'o' RETURN n.name",
        ] {
            assert_eq!(classify(source).unwrap_err(), ParseError::Unsupported);
        }
    }

    #[test]
    fn literals_preserve_native_escapes_and_byte_limit() {
        let source = r#"-- lead
            MATCH /* gap */ (n) WHERE n.name IN ['one', "two\n", '3\t4\q\'']
            // tail
            RETURN n.name"#;
        assert_rows(&fixture(), source, &["one"]);

        let source = r#"'one\n\t\\\'\q'"#;
        assert_eq!(
            tokens(source)
                .unwrap()
                .into_iter()
                .map(|item| item.token)
                .collect::<Vec<_>>(),
            [Token::Literal(Cow::Borrowed(b"one\n\t\\'q"))]
        );

        let source = format!("'{}'", "é".repeat(2048));
        let mut lexed = tokens(&source).unwrap().into_iter();
        let Token::Literal(bytes) = lexed.next().unwrap().token else {
            panic!("expected one literal");
        };
        assert!(lexed.next().is_none());
        assert_eq!(bytes.len(), 4095);
        assert_eq!(bytes[4094], 0xc3);
    }
}
