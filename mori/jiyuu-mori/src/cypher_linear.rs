//! Bounded Cypher evaluator compiled only by the linked acceptance test.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use rusqlite::{Connection, OpenFlags, Row, Statement, params};

const ENGINE_LIMIT: usize = 100_000;
const DEADLINE: Duration = Duration::from_secs(30);
const CHECK_INTERVAL: usize = 1024;
const PROPERTY_LIMIT: usize = 511;
const NODE_COLUMNS: &str = "n.id, n.label, n.name, n.qualified_name, n.file_path, n.start_line, n.end_line, n.properties";

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
            b'(' | b')' | b'[' | b']' | b'{' | b'}' | b':' | b'-' | b'<'
            | b'>' | b'.' | b'=' | b',' => (Token::Symbol(first), 1),
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

type Filter<'a> = (&'a str, &'a str);

#[derive(Debug)]
struct NodePattern<'a> {
    label: Option<&'a str>,
    inline: Vec<Filter<'a>>,
}

#[derive(Clone, Copy, Debug)]
struct Hop<'a> {
    kind: &'a str,
    inbound: bool,
}

#[derive(Debug)]
struct Projection<'a> {
    binding: Binding,
    property: Option<&'a str>,
    name: String,
}

#[derive(Debug)]
pub(crate) struct Query<'a> {
    nodes: Vec<NodePattern<'a>>,
    hops: Vec<Hop<'a>>,
    filters: Vec<(usize, Filter<'a>)>,
    projections: Vec<Projection<'a>>,
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
        self.symbol(b':')?;
        let kind = self.name()?;
        self.symbol(b']')?;
        if inbound {
            self.symbol(b'-')?;
        } else if self.tokens.next() != Some(Token::ArrowOut) {
            return Err(Unsupported);
        }
        Ok(Hop { kind, inbound })
    }

    fn reference(&mut self) -> Result<(&'a str, Binding), Unsupported> {
        let name = self.name()?;
        Ok((name, *self.names.get(name).ok_or(Unsupported)?))
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
    // Native binds all anonymous targets to the same synthetic name. Support a
    // single anonymous endpoint so we never reinterpret that implicit join.
    if hops.is_empty()
        || nodes.len()
            - parser
                .names
                .values()
                .filter(|b| matches!(b, Binding::Node(_)))
                .count()
            > 1
    {
        return Err(Unsupported);
    }
    let mut filters = Vec::new();
    if parser.keyword("WHERE") {
        loop {
            let (_, Binding::Node(node)) = parser.reference()? else {
                return Err(Unsupported);
            };
            parser.symbol(b'.')?;
            let property = node_property(parser.name()?)?;
            parser.symbol(b'=')?;
            filters.push((node, (property, parser.literal()?)));
            if !parser.keyword("AND") {
                break;
            }
        }
    }
    if !parser.keyword("RETURN") {
        return Err(Unsupported);
    }
    let mut projections = Vec::new();
    loop {
        let (name, binding) = parser.reference()?;
        let property =
            parser.consume(b'.').then(|| parser.name()).transpose()?;
        if let (Binding::Node(_), Some(property)) = (binding, property) {
            node_property(property)?;
        }
        let name = property.map_or_else(
            || name.to_owned(),
            |property| format!("{name}.{property}"),
        );
        projections.push(Projection {
            binding,
            property,
            name,
        });
        if !parser.consume(b',') {
            break;
        }
    }
    if projections.len() > 32 {
        return Err(Unsupported);
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
        projections,
        limit,
    })
}

#[derive(Debug)]
struct Node {
    id: i64,
    label: String,
    name: String,
    qualified_name: String,
    file_path: String,
    start_line: i32,
    end_line: i32,
    properties: String,
}

impl Node {
    fn read(row: &Row<'_>, offset: usize) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(offset)?,
            label: row.get(offset + 1)?,
            name: row.get(offset + 2)?,
            qualified_name: row.get(offset + 3)?,
            file_path: row
                .get::<_, Option<String>>(offset + 4)?
                .unwrap_or_default(),
            start_line: row
                .get::<_, Option<i32>>(offset + 5)?
                .unwrap_or_default(),
            end_line: row.get::<_, Option<i32>>(offset + 6)?.unwrap_or_default(),
            properties: row
                .get::<_, Option<String>>(offset + 7)?
                .unwrap_or_default(),
        })
    }

    fn property(&self, name: &str) -> rusqlite::Result<String> {
        let value: &str = match name {
            "label" => &self.label,
            "name" => &self.name,
            "qualified_name" | "qn" => &self.qualified_name,
            "file_path" | "file" | "path" => &self.file_path,
            "start_line" => return Ok(self.start_line.to_string()),
            "end_line" => return Ok(self.end_line.to_string()),
            _ => "",
        };
        if !value.is_empty() {
            return Ok(value.to_owned());
        }
        if self.properties.starts_with('{') {
            property(&self.properties, name)
        } else {
            Ok(String::new())
        }
    }

    fn matches(&self, filters: &[Filter<'_>]) -> rusqlite::Result<bool> {
        for &(name, expected) in filters {
            if self.property(name)? != expected {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

// The C projection returns the stored spelling of scalar/container values.
// Keep raw bytes and the native extraction limit so JSON is not reformatted.
fn property(json: &str, name: &str) -> rusqlite::Result<String> {
    let Some((_, tail)) = json.split_once(&format!("\"{name}\":")) else {
        return Ok(String::new());
    };
    let tail = tail.trim_start_matches([' ', '\t']);
    let bytes = tail.as_bytes();
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
    tail.get(start..end)
        .map(str::to_owned)
        .ok_or(rusqlite::Error::InvalidQuery)
}

#[derive(Debug, PartialEq)]
pub(crate) struct ResultRows {
    pub(crate) columns: Vec<String>,
    pub(crate) rows: Vec<Vec<String>>,
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
    pub(crate) fn execute(
        &self,
        db: &Path,
        project: &str,
    ) -> Result<ResultRows, ExecutionError> {
        let connection =
            Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        self.execute_on(&connection, project)
    }

    /// Read from the graph and snapshot the caller has already selected.
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
        let sql = format!(
            "SELECT {NODE_COLUMNS} FROM nodes n WHERE n.project = ?1{condition}"
        );
        let mut statement = connection.prepare(&sql)?;
        let mut start_nodes = if let Some(label) = label {
            statement.query(params![project, label])?
        } else {
            statement.query([project])?
        };
        let mut adjacency = Adjacency::new(connection)?;
        let mut rows = Vec::new();
        let limit = self.limit.unwrap_or(ENGINE_LIMIT);
        'starts: while let Some(row) = start_nodes.next()? {
            budget.step()?;
            let start = Node::read(row, 0)?;
            if !start.matches(&self.nodes[start_index].inline)? {
                continue;
            }
            let mut hop = self.hops[0];
            hop.inbound ^= reversed;
            for (next, edge) in adjacency.neighbors(start.id, hop, budget)? {
                budget.step()?;
                if !self.matches_node(second_index, &next)? {
                    continue;
                }
                if let Some(&hop) = self.hops.get(1) {
                    for (last, last_edge) in
                        adjacency.neighbors(next.id, hop, budget)?
                    {
                        budget.step()?;
                        if self.matches_node(2, &last)?
                            && self.conditions(&[&start, &next, &last])?
                        {
                            if rows.len() == limit {
                                break 'starts;
                            }
                            rows.push(self.project(
                                &[&start, &next, &last],
                                &[&edge, &last_edge],
                            )?);
                            if rows.len() == ENGINE_LIMIT {
                                return Err(ExecutionError::RowCeiling);
                            }
                        }
                    }
                } else {
                    let nodes = if reversed {
                        [&next, &start]
                    } else {
                        [&start, &next]
                    };
                    if self.conditions(&nodes)? {
                        if rows.len() == limit {
                            break 'starts;
                        }
                        rows.push(self.project(&nodes, &[&edge])?);
                        if rows.len() == ENGINE_LIMIT {
                            return Err(ExecutionError::RowCeiling);
                        }
                    }
                }
            }
        }
        budget.check()?;
        let columns = self
            .projections
            .iter()
            .map(|column| column.name.clone())
            .collect();
        Ok(ResultRows { columns, rows })
    }

    fn selectivity(&self, index: usize) -> u8 {
        let node = &self.nodes[index];
        node.inline
            .iter()
            .chain(
                self.filters
                    .iter()
                    .filter(|(target, _)| *target == index)
                    .map(|(_, filter)| filter),
            )
            .map(|(name, _)| {
                if matches!(*name, "name" | "qualified_name") {
                    3
                } else {
                    2
                }
            })
            .max()
            .unwrap_or(u8::from(node.label.is_some()))
    }

    fn matches_node(&self, index: usize, node: &Node) -> rusqlite::Result<bool> {
        let pattern = &self.nodes[index];
        if pattern.label.is_some_and(|label| label != node.label) {
            return Ok(false);
        }
        node.matches(&pattern.inline)
    }

    fn conditions(&self, nodes: &[&Node]) -> rusqlite::Result<bool> {
        for &(index, (property, value)) in &self.filters {
            if nodes[index].property(property)? != value {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn project(
        &self,
        nodes: &[&Node],
        edges: &[&Edge],
    ) -> rusqlite::Result<Vec<String>> {
        self.projections
            .iter()
            .map(|column| match (column.binding, column.property) {
                (Binding::Node(index), None) => Ok(nodes[index].name.clone()),
                (Binding::Node(index), Some(name)) => {
                    nodes[index].property(name)
                }
                (Binding::Edge(index), None) => {
                    Ok(edges[index].properties.clone())
                }
                (Binding::Edge(index), Some("type")) => {
                    Ok(edges[index].kind.clone())
                }
                (Binding::Edge(index), Some(name)) => {
                    property(&edges[index].properties, name)
                }
            })
            .collect()
    }
}

struct Edge {
    kind: String,
    properties: String,
}

struct Adjacency<'a> {
    incoming: Statement<'a>,
    outgoing: Statement<'a>,
    node: Statement<'a>,
}

impl<'a> Adjacency<'a> {
    fn new(connection: &'a Connection) -> rusqlite::Result<Self> {
        // Match native prepared edge lookups, then fetch endpoints by id.
        // A join can change the SQLite query plan and therefore the row order.
        let edges = "SELECT id, project, source_id, target_id, type, properties FROM edges";
        Ok(Self {
            incoming: connection.prepare(&format!(
                "{edges} WHERE target_id = ?1 AND type = ?2"
            ))?,
            outgoing: connection.prepare(&format!(
                "{edges} WHERE source_id = ?1 AND type = ?2"
            ))?,
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
        let mut edges = statement.query(params![id, hop.kind])?;
        let mut neighbors = Vec::new();
        while let Some(edge) = edges.next()? {
            budget.step()?;
            let neighbor: i64 = edge.get(neighbor_column)?;
            let mut nodes = self.node.query([neighbor])?;
            if let Some(row) = nodes.next()? {
                neighbors.push((
                    Node::read(row, 0)?,
                    Edge {
                        kind: edge.get(4)?,
                        properties: edge
                            .get::<_, Option<String>>(5)?
                            .unwrap_or_else(|| "{}".to_owned()),
                    },
                ));
            }
        }
        Ok(neighbors)
    }
}
