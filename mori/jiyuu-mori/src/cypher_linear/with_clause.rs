use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, c_char, c_int};
use std::mem;

use super::predicates::parse_number;
use super::{
    Binding, Bindings, ENGINE_LIMIT, Edge, ExecutionBudget, ExecutionError,
    Expr, ExpressionContext, MatchQuery, Node, ORDER_KEYS_MAX, PROPERTY_LIMIT,
    ParseError, Parser, Projection, ProjectionExpression, ResultRows,
    Unsupported,
};

const GROUP_KEY_BYTES: usize = 1023;
const MAX_WITH_ITEMS: usize = 16;

#[derive(Clone, Copy, Debug)]
pub(super) enum NumericAggregate {
    Sum,
    Avg,
    Min,
    Max,
}

impl NumericAggregate {
    pub(super) fn parse(parser: &mut Parser<'_>) -> Option<Self> {
        [Self::Sum, Self::Avg, Self::Min, Self::Max]
            .into_iter()
            .find(|agg| parser.keyword(agg.name()))
    }

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Sum => "SUM",
            Self::Avg => "AVG",
            Self::Min => "MIN",
            Self::Max => "MAX",
        }
    }
}

#[derive(Debug)]
struct WithOrderKey {
    column: Option<usize>,
    property: Option<String>,
    descending: bool,
}

#[derive(Debug)]
pub(super) struct WithPlan<'a> {
    items: Vec<Projection<'a>>,
    aggregate: bool,
    distinct: bool,
    order: Vec<WithOrderKey>,
    skip: usize,
    limit: Option<usize>,
    filter: Option<Expr<'a>>,
}

#[derive(Default)]
pub(super) struct WithRows {
    skipped: usize,
    projected: Vec<Vec<Node>>,
    grouped: Groups<Vec<Node>>,
}

impl<'a> WithPlan<'a> {
    pub(super) fn parse(parser: &mut Parser<'a>) -> Result<Self, ParseError> {
        let distinct = parser.keyword("DISTINCT");
        let items = parser.projections(MAX_WITH_ITEMS)?;
        let mut projected_names = HashSet::new();
        for item in &items {
            if !projected_names.insert(item.source_name.as_str()) {
                // Native retains duplicate scalar aliases but can overwrite
                // them when a later item carries a node under the same name.
                return Err(Unsupported.into());
            }
        }
        parser.names = items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                item.scope_name.map(|name| (name, Binding::Node(index)))
            })
            .collect();
        parser.projected_fields = items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.source_name.contains('.'))
            .map(|(index, item)| (item.source_name.clone(), index))
            .collect();
        parser.after_with = true;

        let mut order = Vec::new();
        if parser.keyword("ORDER") {
            if !parser.keyword("BY") {
                return Err(Unsupported.into());
            }
            loop {
                if order.len() == ORDER_KEYS_MAX {
                    return Err(Unsupported.into());
                }
                let name = parser.order_name()?;
                let mut column =
                    items.iter().position(|item| item.source_name == name);
                let mut property = None;
                if column.is_none()
                    && let Some((var, prop)) = name.split_once('.')
                {
                    column =
                        items.iter().position(|item| item.source_name == var);
                    if column.is_some() {
                        property = Some(prop.to_owned());
                    }
                }
                let descending = parser.keyword("DESC");
                if !descending {
                    parser.keyword("ASC");
                }
                order.push(WithOrderKey {
                    column,
                    property,
                    descending,
                });
                if !parser.consume(b',') {
                    break;
                }
            }
        }
        let skip = parser.unsigned("SKIP")?.unwrap_or(0);
        let limit = parser.unsigned("LIMIT")?;
        let filter = parser
            .keyword("WHERE")
            .then(|| parser.where_expr())
            .transpose()?;
        let aggregate = items.iter().any(|item| item.expression.is_aggregate());
        Ok(Self {
            items,
            aggregate,
            distinct,
            order,
            skip,
            limit,
            filter,
        })
    }

    pub(super) fn return_star(&self) -> Vec<Projection<'a>> {
        self.items
            .iter()
            .enumerate()
            .map(|(index, item)| Projection {
                expression: ProjectionExpression::Field(
                    Binding::Node(index),
                    None,
                ),
                name: item.source_name.clone(),
                source_name: item.source_name.clone(),
                scope_name: None,
            })
            .collect()
    }

    pub(super) fn feed(
        &self,
        rows: &mut WithRows,
        nodes: &[Option<&Node>],
        edges: &[Option<&Edge>],
        context: &ExpressionContext<'_>,
    ) {
        if self.projects_window_while_scanning() {
            if rows.skipped < self.skip {
                rows.skipped += 1;
                return;
            }
            if self
                .limit
                .is_some_and(|limit| rows.projected.len() >= limit)
            {
                return;
            }
        }
        let value = |item: &Projection<'_>| -> Node {
            if let ProjectionExpression::Field(Binding::Node(index), None) =
                item.expression
                && let Some(node) = nodes[index].filter(|node| node.id > 0)
            {
                return node.clone();
            }
            Node {
                name: item
                    .expression
                    .field_value(nodes, edges, context)
                    .into_owned(),
                ..Node::default()
            }
        };
        if !self.aggregate {
            rows.projected.push(self.items.iter().map(value).collect());
            return;
        }
        let mut key = Vec::new();
        for item in &self.items {
            if item.expression.is_aggregate() {
                continue;
            }
            if let ProjectionExpression::Field(Binding::Node(index), None) =
                item.expression
                && let Some(node) = nodes[index].filter(|node| node.id > 0)
            {
                append_key(&mut key, format!("#{}", node.id).as_bytes());
            } else {
                append_key(
                    &mut key,
                    &item.expression.field_value(nodes, edges, context),
                );
            }
        }
        let group = rows.grouped.admit(key, &self.items, || {
            self.items
                .iter()
                .map(|item| {
                    if item.expression.is_aggregate() {
                        Node::default()
                    } else {
                        value(item)
                    }
                })
                .collect()
        });
        group.accumulate(&self.items, nodes, edges, false, context);
    }

    fn projects_window_while_scanning(&self) -> bool {
        !self.aggregate && !self.distinct && self.order.is_empty()
    }

    pub(super) fn finish(
        &self,
        query: &MatchQuery<'_>,
        rows: WithRows,
        mut result: ResultRows,
        context: &ExpressionContext<'_>,
        budget: &mut ExecutionBudget<'_>,
    ) -> Result<ResultRows, ExecutionError> {
        let projected = if self.aggregate {
            let mut projected = Vec::with_capacity(rows.grouped.entries.len());
            for mut group in rows.grouped.entries {
                budget.step()?;
                for (node, agg) in group.row.iter_mut().zip(group.aggregates) {
                    if let Some(agg) = agg {
                        node.name = agg.bytes(true);
                    }
                }
                projected.push(group.row);
            }
            projected
        } else {
            rows.projected
        };
        let mut seen = HashSet::new();
        let mut ordered = Vec::with_capacity(projected.len());
        for row in projected {
            budget.step()?;
            if self.distinct {
                let mut key = Vec::new();
                for node in &row {
                    append_key(&mut key, &node.name);
                }
                if !seen.insert(key) {
                    continue;
                }
            }
            let keys = self
                .order
                .iter()
                .map(|order| {
                    let bytes = order.column.map_or_else(Vec::new, |column| {
                        let node = &row[column];
                        order.property.as_deref().map_or_else(
                            || node.name.clone(),
                            |prop| {
                                node.property_bytes(prop, context).into_owned()
                            },
                        )
                    });
                    WithSortCell {
                        number: parse_number(&bytes),
                        bytes,
                    }
                })
                .collect();
            ordered.push(WithSortRow { row, keys });
        }
        drop(seen);
        let order = sort_with_rows(&ordered, &self.order, budget)?;
        let aggregate = query
            .projections
            .iter()
            .any(|item| item.expression.is_aggregate());
        let mut groups = ReturnGroups::default();
        let mut seen = HashSet::new();
        let engine_cap = query.return_star
            || query.limit.is_none()
            || query.ordered
            || query.skip != 0;
        let cap = if engine_cap {
            ENGINE_LIMIT
        } else {
            query.limit.unwrap()
        };
        let (skip, limit) = if self.projects_window_while_scanning() {
            (0, usize::MAX)
        } else {
            (self.skip, self.limit.unwrap_or(usize::MAX))
        };
        let scan_for_regex = self.filter.as_ref().is_some_and(Expr::has_regex);
        let mut projection_done = false;
        for index in order.into_iter().skip(skip).take(limit) {
            budget.step()?;
            let row = &ordered[index].row;
            let bindings = Bindings {
                projected: Some(row),
                ..Bindings::default()
            };
            if self
                .filter
                .as_ref()
                .and_then(|filter| filter.evaluate(&bindings, context))
                == Some(false)
            {
                continue;
            }
            if projection_done {
                continue;
            }
            let nodes: Vec<_> = row.iter().map(Some).collect();
            if aggregate {
                groups.feed(&query.projections, &nodes, &[], false, context);
            } else {
                let row = query.project_row(&nodes, &[], context);
                if query.distinct && !seen.insert(row.clone()) {
                    continue;
                }
                if result.rows.len() == cap {
                    result.truncated |= engine_cap;
                    if scan_for_regex {
                        projection_done = true;
                        continue;
                    }
                    break;
                }
                result.rows.push(row);
            }
        }
        if aggregate {
            groups.emit(query, &mut result, budget)?;
        }
        result.warnings.regex_refused |= context.regex_refused.get();
        query.finish_rows(result, budget)
    }
}

// Group the same bytes as the C snprintf-built key, including delimiters and
// truncation. RETURN and WITH DISTINCT use different native identities.
fn append_key(key: &mut Vec<u8>, value: &[u8]) {
    let value = &value[..value.len().min(GROUP_KEY_BYTES - key.len())];
    key.extend_from_slice(value);
    if key.len() < GROUP_KEY_BYTES {
        key.push(b'|');
    }
}

struct WithSortRow {
    row: Vec<Node>,
    keys: Vec<WithSortCell>,
}

struct WithSortCell {
    bytes: Vec<u8>,
    number: Option<f64>,
}

fn sort_with_rows(
    rows: &[WithSortRow],
    keys: &[WithOrderKey],
    budget: &mut ExecutionBudget<'_>,
) -> Result<Vec<usize>, ExecutionError> {
    let mut indices: Vec<_> = (0..rows.len()).collect();
    if keys.is_empty() {
        return Ok(indices);
    }
    let compare = |a: usize, b: usize| {
        for ((a, b), key) in rows[a].keys.iter().zip(&rows[b].keys).zip(keys) {
            let order = match (a.number, b.number) {
                (Some(a), Some(b)) => {
                    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
                }
                _ => a.bytes.cmp(&b.bytes),
            };
            if order != Ordering::Equal {
                return if key.descending {
                    order.reverse()
                } else {
                    order
                };
            }
        }
        Ordering::Equal
    };

    // Native uses this merge order. Its pairwise numeric-or-lexical comparison
    // is not a total ordering when inputs mix numeric prefixes and other text.
    let mut scratch = vec![0; rows.len()];
    let mut width = 1;
    while width < indices.len() {
        for (run, dest) in
            indices.chunks(width * 2).zip(scratch.chunks_mut(width * 2))
        {
            let (left, right) = run.split_at(width.min(run.len()));
            let mut left = left.iter().copied().peekable();
            let mut right = right.iter().copied().peekable();
            for item in dest {
                budget.step()?;
                *item = match (left.peek(), right.peek()) {
                    (Some(&a), Some(&b))
                        if compare(a, b) != Ordering::Greater =>
                    {
                        left.next()
                    }
                    (_, Some(_)) => right.next(),
                    (_, None) => left.next(),
                }
                .unwrap();
            }
        }
        mem::swap(&mut indices, &mut scratch);
        width *= 2;
    }
    Ok(indices)
}

#[derive(Default)]
pub(super) struct ReturnGroups(Groups<Vec<Vec<u8>>>);

impl ReturnGroups {
    pub(super) fn feed(
        &mut self,
        items: &[Projection<'_>],
        nodes: &[Option<&Node>],
        edges: &[Option<&Edge>],
        count_bound: bool,
        context: &ExpressionContext<'_>,
    ) {
        let mut key = Vec::new();
        let projected = items
            .iter()
            .map(|item| {
                if item.expression.is_aggregate() {
                    return Vec::new();
                }
                let mut cell = item
                    .expression
                    .field_value(nodes, edges, context)
                    .into_owned();
                cell.truncate(
                    PROPERTY_LIMIT.min(super::c_string_bytes(&cell).len()),
                );
                append_key(&mut key, &cell);
                cell
            })
            .collect();
        self.0.admit(key, items, || projected).accumulate(
            items,
            nodes,
            edges,
            count_bound,
            context,
        );
    }

    pub(super) fn emit(
        self,
        query: &MatchQuery<'_>,
        result: &mut ResultRows,
        budget: &mut ExecutionBudget<'_>,
    ) -> Result<(), ExecutionError> {
        let mut seen = HashSet::new();
        for mut group in self.0.entries {
            budget.step()?;
            for (value, agg) in group.row.iter_mut().zip(group.aggregates) {
                if let Some(agg) = agg {
                    *value = agg.bytes(false);
                }
            }
            if !query.distinct || seen.insert(group.row.clone()) {
                result.rows.push(group.row);
            }
        }
        if result.rows.is_empty()
            && query
                .projections
                .iter()
                .all(|p| p.expression.is_aggregate())
        {
            let row = query
                .projections
                .iter()
                .map(|p| match p.expression {
                    ProjectionExpression::Count(_)
                    | ProjectionExpression::CountDistinct(..)
                    | ProjectionExpression::NumericAggregate(
                        NumericAggregate::Sum,
                        ..,
                    ) => b"0".to_vec(),
                    ProjectionExpression::Collect(..) => b"[]".to_vec(),
                    _ => Vec::new(),
                })
                .collect();
            result.rows.push(row);
        }
        Ok(())
    }
}

struct Groups<T> {
    index: HashMap<Vec<u8>, usize>,
    entries: Vec<Group<T>>,
}

impl<T> Default for Groups<T> {
    fn default() -> Self {
        Self {
            index: HashMap::new(),
            entries: Vec::new(),
        }
    }
}

impl<T> Groups<T> {
    fn admit(
        &mut self,
        key: Vec<u8>,
        items: &[Projection<'_>],
        row: impl FnOnce() -> T,
    ) -> &mut Group<T> {
        let index = *self.index.entry(key).or_insert_with(|| {
            let index = self.entries.len();
            self.entries.push(Group {
                row: row(),
                aggregates: items
                    .iter()
                    .map(|item| Accumulator::new(&item.expression))
                    .collect(),
            });
            index
        });
        &mut self.entries[index]
    }
}

struct Group<T> {
    row: T,
    aggregates: Vec<Option<Accumulator>>,
}

impl<T> Group<T> {
    fn accumulate(
        &mut self,
        items: &[Projection<'_>],
        nodes: &[Option<&Node>],
        edges: &[Option<&Edge>],
        count_bound: bool,
        context: &ExpressionContext<'_>,
    ) {
        let is_counted = |binding| {
            !count_bound
                || match binding {
                    Binding::Node(index) => nodes[index].is_some(),
                    Binding::Edge(index) => edges[index].is_some(),
                    Binding::Unbound => false,
                }
        };
        for (agg, item) in self.aggregates.iter_mut().zip(items) {
            match (agg, &item.expression) {
                (
                    Some(Accumulator::Count(count)),
                    ProjectionExpression::Count(binding),
                ) if binding.is_none_or(is_counted) => *count += 1,
                (
                    Some(Accumulator::Distinct(seen)),
                    ProjectionExpression::CountDistinct(binding, prop),
                ) if is_counted(*binding) => {
                    seen.insert(
                        ProjectionExpression::Field(*binding, *prop)
                            .field_value(nodes, edges, context)
                            .into_owned(),
                    );
                }
                (
                    Some(Accumulator::Collect(list)),
                    ProjectionExpression::Collect(binding, prop),
                ) => {
                    let expression =
                        ProjectionExpression::Field(*binding, *prop);
                    list.push(&expression.field_value(nodes, edges, context));
                }
                (
                    Some(Accumulator::Numeric(stats)),
                    ProjectionExpression::NumericAggregate(_, binding, prop),
                ) => {
                    let expression =
                        ProjectionExpression::Field(*binding, *prop);
                    let value = parse_number(
                        &expression.field_value(nodes, edges, context),
                    )
                    .unwrap_or(0.0);
                    stats.record(value);
                }
                _ => {}
            }
        }
    }
}

enum Accumulator {
    Count(usize),
    Distinct(HashSet<Vec<u8>>),
    Numeric(Numeric),
    Collect(super::rows::Collected),
}

impl Accumulator {
    fn new(expression: &ProjectionExpression<'_>) -> Option<Self> {
        match *expression {
            ProjectionExpression::Count(_) => Some(Self::Count(0)),
            ProjectionExpression::CountDistinct(..) => {
                Some(Self::Distinct(HashSet::new()))
            }
            ProjectionExpression::Collect(..) => {
                Some(Self::Collect(Default::default()))
            }
            ProjectionExpression::NumericAggregate(kind, ..) => {
                Some(Self::Numeric(Numeric {
                    kind,
                    count: 0,
                    sum: 0.0,
                    min: 1e308,
                    max: -1e308,
                }))
            }
            _ => None,
        }
    }

    fn bytes(self, with: bool) -> Vec<u8> {
        match self {
            Self::Count(count) => count.to_string().into_bytes(),
            Self::Distinct(values) => values.len().to_string().into_bytes(),
            Self::Numeric(stats) => stats.bytes(),
            Self::Collect(list) => list.bytes(with),
        }
    }
}

struct Numeric {
    kind: NumericAggregate,
    count: usize,
    sum: f64,
    min: f64,
    max: f64,
}

impl Numeric {
    fn record(&mut self, value: f64) {
        self.count += 1;
        self.sum += value;
        if value < self.min {
            self.min = value;
        }
        if value > self.max {
            self.max = value;
        }
    }

    fn bytes(self) -> Vec<u8> {
        let value = match self.kind {
            NumericAggregate::Sum => self.sum,
            NumericAggregate::Avg => self.sum / self.count as f64,
            NumericAggregate::Min => self.min,
            NumericAggregate::Max => self.max,
        };
        unsafe extern "C" {
            fn snprintf(
                dst: *mut c_char,
                size: usize,
                format: *const c_char,
                ...
            ) -> c_int;
        }
        let mut bytes = [0; 64];
        // SAFETY: bytes points to a writable fixed buffer and the format takes
        // exactly one C double. Use the same representation as native Cypher.
        unsafe {
            snprintf(bytes.as_mut_ptr(), bytes.len(), c"%.10g".as_ptr(), value)
        };
        // SAFETY: snprintf always terminates this bounded double representation.
        unsafe { CStr::from_ptr(bytes.as_ptr()) }
            .to_bytes()
            .to_vec()
    }
}

#[cfg(test)]
mod tests {
    use crate::cypher_linear::classify;
    use rusqlite::Connection;

    pub(super) const FIXTURE: &str = r#"
        CREATE TABLE nodes (
            id INTEGER PRIMARY KEY, project TEXT, label TEXT, name TEXT,
            qualified_name TEXT, file_path TEXT, start_line INTEGER,
            end_line INTEGER, properties TEXT
        );
        CREATE TABLE edges (
            id INTEGER PRIMARY KEY, project TEXT, source_id INTEGER,
            target_id INTEGER, type TEXT, properties TEXT
        );
        CREATE INDEX idx_nodes_project_label ON nodes(project, label);
        CREATE INDEX idx_edges_source_type ON edges(source_id, type);
        CREATE INDEX idx_edges_target_type ON edges(target_id, type);
        INSERT INTO nodes VALUES
            (1, 'test', 'Caller', 'one', 'test.one', 'a.py', 10, 19, '{"level":1.25}'),
            (2, 'test', 'Caller', 'two', 'test.two', 'a.py', 20, 29, '{"level":2.75}'),
            (3, 'test', 'Caller', 'three', 'test.three', 'b.py', 30, 39, '{"level":4}'),
            (4, 'test', 'Target', 'twin', 'test.first', 'a.py', 100, 109, '{"mark":"first"}'),
            (5, 'test', 'Target', 'twin', 'test.second', 'b.py', 200, 209, '{"mark":"second"}'),
            (6, 'test', 'Target', 'other', 'test.third', 'b.py', 300, 309, '{}');
        INSERT INTO edges VALUES
            (1, 'test', 1, 4, 'CALLS', '{"line":10}'),
            (2, 'test', 2, 4, 'CALLS', '{"line":20}'),
            (3, 'test', 3, 4, 'CALLS', '{}'),
            (4, 'test', 1, 5, 'CALLS', '{"line":10}'),
            (5, 'test', 3, 6, 'CALLS', '{"line":30}'),
            (6, 'test', 4, 6, 'CALLS', '{"line":100}'),
            (7, 'test', 1, 6, 'IMPORTS', '{"origin":"fixture"}'),
            (8, 'test', 2, 1, 'IMPORTS', '{}');
    "#;

    pub(super) const CASES: &[(&str, &[&[&str]])] = &[
        (
            "MATCH (n:Target) WITH n AS item, labels(n) AS ls, n.name AS title WHERE (item.mark = 'first' OR item.mark = 'second') AND title = 'twin' RETURN item.qualified_name, ls, title.file_path, labels(title) ORDER BY item.qualified_name",
            &[
                &["test.first", "[\"Target\"]", "", "[]"],
                &["test.second", "[\"Target\"]", "", "[]"],
            ],
        ),
        (
            "MATCH (a:Caller)-[:CALLS]->(b) WITH b AS target, count(a) AS c WHERE c >= 1 RETURN target.qualified_name, c ORDER BY c DESC, target.qualified_name SKIP 1 LIMIT 2",
            &[&["test.second", "1"], &["test.third", "1"]],
        ),
        (
            "MATCH (a:Caller)-[r:CALLS]->(b) WITH DISTINCT a.file_path AS path, type(r) AS rel ORDER BY path DESC SKIP 1 LIMIT 1 WHERE rel = 'CALLS' RETURN *",
            &[&["a.py", "CALLS"]],
        ),
        (
            "MATCH (a:Caller)-[:CALLS]->(b) WITH b AS target, count(DISTINCT a.file_path) AS sources, SUM(a.level) AS weight RETURN target.name, sources, weight ORDER BY target.name, sources DESC",
            &[
                &["other", "1", "4"],
                &["twin", "2", "8"],
                &["twin", "1", "1.25"],
            ],
        ),
        (
            "MATCH (a:Caller)-[:CALLS]->(b) WITH b.name AS name, a.file_path AS path RETURN name, path, count(*) AS total ORDER BY name, path",
            &[
                &["other", "b.py", "1"],
                &["twin", "a.py", "3"],
                &["twin", "b.py", "1"],
            ],
        ),
        (
            "MATCH (a:Caller)-[:CALLS]->(b) WITH a.file_path AS path, b.name AS name RETURN name, count(DISTINCT path) AS total ORDER BY total DESC",
            &[&["twin", "2"], &["other", "1"]],
        ),
        (
            "MATCH (a:Caller) WITH a AS n ORDER BY n.start_line DESC SKIP 1 LIMIT 1 WHERE n.start_line > 20 RETURN n.name",
            &[],
        ),
        (
            "MATCH (a:Caller) WITH a AS n, a.name ORDER BY a.name DESC SKIP 1 LIMIT 1 RETURN *",
            &[&["three", "three"]],
        ),
        ("MATCH (a:Missing) WITH count(*) AS c RETURN c", &[]),
        (
            "MATCH (a:Missing) WITH a RETURN count(*) AS c, SUM(a.start_line) AS s, AVG(a.start_line) AS avg_score",
            &[&["0", "0", ""]],
        ),
        (
            "MATCH (a:Caller)-[:CALLS]->(b)-[:CALLS]->(c) WITH b AS n, count(c) AS total RETURN n.name, total",
            &[&["twin", "3"]],
        ),
        (
            "MATCH (a:Caller) WITH a LIMIT 0 RETURN count(*) AS c",
            &[&["0"]],
        ),
        (
            "MATCH (a:Caller) WITH a AS n SKIP 1 LIMIT 1 RETURN n.name",
            &[&["two"]],
        ),
        (
            "MATCH (a:Caller) WITH a AS n ORDER BY n.start_line DESC RETURN n.name ORDER BY n.start_line LIMIT 2",
            &[&["three"], &["two"]],
        ),
        (
            "MATCH (a:Caller)-[r:CALLS]->(b) WITH r, type(r) AS kind RETURN type(r), kind, id(r) LIMIT 1",
            &[&["", "CALLS", "0"]],
        ),
        (
            "MATCH (a:Caller) WITH a.name AS name WHERE a.file_path = 'no.py' RETURN name LIMIT 1",
            &[&["one"]],
        ),
        (
            "MATCH (a:Caller) WITH a.name AS name WHERE NOT name = lost RETURN name",
            &[],
        ),
        (
            "MATCH (a:Caller)-[:CALLS]->(b) WHERE coalesce(a.missing, b.mark) = 'first' WITH coalesce(b.missing, b.mark) AS label, coalesce(a.missing, a.name) AS name WHERE (coalesce(label, 'absent') = 'first' AND NOT name = 'two') RETURN name ORDER BY name",
            &[&["one"], &["three"]],
        ),
        (
            "MATCH (a:Caller)-[r:IMPORTS|CALLS|IMPORTS]-(b) WHERE a.name = 'one' RETURN type(r), b.qualified_name",
            &[
                &["IMPORTS", "test.third"],
                &["CALLS", "test.first"],
                &["CALLS", "test.second"],
                &["IMPORTS", "test.third"],
                &["IMPORTS", "test.two"],
                &["IMPORTS", "test.two"],
            ],
        ),
        (
            "MATCH ()<-[r:CALLS|IMPORTS]-(a:Caller) WHERE a.name = 'one' RETURN type(r), a.name",
            &[&["CALLS", "one"], &["CALLS", "one"], &["IMPORTS", "one"]],
        ),
        (
            "MATCH (a:Caller)-[:IMPORTS]- >(b) WHERE a.name = 'one' RETURN b.name",
            &[&["other"]],
        ),
        ("MATCH ()-[:CALLS]->()-[:IMPORTS]->(b) RETURN b.name", &[]),
        (
            "MATCH (a:Caller)-[:CALLS]->()-[:CALLS]->() RETURN a.name",
            &[],
        ),
        (
            "MATCH (a:Caller)-[r:CALLS]->(b) RETURN labels(a) AS source, r.line AS marker, count(*) AS hits ORDER BY hits DESC, marker",
            &[
                &["[\"Caller\"]", "10", "2"],
                &["[\"Caller\"]", "", "1"],
                &["[\"Caller\"]", "20", "1"],
                &["[\"Caller\"]", "30", "1"],
            ],
        ),
        (
            "MATCH (a:Caller)-[r:CALLS]->(b) RETURN labels(b) AS target, a.file_path AS path, r.line AS marker, count(*) AS hits ORDER BY hits DESC, path, marker SKIP 1 LIMIT 2",
            &[
                &["[\"Target\"]", "a.py", "20", "1"],
                &["[\"Target\"]", "b.py", "", "1"],
            ],
        ),
        (
            "MATCH (a:Caller) RETURN a.file_path AS path, SUM(a.level) AS total, AVG(a.level) AS mean, MIN(a.level) AS smallest, MAX(a.level) AS largest ORDER BY total, path",
            &[
                &["a.py", "4", "2", "1.25", "2.75"],
                &["b.py", "4", "4", "4", "4"],
            ],
        ),
        (
            "MATCH (a:Caller)-[:CALLS]->(b) RETURN b.name AS name, count(DISTINCT a.file_path) AS total ORDER BY total DESC",
            &[&["twin", "2"], &["other", "1"]],
        ),
        (
            "MATCH (a:Caller) RETURN a.name AS output ORDER BY a.name DESC, output SKIP 1 LIMIT 1",
            &[&["three"]],
        ),
        (
            "MATCH (a:Caller) RETURN a.name AS output SKIP 1 LIMIT 1",
            &[&["two"]],
        ),
        (
            "MATCH (a:Caller) RETURN a.file_path AS path, COUNT(*) ORDER BY COUNT(*) DESC, path",
            &[&["a.py", "2"], &["b.py", "1"]],
        ),
    ];

    #[test]
    fn linear_projections_group_filter_order_and_paginate() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(FIXTURE).unwrap();
        for (source, expected) in CASES {
            let query = classify(source)
                .unwrap_or_else(|err| panic!("{source}: {err:?}"));
            let result = query.execute_on(&db, "test").unwrap();
            let actual: Vec<Vec<_>> = result
                .rows
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|cell| std::str::from_utf8(cell).unwrap())
                        .collect()
                })
                .collect();
            assert_eq!(&actual, expected, "{source}");
        }
    }

    #[test]
    fn compound_group_keys_preserve_native_delimiters_and_cap() {
        let mut a = Vec::new();
        super::append_key(&mut a, b"one|two");
        super::append_key(&mut a, b"");
        let mut b = Vec::new();
        super::append_key(&mut b, b"one");
        super::append_key(&mut b, b"two|");
        assert_eq!(a, b);
        let max = vec![b'x'; super::GROUP_KEY_BYTES + 1];
        super::append_key(&mut a, &max);
        super::append_key(&mut a, b"extra");
        assert_eq!(a.len(), super::GROUP_KEY_BYTES);
    }

    #[test]
    fn with_reports_native_projection_scope_errors() {
        for (query, expected) in [
            (
                "MATCH (f) WITH f.name AS title RETURN f.name",
                "variable 'f' is not in scope for RETURN — the WITH clause did not carry it through",
            ),
            (
                "MATCH (f) WITH f.name AS title RETURN substring(f.name, 0, 1)",
                "variable 'f' is not in scope for RETURN — the WITH clause did not carry it through",
            ),
            (
                "MATCH (f) WITH missing.name AS title RETURN title",
                "variable 'missing' is not in scope for WITH — no pattern in this query names it",
            ),
            (
                "MATCH (f) WITH f WITH f RETURN f",
                "unexpected input at pos 17 ('WITH') — the query was not fully parsed. Note that only one WITH clause is supported.",
            ),
        ] {
            let error = classify(query).unwrap_err();
            assert_eq!(error.diagnostic().as_deref(), Some(expected));
        }
        let error = classify(
            "MATCH (n) WHERE n.file_path STARTS WITH 'src/' WITH split(n.file_path, '/')[2] AS dir, labels(n)[0] AS label RETURN dir, label, count(*) AS c ORDER BY c DESC LIMIT 20"
        ).unwrap_err();
        assert!(
            error
                .diagnostic()
                .unwrap()
                .starts_with("unsupported function 'split'")
        );
    }

    #[test]
    fn incomplete_patterns_report_native_token_ids_and_byte_positions() {
        for (query, token, expected, actual) in [
            ("MATCH (a)-[r:CALLS|IMPORTS]- RETURN a", "RETURN", 66, 2),
            ("MATCH (a:Caller WHERE", "WHERE", 67, 1),
            ("MATCH (a)-[:CALLS|]->(b) RETURN a", "]", 85, 69),
            ("MATCH (a:Caller|) RETURN a", ")", 85, 67),
        ] {
            let offset = query.find(token).unwrap();
            assert_eq!(
                classify(query).unwrap_err().diagnostic().as_deref(),
                Some(format!("expected token type {expected}, got {actual} at pos {offset}").as_str()),
            );
        }
    }

    #[test]
    fn projection_and_order_delimiters_report_native_diagnostics() {
        for (query, delimiter) in [
            (
                "MATCH (a)-[r]->(b) RETURN type(r), count(*) ORDER BY type(r)",
                '(',
            ),
            (
                "MATCH (a) WHERE a.name STARTS WITH 'é' RETURN labels(a)[0] AS l",
                '[',
            ),
        ] {
            let offset = query.rfind(delimiter).unwrap();
            assert_eq!(
                classify(query).unwrap_err().diagnostic().as_deref(),
                Some(format!("unexpected input at pos {offset} ('{delimiter}') — the query was not fully parsed.").as_str()),
            );
        }
        assert_eq!(
            classify("MATCH (a) RETURN a.name[0]")
                .unwrap_err()
                .diagnostic()
                .as_deref(),
            Some(
                "unsupported expression: list indexing/slicing '[...]' is not supported"
            ),
        );
        assert!(
            classify("MATCH (a) RETURN split(a.name, '/')[0]")
                .unwrap_err()
                .diagnostic()
                .unwrap()
                .starts_with("unsupported function 'split'")
        );
    }
}
