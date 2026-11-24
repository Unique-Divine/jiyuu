//! Bounded Cypher evaluator for a verified subset of query patterns.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::ffi::c_long;
#[cfg(test)]
use std::path::Path;
use std::time::{Duration, Instant};

#[cfg(test)]
use rusqlite::OpenFlags;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, Row, Statement, params};

const ENGINE_LIMIT: usize = 100_000;
const DEADLINE: Duration = Duration::from_secs(30);
const CHECK_INTERVAL: usize = 1024;
const PROPERTY_LIMIT: usize = 511;
const ORDER_KEYS_MAX: usize = 8;
const NATIVE_EDGE_PROPERTY_BUFFERS: usize = 8;
// CAST uses SQLite's integer conversion, as sqlite3_column_int does in C.
const NODE_COLUMNS: &str = "n.id, n.label, n.name, n.qualified_name, n.file_path, CAST(n.start_line AS INTEGER), CAST(n.end_line AS INTEGER), n.properties";

#[derive(Debug)]
pub(crate) struct Unsupported;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Token<'a> {
    Word(&'a str),
    Literal(&'a str),
    Number(&'a str),
    Symbol(u8),
    ArrowIn,
    ArrowOut,
    NotEqual,
    LessOrEqual,
    GreaterOrEqual,
}

fn tokens(mut query: &str) -> Result<Vec<Token<'_>>, Unsupported> {
    let mut result = Vec::new();
    while !query.is_empty() {
        query = query.trim_start_matches(|c: char| c.is_ascii_whitespace());
        let Some(&first) = query.as_bytes().first() else {
            break;
        };
        let (token, length) = match first {
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                let length = query
                    .bytes()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == b'_')
                    .count();
                (Token::Word(&query[..length]), length)
            }
            b'0'..=b'9' => {
                let length =
                    query.bytes().take_while(u8::is_ascii_digit).count();
                (Token::Number(&query[..length]), length)
            }
            b'\'' | b'"' => {
                let length = query.as_bytes()[1..]
                    .iter()
                    .position(|&c| c == first)
                    .ok_or(Unsupported)?
                    + 1;
                let value = &query[1..length];
                if value.len() >= 4096
                    || value.bytes().any(|c| c == b'\\' || c.is_ascii_control())
                {
                    return Err(Unsupported);
                }
                (Token::Literal(value), length + 1)
            }
            b'<' if query.starts_with("<-") => (Token::ArrowIn, 2),
            b'-' if query.starts_with("->") => (Token::ArrowOut, 2),
            b'<' if query.starts_with("<=") => (Token::LessOrEqual, 2),
            b'>' if query.starts_with(">=") => (Token::GreaterOrEqual, 2),
            b'<' if query.starts_with("<>") => (Token::NotEqual, 2),
            b'!' if query.starts_with("!=") => (Token::NotEqual, 2),
            b'(' | b')' | b'[' | b']' | b'{' | b'}' | b':' | b'-' | b'<'
            | b'>' | b'.' | b'=' | b',' | b'*' => (Token::Symbol(first), 1),
            _ => return Err(Unsupported),
        };
        result.push(token);
        query = &query[length..];
    }
    Ok(result)
}

struct Parser<'a> {
    tokens: std::iter::Peekable<std::vec::IntoIter<Token<'a>>>,
    names: HashMap<&'a str, Binding>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Binding {
    Node(usize),
    Edge(usize),
}

#[derive(Debug)]
enum Comparison<'a> {
    Equal(&'a str),
    NotEqual(&'a str),
    Contains(&'a str),
    StartsWith(&'a str),
    EndsWith(&'a str),
    Line(Ordering, bool, i32),
}

#[derive(Debug)]
struct Predicate<'a> {
    node: usize,
    property: &'a str,
    comparison: Comparison<'a>,
    negated: bool,
}

impl Predicate<'_> {
    fn is_seed_equality(&self) -> bool {
        !self.negated && matches!(self.comparison, Comparison::Equal(_))
    }

    fn matches(&self, node: &Node) -> bool {
        let matches = match self.comparison {
            Comparison::Line(direction, inclusive, expected) => {
                let actual = if self.property == "start_line" {
                    node.start_line
                } else {
                    node.end_line
                };
                let order = actual.cmp(&expected);
                order == direction || (inclusive && order == Ordering::Equal)
            }
            Comparison::Equal(expected) => {
                node.property_bytes(self.property).as_ref()
                    == expected.as_bytes()
            }
            Comparison::NotEqual(expected) => {
                node.property_bytes(self.property).as_ref()
                    != expected.as_bytes()
            }
            Comparison::StartsWith(expected) => node
                .property_bytes(self.property)
                .starts_with(expected.as_bytes()),
            Comparison::EndsWith(expected) => node
                .property_bytes(self.property)
                .ends_with(expected.as_bytes()),
            Comparison::Contains(expected) => {
                expected.is_empty()
                    || node
                        .property_bytes(self.property)
                        .windows(expected.len())
                        .any(|part| part == expected.as_bytes())
            }
        };
        matches != self.negated
    }
}

#[derive(Debug)]
struct NodePattern<'a> {
    label: Option<&'a str>,
    inline: Vec<(&'a str, &'a str)>,
}

#[derive(Clone, Copy, Debug)]
struct Hop<'a> {
    kind: Option<&'a str>,
    inbound: bool,
}

#[derive(Debug)]
enum ProjectionExpression<'a> {
    Field(Binding, Option<&'a str>),
    NodeLabels(usize),
    Count,
}

impl ProjectionExpression<'_> {
    fn field_value<'a>(
        &self,
        nodes: &[&'a Node],
        edges: &[&'a Edge],
    ) -> Cow<'a, [u8]> {
        match *self {
            Self::Field(Binding::Node(index), None) => {
                Cow::Borrowed(&nodes[index].name)
            }
            Self::Field(Binding::Node(index), Some(name)) => {
                nodes[index].property_bytes(name)
            }
            Self::Field(Binding::Edge(index), None) => {
                Cow::Borrowed(&edges[index].properties)
            }
            Self::Field(Binding::Edge(index), Some("type")) => {
                Cow::Borrowed(&edges[index].kind)
            }
            Self::Field(Binding::Edge(index), Some(name)) => Cow::Borrowed(
                json_property_bytes(&edges[index].properties, name),
            ),
            Self::NodeLabels(index) => Cow::Owned(
                b"[\""
                    .iter()
                    .chain(&nodes[index].label)
                    .chain(b"\"]")
                    .take(PROPERTY_LIMIT)
                    .copied()
                    .collect(),
            ),
            Self::Count => unreachable!("aggregate has no per-binding value"),
        }
    }
}

#[derive(Debug)]
struct Projection<'a> {
    expression: ProjectionExpression<'a>,
    name: String,
}

#[derive(Debug)]
struct OrderKey {
    column: usize,
    descending: bool,
}

#[derive(Debug)]
pub(crate) struct Query<'a> {
    nodes: Vec<NodePattern<'a>>,
    hops: Vec<Hop<'a>>,
    filters: Vec<Predicate<'a>>,
    any_filter: bool,
    projections: Vec<Projection<'a>>,
    order: Vec<OrderKey>,
    limit: Option<usize>,
}

impl<'a> Parser<'a> {
    fn keyword(&mut self, expected: &str) -> bool {
        self.tokens.next_if(|token| matches!(token, Token::Word(word) if word.eq_ignore_ascii_case(expected))).is_some()
    }

    fn consume(&mut self, symbol: u8) -> bool {
        self.tokens.next_if_eq(&Token::Symbol(symbol)).is_some()
    }

    fn symbol(&mut self, symbol: u8) -> Result<(), Unsupported> {
        self.consume(symbol).then_some(()).ok_or(Unsupported)
    }

    fn name(&mut self) -> Result<&'a str, Unsupported> {
        match self.tokens.next() {
            Some(Token::Word(name)) if valid_name(name) => Ok(name),
            _ => Err(Unsupported),
        }
    }

    fn literal(&mut self) -> Result<&'a str, Unsupported> {
        match self.tokens.next() {
            Some(Token::Literal(value)) => Ok(value),
            _ => Err(Unsupported),
        }
    }

    fn alias(&mut self, target: Binding) -> Result<(), Unsupported> {
        if matches!(self.tokens.peek(), Some(Token::Word(_))) {
            let name = self.name()?;
            if self.names.insert(name, target).is_some() {
                return Err(Unsupported);
            }
        }
        Ok(())
    }

    fn node(&mut self, index: usize) -> Result<NodePattern<'a>, Unsupported> {
        self.symbol(b'(')?;
        self.alias(Binding::Node(index))?;
        let label = self.consume(b':').then(|| self.name()).transpose()?;
        let mut inline = Vec::new();
        if self.consume(b'{') {
            loop {
                let property = node_property(self.name()?)?;
                self.symbol(b':')?;
                let value = self.literal()?;
                // The native inline parser interprets these characters as regex.
                if value.contains(".*")
                    || value.contains(".+")
                    || value.contains(['[', '(', '|', '^', '$'])
                {
                    return Err(Unsupported);
                }
                inline.push((property, value));
                if !self.consume(b',') {
                    break;
                }
            }
            self.symbol(b'}')?;
        }
        self.symbol(b')')?;
        Ok(NodePattern { label, inline })
    }

    fn hop(
        &mut self,
        index: usize,
        inbound: bool,
    ) -> Result<Hop<'a>, Unsupported> {
        if !inbound {
            self.symbol(b'-')?;
        }
        self.symbol(b'[')?;
        self.alias(Binding::Edge(index))?;
        let kind = self.consume(b':').then(|| self.name()).transpose()?;
        self.symbol(b']')?;
        if inbound {
            self.symbol(b'-')?;
        } else if self.tokens.next() != Some(Token::ArrowOut) {
            return Err(Unsupported);
        }
        Ok(Hop { kind, inbound })
    }

    fn binding(&self, name: &str) -> Result<Binding, Unsupported> {
        self.names.get(name).copied().ok_or(Unsupported)
    }

    fn reference(&mut self) -> Result<(&'a str, Binding), Unsupported> {
        let name = self.name()?;
        Ok((name, self.binding(name)?))
    }

    fn named_projection(
        &mut self,
    ) -> Result<(ProjectionExpression<'a>, String), Unsupported> {
        let name = self.name()?;
        if self.consume(b'(') {
            let (target, binding) = self.reference()?;
            self.symbol(b')')?;
            return match binding {
                Binding::Edge(_) if name.eq_ignore_ascii_case("type") => Ok((
                    ProjectionExpression::Field(binding, Some("type")),
                    format!("type({target})"),
                )),
                Binding::Node(index) if name.eq_ignore_ascii_case("labels") => {
                    Ok((
                        ProjectionExpression::NodeLabels(index),
                        format!("labels({target})"),
                    ))
                }
                _ => Err(Unsupported),
            };
        }
        let binding = self.binding(name)?;
        let property = self.consume(b'.').then(|| self.name()).transpose()?;
        if let (Binding::Node(_), Some(property)) = (binding, property) {
            node_property(property)?;
        }
        Ok((
            ProjectionExpression::Field(binding, property),
            projection_name(name, property),
        ))
    }

    fn count_name(
        &mut self,
        allow_property: bool,
    ) -> Result<String, Unsupported> {
        self.symbol(b'(')?;
        let variable = if self.consume(b'*') {
            "*"
        } else {
            let (name, _) = self.reference()?;
            if allow_property && self.consume(b'.') {
                // Native COUNT(var.prop) tests whether var is bound, even
                // when the projected property is missing. Its default column
                // name also omits the property.
                self.name()?;
            }
            name
        };
        self.symbol(b')')?;
        Ok(format!("COUNT({variable})"))
    }

    fn projection(&mut self) -> Result<Projection<'a>, Unsupported> {
        let (expression, default_name) = if self.keyword("COUNT") {
            (ProjectionExpression::Count, self.count_name(true)?)
        } else {
            self.named_projection()?
        };
        let name = if self.keyword("AS") {
            self.name()?.to_owned()
        } else {
            default_name
        };
        Ok(Projection { expression, name })
    }

    fn predicate(&mut self) -> Result<Predicate<'a>, Unsupported> {
        let negated = self.keyword("NOT");
        let (_, Binding::Node(node)) = self.reference()? else {
            return Err(Unsupported);
        };
        self.symbol(b'.')?;
        let property = node_property(self.name()?)?;
        let comparison = match self.tokens.next().ok_or(Unsupported)? {
            Token::Symbol(b'=') => Comparison::Equal(self.literal()?),
            Token::NotEqual => Comparison::NotEqual(self.literal()?),
            Token::Word(word) if word.eq_ignore_ascii_case("CONTAINS") => {
                Comparison::Contains(self.literal()?)
            }
            Token::Word(word) if word.eq_ignore_ascii_case("STARTS") => {
                if !self.keyword("WITH") {
                    return Err(Unsupported);
                }
                Comparison::StartsWith(self.literal()?)
            }
            Token::Word(word) if word.eq_ignore_ascii_case("ENDS") => {
                if !self.keyword("WITH") {
                    return Err(Unsupported);
                }
                Comparison::EndsWith(self.literal()?)
            }
            op if matches!(property, "start_line" | "end_line") => {
                let (direction, inclusive) = match op {
                    Token::Symbol(b'<') => (Ordering::Less, false),
                    Token::LessOrEqual => (Ordering::Less, true),
                    Token::Symbol(b'>') => (Ordering::Greater, false),
                    Token::GreaterOrEqual => (Ordering::Greater, true),
                    _ => return Err(Unsupported),
                };
                let Some(Token::Number(value)) = self.tokens.next() else {
                    return Err(Unsupported);
                };
                Comparison::Line(
                    direction,
                    inclusive,
                    value.parse().map_err(|_| Unsupported)?,
                )
            }
            _ => return Err(Unsupported),
        };
        Ok(Predicate {
            node,
            property,
            comparison,
            negated,
        })
    }
}

fn valid_name(name: &str) -> bool {
    // Skip reserved lexer keywords and native's generated anonymous aliases.
    const RESERVED: &str = "MATCH OPTIONAL WHERE RETURN LIMIT AND OR XOR NOT AS IN IS NULL TRUE FALSE CONTAINS STARTS ENDS WITH ORDER BY DESC ASC SKIP DISTINCT UNWIND UNION ALL CASE WHEN THEN ELSE END EXISTS CREATE DELETE REMOVE SET MERGE CALL YIELD COUNT SUM AVG MIN MAX COLLECT TOLOWER TOUPPER TOSTRING DETACH MANDATORY FOREACH ON ADD CONSTRAINT DO DROP FOR FROM GRAPH OF REQUIRE SCALAR UNIQUE";
    name.len() < 64
        && !name.starts_with('_')
        && !RESERVED
            .split_ascii_whitespace()
            .any(|word| name.eq_ignore_ascii_case(word))
}

fn node_property(name: &str) -> Result<&str, Unsupported> {
    // Degree requires a separate derived read, which is outside this slice.
    if matches!(name, "in_degree" | "out_degree") {
        Err(Unsupported)
    } else {
        Ok(name)
    }
}

pub(crate) fn classify(source: &str) -> Result<Query<'_>, Unsupported> {
    let mut parser = Parser {
        tokens: tokens(source)?.into_iter().peekable(),
        names: HashMap::new(),
    };
    if !parser.keyword("MATCH") {
        return Err(Unsupported);
    }
    let mut nodes = vec![parser.node(0)?];
    let mut hops = Vec::new();
    while hops.len() < 2
        && matches!(
            parser.tokens.peek(),
            Some(Token::Symbol(b'-') | Token::ArrowIn)
        )
    {
        let inbound = parser.tokens.next_if_eq(&Token::ArrowIn).is_some();
        hops.push(parser.hop(hops.len(), inbound)?);
        nodes.push(parser.node(nodes.len())?);
    }
    if hops.len() > 1 {
        // The head has its own synthetic name. Later anonymous endpoints
        // reuse a single name in C, so keep multi-hop queries with two
        // anonymous bindings on the reference path.
        let named_nodes = parser
            .names
            .values()
            .filter(|binding| matches!(binding, Binding::Node(_)))
            .count();
        if nodes.len() - named_nodes > 1
            || hops.iter().any(|hop| hop.kind.is_none())
        {
            return Err(Unsupported);
        }
    }
    let mut filters = Vec::new();
    let mut any_filter = false;
    if parser.keyword("WHERE") {
        let mut disjunction = None;
        loop {
            filters.push(parser.predicate()?);
            let next_any = if hops.is_empty() && parser.keyword("OR") {
                true
            } else if parser.keyword("AND") {
                false
            } else {
                break;
            };
            // Keep mixed operators and disjunctions over multiple bindings
            // in C. The Graph UI emits a flat OR over one node.
            if disjunction.is_some_and(|previous| previous != next_any) {
                return Err(Unsupported);
            }
            disjunction = Some(next_any);
        }
        any_filter = disjunction.unwrap_or(false);
    }
    if !parser.keyword("RETURN") {
        return Err(Unsupported);
    }
    let mut projections = Vec::new();
    loop {
        projections.push(parser.projection()?);
        if !parser.consume(b',') {
            break;
        }
    }
    if projections.len() > 32 {
        return Err(Unsupported);
    }
    // A single grouping field covers the common count breakdown without
    // interpreting the native compound key's delimiter and byte truncation.
    let field_count = projections
        .iter()
        .filter(|column| {
            !matches!(column.expression, ProjectionExpression::Count)
        })
        .count();
    if field_count < projections.len() && field_count > 1 {
        return Err(Unsupported);
    }
    // C can retain a pointer to an empty edge-property buffer while
    // projecting later columns. A ninth lookup can overwrite that cell.
    if projections
        .iter()
        .filter(|p| {
            matches!(p.expression,
                ProjectionExpression::Field(Binding::Edge(_), Some(name)) if name != "type"
            )
        })
        .count()
        > NATIVE_EDGE_PROPERTY_BUFFERS
    {
        return Err(Unsupported);
    }
    let mut order = Vec::new();
    if parser.keyword("ORDER") {
        if hops.len() > 1 || !parser.keyword("BY") {
            return Err(Unsupported);
        }
        loop {
            if order.len() == ORDER_KEYS_MAX {
                return Err(Unsupported);
            }
            let name = if parser.keyword("COUNT") {
                parser.count_name(false)?
            } else {
                let name = parser.name()?;
                let property =
                    parser.consume(b'.').then(|| parser.name()).transpose()?;
                projection_name(name, property)
            };
            let column = projections
                .iter()
                .position(|p| p.name == name)
                .ok_or(Unsupported)?;
            let descending = parser.keyword("DESC");
            if !descending {
                parser.keyword("ASC");
            }
            order.push(OrderKey { column, descending });
            if !parser.consume(b',') {
                break;
            }
        }
    }
    let limit = if parser.keyword("LIMIT") {
        let Some(Token::Number(limit)) = parser.tokens.next() else {
            return Err(Unsupported);
        };
        let n = limit.parse().map_err(|_| Unsupported)?;
        if n > ENGINE_LIMIT {
            return Err(Unsupported);
        }
        Some(n)
    } else {
        None
    };
    if parser.tokens.next().is_some() {
        return Err(Unsupported);
    }
    Ok(Query {
        nodes,
        hops,
        filters,
        any_filter,
        projections,
        order,
        limit,
    })
}

fn projection_name(name: &str, property: Option<&str>) -> String {
    property
        .map_or_else(|| name.to_owned(), |property| format!("{name}.{property}"))
}

#[derive(Debug)]
struct Node {
    id: i64,
    label: Vec<u8>,
    name: Vec<u8>,
    qualified_name: Vec<u8>,
    file_path: Vec<u8>,
    start_line: i32,
    end_line: i32,
    properties: Vec<u8>,
}

impl Node {
    fn read(row: &Row<'_>, offset: usize) -> rusqlite::Result<Self> {
        // Native decodes both values as a C int, keeping the low 32 bits.
        let line = |index| {
            row.get::<_, Option<i64>>(index)
                .map(|value| value.unwrap_or_default() as i32)
        };
        Ok(Self {
            id: row.get(offset)?,
            label: read_text_bytes(row, offset + 1)?.unwrap_or_default(),
            name: read_text_bytes(row, offset + 2)?.unwrap_or_default(),
            qualified_name: read_text_bytes(row, offset + 3)?
                .unwrap_or_default(),
            file_path: read_text_bytes(row, offset + 4)?.unwrap_or_default(),
            start_line: line(offset + 5)?,
            end_line: line(offset + 6)?,
            properties: read_text_bytes(row, offset + 7)?.unwrap_or_default(),
        })
    }

    fn property_bytes(&self, name: &str) -> Cow<'_, [u8]> {
        let value: &[u8] = match name {
            "label" => &self.label,
            "name" => &self.name,
            "qualified_name" | "qn" => &self.qualified_name,
            "file_path" | "file" | "path" => &self.file_path,
            "start_line" => {
                return self.start_line.to_string().into_bytes().into();
            }
            "end_line" => return self.end_line.to_string().into_bytes().into(),
            _ => b"",
        };
        if !value.is_empty() {
            return value.into();
        }
        if self.properties.starts_with(b"{") {
            json_property_bytes(&self.properties, name).into()
        } else {
            Cow::Borrowed(b"")
        }
    }
}

// SQLite's TEXT affinity leaves BLOBs intact. Like sqlite3_column_text
// followed by strdup, copy either byte representation up to the first NUL.
fn read_text_bytes(
    row: &Row<'_>,
    column: usize,
) -> rusqlite::Result<Option<Vec<u8>>> {
    match row.get_ref(column)? {
        ValueRef::Null => Ok(None),
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => {
            Ok(Some(c_string_bytes(bytes).to_vec()))
        }
        value => Err(rusqlite::Error::InvalidColumnType(
            column,
            row.as_ref().column_name(column)?.to_owned(),
            value.data_type(),
        )),
    }
}

// C compares and projects the raw extraction, including when its byte cap
// splits UTF-8. The native result sink handles serialization.
fn json_property_bytes<'a>(json: &'a [u8], name: &str) -> &'a [u8] {
    let key = format!("\"{name}\":");
    let Some(start) = json
        .windows(key.len())
        .position(|bytes| bytes == key.as_bytes())
    else {
        return b"";
    };
    let tail = &json[start + key.len()..];
    let padding = tail
        .iter()
        .take_while(|&&byte| matches!(byte, b' ' | b'\t'))
        .count();
    let bytes = &tail[padding..];
    let (quoted, open, close, start) = match bytes.first() {
        Some(b'"') => (true, 0, 0, 1),
        Some(b'{') => (false, b'{', b'}', 0),
        Some(b'[') => (false, b'[', b']', 0),
        _ => (false, 0, 0, 0),
    };
    let mut in_string = quoted;
    let mut depth = 0;
    let mut end = start;
    let limit = bytes.len().min(start + PROPERTY_LIMIT);
    while end < limit {
        let c = bytes[end];
        if in_string {
            if c == b'"' {
                if quoted {
                    break;
                }
                in_string = false;
            } else if c == b'\\' && end + 1 < limit {
                end += 1;
            }
        } else if open == 0 {
            if matches!(c, b',' | b'}' | b' ') {
                break;
            }
        } else if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                end += 1;
                break;
            }
        } else if c == b'"' {
            in_string = true;
        }
        end += 1;
    }
    &bytes[start..end]
}

#[derive(Debug, PartialEq)]
pub(crate) struct ResultRows {
    pub(crate) columns: Vec<String>,
    // Cells can end on a split UTF-8 scalar. C hashes and budgets these bytes
    // before rendering the response.
    pub(crate) rows: Vec<Vec<Vec<u8>>>,
    pub(crate) truncated: bool,
}

struct CountGroups {
    column: usize,
    max_groups: usize,
    // Each map value records the group's first-seen position and count.
    groups: HashMap<Vec<u8>, (usize, usize)>,
}

impl CountGroups {
    fn record(&mut self, key: &[u8]) {
        // Native clips grouped cells before grouping and output.
        let key = c_string_bytes(key);
        let key = &key[..key.len().min(PROPERTY_LIMIT)];
        if let Some((_, count)) = self.groups.get_mut(key) {
            *count += 1;
        } else if self.groups.len() < self.max_groups {
            self.groups.insert(key.to_vec(), (self.groups.len(), 1));
        }
    }
}

enum CountRows {
    Total(usize),
    Grouped(CountGroups),
}

impl CountRows {
    fn rows(
        self,
        columns: usize,
        limit: usize,
        budget: &mut ExecutionBudget<'_>,
    ) -> Result<Vec<Vec<Vec<u8>>>, ExecutionError> {
        match self {
            Self::Total(count) => {
                Ok(vec![vec![count.to_string().into_bytes(); columns]])
            }
            Self::Grouped(CountGroups { column, groups, .. }) => {
                if limit == ENGINE_LIMIT && groups.len() >= ENGINE_LIMIT {
                    return Err(ExecutionError::RowCeiling);
                }
                let mut rows = vec![Vec::new(); groups.len()];
                for (key, (index, count)) in groups {
                    budget.step()?;
                    let mut row = vec![count.to_string().into_bytes(); columns];
                    row[column] = key;
                    rows[index] = row;
                }
                Ok(rows)
            }
        }
    }
}

#[derive(Debug)]
pub(crate) enum ExecutionError {
    Storage(rusqlite::Error),
    RowCeiling,
    Deadline,
    Cancelled,
}

impl From<rusqlite::Error> for ExecutionError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error)
    }
}

pub(crate) struct ExecutionBudget<'a> {
    deadline: Instant,
    now: &'a dyn Fn() -> Instant,
    cancelled: &'a dyn Fn() -> bool,
    remaining: usize,
}

impl<'a> ExecutionBudget<'a> {
    pub(crate) fn new(
        now: &'a dyn Fn() -> Instant,
        cancelled: &'a dyn Fn() -> bool,
    ) -> Self {
        Self {
            deadline: now() + DEADLINE,
            now,
            cancelled,
            remaining: CHECK_INTERVAL,
        }
    }

    fn check(&self) -> Result<(), ExecutionError> {
        if (self.cancelled)() {
            return Err(ExecutionError::Cancelled);
        }
        if (self.now)() >= self.deadline {
            return Err(ExecutionError::Deadline);
        }
        Ok(())
    }

    fn step(&mut self) -> Result<(), ExecutionError> {
        self.remaining -= 1;
        if self.remaining == 0 {
            self.remaining = CHECK_INTERVAL;
            self.check()?;
        }
        Ok(())
    }
}

impl Query<'_> {
    #[cfg(test)]
    pub(crate) fn execute(
        &self,
        db: &Path,
        project: &str,
    ) -> Result<ResultRows, ExecutionError> {
        let connection =
            Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
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

    /// Read from the graph and snapshot the caller has already selected.
    pub(crate) fn execute_with_budget(
        &self,
        connection: &Connection,
        project: &str,
        budget: &mut ExecutionBudget<'_>,
    ) -> Result<ResultRows, ExecutionError> {
        budget.check()?;
        let reversed =
            self.hops.len() == 1 && self.selectivity(1) > self.selectivity(0);
        let start_index = usize::from(reversed);
        let second_index = usize::from(!reversed);
        let label = self.nodes[start_index].label;
        let condition = if label.is_some() {
            " AND n.label = ?2"
        } else {
            ""
        };
        // Use the same unordered lookup as C. Its paged scan follows this
        // lookup's index order and uses the lookup directly for other plans.
        let sql = format!(
            "SELECT {NODE_COLUMNS} FROM nodes n WHERE n.project = ?1{condition}"
        );
        let mut statement = connection.prepare(&sql)?;
        let mut start_nodes = if let Some(label) = label {
            statement.query(params![project, label])?
        } else {
            statement.query([project])?
        };
        let mut adjacency = self
            .hops
            .first()
            .map(|hop| Adjacency::new(connection, hop.kind.is_some()))
            .transpose()?;
        let mut counts = self.count_rows();
        let mut result = ResultRows {
            columns: self
                .projections
                .iter()
                .map(|column| column.name.clone())
                .collect(),
            rows: Vec::new(),
            truncated: false,
        };
        let limit = self.limit.unwrap_or(ENGINE_LIMIT);
        'starts: loop {
            if counts.is_none()
                && adjacency.is_none()
                && self.order.is_empty()
                && result.rows.len() == limit
            {
                break;
            }
            let Some(row) = start_nodes.next()? else {
                break;
            };
            budget.step()?;
            let start = Node::read(row, 0)?;
            if !self.matches_node(start_index, &start) {
                continue;
            }
            let Some(adjacency) = &mut adjacency else {
                if !self.collect_row(
                    &mut result,
                    &mut counts,
                    &[&start],
                    &[],
                    budget,
                )? {
                    break;
                }
                continue;
            };
            let mut hop = self.hops[0];
            hop.inbound ^= reversed;
            for (next, edge) in adjacency.neighbors(start.id, hop, budget)? {
                budget.step()?;
                if !self.matches_node(second_index, &next) {
                    continue;
                }
                if let Some(&hop) = self.hops.get(1) {
                    for (last, last_edge) in
                        adjacency.neighbors(next.id, hop, budget)?
                    {
                        budget.step()?;
                        if !self.matches_node(2, &last) {
                            continue;
                        }
                        if !self.collect_row(
                            &mut result,
                            &mut counts,
                            &[&start, &next, &last],
                            &[&edge, &last_edge],
                            budget,
                        )? {
                            break 'starts;
                        }
                    }
                } else {
                    let nodes = if reversed {
                        [&next, &start]
                    } else {
                        [&start, &next]
                    };
                    if !self.collect_row(
                        &mut result,
                        &mut counts,
                        &nodes,
                        &[&edge],
                        budget,
                    )? {
                        break 'starts;
                    }
                }
            }
        }
        budget.check()?;
        if let Some(counts) = counts {
            result.rows = counts.rows(result.columns.len(), limit, budget)?;
            if self.order.is_empty() {
                result.rows.truncate(limit);
            }
        }
        if !self.order.is_empty() {
            order_rows(&mut result.rows, &self.order, limit, budget)?;
        }
        Ok(result)
    }

    fn count_rows(&self) -> Option<CountRows> {
        self.projections
            .iter()
            .any(|column| {
                matches!(column.expression, ProjectionExpression::Count)
            })
            .then(|| {
                if let Some(column) =
                    self.projections.iter().position(|column| {
                        !matches!(column.expression, ProjectionExpression::Count)
                    })
                {
                    let limit = self.limit.unwrap_or(ENGINE_LIMIT);
                    // A small ordered limit needs every key to rank groups.
                    // An unordered limit keeps first-seen keys. At the
                    // engine ceiling, extra keys cannot change the error.
                    let max_groups =
                        if self.order.is_empty() || limit == ENGINE_LIMIT {
                            limit
                        } else {
                            usize::MAX
                        };
                    CountRows::Grouped(CountGroups {
                        column,
                        max_groups,
                        groups: HashMap::new(),
                    })
                } else {
                    CountRows::Total(0)
                }
            })
    }

    fn collect_row(
        &self,
        result: &mut ResultRows,
        counts: &mut Option<CountRows>,
        nodes: &[&Node],
        edges: &[&Edge],
        budget: &ExecutionBudget<'_>,
    ) -> Result<bool, ExecutionError> {
        match counts {
            Some(CountRows::Total(count)) => {
                *count += 1;
                return Ok(true);
            }
            Some(CountRows::Grouped(counts)) => {
                let key = self.projections[counts.column]
                    .expression
                    .field_value(nodes, edges);
                counts.record(&key);
                return Ok(true);
            }
            None => {}
        }
        let limit = self.limit.unwrap_or(ENGINE_LIMIT);
        if self.order.is_empty() && result.rows.len() == limit {
            return Ok(false);
        }
        if result.rows.len() == ENGINE_LIMIT {
            // Native sorts the capped prefix and reports a lower bound when
            // another match exists. Keep the prefix under this request budget.
            result.truncated = true;
            return Ok(false);
        }
        result.rows.push(
            self.projections
                .iter()
                .map(|column| {
                    column.expression.field_value(nodes, edges).into_owned()
                })
                .collect(),
        );
        if result.rows.len() == ENGINE_LIMIT && limit == ENGINE_LIMIT {
            budget.check()?;
            return Err(ExecutionError::RowCeiling);
        }
        Ok(true)
    }

    fn selectivity(&self, index: usize) -> u8 {
        let node = &self.nodes[index];
        node.inline
            .iter()
            .map(|(name, _)| *name)
            .chain(
                self.filters
                    .iter()
                    .filter(|filter| {
                        filter.node == index && filter.is_seed_equality()
                    })
                    .map(|filter| filter.property),
            )
            .map(|name| {
                if matches!(name, "name" | "qualified_name") {
                    3
                } else {
                    2
                }
            })
            .max()
            .unwrap_or(u8::from(node.label.is_some()))
    }

    fn matches_node(&self, index: usize, node: &Node) -> bool {
        let pattern = &self.nodes[index];
        if pattern
            .label
            .is_some_and(|label| label.as_bytes() != node.label)
            || !pattern.inline.iter().all(|&(name, expected)| {
                node.property_bytes(name).as_ref() == expected.as_bytes()
            })
        {
            return false;
        }
        // The classifier accepts single-binding predicates. Evaluate each
        // when its node is bound so rejected nodes never expand adjacency.
        for predicate in
            self.filters.iter().filter(|filter| filter.node == index)
        {
            let matches = predicate.matches(node);
            if matches == self.any_filter {
                return matches;
            }
        }
        !self.any_filter
    }
}

fn order_rows(
    rows: &mut Vec<Vec<Vec<u8>>>,
    keys: &[OrderKey],
    limit: usize,
    budget: &mut ExecutionBudget<'_>,
) -> Result<(), ExecutionError> {
    if rows.len() < 2 || limit == 0 {
        rows.truncate(limit);
        return Ok(());
    }
    let mut numeric = [false; ORDER_KEYS_MAX];
    for (index, key) in keys.iter().enumerate() {
        for row in rows.iter() {
            budget.step()?;
            let cell = c_string_bytes(&row[key.column]);
            if !cell.is_empty() {
                let digits = cell.strip_prefix(b"-").unwrap_or(cell);
                numeric[index] =
                    !digits.is_empty() && digits.iter().all(u8::is_ascii_digit);
                break;
            }
        }
    }

    let mut sortable = Vec::with_capacity(rows.len());
    for row in rows.drain(..) {
        budget.step()?;
        let mut numbers = [0; ORDER_KEYS_MAX];
        for (index, key) in keys.iter().enumerate() {
            if numeric[index] {
                numbers[index] =
                    native_sort_integer(c_string_bytes(&row[key.column]));
            }
        }
        sortable.push((row, numbers));
    }
    budget.check()?;
    // Stable ties preserve the scan's node and adjacency order.
    sortable.sort_by(|(left, lhs), (right, rhs)| {
        for (index, key) in keys.iter().enumerate() {
            let comparison = if numeric[index] {
                lhs[index].cmp(&rhs[index])
            } else {
                c_string_bytes(&left[key.column])
                    .cmp(c_string_bytes(&right[key.column]))
            };
            if comparison != Ordering::Equal {
                return if key.descending {
                    comparison.reverse()
                } else {
                    comparison
                };
            }
        }
        Ordering::Equal
    });
    budget.check()?;
    rows.extend(sortable.into_iter().take(limit).map(|(row, _)| row));
    Ok(())
}

fn c_string_bytes(cell: &[u8]) -> &[u8] {
    cell.split(|&byte| byte == 0).next().unwrap_or_default()
}

// C picks the numeric mode from the first nonempty cell. Later cells follow
// (int)strtol, including leading whitespace, prefixes and overflow saturation.
fn native_sort_integer(cell: &[u8]) -> i32 {
    let start = cell
        .iter()
        .position(|&byte| !matches!(byte, b' ' | b'\t'..=b'\r'))
        .unwrap_or(cell.len());
    let cell = &cell[start..];
    let negative = cell.starts_with(b"-");
    let digits = cell
        .strip_prefix(b"-")
        .or_else(|| cell.strip_prefix(b"+"))
        .unwrap_or(cell);
    let mut number: c_long = 0;
    for &digit in digits.iter().take_while(|digit| digit.is_ascii_digit()) {
        let digit = c_long::from(digit - b'0');
        number = number.saturating_mul(10);
        number = if negative {
            number.saturating_sub(digit)
        } else {
            number.saturating_add(digit)
        };
    }
    number as i32
}

struct Edge {
    kind: Vec<u8>,
    properties: Vec<u8>,
}

struct Adjacency<'a> {
    incoming: Statement<'a>,
    outgoing: Statement<'a>,
    node: Statement<'a>,
}

impl<'a> Adjacency<'a> {
    fn new(connection: &'a Connection, typed: bool) -> rusqlite::Result<Self> {
        // Match native prepared edge lookups, then fetch endpoints by id.
        // A join can change the SQLite query plan and therefore the row order.
        // Untyped traversal is limited to a single hop, so the SQL is fixed
        // for all hops in a query.
        let edges = "SELECT id, project, source_id, target_id, type, properties FROM edges";
        let filter = if typed { " AND type = ?2" } else { "" };
        Ok(Self {
            incoming: connection
                .prepare(&format!("{edges} WHERE target_id = ?1{filter}"))?,
            outgoing: connection
                .prepare(&format!("{edges} WHERE source_id = ?1{filter}"))?,
            node: connection.prepare(&format!(
                "SELECT {NODE_COLUMNS} FROM nodes n WHERE id = ?1"
            ))?,
        })
    }

    fn neighbors(
        &mut self,
        id: i64,
        hop: Hop<'_>,
        budget: &mut ExecutionBudget<'_>,
    ) -> Result<Vec<(Node, Edge)>, ExecutionError> {
        let (statement, neighbor_column) = if hop.inbound {
            (&mut self.incoming, 2)
        } else {
            (&mut self.outgoing, 3)
        };
        let mut edges = if let Some(kind) = hop.kind {
            statement.query(params![id, kind])?
        } else {
            statement.query([id])?
        };
        let mut neighbors = Vec::new();
        while let Some(edge) = edges.next()? {
            budget.step()?;
            let neighbor: i64 = edge.get(neighbor_column)?;
            let mut nodes = self.node.query([neighbor])?;
            if let Some(row) = nodes.next()? {
                neighbors.push((
                    Node::read(row, 0)?,
                    Edge {
                        kind: read_text_bytes(edge, 4)?.unwrap_or_default(),
                        properties: read_text_bytes(edge, 5)?
                            .unwrap_or_else(|| b"{}".to_vec()),
                    },
                ));
            }
        }
        Ok(neighbors)
    }
}

#[cfg(test)]
mod count_tests {
    use super::*;

    #[test]
    fn unordered_count_limit_keeps_only_first_keys_and_counts_late_matches() {
        let query =
            classify("MATCH (n) RETURN n.name, count(*) LIMIT 1").unwrap();
        let Some(CountRows::Grouped(mut counts)) = query.count_rows() else {
            panic!("expected grouped count");
        };
        for key in [
            b"alpha".as_slice(),
            b"discarded",
            b"discarded-again",
            b"alpha",
        ] {
            counts.record(key);
        }
        assert_eq!(counts.groups.len(), 1);
        assert_eq!(counts.groups.get(b"alpha".as_slice()), Some(&(0, 2)));
    }

    #[test]
    fn group_ceiling_fails_before_rendering() {
        let query = classify(
            "MATCH (n) RETURN n.name, count(*) AS total ORDER BY total",
        )
        .unwrap();
        let Some(CountRows::Grouped(mut counts)) = query.count_rows() else {
            panic!("expected grouped count");
        };
        for index in 0..=ENGINE_LIMIT {
            counts.record(&index.to_string().into_bytes());
        }
        assert_eq!(counts.groups.len(), ENGINE_LIMIT);
        let mut budget = ExecutionBudget::new(&Instant::now, &|| true);
        // Any rendering work would consume the next budget step.
        budget.remaining = 1;
        assert!(matches!(
            CountRows::Grouped(counts).rows(2, ENGINE_LIMIT, &mut budget),
            Err(ExecutionError::RowCeiling)
        ));
    }
}
