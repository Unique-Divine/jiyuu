//! Bounded Cypher evaluator for a verified subset of query patterns.

use std::borrow::Cow;
use std::cell::Cell;
use std::cmp::Ordering;
use std::collections::{HashMap, hash_map::RandomState};
use std::ffi::{CString, c_char, c_int, c_long};
use std::hash::BuildHasher;
#[cfg(test)]
use std::time::{Duration, Instant};

use rusqlite::types::ValueRef;
use rusqlite::{Connection, Row, Statement, params};

#[path = "cypher_linear/predicates.rs"]
mod predicates;

#[path = "cypher_linear/rows.rs"]
mod rows;

#[path = "cypher_linear/with_clause.rs"]
mod with_clause;

#[path = "cypher_linear/ordered_count.rs"]
mod ordered_count;

#[path = "cypher_linear/paths.rs"]
mod paths;

use ordered_count::OrderedCount;
use predicates::{Bindings, Expr};
use rows::Query;
use with_clause::{ReturnGroups, WithPlan, WithRows};

const ENGINE_LIMIT: usize = 100_000;
const ORDERED_COUNT_GROUP_LIMIT: usize = 16_384;
const ORDERED_COUNT_REPLAY_MAX_MS: u64 = 500;
const ORDERED_COUNT_FALLBACK_RESERVE_MS: u64 = 1_000;
const ORDERED_COUNT_REPLAY_MAX_STEPS: usize = 1_048_576;
#[cfg(test)]
const DEADLINE: Duration = Duration::from_secs(30);
const CHECK_INTERVAL: usize = 1024;
const PROPERTY_LIMIT: usize = 511;
const ORDER_KEYS_MAX: usize = 8;
const NATIVE_EDGE_PROPERTY_BUFFERS: usize = 8;
// CAST uses SQLite's integer conversion, as sqlite3_column_int does in C.
const NODE_COLUMNS: &str = "n.id, n.label, n.name, n.qualified_name, n.file_path, CAST(n.start_line AS INTEGER), CAST(n.end_line AS INTEGER), n.properties";

#[derive(Debug)]
pub(crate) struct Unsupported;

#[derive(Debug, PartialEq)]
pub(crate) enum ParseError {
    Unsupported,
    Depth,
    UnexpectedOperator(usize),
    UnsupportedProjectionFunction(String),
    UnsupportedListIndex,
    TrailingDelimiter {
        offset: usize,
        delimiter: char,
        with_hint: bool,
    },
    UnsupportedWhereFunction(String),
    ExpectedToken {
        expected: u8,
        actual: u8,
        offset: usize,
    },
    OutOfScope {
        name: String,
        clause: &'static str,
        reason: &'static str,
    },
    TrailingInput {
        offset: usize,
        word: String,
        with_hint: bool,
    },
    SecondWith {
        offset: usize,
        word: String,
    },
}

impl From<Unsupported> for ParseError {
    fn from(_: Unsupported) -> Self {
        Self::Unsupported
    }
}

impl ParseError {
    pub(crate) fn diagnostic(&self) -> Option<Cow<'static, str>> {
        match self {
            Self::Unsupported => None,
            Self::Depth => {
                Some("expression nested deeper than 256 levels".into())
            }
            Self::UnexpectedOperator(offset) => {
                Some(format!("unexpected operator at pos {offset}").into())
            }
            Self::UnsupportedProjectionFunction(name) => Some(format!(
                "unsupported function '{name}' (supported: count, sum, avg, min, max, collect, \
                 toLower, toUpper, toString, toInteger, toFloat, toBoolean, size, length, \
                 trim, ltrim, rtrim, reverse, labels, type, id, keys, properties)"
            ).into()),
            Self::UnsupportedListIndex => Some(
                "unsupported expression: list indexing/slicing '[...]' is not supported".into()
            ),
            Self::TrailingDelimiter { offset, delimiter, with_hint } => {
                let hint = if *with_hint {
                    " Note that only one WITH clause is supported."
                } else {
                    ""
                };
                Some(format!(
                    "unexpected input at pos {offset} ('{delimiter}') — the query was not fully parsed.{hint}"
                ).into())
            }
            Self::ExpectedToken { expected, actual, offset } => Some(format!(
                "expected token type {expected}, got {actual} at pos {offset}"
            ).into()),
            Self::UnsupportedWhereFunction(name) => Some(format!(
                "unsupported function '{name}' in WHERE (supported: coalesce, substring, replace, left, right)"
            ).into()),
            Self::OutOfScope { name, clause, reason } => Some(
                format!("variable '{name}' is not in scope for {clause} — {reason}").into()
            ),
            Self::TrailingInput { offset, word, with_hint } => {
                let hint = if *with_hint {
                    " Note that only one WITH clause is supported."
                } else {
                    ""
                };
                Some(format!(
                    "unexpected input at pos {offset} ('{word}') — the query was not fully parsed.{hint}"
                ).into())
            }
            Self::SecondWith { offset, word } => Some(format!(
                "unexpected input at pos {offset} ('{word}') — the query was not fully parsed. Note that only one WITH clause is supported."
            ).into()),
        }
    }
}

#[derive(Debug, PartialEq)]
enum Token<'a> {
    Word(&'a str),
    Literal(Cow<'a, [u8]>),
    Number(&'a str),
    Symbol(u8),
    DotDot,
    NotEqual,
    Regex,
    LessOrEqual,
    GreaterOrEqual,
}

struct LocatedToken<'a> {
    token: Token<'a>,
    offset: usize,
}

struct Tokens<'a> {
    items: std::iter::Peekable<std::vec::IntoIter<LocatedToken<'a>>>,
    end: usize,
    path_syntax: bool,
}

impl<'a> Tokens<'a> {
    fn new(source: &'a str) -> Result<Self, Unsupported> {
        let items = tokens(source)?;
        let path_syntax = paths::has_path_syntax(&items);
        Ok(Self {
            items: items.into_iter().peekable(),
            end: source.len(),
            path_syntax,
        })
    }

    fn peek(&mut self) -> Option<&Token<'a>> {
        self.items.peek().map(|item| &item.token)
    }

    fn next(&mut self) -> Option<Token<'a>> {
        self.items.next().map(|item| item.token)
    }

    fn next_if(
        &mut self,
        matches: impl FnOnce(&Token<'a>) -> bool,
    ) -> Option<Token<'a>> {
        self.items
            .next_if(|item| matches(&item.token))
            .map(|item| item.token)
    }

    fn offset(&mut self) -> usize {
        self.items.peek().map_or(self.end, |item| item.offset)
    }
}

fn tokens(source: &str) -> Result<Vec<LocatedToken<'_>>, Unsupported> {
    let mut query = source;
    let mut result = Vec::new();
    while !query.is_empty() {
        query = query.trim_start_matches(|c| matches!(c, ' ' | '\t'..='\r'));
        let Some(&first) = query.as_bytes().first() else {
            break;
        };
        if query.starts_with("//") || query.starts_with("--") {
            query = query.find('\n').map_or("", |end| &query[end..]);
            continue;
        }
        if let Some(comment) = query.strip_prefix("/*") {
            let (_, tail) = comment.split_once("*/").ok_or(Unsupported)?;
            query = tail;
            continue;
        }
        let (token, length) = match first {
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                let length = query
                    .bytes()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == b'_')
                    .count();
                (Token::Word(&query[..length]), length)
            }
            byte if byte.is_ascii_digit()
                || byte == b'.'
                    && query
                        .as_bytes()
                        .get(1)
                        .is_some_and(u8::is_ascii_digit) =>
            {
                let bytes = query.as_bytes();
                let length = bytes
                    .iter()
                    .enumerate()
                    .take_while(|&(index, byte)| {
                        byte.is_ascii_digit()
                            || *byte == b'.'
                                && bytes
                                    .get(index + 1)
                                    .is_some_and(|&next| next != b'.')
                    })
                    .count();
                (Token::Number(&query[..length]), length)
            }
            b'\'' | b'"' => {
                let (value, used) = string_literal(query.as_bytes(), first)?;
                (Token::Literal(value), used)
            }
            b'.' if query.starts_with("..") => (Token::DotDot, 2),
            b'=' if query.starts_with("=~") => (Token::Regex, 2),
            b'<' if query.starts_with("<=") => (Token::LessOrEqual, 2),
            b'>' if query.starts_with(">=") => (Token::GreaterOrEqual, 2),
            b'<' if query.starts_with("<>") => (Token::NotEqual, 2),
            b'!' if query.starts_with("!=") => (Token::NotEqual, 2),
            b'(' | b')' | b'[' | b']' | b'{' | b'}' | b':' | b'-' | b'<'
            | b'>' | b'.' | b'=' | b',' | b'*' | b'|' => {
                (Token::Symbol(first), 1)
            }
            _ => return Err(Unsupported),
        };
        result.push(LocatedToken {
            token,
            offset: source.len() - query.len(),
        });
        query = &query[length..];
    }
    Ok(result)
}

fn string_literal(
    input: &[u8],
    quote: u8,
) -> Result<(Cow<'_, [u8]>, usize), Unsupported> {
    // The native token buffer keeps the first 4095 decoded bytes.
    const MAX_LITERAL_BYTES: usize = 4095;
    let mut decoded = Vec::new();
    let mut start = 1;
    let mut pos = 1;
    while let Some(&byte) = input.get(pos) {
        match byte {
            byte if byte == quote => {
                let segment = &input[start..pos];
                if start == 1 {
                    let segment =
                        &segment[..segment.len().min(MAX_LITERAL_BYTES)];
                    return Ok((Cow::Borrowed(segment), pos + 1));
                }
                let remaining = MAX_LITERAL_BYTES.saturating_sub(decoded.len());
                decoded
                    .extend_from_slice(&segment[..segment.len().min(remaining)]);
                return Ok((Cow::Owned(decoded), pos + 1));
            }
            b'\\' => {
                let &escaped = input.get(pos + 1).ok_or(Unsupported)?;
                let segment = &input[start..pos];
                let remaining = MAX_LITERAL_BYTES.saturating_sub(decoded.len());
                decoded
                    .extend_from_slice(&segment[..segment.len().min(remaining)]);
                if decoded.len() < MAX_LITERAL_BYTES {
                    decoded.push(match escaped {
                        b'n' => b'\n',
                        b't' => b'\t',
                        other => other,
                    });
                }
                pos += 1;
                start = pos + 1;
            }
            _ => {}
        }
        pos += 1;
    }
    Err(Unsupported)
}

struct Parser<'a> {
    tokens: Tokens<'a>,
    names: HashMap<&'a str, Binding>,
    projected_fields: HashMap<String, usize>,
    missing_projection: Option<&'a str>,
    after_with: bool,
    row_syntax: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Binding {
    Node(usize),
    Edge(usize),
    Unbound,
}

#[derive(Debug)]
struct NodePattern<'a> {
    named: bool,
    labels: Vec<&'a str>,
    inline: Vec<(&'a str, Cow<'a, [u8]>)>,
}

#[derive(Clone, Copy, Debug)]
enum Direction {
    Inbound,
    Outbound,
    Either,
}

impl Direction {
    fn reversed(self) -> Self {
        match self {
            Self::Inbound => Self::Outbound,
            Self::Outbound => Self::Inbound,
            Self::Either => Self::Either,
        }
    }

    fn inbound_scans(self) -> &'static [bool] {
        match self {
            Self::Inbound => &[true],
            Self::Outbound => &[false],
            Self::Either => &[false, true],
        }
    }
}

#[derive(Debug)]
struct Hop<'a> {
    kinds: Vec<&'a str>,
    direction: Direction,
    range: Option<paths::HopRange>,
}

#[derive(Debug)]
enum ProjectionExpression<'a> {
    Field(Binding, Option<&'a str>),
    NodeLabels(usize),
    Id(Binding),
    Constant(&'static [u8]),
    Coalesce(predicates::Coalesce<'a>),
    Substring(predicates::Substring<'a>),
    Count(Option<Binding>),
    CountDistinct(Binding, Option<&'a str>),
    Collect(Binding, Option<&'a str>),
    StarField(usize, rows::StarField),
    NumericAggregate(with_clause::NumericAggregate, Binding, Option<&'a str>),
}

impl ProjectionExpression<'_> {
    fn is_aggregate(&self) -> bool {
        matches!(
            self,
            Self::Count(_)
                | Self::CountDistinct(..)
                | Self::Collect(..)
                | Self::NumericAggregate(..)
        )
    }

    fn field_value<'a>(
        &self,
        nodes: &[Option<&'a Node>],
        edges: &[Option<&'a Edge>],
        context: &ExpressionContext<'_>,
    ) -> Cow<'a, [u8]> {
        let node = |index: usize| nodes.get(index).copied().flatten();
        let edge = |index: usize| edges.get(index).copied().flatten();
        match *self {
            Self::Field(Binding::Node(index), None) => node(index)
                .map_or(Cow::Borrowed(b""), |node| Cow::Borrowed(&node.name)),
            Self::Field(Binding::Node(index), Some(name)) => node(index)
                .map_or(Cow::Borrowed(b""), |node| {
                    node.property_bytes(name, context)
                }),
            Self::Field(Binding::Edge(index), None) => edge(index)
                .map_or(Cow::Borrowed(b""), |edge| {
                    Cow::Borrowed(&edge.properties)
                }),
            Self::Field(Binding::Edge(index), Some(name)) => edge(index)
                .map_or(Cow::Borrowed(b""), |edge| {
                    edge.property_bytes(Some(name))
                }),
            Self::NodeLabels(index)
                if node(index).is_none_or(|node| node.id == 0) =>
            {
                Cow::Borrowed(b"[]")
            }
            Self::NodeLabels(index) => Cow::Owned(
                b"[\""
                    .iter()
                    .chain(&node(index).unwrap().label)
                    .chain(b"\"]")
                    .take(PROPERTY_LIMIT)
                    .copied()
                    .collect(),
            ),
            Self::Id(binding) => {
                let id = match binding {
                    Binding::Node(index) => node(index).map(|node| node.id),
                    Binding::Edge(index) => edge(index).map(|edge| edge.id),
                    Binding::Unbound => None,
                };
                id.map_or(Cow::Borrowed(b""), |id| {
                    Cow::Owned(id.to_string().into_bytes())
                })
            }
            Self::Field(Binding::Unbound, _) => Cow::Borrowed(b""),
            Self::Constant(bytes) => Cow::Borrowed(bytes),
            Self::Coalesce(ref call) => Cow::Owned(
                call.value(|binding, prop| {
                    Some(
                        ProjectionExpression::Field(binding, prop)
                            .field_value(nodes, edges, context),
                    )
                })
                .unwrap_or_default()
                .into_owned(),
            ),
            Self::Substring(ref call) => Cow::Owned(
                call.value(|binding, prop| {
                    Some(
                        ProjectionExpression::Field(binding, prop)
                            .field_value(nodes, edges, context),
                    )
                })
                .unwrap_or_default(),
            ),
            Self::StarField(index, field) => node(index)
                .map_or(Cow::Borrowed(b""), |node| {
                    Cow::Borrowed(field.value(node))
                }),
            Self::Count(_)
            | Self::CountDistinct(..)
            | Self::Collect(..)
            | Self::NumericAggregate(..) => {
                unreachable!("aggregate has no per-binding value")
            }
        }
    }
}

#[derive(Debug)]
struct Projection<'a> {
    expression: ProjectionExpression<'a>,
    name: String,
    source_name: String,
    scope_name: Option<&'a str>,
}

#[derive(Debug)]
struct OrderKey {
    column: usize,
    descending: bool,
}

#[derive(Debug)]
struct MatchQuery<'a> {
    nodes: Vec<NodePattern<'a>>,
    hops: Vec<Hop<'a>>,
    paths: Option<paths::Plan<'a>>,
    filter: Option<Expr<'a>>,
    with: Option<WithPlan<'a>>,
    projections: Vec<Projection<'a>>,
    return_star: bool,
    distinct: bool,
    order: Vec<OrderKey>,
    ordered: bool,
    skip: usize,
    limit: Option<usize>,
}

impl<'a> Parser<'a> {
    fn keyword(&mut self, expected: &str) -> bool {
        self.tokens.next_if(|token| matches!(token, Token::Word(word) if word.eq_ignore_ascii_case(expected))).is_some()
    }

    fn consume(&mut self, symbol: u8) -> bool {
        self.tokens
            .next_if(|token| *token == Token::Symbol(symbol))
            .is_some()
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

    fn literal(&mut self) -> Result<Cow<'a, [u8]>, Unsupported> {
        match self.tokens.next() {
            Some(Token::Literal(value)) => Ok(value),
            _ => Err(Unsupported),
        }
    }

    fn expect_pattern_symbol(&mut self, symbol: u8) -> Result<(), ParseError> {
        if self.consume(symbol) {
            return Ok(());
        }
        Err(
            self.expected_pattern_token(native_token_id(Some(&Token::Symbol(
                symbol,
            )))),
        )
    }

    fn expected_pattern_token(&mut self, expected: u8) -> ParseError {
        ParseError::ExpectedToken {
            expected,
            actual: native_token_id(self.tokens.peek()),
            offset: self.tokens.offset(),
        }
    }

    fn pattern_name(&mut self) -> Result<&'a str, ParseError> {
        if native_token_id(self.tokens.peek()) != NATIVE_IDENT {
            return Err(self.expected_pattern_token(NATIVE_IDENT));
        }
        Ok(self.name()?)
    }

    fn alias(&mut self, target: Binding) -> Result<bool, ParseError> {
        if native_token_id(self.tokens.peek()) != NATIVE_IDENT {
            return Ok(false);
        }
        let name = self.name()?;
        if self.names.insert(name, target).is_some() {
            return Err(Unsupported.into());
        }
        Ok(true)
    }

    fn node(
        &mut self,
        index: usize,
    ) -> Result<(NodePattern<'a>, usize), ParseError> {
        self.expect_pattern_symbol(b'(')?;
        let named = native_token_id(self.tokens.peek()) == NATIVE_IDENT;
        let mut slot = index;
        if named {
            let name = self.name()?;
            if !self.tokens.path_syntax && self.names.contains_key(name) {
                return Err(Unsupported.into());
            }
            slot = match self.names.entry(name).or_insert(Binding::Node(index)) {
                Binding::Node(slot) => *slot,
                _ => return Err(Unsupported.into()),
            };
        }
        Ok((self.node_constraints(named)?, slot))
    }

    fn node_constraints(
        &mut self,
        named: bool,
    ) -> Result<NodePattern<'a>, ParseError> {
        let mut labels = Vec::new();
        if self.consume(b':') {
            let mut used = 0;
            loop {
                let label = self.pattern_name()?;
                used += label.len() + usize::from(!labels.is_empty());
                // The native label buffer truncates at 255 bytes.
                if used >= 256 {
                    return Err(Unsupported.into());
                }
                labels.push(label);
                if !self.consume(b'|') {
                    break;
                }
            }
        }
        let mut inline = Vec::new();
        if self.consume(b'{') {
            loop {
                let property = self.name()?;
                self.symbol(b':')?;
                let value = self.literal()?;
                inline.push((property, value));
                if !self.consume(b',') {
                    break;
                }
            }
            self.symbol(b'}')?;
        }
        self.expect_pattern_symbol(b')')?;
        Ok(NodePattern {
            named,
            labels,
            inline,
        })
    }

    fn hop(&mut self, index: usize) -> Result<Hop<'a>, ParseError> {
        let inbound = self.consume(b'<');
        self.expect_pattern_symbol(b'-')?;
        let mut kinds = Vec::new();
        let mut range = None;
        if self.consume(b'[') {
            self.alias(Binding::Edge(index))?;
            if self.consume(b':') {
                kinds.push(self.pattern_name()?);
                while self.consume(b'|') {
                    kinds.push(self.pattern_name()?);
                }
            }
            if self.consume(b'*') {
                range = Some(paths::HopRange::parse(self)?);
            }
            self.expect_pattern_symbol(b']')?;
        }
        self.expect_pattern_symbol(b'-')?;
        let direction = match (inbound, self.consume(b'>')) {
            (true, false) => Direction::Inbound,
            (false, true) => Direction::Outbound,
            _ => Direction::Either,
        };
        Ok(Hop {
            kinds,
            direction,
            range,
        })
    }

    fn binding(&self, name: &str) -> Result<Binding, Unsupported> {
        self.names.get(name).copied().ok_or(Unsupported)
    }

    fn projection_binding(&mut self, name: &'a str) -> Binding {
        if let Ok(binding) = self.binding(name) {
            return binding;
        }
        // Validate scope after parsing the whole statement, as native does.
        self.missing_projection.get_or_insert(name);
        Binding::Unbound
    }

    fn trailing_delimiter(&mut self) -> Option<ParseError> {
        let offset = self.tokens.offset();
        let Token::Symbol(delimiter @ (b'(' | b'[')) = self.tokens.peek()?
        else {
            return None;
        };
        let delimiter = char::from(*delimiter);
        let mut starts = false;
        let mut with_hint = false;
        while let Some(token) = self.tokens.next() {
            if matches!(token, Token::Word(word) if word.eq_ignore_ascii_case("WITH"))
                && !starts
            {
                with_hint = true;
                break;
            }
            starts = matches!(token, Token::Word(word) if word.eq_ignore_ascii_case("STARTS"));
        }
        Some(ParseError::TrailingDelimiter {
            offset,
            delimiter,
            with_hint,
        })
    }

    fn second_with(&mut self) -> Option<ParseError> {
        let offset = self.tokens.offset();
        if let Some(Token::Word(word)) = self.tokens.peek()
            && word.eq_ignore_ascii_case("WITH")
        {
            return Some(ParseError::SecondWith {
                offset,
                word: (*word).to_owned(),
            });
        }
        None
    }

    fn named_projection(&mut self) -> Result<Projection<'a>, ParseError> {
        let name = self.name()?;
        if self.consume(b'(') {
            if !native_named_function(name) {
                return Err(ParseError::UnsupportedProjectionFunction(
                    name.to_owned(),
                ));
            }
            if name.eq_ignore_ascii_case("coalesce") {
                let call = self.coalesce(true)?;
                let (var, property) =
                    call.first_reference().ok_or(Unsupported)?;
                return Ok(Projection {
                    expression: ProjectionExpression::Coalesce(call),
                    name: format!("coalesce({var})"),
                    source_name: projection_name(var, property),
                    scope_name: None,
                });
            }
            if name.eq_ignore_ascii_case("substring") {
                let call = self.substring(true)?;
                let reference = call.first_reference();
                if reference.is_none() && !self.tokens.peek().is_some_and(
                    |token| matches!(token, Token::Word(word) if word.eq_ignore_ascii_case("AS"))
                ) {
                    // The native column name formats a null C pointer.
                    return Err(Unsupported.into());
                }
                let (var, property) = reference.unwrap_or(("", None));
                return Ok(Projection {
                    expression: ProjectionExpression::Substring(call),
                    name: format!("substring({var})"),
                    source_name: projection_name(var, property),
                    scope_name: None,
                });
            }
            let target = self.name()?;
            let binding = self.projection_binding(target);
            self.symbol(b')')?;
            let (expression, canonical) = match binding {
                Binding::Edge(_) if name.eq_ignore_ascii_case("type") => {
                    (ProjectionExpression::Field(binding, Some("type")), "type")
                }
                Binding::Node(index) if name.eq_ignore_ascii_case("labels") => {
                    (ProjectionExpression::NodeLabels(index), "labels")
                }
                Binding::Node(_) if name.eq_ignore_ascii_case("type") => {
                    (ProjectionExpression::Constant(b""), "type")
                }
                Binding::Edge(_) if name.eq_ignore_ascii_case("labels") => {
                    (ProjectionExpression::Constant(b"[]"), "labels")
                }
                _ if name.eq_ignore_ascii_case("id") => {
                    (ProjectionExpression::Id(binding), "id")
                }
                Binding::Unbound if name.eq_ignore_ascii_case("labels") => {
                    (ProjectionExpression::Constant(b"[]"), "labels")
                }
                Binding::Unbound if name.eq_ignore_ascii_case("type") => {
                    (ProjectionExpression::Constant(b""), "type")
                }
                _ => return Err(Unsupported.into()),
            };
            return Ok(Projection {
                expression,
                name: format!("{canonical}({target})"),
                source_name: target.to_owned(),
                scope_name: None,
            });
        }
        let binding = self.projection_binding(name);
        let property = self.consume(b'.').then(|| self.name()).transpose()?;
        if self.tokens.peek() == Some(&Token::Symbol(b'[')) {
            return Err(ParseError::UnsupportedListIndex);
        }
        let source_name = projection_name(name, property);
        Ok(Projection {
            expression: ProjectionExpression::Field(binding, property),
            name: source_name.clone(),
            source_name,
            scope_name: property.is_none().then_some(name),
        })
    }

    fn order_name(&mut self) -> Result<String, Unsupported> {
        let aggregate = if self.keyword("COUNT") {
            Some("COUNT")
        } else if self.keyword("COLLECT") {
            self.consume(b'(');
            let name = if self.consume(b'*') {
                "*"
            } else if native_token_id(self.tokens.peek()) == NATIVE_IDENT {
                self.name()?
            } else {
                ""
            };
            self.consume(b')');
            return Ok(format!("COLLECT({name})"));
        } else {
            with_clause::NumericAggregate::parse(self)
                .map(with_clause::NumericAggregate::name)
        };
        if let Some(aggregate) = aggregate {
            self.symbol(b'(')?;
            let name = if self.consume(b'*') {
                "*"
            } else {
                self.name()?
            };
            self.symbol(b')')?;
            Ok(format!("{aggregate}({name})"))
        } else {
            let name = self.name()?;
            let property =
                self.consume(b'.').then(|| self.name()).transpose()?;
            Ok(projection_name(name, property))
        }
    }

    fn projection(&mut self) -> Result<Projection<'a>, ParseError> {
        let collect = self.keyword("COLLECT");
        let aggregate = if collect {
            None
        } else if self.keyword("COUNT") {
            Some(None)
        } else {
            with_clause::NumericAggregate::parse(self).map(Some)
        };
        let mut projection = if collect {
            self.collect_projection()?
        } else if let Some(aggregate) = aggregate {
            self.symbol(b'(')?;
            let unique = self.keyword("DISTINCT");
            let (name, argument) = if self.consume(b'*') {
                ("*", None)
            } else {
                let name = self.name()?;
                let binding = self.projection_binding(name);
                let property =
                    self.consume(b'.').then(|| self.name()).transpose()?;
                (name, Some((binding, property)))
            };
            self.symbol(b')')?;
            let expression = match (aggregate, unique, argument) {
                (None, false, argument) => ProjectionExpression::Count(
                    argument.map(|(binding, _)| binding),
                ),
                (None, true, Some((binding, prop))) => {
                    ProjectionExpression::CountDistinct(binding, prop)
                }
                (Some(aggregate), false, Some((binding, prop))) => {
                    ProjectionExpression::NumericAggregate(
                        aggregate, binding, prop,
                    )
                }
                _ => return Err(Unsupported.into()),
            };
            let function =
                aggregate.map_or("COUNT", with_clause::NumericAggregate::name);
            let property = argument.and_then(|(_, property)| property);
            Projection {
                expression,
                name: format!("{function}({name})"),
                source_name: projection_name(name, property),
                scope_name: None,
            }
        } else {
            self.named_projection()?
        };
        if self.keyword("AS") {
            let name = self.name()?;
            projection.name = name.to_owned();
            projection.source_name = name.to_owned();
            projection.scope_name = Some(name);
        }
        Ok(projection)
    }

    fn projections(
        &mut self,
        width: usize,
    ) -> Result<Vec<Projection<'a>>, ParseError> {
        let mut projections = vec![self.projection()?];
        while self.consume(b',') {
            if projections.len() == width {
                return Err(Unsupported.into());
            }
            projections.push(self.projection()?);
        }
        Ok(projections)
    }

    fn unsigned(&mut self, keyword: &str) -> Result<Option<usize>, Unsupported> {
        if !self.keyword(keyword) {
            return Ok(None);
        }
        self.number().map(Some)
    }

    fn return_unsigned(
        &mut self,
        keyword: &str,
    ) -> Result<Option<usize>, ParseError> {
        if !self.keyword(keyword) {
            return Ok(None);
        }
        if !matches!(self.tokens.peek(), Some(Token::Number(_))) {
            return Err(self.expected_pattern_token(87));
        }
        Ok(Some(self.number()?))
    }

    fn number(&mut self) -> Result<usize, Unsupported> {
        let Some(Token::Number(number)) = self.tokens.next() else {
            return Err(Unsupported);
        };
        let integer = number.split('.').next().unwrap_or_default();
        if integer.is_empty() {
            return Ok(0);
        }
        integer
            .parse::<i32>()
            .map(|value| value as usize)
            .map_err(|_| Unsupported)
    }
}

// These IDs occur in native parser diagnostics. Keep the frozen lexer enum
// order here while the C and Rust tool replies have to compare byte for byte.
const NATIVE_KEYWORDS: [&str; 66] = [
    "MATCH",
    "WHERE",
    "RETURN",
    "ORDER",
    "BY",
    "LIMIT",
    "AND",
    "OR",
    "AS",
    "DISTINCT",
    "COUNT",
    "CONTAINS",
    "STARTS",
    "WITH",
    "NOT",
    "ASC",
    "DESC",
    "<>",
    "ENDS",
    "IN",
    "IS",
    "NULL",
    "XOR",
    "SKIP",
    "UNION",
    "UNWIND",
    "SUM",
    "AVG",
    "MIN",
    "MAX",
    "COLLECT",
    "TOLOWER",
    "TOUPPER",
    "TOSTRING",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "CREATE",
    "DELETE",
    "DETACH",
    "SET",
    "REMOVE",
    "MERGE",
    "OPTIONAL",
    "YIELD",
    "CALL",
    "ALL",
    "TRUE",
    "FALSE",
    "EXISTS",
    "MANDATORY",
    "FOREACH",
    "ON",
    "ADD",
    "CONSTRAINT",
    "DO",
    "DROP",
    "FOR",
    "FROM",
    "GRAPH",
    "OF",
    "REQUIRE",
    "SCALAR",
    "UNIQUE",
];
const NATIVE_IDENT: u8 = 85;

fn native_token_id(token: Option<&Token<'_>>) -> u8 {
    match token {
        Some(Token::Word(word)) => NATIVE_KEYWORDS
            .iter()
            .position(|keyword| word.eq_ignore_ascii_case(keyword))
            .map_or(NATIVE_IDENT, |index| index as u8),
        Some(Token::Symbol(symbol)) => match symbol {
            b'|' => 83,
            // The first fourteen punctuation IDs are consecutive in C.
            _ => b"()[]-><:.{}*,="
                .iter()
                .position(|byte| symbol == byte)
                .map_or(88, |index| 66 + index as u8),
        },
        Some(Token::DotDot) => 84,
        Some(Token::NotEqual) => 17,
        Some(Token::Regex) => 80,
        Some(Token::GreaterOrEqual) => 81,
        Some(Token::LessOrEqual) => 82,
        Some(Token::Literal(_)) => 86,
        Some(Token::Number(_)) => 87,
        None => 88,
    }
}

fn valid_name(name: &str) -> bool {
    // Skip reserved lexer keywords and native's generated anonymous aliases.
    name.len() < 64
        && !name.starts_with('_')
        && native_token_id(Some(&Token::Word(name))) == NATIVE_IDENT
}

pub(crate) fn classify(source: &str) -> Result<Query<'_>, ParseError> {
    rows::parse(source)
}

fn match_query<'a>(
    parser: &mut Parser<'a>,
    optional: bool,
) -> Result<(MatchQuery<'a>, Option<ParseError>), ParseError> {
    let (first, first_slot) = parser.node(0)?;
    let mut nodes = vec![first];
    let mut slots = vec![first_slot];
    let mut hops = Vec::new();
    let hop_limit = if parser.tokens.path_syntax {
        paths::EDGE_SLOTS
    } else {
        2
    };
    while hops.len() < hop_limit
        && matches!(parser.tokens.peek(), Some(Token::Symbol(b'-' | b'<')))
    {
        hops.push(parser.hop(hops.len())?);
        let (node, slot) = parser.node(nodes.len())?;
        nodes.push(node);
        slots.push(slot);
    }
    let mut paths = paths::Plan::new(optional, &nodes, &hops, slots);
    paths.parse_clauses(parser)?;
    let filter = parser
        .keyword("WHERE")
        .then(|| parser.where_expr())
        .transpose()?;
    paths.parse_clauses(parser)?;
    let paths = paths.into_optional()?;
    let with = parser
        .keyword("WITH")
        .then(|| WithPlan::parse(parser))
        .transpose()?;
    let missing_with = parser.missing_projection.take();
    if !parser.keyword("RETURN") {
        return Err(parser.second_with().unwrap_or(ParseError::Unsupported));
    }
    let distinct = parser.keyword("DISTINCT");
    let return_star = parser.consume(b'*');
    parser.row_syntax |= return_star;
    let projections = match &with {
        Some(with) if return_star => with.return_star(),
        None if return_star => rows::star_projections(&parser.names),
        _ => parser.projections(32)?,
    };
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
        return Err(ParseError::Unsupported);
    }
    let mut order = Vec::new();
    let ordered = parser.keyword("ORDER");
    if ordered {
        if !parser.keyword("BY") {
            return Err(ParseError::Unsupported);
        }
        for index in 0..ORDER_KEYS_MAX {
            let name = parser.order_name()?;
            let column = projections.iter().position(|p| p.name == name);
            let descending = parser.keyword("DESC");
            if !descending {
                parser.keyword("ASC");
            }
            if let Some(column) = column {
                order.push(OrderKey { column, descending });
            }
            if !parser.consume(b',') {
                break;
            }
            if index + 1 == ORDER_KEYS_MAX {
                return Err(ParseError::Unsupported);
            }
        }
    }
    let skip = parser.return_unsigned("SKIP")?.unwrap_or_default();
    let limit = parser.return_unsigned("LIMIT")?;
    if with.is_none() && limit.is_some_and(|n| n > ENGINE_LIMIT) {
        return Err(ParseError::Unsupported);
    }
    let scope_error = if let Some(name) = missing_with {
        Some(ParseError::OutOfScope {
            name: name.to_owned(),
            clause: "WITH",
            reason: "no pattern in this query names it",
        })
    } else {
        parser
            .missing_projection
            .map(|name| ParseError::OutOfScope {
                name: name.to_owned(),
                clause: "RETURN",
                reason: if with.is_some() {
                    "the WITH clause did not carry it through"
                } else {
                    "no pattern in this query names it"
                },
            })
    };
    Ok((
        MatchQuery {
            nodes,
            hops,
            paths,
            filter,
            with,
            projections,
            return_star,
            distinct,
            order,
            ordered,
            skip,
            limit,
        },
        scope_error,
    ))
}

fn native_named_function(name: &str) -> bool {
    // C routes these identifier calls to a native scalar parser. Some still
    // use the C evaluator, while names outside the list are syntax errors.
    [
        "labels",
        "type",
        "id",
        "keys",
        "properties",
        "toInteger",
        "toFloat",
        "toBoolean",
        "size",
        "length",
        "trim",
        "ltrim",
        "rtrim",
        "reverse",
        "coalesce",
        "substring",
        "replace",
        "left",
        "right",
    ]
    .iter()
    .any(|function| function.eq_ignore_ascii_case(name))
}

fn projection_name(name: &str, property: Option<&str>) -> String {
    property
        .map_or_else(|| name.to_owned(), |property| format!("{name}.{property}"))
}

const REGEX_WARNING: &str = "a =~ or property regex pattern too large to compile: reduce nested {m,n} repetition or alternation — the comparison matched nothing";

struct ExpressionContext<'a> {
    connection: &'a Connection,
    regex_refused: Cell<bool>,
}

impl ExpressionContext<'_> {
    fn degree(&self, id: i64, property: &str) -> i32 {
        let sql = if property == "in_degree" {
            "SELECT COUNT(*) FROM edges WHERE target_id = ?1 AND type = 'CALLS'"
        } else {
            "SELECT COUNT(*) FROM edges WHERE source_id = ?1 AND type = 'CALLS'"
        };
        // The native degree helper defaults to zero when SQLite cannot read it
        // and converts COUNT with sqlite3_column_int.
        self.connection
            .query_row(sql, [id], |row| row.get::<_, i64>(0))
            .unwrap_or_default() as i32
    }

    fn regex_matches(
        &self,
        actual: &[u8],
        pattern: &[u8],
        inline: bool,
    ) -> bool {
        unsafe extern "C" {
            fn cbm_regex_match_extended(
                actual: *const c_char,
                pattern: *const c_char,
            ) -> c_int;
        }
        let actual =
            CString::new(c_string_bytes(actual)).expect("NUL-trimmed subject");
        let pattern =
            CString::new(c_string_bytes(pattern)).expect("NUL-trimmed pattern");
        // SAFETY: The wrapper only reads the two NUL-terminated byte strings.
        match unsafe {
            cbm_regex_match_extended(actual.as_ptr(), pattern.as_ptr())
        } {
            1 => true,
            0 => false,
            status => {
                // Inline filters compare literally after any compile failure.
                let equal = inline && actual == pattern;
                if status == -2 && !equal {
                    self.regex_refused.set(true);
                }
                equal
            }
        }
    }
}

fn looks_like_regex(value: &[u8]) -> bool {
    value.windows(2).any(|pair| matches!(pair, b".*" | b".+"))
        || value.iter().any(|byte| b"[(|^$".contains(byte))
}

#[derive(Clone, Debug, Default)]
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

    fn property_bytes(
        &self,
        name: &str,
        context: &ExpressionContext<'_>,
    ) -> Cow<'_, [u8]> {
        let value: &[u8] = match name {
            "label" => &self.label,
            "name" => &self.name,
            "qualified_name" | "qn" => &self.qualified_name,
            "file_path" | "file" | "path" => &self.file_path,
            "start_line" => {
                return self.start_line.to_string().into_bytes().into();
            }
            "end_line" => return self.end_line.to_string().into_bytes().into(),
            "in_degree" | "out_degree" => {
                return context
                    .degree(self.id, name)
                    .to_string()
                    .into_bytes()
                    .into();
            }
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
    warnings: Warnings,
}

#[derive(Default, Debug, PartialEq)]
struct Warnings {
    depth_cap: Option<u32>,
    traversal_exhausted: bool,
    regex_refused: bool,
}

impl Warnings {
    fn merge(&mut self, other: Self) {
        self.depth_cap = other.depth_cap.or(self.depth_cap);
        self.traversal_exhausted |= other.traversal_exhausted;
        self.regex_refused |= other.regex_refused;
    }
}

impl ResultRows {
    pub(crate) fn warning(&self) -> Option<String> {
        let mut warning = match (
            self.warnings.depth_cap,
            self.warnings.traversal_exhausted,
        ) {
            (Some(cap), true) => format!(
                "variable-length hop range clamped to the engine ceiling ({cap}) and traversal budget was exhausted — results may be partial"
            ),
            (Some(cap), false) => format!(
                "variable-length hop range clamped to the engine ceiling ({cap}) — an empty result may mean \"clamped\", not \"no such path\""
            ),
            (None, true) => String::from(
                "variable-length traversal budget was exhausted — results may be partial",
            ),
            (None, false) => String::new(),
        };
        if self.warnings.regex_refused {
            if !warning.is_empty() {
                warning.push_str("; ");
            }
            warning.push_str(REGEX_WARNING);
        }
        (!warning.is_empty()).then_some(warning)
    }
}

#[derive(Default)]
struct DistinctRows {
    hash_builder: RandomState,
    last_by_hash: HashMap<u64, usize>,
    previous_by_row: Vec<Option<usize>>,
    cap: usize,
    engine_cap: bool,
    done: bool,
}

impl DistinctRows {
    fn admit(&mut self, mut row: Vec<Vec<u8>>, result: &mut ResultRows) {
        for cell in &mut row {
            cell.truncate(c_string_bytes(cell).len());
        }
        // Keep each admitted row once. The hash covers cell lengths, and a
        // collision is resolved against the original result rows.
        let hash = self.hash_builder.hash_one(&row);
        let previous = self.last_by_hash.get(&hash).copied();
        let mut index = previous;
        while let Some(row_index) = index {
            if result.rows[row_index] == row {
                return;
            }
            index = self.previous_by_row[row_index];
        }
        if result.rows.len() == self.cap {
            // A duplicate past the cap is not evidence of truncation.
            result.truncated |= self.engine_cap;
            self.done = true;
            return;
        }
        self.last_by_hash.insert(hash, result.rows.len());
        self.previous_by_row.push(previous);
        result.rows.push(row);
    }
}

enum RowCollector {
    With(WithRows),
    All,
    Count(CountRows),
    OrderedCount(OrderedCount),
    Aggregate(ReturnGroups),
    Distinct(DistinctRows),
}

struct CountGroups {
    column: usize,
    max_groups: usize,
    // Each map value records the group's first-seen position and count.
    groups: HashMap<Vec<u8>, (usize, usize)>,
}

impl CountGroups {
    fn record(&mut self, key: &[u8]) {
        let key = count_group_key(key);
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
    Unsupported,
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

enum BudgetCheck<'a> {
    #[cfg(test)]
    Local {
        deadline: Instant,
        now: &'a dyn Fn() -> Instant,
        cancelled: &'a dyn Fn() -> bool,
    },
    #[cfg(feature = "c-backend")]
    Shared {
        status: &'a dyn Fn() -> Result<(), ExecutionError>,
        remaining_ms: &'a dyn Fn() -> u64,
    },
}

struct OrderedCountReplay {
    stop_at_remaining_ms: u64,
    remaining_steps: usize,
}

pub(crate) struct ExecutionBudget<'a> {
    check: BudgetCheck<'a>,
    remaining: usize,
    ordered_count_group_limit: usize,
    ordered_count_replay: Option<OrderedCountReplay>,
    pub(crate) path_depth_cap: u32,
}

impl<'a> ExecutionBudget<'a> {
    #[cfg(test)]
    pub(crate) fn new(
        now: &'a dyn Fn() -> Instant,
        cancelled: &'a dyn Fn() -> bool,
    ) -> Self {
        Self::with_check(BudgetCheck::Local {
            deadline: now() + DEADLINE,
            now,
            cancelled,
        })
    }

    #[cfg(feature = "c-backend")]
    pub(crate) fn shared(
        check: &'a dyn Fn() -> Result<(), ExecutionError>,
        remaining_ms: &'a dyn Fn() -> u64,
    ) -> Self {
        Self::with_check(BudgetCheck::Shared {
            status: check,
            remaining_ms,
        })
    }

    fn with_check(check: BudgetCheck<'a>) -> Self {
        Self {
            check,
            remaining: CHECK_INTERVAL,
            ordered_count_group_limit: ORDERED_COUNT_GROUP_LIMIT,
            ordered_count_replay: None,
            path_depth_cap: 10,
        }
    }

    #[cfg(debug_assertions)]
    pub(crate) fn set_ordered_count_group_limit_for_tests(
        &mut self,
        limit: usize,
    ) {
        self.ordered_count_group_limit = limit.min(ORDERED_COUNT_GROUP_LIMIT);
    }

    pub(crate) fn check(&self) -> Result<(), ExecutionError> {
        match self.check {
            #[cfg(test)]
            BudgetCheck::Local {
                deadline,
                now,
                cancelled,
            } => {
                if cancelled() {
                    return Err(ExecutionError::Cancelled);
                }
                if now() >= deadline {
                    return Err(ExecutionError::Deadline);
                }
            }
            #[cfg(feature = "c-backend")]
            BudgetCheck::Shared { status, .. } => status()?,
        }
        if let Some(replay) = &self.ordered_count_replay
            && (replay.remaining_steps == 0
                || self.remaining_ms() <= replay.stop_at_remaining_ms)
        {
            return Err(ExecutionError::Unsupported);
        }
        Ok(())
    }

    fn remaining_ms(&self) -> u64 {
        match self.check {
            #[cfg(test)]
            BudgetCheck::Local { deadline, now, .. } => {
                deadline.saturating_duration_since(now()).as_millis() as u64
            }
            #[cfg(feature = "c-backend")]
            BudgetCheck::Shared { remaining_ms, .. } => remaining_ms(),
        }
    }

    fn begin_ordered_count_replay(&mut self) -> Result<(), ExecutionError> {
        if self.ordered_count_replay.is_some() {
            return Ok(());
        }
        self.check()?;
        let remaining = self.remaining_ms();
        // Count time and work from the first overflow, including the rest of
        // that scan and all later batches. Leave time in the same deadline
        // for C to evaluate the query if Rust gives up.
        let allowance = ORDERED_COUNT_REPLAY_MAX_MS
            .min(remaining / 2)
            .min(remaining.saturating_sub(ORDERED_COUNT_FALLBACK_RESERVE_MS));
        if allowance == 0 {
            return Err(ExecutionError::Unsupported);
        }
        self.ordered_count_replay = Some(OrderedCountReplay {
            stop_at_remaining_ms: remaining - allowance,
            remaining_steps: ORDERED_COUNT_REPLAY_MAX_STEPS,
        });
        Ok(())
    }

    fn step(&mut self) -> Result<(), ExecutionError> {
        if let Some(replay) = &mut self.ordered_count_replay {
            replay.remaining_steps = replay.remaining_steps.saturating_sub(1);
            if replay.remaining_steps == 0 {
                return self.check();
            }
        }
        self.remaining -= 1;
        if self.remaining == 0 {
            self.remaining = CHECK_INTERVAL;
            self.check()?;
        }
        Ok(())
    }
}

impl MatchQuery<'_> {
    fn execute(
        &self,
        connection: &Connection,
        project: &str,
        standalone: bool,
        budget: &mut ExecutionBudget<'_>,
    ) -> Result<ResultRows, ExecutionError> {
        let context = ExpressionContext {
            connection,
            regex_refused: Cell::new(false),
        };
        let limit = self.limit.unwrap_or(ENGINE_LIMIT);
        let mut collector = if self.with.is_some() {
            RowCollector::With(WithRows::default())
        // A UNION needs each branch's ordered rows and truncation status
        // before it can apply the final result ceiling.
        } else if standalone
            && let Some(counts) =
                self.count_collector(budget.ordered_count_group_limit)
        {
            counts
        } else if self
            .projections
            .iter()
            .any(|item| item.expression.is_aggregate())
        {
            RowCollector::Aggregate(ReturnGroups::default())
        } else if self.distinct {
            let engine_cap =
                self.limit.is_none() || !self.can_limit_during_scan();
            RowCollector::Distinct(DistinctRows {
                cap: if engine_cap { ENGINE_LIMIT } else { limit },
                engine_cap,
                ..DistinctRows::default()
            })
        } else {
            RowCollector::All
        };
        let mut result = ResultRows {
            columns: self
                .projections
                .iter()
                .map(|column| column.name.clone())
                .collect(),
            rows: Vec::new(),
            truncated: false,
            warnings: Warnings::default(),
        };
        // MCP already lends us a read-batched handle. Direct callers may
        // arrive in autocommit mode, so pin their snapshot across scans too.
        let _snapshot = (matches!(collector, RowCollector::OrderedCount(_))
            && connection.is_autocommit())
        .then(|| connection.unchecked_transaction())
        .transpose()?;
        loop {
            self.scan(
                project,
                standalone,
                &context,
                &mut collector,
                &mut result,
                budget,
            )?;
            let RowCollector::OrderedCount(counts) = &mut collector else {
                break;
            };
            if let Some(rows) =
                counts.finish_batch(&self.order, result.columns.len(), budget)?
            {
                result.rows = rows;
                result.warnings.regex_refused |= context.regex_refused.get();
                budget.check()?;
                return Ok(result);
            }
        }
        match collector {
            RowCollector::With(rows) => {
                return self
                    .with
                    .as_ref()
                    .unwrap()
                    .finish(self, rows, result, &context, budget);
            }
            RowCollector::Count(counts) => {
                result.rows =
                    counts.rows(result.columns.len(), limit, budget)?;
            }
            RowCollector::Aggregate(groups) => {
                groups.emit(self, &mut result, budget)?
            }
            RowCollector::OrderedCount(_) => unreachable!(),
            RowCollector::All | RowCollector::Distinct(_) => {}
        }
        result.warnings.regex_refused |= context.regex_refused.get();
        self.finish_rows(result, budget)
    }

    fn scan(
        &self,
        project: &str,
        standalone: bool,
        context: &ExpressionContext<'_>,
        collector: &mut RowCollector,
        result: &mut ResultRows,
        budget: &mut ExecutionBudget<'_>,
    ) -> Result<(), ExecutionError> {
        budget.check()?;
        let connection = context.connection;
        let reversed = self.paths.as_ref().is_none_or(paths::Plan::can_reverse)
            && self.hops.len() == 1
            && self.selectivity(1) > self.selectivity(0);
        // Native MATCH derives star column order from the bindings it emits.
        if self.return_star
            && self.with.is_none()
            && (reversed || self.paths.is_some())
        {
            return Err(ExecutionError::Unsupported);
        }
        let start_index = usize::from(reversed);
        let second_index = usize::from(!reversed);
        // Regex refusals are observable even after RETURN fills its window.
        let scan_for_regex = self.filter.as_ref().is_some_and(Expr::has_regex)
            || self.nodes.iter().any(|node| {
                node.inline.iter().any(|(_, value)| looks_like_regex(value))
            });
        let labels = &self.nodes[start_index].labels;
        let condition = if labels.is_empty() {
            ""
        } else {
            " AND n.label = ?2"
        };
        // One scan per written label in order, including repeated labels.
        let sql = format!(
            "SELECT {NODE_COLUMNS} FROM nodes n WHERE n.project = ?1{condition}"
        );
        let mut statement = connection.prepare(&sql)?;
        let mut walk = self
            .paths
            .as_ref()
            .map(|plan| paths::Walk::new(plan, connection, project, context))
            .transpose()?;
        let mut adjacency = (walk.is_none() && !self.hops.is_empty())
            .then(|| Adjacency::new(connection))
            .transpose()?;
        let limit = self.limit.unwrap_or(ENGINE_LIMIT);
        let mut projection_done = false;
        let mut collect =
            |result: &mut ResultRows,
             collector: &mut RowCollector,
             nodes: &[Option<&Node>],
             edges: &[Option<&Edge>],
             budget: &mut ExecutionBudget<'_>| {
                if projection_done {
                    return Ok::<bool, ExecutionError>(true);
                }
                let more = self.collect_row(
                    result, collector, nodes, edges, context, budget,
                )?;
                // This result already hits the final cap. Another start-node
                // read cannot change the standalone query's answer.
                if standalone
                    && self.skip == 0
                    && limit == ENGINE_LIMIT
                    && matches!(collector, RowCollector::All)
                    && result.rows.len() == ENGINE_LIMIT
                {
                    budget.check()?;
                    return Err(ExecutionError::RowCeiling);
                }
                if more {
                    return Ok(true);
                }
                projection_done = scan_for_regex;
                Ok(projection_done)
            };
        let scans = labels.iter().copied().map(Some);
        'starts: for label in
            labels.is_empty().then_some(None).into_iter().chain(scans)
        {
            let mut start_nodes = if let Some(label) = label {
                statement.query(params![project, label])?
            } else {
                statement.query([project])?
            };
            loop {
                if !scan_for_regex
                    && matches!(collector, RowCollector::All)
                    && walk.is_none()
                    && adjacency.is_none()
                    && self.limit.is_some()
                    && self.can_limit_during_scan()
                    && result.rows.len() == limit
                {
                    break 'starts;
                }
                let Some(row) = start_nodes.next()? else {
                    break;
                };
                budget.step()?;
                let start = Node::read(row, 0)?;
                if !self.matches_node(start_index, &start, context) {
                    continue;
                }
                let mut seed = Bindings::default();
                let seed_slot = self
                    .paths
                    .as_ref()
                    .map_or(start_index, |plan| plan.first_slot(start_index));
                seed.nodes[seed_slot] = Some(&start);
                if !self.matches_where(&seed, context) {
                    continue;
                }
                if let Some(walk) = &mut walk {
                    walk.expand_seed(
                        self,
                        &seed,
                        reversed,
                        budget,
                        &mut |path, budget| {
                            collect(
                                result,
                                collector,
                                &path.nodes,
                                &path.edges,
                                budget,
                            )?;
                            Ok(())
                        },
                    )?;
                    continue;
                }
                let Some(adjacency) = &mut adjacency else {
                    if !collect(result, collector, &[Some(&start)], &[], budget)?
                    {
                        break 'starts;
                    }
                    continue;
                };
                let hop = &self.hops[0];
                let direction = if reversed {
                    hop.direction.reversed()
                } else {
                    hop.direction
                };
                for (next, edge) in
                    adjacency.neighbors(start.id, hop, direction, budget)?
                {
                    budget.step()?;
                    if !self.matches_node(second_index, &next, context) {
                        continue;
                    }
                    let mut path = Bindings {
                        nodes: seed.nodes,
                        ..Bindings::default()
                    };
                    path.nodes[second_index] = Some(&next);
                    path.edges[0] = Some(&edge);
                    // For two-hop queries with regexes, preserve the native
                    // order: check the terminal node before evaluating WHERE.
                    // Both checks can emit warnings.
                    if (self.hops.len() == 1 || !scan_for_regex)
                        && !self.matches_where(&path, context)
                    {
                        continue;
                    }
                    if let Some(hop) = self.hops.get(1) {
                        for (last, last_edge) in adjacency.neighbors(
                            next.id,
                            hop,
                            hop.direction,
                            budget,
                        )? {
                            budget.step()?;
                            // The native parser uses one generated binding for
                            // each anonymous endpoint after the start node.
                            if (!self.nodes[1].named
                                && !self.nodes[2].named
                                && next.id != last.id)
                                || !self.matches_node(2, &last, context)
                            {
                                continue;
                            }
                            let mut path = path;
                            path.nodes[2] = Some(&last);
                            path.edges[1] = Some(&last_edge);
                            if !self.matches_where(&path, context) {
                                continue;
                            }
                            if !collect(
                                result,
                                collector,
                                &path.nodes,
                                &path.edges,
                                budget,
                            )? {
                                break 'starts;
                            }
                        }
                    } else {
                        if !collect(
                            result,
                            collector,
                            &path.nodes,
                            &path.edges,
                            budget,
                        )? {
                            break 'starts;
                        }
                    }
                }
            }
        }
        budget.check()?;
        if let Some(walk) = walk {
            walk.finish(result);
        }
        Ok(())
    }

    fn can_limit_during_scan(&self) -> bool {
        !self.ordered && self.skip == 0
    }

    fn finish_rows(
        &self,
        mut result: ResultRows,
        budget: &mut ExecutionBudget<'_>,
    ) -> Result<ResultRows, ExecutionError> {
        let limit = self.limit.unwrap_or(ENGINE_LIMIT);
        if self.limit.is_none()
            && result.rows.len().saturating_sub(self.skip) > limit
        {
            result.truncated = true;
        }
        if !self.order.is_empty() {
            order_rows(
                &mut result.rows,
                &self.order,
                self.skip.saturating_add(limit),
                budget,
            )?;
        }
        result.rows.drain(..self.skip.min(result.rows.len()));
        result.rows.truncate(limit);
        budget.check()?;
        Ok(result)
    }

    fn count_collector(
        &self,
        ordered_group_limit: usize,
    ) -> Option<RowCollector> {
        if self.skip != 0 || self.distinct {
            return None;
        }
        let fast_count = |expression: &ProjectionExpression<'_>| {
            matches!(expression, ProjectionExpression::Count(None))
                || (self.paths.is_none()
                    && matches!(expression, ProjectionExpression::Count(_)))
        };
        let mut fields = 0;
        for column in &self.projections {
            if !column.expression.is_aggregate() {
                fields += 1;
            } else if !fast_count(&column.expression) {
                return None;
            }
        }
        if fields > 1 {
            return None;
        }
        self.projections
            .iter()
            .any(|column| fast_count(&column.expression))
            .then(|| {
                if let Some(column) = self
                    .projections
                    .iter()
                    .position(|column| !fast_count(&column.expression))
                {
                    let limit = self.limit.unwrap_or(ENGINE_LIMIT);
                    if !self.order.is_empty()
                        && (1..ENGINE_LIMIT).contains(&limit)
                    {
                        RowCollector::OrderedCount(OrderedCount::new(
                            column,
                            ordered_group_limit,
                            limit,
                        ))
                    } else {
                        // An unordered limit needs only the first-seen keys.
                        // More keys at the engine ceiling cannot change the error.
                        RowCollector::Count(CountRows::Grouped(CountGroups {
                            column,
                            max_groups: limit,
                            groups: HashMap::new(),
                        }))
                    }
                } else {
                    RowCollector::Count(CountRows::Total(0))
                }
            })
    }

    #[allow(clippy::too_many_arguments)]
    fn collect_row(
        &self,
        result: &mut ResultRows,
        collector: &mut RowCollector,
        nodes: &[Option<&Node>],
        edges: &[Option<&Edge>],
        context: &ExpressionContext<'_>,
        budget: &ExecutionBudget<'_>,
    ) -> Result<bool, ExecutionError> {
        let limit = self.limit.unwrap_or(ENGINE_LIMIT);
        match collector {
            RowCollector::With(rows) => {
                self.with
                    .as_ref()
                    .unwrap()
                    .feed(rows, nodes, edges, context);
                return Ok(true);
            }
            RowCollector::Aggregate(groups) => {
                groups.feed(
                    &self.projections,
                    nodes,
                    edges,
                    self.paths.is_some(),
                    context,
                );
                return Ok(true);
            }
            RowCollector::Count(CountRows::Total(count)) => {
                *count += 1;
                return Ok(true);
            }
            RowCollector::Count(CountRows::Grouped(counts)) => {
                if counts.max_groups == 0 {
                    // LIMIT 0 still reads the graph and checks its budget.
                    return Ok(true);
                }
                let key = self.projections[counts.column]
                    .expression
                    .field_value(nodes, edges, context);
                counts.record(&key);
                return Ok(true);
            }
            RowCollector::OrderedCount(counts) => {
                budget.step()?;
                let key = self.projections[counts.column]
                    .expression
                    .field_value(nodes, edges, context);
                counts.record(&key, budget)?;
                return Ok(true);
            }
            RowCollector::Distinct(distinct) => {
                // Native scans and checks the budget even after it stops
                // projecting. A later read can still fail this request.
                if !distinct.done && distinct.cap != 0 {
                    distinct
                        .admit(self.project_row(nodes, edges, context), result);
                }
                return Ok(true);
            }
            RowCollector::All => {}
        }
        if self.limit.is_some()
            && self.can_limit_during_scan()
            && result.rows.len() == limit
        {
            return Ok(false);
        }
        if result.rows.len() == ENGINE_LIMIT {
            // Native sorts the capped prefix and reports a lower bound when
            // another match exists. Keep the prefix under this request budget.
            result.truncated = true;
            return Ok(false);
        }
        result.rows.push(self.project_row(nodes, edges, context));
        Ok(true)
    }

    fn project_row(
        &self,
        nodes: &[Option<&Node>],
        edges: &[Option<&Edge>],
        context: &ExpressionContext<'_>,
    ) -> Vec<Vec<u8>> {
        self.projections
            .iter()
            .map(|column| {
                column
                    .expression
                    .field_value(nodes, edges, context)
                    .into_owned()
            })
            .collect()
    }

    fn selectivity(&self, index: usize) -> u8 {
        let node = &self.nodes[index];
        let inline = node
            .inline
            .iter()
            .map(|(name, _)| property_selectivity(name))
            .max()
            .unwrap_or(0);
        let filter = self.filter.as_ref().map_or(0, |expr| {
            let slot = self
                .paths
                .as_ref()
                .map_or(index, |plan| plan.first_slot(index));
            expr.selectivity(slot)
        });
        inline.max(filter).max(u8::from(!node.labels.is_empty()))
    }

    fn matches_node(
        &self,
        index: usize,
        node: &Node,
        context: &ExpressionContext<'_>,
    ) -> bool {
        self.nodes[index].matches(node, context)
    }

    fn matches_where(
        &self,
        bindings: &Bindings<'_>,
        context: &ExpressionContext<'_>,
    ) -> bool {
        self.filter
            .as_ref()
            .and_then(|expr| expr.evaluate(bindings, context))
            != Some(false)
    }
}

impl NodePattern<'_> {
    fn matches(&self, node: &Node, context: &ExpressionContext<'_>) -> bool {
        (self.labels.is_empty()
            || self
                .labels
                .iter()
                .any(|label| label.as_bytes() == node.label))
            && self.inline.iter().all(|(name, expected)| {
                let actual = node.property_bytes(name, context);
                if looks_like_regex(expected) {
                    context.regex_matches(&actual, expected, true)
                } else {
                    actual == *expected
                }
            })
    }
}

fn property_selectivity(name: &str) -> u8 {
    if matches!(name, "name" | "qualified_name") {
        3
    } else {
        2
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
                numeric[index] = is_native_sort_number(cell);
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

fn count_group_key(key: &[u8]) -> &[u8] {
    // Native clips grouped cells before grouping and output.
    let key = c_string_bytes(key);
    &key[..key.len().min(PROPERTY_LIMIT)]
}

fn is_native_sort_number(cell: &[u8]) -> bool {
    let digits = cell.strip_prefix(b"-").unwrap_or(cell);
    !digits.is_empty() && digits.iter().all(u8::is_ascii_digit)
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
    id: i64,
    kind: Vec<u8>,
    properties: Vec<u8>,
}

impl Edge {
    fn property_bytes(&self, property: Option<&str>) -> Cow<'_, [u8]> {
        Cow::Borrowed(match property {
            Some("type") => &self.kind,
            Some(name) => json_property_bytes(&self.properties, name),
            None => b"",
        })
    }
}

struct Adjacency<'a> {
    untyped: [Statement<'a>; 2],
    typed: [Statement<'a>; 2],
    node: Statement<'a>,
}

impl<'a> Adjacency<'a> {
    fn new(connection: &'a Connection) -> rusqlite::Result<Self> {
        // Match native prepared edge lookups, then fetch endpoints by id.
        // A join can change the SQLite query plan and therefore the row order.
        let edges = "SELECT id, project, source_id, target_id, type, properties FROM edges";
        let scans = |filter| {
            Ok::<_, rusqlite::Error>([
                connection
                    .prepare(&format!("{edges} WHERE source_id = ?1{filter}"))?,
                connection
                    .prepare(&format!("{edges} WHERE target_id = ?1{filter}"))?,
            ])
        };
        Ok(Self {
            untyped: scans("")?,
            typed: scans(" AND type = ?2")?,
            node: connection.prepare(&format!(
                "SELECT {NODE_COLUMNS} FROM nodes n WHERE id = ?1"
            ))?,
        })
    }

    fn neighbors(
        &mut self,
        id: i64,
        hop: &Hop<'_>,
        direction: Direction,
        budget: &mut ExecutionBudget<'_>,
    ) -> Result<Vec<(Node, Edge)>, ExecutionError> {
        let mut neighbors = Vec::new();
        for &inbound in direction.inbound_scans() {
            let index = usize::from(inbound);
            let neighbor_column = if inbound { 2 } else { 3 };
            if hop.kinds.is_empty() {
                let mut rows = self.untyped[index].query([id])?;
                read_neighbors(
                    &mut rows,
                    &mut self.node,
                    neighbor_column,
                    &mut neighbors,
                    budget,
                )?;
            } else {
                for kind in &hop.kinds {
                    let mut rows = self.typed[index].query(params![id, kind])?;
                    read_neighbors(
                        &mut rows,
                        &mut self.node,
                        neighbor_column,
                        &mut neighbors,
                        budget,
                    )?;
                }
            }
        }
        Ok(neighbors)
    }
}

fn read_neighbors(
    rows: &mut rusqlite::Rows<'_>,
    node: &mut Statement<'_>,
    neighbor_column: usize,
    neighbors: &mut Vec<(Node, Edge)>,
    budget: &mut ExecutionBudget<'_>,
) -> Result<(), ExecutionError> {
    while let Some(edge) = rows.next()? {
        budget.step()?;
        let neighbor: i64 = edge.get(neighbor_column)?;
        let mut nodes = node.query([neighbor])?;
        if let Some(row) = nodes.next()? {
            neighbors.push((
                Node::read(row, 0)?,
                Edge {
                    id: edge.get(0)?,
                    kind: read_text_bytes(edge, 4)?.unwrap_or_default(),
                    properties: read_text_bytes(edge, 5)?
                        .unwrap_or_else(|| b"{}".to_vec()),
                },
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod distinct_tests {
    use super::*;

    #[test]
    fn distinct_cap_ignores_duplicates_until_another_unique_row() {
        let mut distinct = DistinctRows {
            cap: 2,
            engine_cap: true,
            ..DistinctRows::default()
        };
        let mut result = ResultRows {
            columns: vec!["n.name".to_owned()],
            rows: Vec::new(),
            truncated: false,
            warnings: Warnings::default(),
        };
        for name in [b"a", b"b", b"a", b"b"] {
            distinct.admit(vec![name.to_vec()], &mut result);
        }
        assert_eq!(result.rows, [vec![b"a".to_vec()], vec![b"b".to_vec()]]);
        assert!(!result.truncated);
        assert!(!distinct.done);

        distinct.admit(vec![b"c".to_vec()], &mut result);
        assert_eq!(result.rows.len(), 2);
        assert!(result.truncated);
        assert!(distinct.done);
        assert_eq!(distinct.previous_by_row.len(), 2);
    }
}

#[cfg(test)]
mod count_tests {
    use super::*;

    #[test]
    fn unordered_count_limit_keeps_only_first_keys_and_counts_late_matches() {
        let query =
            classify("MATCH (n) RETURN n.name, count(*) LIMIT 1").unwrap();
        let Some(RowCollector::Count(CountRows::Grouped(mut counts))) = query
            .first_branch()
            .count_collector(ORDERED_COUNT_GROUP_LIMIT)
        else {
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
        let Some(RowCollector::Count(CountRows::Grouped(mut counts))) = query
            .first_branch()
            .count_collector(ORDERED_COUNT_GROUP_LIMIT)
        else {
            panic!("expected grouped count");
        };
        for index in 0..=ENGINE_LIMIT {
            counts.record(index.to_string().as_bytes());
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
