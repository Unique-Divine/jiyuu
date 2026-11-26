//! Connected MATCH clauses, OPTIONAL MATCH and bounded Cypher trails.

use std::cell::Cell;
use std::ffi::{c_int, c_void};
use std::iter;

use rusqlite::{Connection, Rows, ToSql, ffi, params_from_iter};

use super::{
    Adjacency, Binding, Bindings, Direction, Edge, ExecutionBudget,
    ExecutionError, ExpressionContext, Hop, LocatedToken, MatchQuery,
    NATIVE_IDENT, NODE_COLUMNS, Node, NodePattern, ParseError, Parser,
    ResultRows, Token, Unsupported,
};

pub(super) const NODE_SLOTS: usize = 16;
pub(super) const EDGE_SLOTS: usize = 8;
const VISITED_LIMIT: usize = 100;
const TRAIL_ROW_LIMIT: u32 = 4096;

// Keep the existing grammar and diagnostics for queries outside this lane.
pub(super) fn has_path_syntax(items: &[LocatedToken<'_>]) -> bool {
    let mut in_relationship = false;
    for pair in items.windows(2) {
        match (&pair[0].token, &pair[1].token) {
            (Token::Word(a), Token::Word(b))
                if a.eq_ignore_ascii_case("OPTIONAL")
                    && b.eq_ignore_ascii_case("MATCH") =>
            {
                return true;
            }
            (Token::Symbol(b'-'), Token::Symbol(b'[')) => in_relationship = true,
            (_, Token::Symbol(b']')) => in_relationship = false,
            (_, Token::Symbol(b'*')) if in_relationship => return true,
            _ => {}
        }
    }
    false
}

#[derive(Clone, Copy, Debug)]
pub(super) struct HopRange {
    min: u32,
    max: Option<u32>,
}

impl HopRange {
    pub(super) fn parse(parser: &mut Parser<'_>) -> Result<Self, Unsupported> {
        let first = Self::number(parser)?;
        let (min, max) = if parser
            .tokens
            .next_if(|token| *token == Token::DotDot)
            .is_some()
        {
            (first.unwrap_or(1), Self::number(parser)?)
        } else {
            (1, first)
        };
        // The native parser uses zero to represent an open upper bound.
        let max = max.filter(|&max| max > 0);
        Ok(Self { min, max })
    }

    fn variable(self) -> bool {
        self.min != 1 || self.max != Some(1)
    }

    fn number(parser: &mut Parser<'_>) -> Result<Option<u32>, Unsupported> {
        let Some(Token::Number(number)) = parser
            .tokens
            .next_if(|token| matches!(token, Token::Number(_)))
        else {
            return Ok(None);
        };
        // Defer decimal spelling and C integer overflow cases.
        let number: i32 = number.parse().map_err(|_| Unsupported)?;
        Ok(Some(number as u32))
    }
}

#[derive(Debug)]
struct Endpoint<'a> {
    slot: usize,
    pattern: NodePattern<'a>,
}

#[derive(Debug)]
struct Clause<'a> {
    optional: bool,
    head: Endpoint<'a>,
    hops: Vec<Hop<'a>>,
    tails: Vec<Endpoint<'a>>,
    first_edge: usize,
}

#[derive(Debug)]
pub(super) struct Plan<'a> {
    first_optional: bool,
    first_slots: Vec<usize>,
    ranged: bool,
    next_node: usize,
    next_edge: usize,
    anonymous_tail: Option<usize>,
    anonymous_head: Option<usize>,
    clauses: Vec<Clause<'a>>,
}

impl<'a> Plan<'a> {
    pub(super) fn new(
        optional: bool,
        nodes: &[NodePattern<'_>],
        hops: &[Hop<'_>],
        mut first_slots: Vec<usize>,
    ) -> Self {
        let anonymous_tail = (1..nodes.len()).find(|&index| !nodes[index].named);
        if let Some(slot) = anonymous_tail {
            for (index, node) in nodes.iter().enumerate().skip(slot + 1) {
                if !node.named {
                    first_slots[index] = slot;
                }
            }
        }
        Self {
            first_slots,
            first_optional: optional,
            ranged: hops.iter().any(|hop| hop.range.is_some()),
            next_node: nodes.len(),
            next_edge: hops.len(),
            anonymous_tail,
            anonymous_head: None,
            clauses: Vec::new(),
        }
    }

    pub(super) fn parse_clauses(
        &mut self,
        parser: &mut Parser<'a>,
    ) -> Result<(), ParseError> {
        loop {
            let optional = parser.keyword("OPTIONAL");
            if !parser.keyword("MATCH") {
                return if optional {
                    Err(Unsupported.into())
                } else {
                    Ok(())
                };
            }
            if self.clauses.len() == NODE_SLOTS {
                return Err(Unsupported.into());
            }
            let head = self.endpoint(parser, true)?;
            let first_edge = self.next_edge;
            let mut hops = Vec::new();
            let mut tails = Vec::new();
            while matches!(
                parser.tokens.peek(),
                Some(Token::Symbol(b'-' | b'<'))
            ) {
                if self.next_edge == EDGE_SLOTS {
                    return Err(Unsupported.into());
                }
                let hop = parser.hop(self.next_edge)?;
                self.ranged |= hop.range.is_some();
                self.next_edge += 1;
                hops.push(hop);
                tails.push(self.endpoint(parser, false)?);
            }
            self.clauses.push(Clause {
                optional,
                head,
                hops,
                tails,
                first_edge,
            });
        }
    }

    fn endpoint(
        &mut self,
        parser: &mut Parser<'a>,
        head: bool,
    ) -> Result<Endpoint<'a>, ParseError> {
        parser.expect_pattern_symbol(b'(')?;
        let name = (super::native_token_id(parser.tokens.peek())
            == NATIVE_IDENT)
            .then(|| parser.name())
            .transpose()?;
        let slot = if let Some(name) = name {
            match parser.names.get(name) {
                Some(Binding::Node(slot)) => *slot,
                Some(_) => return Err(Unsupported.into()),
                None => {
                    let slot = self.next_node;
                    self.next_node += 1;
                    parser.names.insert(name, Binding::Node(slot));
                    slot
                }
            }
        } else {
            let slot = if head {
                &mut self.anonymous_head
            } else {
                &mut self.anonymous_tail
            };
            *slot.get_or_insert_with(|| {
                let current = self.next_node;
                self.next_node += 1;
                current
            })
        };
        if slot >= NODE_SLOTS {
            return Err(Unsupported.into());
        }
        Ok(Endpoint {
            slot,
            pattern: parser.node_constraints(name.is_some())?,
        })
    }

    pub(super) fn into_optional(self) -> Result<Option<Self>, Unsupported> {
        if self.first_optional
            || self.clauses.iter().any(|clause| clause.optional)
        {
            return Ok(Some(self));
        }
        if self.clauses.is_empty() {
            return if self.ranged {
                Ok(Some(self))
            } else if self.next_edge <= 2 && self.unique_first_bindings() {
                Ok(None)
            } else {
                Err(Unsupported)
            };
        }

        // Each mandatory single hop must attach to a written node alias
        // from the preceding binding, which includes every earlier clause.
        if self.ranged
            || self.clauses.len() > 2
            || self.clauses[0].first_edge != 1
            || !self.unique_first_bindings()
        {
            return Err(Unsupported);
        }
        let mut bound = [false; NODE_SLOTS];
        for &slot in &self.first_slots {
            bound[slot] = true;
        }
        for clause in &self.clauses {
            let [tail] = clause.tails.as_slice() else {
                return Err(Unsupported);
            };
            let endpoints = [&clause.head, tail];
            if !endpoints
                .iter()
                .any(|node| node.pattern.named && bound[node.slot])
            {
                return Err(Unsupported);
            }
            for node in endpoints {
                bound[node.slot] = true;
            }
        }
        Ok(Some(self))
    }

    fn unique_first_bindings(&self) -> bool {
        self.first_slots.iter().enumerate().all(|(index, &slot)| {
            index == slot || Some(slot) == self.anonymous_tail
        })
    }

    pub(super) fn first_slot(&self, index: usize) -> usize {
        self.first_slots[index]
    }

    pub(super) fn can_reverse(&self) -> bool {
        !self.first_optional && self.clauses.is_empty()
    }
}

#[derive(Clone, Copy)]
enum Anchor {
    Head,
    Terminal,
    Scan,
}

type Emit<'a> = dyn FnMut(&Bindings<'_>, &mut ExecutionBudget<'_>) -> Result<(), ExecutionError>
    + 'a;

pub(super) struct Walk<'plan, 'query, 'db> {
    plan: &'plan Plan<'query>,
    connection: &'db Connection,
    project: &'db str,
    context: &'plan ExpressionContext<'db>,
    adjacency: Adjacency<'db>,
    anchors: [Option<Anchor>; NODE_SLOTS],
    depth_clamped: Option<u32>,
    trail_truncated: bool,
    beyond_depth: bool,
}

impl<'plan, 'query, 'db> Walk<'plan, 'query, 'db> {
    pub(super) fn new(
        plan: &'plan Plan<'query>,
        connection: &'db Connection,
        project: &'db str,
        context: &'plan ExpressionContext<'db>,
    ) -> rusqlite::Result<Self> {
        Ok(Self {
            plan,
            connection,
            project,
            context,
            adjacency: Adjacency::new(connection)?,
            anchors: [None; NODE_SLOTS],
            depth_clamped: None,
            trail_truncated: false,
            beyond_depth: false,
        })
    }

    pub(super) fn expand_seed(
        &mut self,
        query: &MatchQuery<'query>,
        seed: &Bindings<'_>,
        reversed: bool,
        budget: &mut ExecutionBudget<'_>,
        emit: &mut Emit<'_>,
    ) -> Result<(), ExecutionError> {
        self.first_pattern(query, 0, seed, reversed, budget, emit)
    }

    fn first_pattern(
        &mut self,
        query: &MatchQuery<'query>,
        index: usize,
        path: &Bindings<'_>,
        reversed: bool,
        budget: &mut ExecutionBudget<'_>,
        emit: &mut Emit<'_>,
    ) -> Result<(), ExecutionError> {
        let Some(hop) = query.hops.get(index) else {
            return self.clause(query, 0, path, budget, emit);
        };
        let (from, to, direction) = if reversed {
            (
                self.plan.first_slot(1),
                self.plan.first_slot(0),
                hop.direction.reversed(),
            )
        } else {
            (
                self.plan.first_slot(index),
                self.plan.first_slot(index + 1),
                hop.direction,
            )
        };
        let target = &query.nodes[if reversed { 0 } else { index + 1 }];
        self.expand(
            path,
            from,
            to,
            index,
            target,
            hop,
            direction,
            self.plan.first_optional,
            budget,
            &mut |this, path, budget| {
                this.first_pattern(
                    query,
                    index + 1,
                    path,
                    reversed,
                    budget,
                    emit,
                )
            },
        )
        .map(|_| ())
    }

    fn clause(
        &mut self,
        query: &MatchQuery<'query>,
        index: usize,
        path: &Bindings<'_>,
        budget: &mut ExecutionBudget<'_>,
        emit: &mut Emit<'_>,
    ) -> Result<(), ExecutionError> {
        let Some(clause) = self.plan.clauses.get(index) else {
            budget.step()?;
            let path = Bindings {
                complete_path: true,
                ..*path
            };
            if query.matches_where(&path, self.context) {
                emit(&path, budget)?;
            }
            return Ok(());
        };
        let head = clause.head.slot;
        // C selects the anchor using the first incoming row for each clause.
        let anchor = *self.anchors[index].get_or_insert_with(|| {
            if !clause.hops.is_empty() && path.nodes[head].is_some() {
                Anchor::Head
            } else if clause.tails.len() == 1
                && clause.tails[0].pattern.named
                && path.nodes[clause.tails[0].slot].is_some()
            {
                Anchor::Terminal
            } else {
                Anchor::Scan
            }
        });
        match anchor {
            Anchor::Head => {
                self.clause_hops(query, index, 0, path, budget, emit)
            }
            Anchor::Terminal => {
                let original = &clause.hops[0];
                // The native terminal-anchored lookup uses one fixed edge and
                // checks the new endpoint's label.
                let hop = Hop {
                    kinds: original.kinds.clone(),
                    direction: match original.direction {
                        Direction::Inbound => Direction::Outbound,
                        _ => Direction::Inbound,
                    },
                    range: None,
                };
                let target = NodePattern {
                    named: true,
                    labels: clause.head.pattern.labels.clone(),
                    inline: Vec::new(),
                };
                self.expand(
                    path,
                    clause.tails[0].slot,
                    head,
                    clause.first_edge,
                    &target,
                    &hop,
                    hop.direction,
                    false,
                    budget,
                    &mut |this, path, budget| {
                        this.clause(query, index + 1, path, budget, emit)
                    },
                )
                .and_then(|matched| {
                    // A later row can have a missing terminal even though the
                    // first row selected terminal anchoring.
                    if !matched && clause.optional {
                        self.clause(query, index + 1, path, budget, emit)?;
                    }
                    Ok(())
                })
            }
            Anchor::Scan => {
                let labels = &clause.head.pattern.labels;
                let condition = if labels.is_empty() {
                    ""
                } else {
                    " AND n.label = ?2"
                };
                let mut statement = self.connection.prepare(&format!(
                    "SELECT {NODE_COLUMNS} FROM nodes n WHERE n.project = ?1{condition}"
                ))?;
                let mut matched = false;
                let scans = labels.iter().copied().map(Some);
                for label in
                    labels.is_empty().then_some(None).into_iter().chain(scans)
                {
                    let mut rows = if let Some(label) = label {
                        statement.query([self.project, label])?
                    } else {
                        statement.query([self.project])?
                    };
                    while let Some(row) = rows.next()? {
                        budget.step()?;
                        let node = Node::read(row, 0)?;
                        if clause.head.pattern.matches(&node, self.context) {
                            matched = true;
                            let mut path = *path;
                            path.nodes[head] = Some(&node);
                            self.clause_hops(
                                query, index, 0, &path, budget, emit,
                            )?;
                        }
                    }
                }
                if !matched && clause.optional {
                    self.clause(query, index + 1, path, budget, emit)?;
                }
                Ok(())
            }
        }
    }

    fn clause_hops(
        &mut self,
        query: &MatchQuery<'query>,
        clause_index: usize,
        index: usize,
        path: &Bindings<'_>,
        budget: &mut ExecutionBudget<'_>,
        emit: &mut Emit<'_>,
    ) -> Result<(), ExecutionError> {
        let clause = &self.plan.clauses[clause_index];
        let Some(hop) = clause.hops.get(index) else {
            return self.clause(query, clause_index + 1, path, budget, emit);
        };
        let from = if index == 0 {
            clause.head.slot
        } else {
            clause.tails[index - 1].slot
        };
        let to = &clause.tails[index];
        self.expand(
            path,
            from,
            to.slot,
            clause.first_edge + index,
            &to.pattern,
            hop,
            hop.direction,
            clause.optional,
            budget,
            &mut |this, path, budget| {
                this.clause_hops(
                    query,
                    clause_index,
                    index + 1,
                    path,
                    budget,
                    emit,
                )
            },
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn expand(
        &mut self,
        path: &Bindings<'_>,
        from: usize,
        to: usize,
        edge_slot: usize,
        target: &NodePattern<'_>,
        hop: &Hop<'_>,
        direction: Direction,
        optional: bool,
        budget: &mut ExecutionBudget<'_>,
        emit: &mut impl FnMut(
            &mut Self,
            &Bindings<'_>,
            &mut ExecutionBudget<'_>,
        ) -> Result<(), ExecutionError>,
    ) -> Result<bool, ExecutionError> {
        let Some(start) = path.nodes[from] else {
            return Ok(false);
        };
        let bound = path.nodes[to].map(|node| node.id);
        let mut matched = false;
        let mut visit =
            |this: &mut Self,
             node: &Node,
             edge: Option<&Edge>,
             budget: &mut ExecutionBudget<'_>| {
                if bound.is_none_or(|id| id == node.id)
                    && target.matches(node, this.context)
                {
                    matched = true;
                    let mut path = *path;
                    path.nodes[to] = Some(node);
                    path.edges[edge_slot] = edge;
                    emit(this, &path, budget)?;
                }
                Ok::<_, ExecutionError>(())
            };
        if let Some(range) = hop.range.filter(|range| range.variable()) {
            for node in self.trail(start.id, hop, range, direction, budget)? {
                visit(self, &node, None, budget)?;
            }
        } else {
            for (node, edge) in
                self.adjacency.neighbors(start.id, hop, direction, budget)?
            {
                visit(self, &node, Some(&edge), budget)?;
            }
        }
        if optional && !matched {
            emit(self, path, budget)?;
        }
        Ok(matched)
    }

    fn trail(
        &mut self,
        start: i64,
        hop: &Hop<'_>,
        range: HopRange,
        direction: Direction,
        budget: &mut ExecutionBudget<'_>,
    ) -> Result<Vec<Node>, ExecutionError> {
        let cap = budget.path_depth_cap;
        let max = range.max.unwrap_or(cap).min(cap);
        let probe_depth = range.max.is_none_or(|requested| requested > cap);
        if range.max.is_some_and(|requested| requested > cap) {
            self.depth_clamped = Some(cap);
        }
        let depth = max + u32::from(probe_depth);
        let cte_limit = (VISITED_LIMIT as u32 * 8 + 1)
            .saturating_mul(depth)
            .min(TRAIL_ROW_LIMIT);
        let types = if hop.kinds.is_empty() {
            &["CALLS"][..]
        } else {
            &hop.kinds[..]
        };
        // Native trail walks treat an undirected range as outbound. Fixed
        // undirected hops continue to scan outgoing and incoming edges.
        let (near, far) = match direction {
            Direction::Inbound => ("target_id", "source_id"),
            _ => ("source_id", "target_id"),
        };
        let placeholders = iter::repeat_n("?", types.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "WITH RECURSIVE bfs(node_id, hop, edge_path) AS (
                SELECT ?1, 0, ''
                UNION
                SELECT e.{far}, bfs.hop + 1,
                       CASE WHEN bfs.edge_path = '' THEN CAST(e.id AS TEXT)
                            ELSE bfs.edge_path || ',' || e.id END
                FROM bfs JOIN edges e ON e.{near} = bfs.node_id
                WHERE bfs.hop < ?2 AND e.type IN ({placeholders})
                  AND instr(',' || bfs.edge_path || ',', ',' || e.id || ',') = 0
                ORDER BY 2 DESC, 1 ASC, 3 ASC
                LIMIT {}
            )
            SELECT DISTINCT {NODE_COLUMNS}, bfs.hop, (SELECT count(*) FROM bfs)
            FROM bfs JOIN nodes n ON n.id = bfs.node_id
            WHERE bfs.hop > 0 ORDER BY bfs.hop, n.id LIMIT {}",
            cte_limit + 1,
            VISITED_LIMIT + 1,
        );
        budget.check()?;
        let context = ProgressContext {
            budget,
            interrupted: Cell::new(None),
        };
        let progress = Progress::new(self.connection, &context);
        let result = (|| {
            let mut statement = self.connection.prepare(&sql)?;
            let parameters = [&start as &dyn ToSql, &depth]
                .into_iter()
                .chain(types.iter().map(|kind| kind as &dyn ToSql));
            let mut rows = statement.query(params_from_iter(parameters))?;
            self.read_trail(&mut rows, cte_limit, range.min, max)
        })();
        drop(progress);
        if let Some(error) = context.interrupted.take() {
            return Err(error);
        }
        budget.check()?;
        Ok(result?)
    }

    fn read_trail(
        &mut self,
        rows: &mut Rows<'_>,
        cte_limit: u32,
        min: u32,
        max: u32,
    ) -> rusqlite::Result<Vec<Node>> {
        let mut nodes = Vec::new();
        for index in 0..=VISITED_LIMIT {
            let Some(row) = rows.next()? else { break };
            if index == VISITED_LIMIT || row.get::<_, u32>(9)? > cte_limit {
                self.trail_truncated = true;
            }
            if index == VISITED_LIMIT {
                break;
            }
            let depth: u32 = row.get(8)?;
            if depth > max {
                self.beyond_depth = true;
            } else if depth >= min {
                nodes.push(Node::read(row, 0)?);
            }
        }
        Ok(nodes)
    }

    pub(super) fn finish(self, result: &mut ResultRows) {
        result.truncated |= self.trail_truncated || self.beyond_depth;
        result.warnings.depth_cap = self.depth_clamped;
        result.warnings.traversal_exhausted = self.trail_truncated;
    }
}

// The query handler exclusively lends its read-batched handle for this call.
// Scope the SQLite callback to one trail statement, including its preparation.
struct Progress<'a>(&'a Connection);

struct ProgressContext<'a, 'budget> {
    budget: &'a ExecutionBudget<'budget>,
    interrupted: Cell<Option<ExecutionError>>,
}

impl<'a> Progress<'a> {
    fn new(
        connection: &'a Connection,
        context: &'a ProgressContext<'_, '_>,
    ) -> Self {
        unsafe extern "C" fn check(context: *mut c_void) -> c_int {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                // SAFETY: The call keeps the context alive until unregistering.
                let context =
                    unsafe { &*context.cast::<ProgressContext<'_, '_>>() };
                match context.budget.check() {
                    Ok(()) => 0,
                    Err(error) => {
                        context.interrupted.set(Some(error));
                        1
                    }
                }
            }))
            .unwrap_or(1)
        }
        // SAFETY: The borrowed handle is live and exclusive. The context
        // outlives the statement and the callback registration.
        unsafe {
            ffi::sqlite3_progress_handler(
                connection.handle(),
                1024,
                Some(check),
                std::ptr::from_ref(context).cast_mut().cast(),
            );
        }
        Self(connection)
    }
}

impl Drop for Progress<'_> {
    fn drop(&mut self) {
        // SAFETY: The connection outlives this registration.
        unsafe {
            ffi::sqlite3_progress_handler(
                self.0.handle(),
                0,
                None,
                std::ptr::null_mut(),
            )
        };
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::cypher_linear::classify;

    const GRAPH: &str = "
        CREATE TABLE generation (id INTEGER);
        INSERT INTO generation VALUES (1);
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
            (1, 'p', 'Start', 'one', 'one', '', 1, 2, '{}'),
            (2, 'p', 'Start', 'two', 'two', '', 1, 2, '{}'),
            (3, 'p', 'Target', 'three', 'three', '', 1, 2, '{}');
        INSERT INTO edges VALUES (1, 'p', 1, 3, 'CALLS', '{}');
    ";

    #[test]
    fn mandatory_join_orders_rows_before_paging() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(GRAPH).unwrap();
        db.execute_batch(
            "INSERT INTO nodes VALUES
                (4, 'p', 'Leaf', 'zeta', 'zeta', '', 1, 2, '{}'),
                (5, 'p', 'Leaf', 'alpha', 'alpha', '', 1, 2, '{}'),
                (6, 'p', 'Target', 'dead', 'dead', '', 1, 2, '{}');
             INSERT INTO edges VALUES
                (2, 'p', 2, 3, 'CALLS', '{}'),
                (3, 'p', 1, 6, 'CALLS', '{}'),
                (4, 'p', 3, 4, 'NEXT', '{}'),
                (5, 'p', 3, 5, 'NEXT', '{}');",
        )
        .unwrap();

        let expected = [
            [b"two".to_vec(), b"alpha".to_vec(), b"NEXT".to_vec()],
            [b"one".to_vec(), b"alpha".to_vec(), b"NEXT".to_vec()],
            [b"two".to_vec(), b"zeta".to_vec(), b"NEXT".to_vec()],
            [b"one".to_vec(), b"zeta".to_vec(), b"NEXT".to_vec()],
        ];
        for join in
            ["(hub)-[r:NEXT]->(leaf:Leaf)", "(leaf:Leaf)<-[r:NEXT]-(hub)"]
        {
            let source = format!(
                "MATCH (s:Start)-[:CALLS]->(hub:Target) MATCH {join}
                 WHERE leaf.name <> 'unused'
                 RETURN s.name AS caller, leaf.name AS target, type(r) AS rel
                 ORDER BY target, caller DESC"
            );
            let rows = classify(&source).unwrap().execute_on(&db, "p").unwrap();
            assert_eq!(rows.columns, ["caller", "target", "rel"]);
            assert_eq!(rows.rows, expected);
            assert!(!rows.truncated);
            for offset in 0..=expected.len() {
                let page = format!("{source} SKIP {offset} LIMIT 1");
                let page =
                    classify(&page).unwrap().execute_on(&db, "p").unwrap();
                let end = (offset + 1).min(expected.len());
                assert_eq!(page.rows, expected[offset..end]);
                assert!(!page.truncated);
            }
            let empty = format!("{source} LIMIT 0");
            assert!(
                classify(&empty)
                    .unwrap()
                    .execute_on(&db, "p")
                    .unwrap()
                    .rows
                    .is_empty()
            );
        }

        // A second clause with both endpoints bound must test the original
        // pair. The same relationship can occur in separate MATCH clauses.
        let bound_pair = classify(
            "MATCH (s:Start)-[:CALLS]->(hub:Target {name: 'three'})
             MATCH (s)-[t:CALLS]->(hub)
             RETURN s.name, t.type ORDER BY s.name",
        )
        .unwrap()
        .execute_on(&db, "p")
        .unwrap();
        assert_eq!(
            bound_pair.rows,
            [
                [b"one".to_vec(), b"CALLS".to_vec()],
                [b"two".to_vec(), b"CALLS".to_vec()]
            ]
        );
    }

    #[test]
    fn mandatory_joins_outside_connected_fixed_hops_use_c() {
        assert!(
            classify(
                "MATCH (a)-[]->(b) MATCH (b)-[]->(c) MATCH (c)-[]->(d) RETURN d.name"
            )
            .is_ok()
        );
        for pattern in [
            "MATCH (a)-[:CALLS]->(b) MATCH (c)-[:CALLS]->(d)",
            "MATCH (a)-[:CALLS]->() MATCH (c)-[:CALLS]->()",
            "MATCH (a)-[:CALLS*1..2]->(b) MATCH (b)-[:CALLS]->(c)",
            "MATCH (a)-[:CALLS]->(b) MATCH (b)-[:CALLS*1]->(c)",
            "MATCH (a) MATCH (a)-[:CALLS]->(b)",
            "MATCH (a)-[:CALLS]->(b) MATCH (b)",
            "MATCH (a)-[:CALLS]->(b)-[:CALLS]->(c) MATCH (c)-[:CALLS]->(d)",
            "MATCH (a)-[:CALLS]->(b) MATCH (b)-[:CALLS]->(c)-[:CALLS]->(d)",
            "MATCH (a)-[]->(b) MATCH (b)-[]->(c) MATCH (d)-[]->(e)",
            "MATCH (a)-[]->(b) MATCH (c)-[]->(d) MATCH (b)-[]->(c)",
            "MATCH (a)-[]->(b) MATCH (b)-[]->(c) MATCH (c)-[:CALLS*1]->(d)",
            "MATCH (a)-[]->(b) MATCH (b)-[]->(c) MATCH (c)-[]->(d)-[]->(e)",
            "MATCH (a)-[]->(b) MATCH (b)-[]->(c) MATCH (c)",
            "MATCH (a)-[]->(b) MATCH (b)-[]->(c) MATCH (c)-[]->(d) MATCH (d)-[]->(e)",
            "MATCH (a)-[]-> (b) MATCH (b)-[]->() MATCH ()-[]->(d)",
            "MATCH (a)-[r:CALLS]->(b) MATCH (b)-[r:CALLS]->(c)",
            "MATCH (a)-[r:CALLS]->(b) MATCH (b)-[]->(c) MATCH (c)-[r:CALLS]->(d)",
        ] {
            let source = format!("{pattern} RETURN a.name");
            assert_eq!(
                classify(&source).unwrap_err(),
                ParseError::Unsupported,
                "{source}"
            );
        }
        assert_eq!(
            classify(
                "MATCH (a)-[]->(b) MATCH (b)-[]->(c) RETURN c.name LIMIT 100001"
            )
            .unwrap_err(),
            ParseError::Unsupported,
        );
    }

    #[test]
    fn three_match_cancellation_interrupts_the_final_adjacency_scan() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(GRAPH).unwrap();
        db.execute_batch(
            "INSERT INTO edges VALUES (2, 'p', 3, 1, 'NEXT', '{}');
             WITH RECURSIVE ids(i) AS (
                 SELECT 3 UNION ALL SELECT i + 1 FROM ids WHERE i < 1100
             )
             INSERT INTO edges SELECT i, 'p', 1, 2, 'LAST', '{}' FROM ids;",
        )
        .unwrap();
        let query = classify(
            "MATCH (s:Start)-[:CALLS]->(hub)
             MATCH (hub)-[:NEXT]->(again)
             MATCH (again)-[:LAST]->(leaf)
             RETURN leaf.name LIMIT 1",
        )
        .unwrap();
        assert_eq!(
            query.execute_on(&db, "p").unwrap().rows,
            [[b"two".to_vec()]]
        );

        // The last edge reaches a node that raises a SQLite read error. The
        // unindexed edge scan reads it after the valid low-ID edges, before
        // the walker emits a row or performs its final cancellation check.
        db.execute_batch(
            "INSERT INTO nodes VALUES
                (4, 'p', 'Tail', 'bad', 'bad', '', 1, 2, '{}');
             INSERT INTO edges VALUES (1101, 'p', 1, 4, 'LAST', '{}');
             CREATE TEMP VIEW nodes AS
             SELECT id, project, label, name, qualified_name, file_path, start_line, end_line,
                    CASE WHEN id = 4 THEN abs(-9223372036854775808) ELSE properties END AS properties
             FROM main.nodes;",
        )
        .unwrap();
        assert!(matches!(
            query.execute_on(&db, "p"),
            Err(ExecutionError::Storage(rusqlite::Error::SqliteFailure(_, Some(message))))
                if message == "integer overflow"
        ));

        let polls = Cell::new(0);
        let cancelled = || {
            polls.set(polls.get() + 1);
            polls.get() == 2
        };
        let mut budget = ExecutionBudget::new(&Instant::now, &cancelled);
        assert!(matches!(
            query.execute_with_budget(&db, "p", &mut budget),
            Err(ExecutionError::Cancelled)
        ));
        assert_eq!(polls.get(), 2);
    }

    #[test]
    fn optional_paths_keep_unbound_columns_and_count_only_bound_aliases() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(GRAPH).unwrap();
        let query = classify(
            "MATCH (s:Start) OPTIONAL MATCH (s)-[r:CALLS]->(e:Target)
             WHERE NOT e:Target RETURN s.name, e.start_line, r.type, count(e)",
        )
        .unwrap();
        let rows = query.execute_on(&db, "p").unwrap().rows;
        assert_eq!(rows, [vec![b"two".to_vec(), vec![], vec![], b"0".to_vec()]]);
    }

    #[test]
    fn trail_timeout_interrupts_sql_and_releases_the_progress_handler() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(GRAPH).unwrap();
        db.execute_batch(
            "WITH RECURSIVE ids(i) AS (SELECT 4 UNION ALL SELECT i+1 FROM ids WHERE i<85)
             INSERT INTO nodes SELECT i, 'p', 'Target', 'dense', 'dense', '', 1, 2, '{}' FROM ids;
             INSERT INTO edges (project, source_id, target_id, type, properties)
             SELECT 'p', a.id, b.id, 'CALLS', '{}' FROM nodes a CROSS JOIN nodes b;",
        )
        .unwrap();

        let query = classify(
            "MATCH (s:Start {name: 'one'})-[:CALLS*2..4]->(e) RETURN e.name",
        )
        .unwrap();
        let checks = Cell::new(0);
        let start = Instant::now();
        let clock = || {
            checks.set(checks.get() + 1);
            if checks.get() <= 3 {
                start
            } else {
                start + Duration::from_secs(31)
            }
        };
        let mut budget = ExecutionBudget::new(&clock, &|| false);
        assert!(matches!(
            query.execute_with_budget(&db, "p", &mut budget),
            Err(ExecutionError::Deadline)
        ));
        // The fourth clock check came from SQLite.
        assert_eq!(checks.get(), 4);

        let cancel_checks = Cell::new(0);
        let cancelled = || {
            cancel_checks.set(cancel_checks.get() + 1);
            cancel_checks.get() == 3
        };
        let mut budget = ExecutionBudget::new(&Instant::now, &cancelled);
        assert!(matches!(
            query.execute_with_budget(&db, "p", &mut budget),
            Err(ExecutionError::Cancelled)
        ));
        assert_eq!(cancel_checks.get(), 3);
        let result = query.execute_on(&db, "p").unwrap();
        assert!(result.truncated);
        assert!(
            result
                .warning()
                .unwrap()
                .contains("traversal budget was exhausted")
        );
    }

    #[test]
    fn path_queries_follow_the_generation_on_the_borrowed_connection() {
        let path = std::env::temp_dir()
            .join(format!("mori-path-snapshot-{}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        let file = path.join("graph.db");
        let writer = Connection::open(&file).unwrap();
        writer.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        writer.execute_batch(GRAPH).unwrap();
        let reader = Connection::open(&file).unwrap();
        reader.execute_batch("BEGIN DEFERRED;").unwrap();
        let generation = || {
            reader
                .query_row("SELECT id FROM generation", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap()
        };
        assert_eq!(generation(), 1);

        writer
            .execute_batch(
                "BEGIN IMMEDIATE;
             UPDATE generation SET id=2;
             DELETE FROM edges;
             INSERT INTO edges VALUES (2, 'p', 2, 3, 'CALLS', '{}');
             COMMIT;",
            )
            .unwrap();
        let query = classify(
            "MATCH (s:Start) OPTIONAL MATCH (s)-[:CALLS*1..2]->(e) RETURN s.name,e.name ORDER BY s.name",
        ).unwrap();
        assert_eq!(
            query.execute_on(&reader, "p").unwrap().rows,
            [
                [b"one".to_vec(), b"three".to_vec()],
                [b"two".to_vec(), vec![]]
            ]
        );
        assert_eq!(generation(), 1);

        reader.execute_batch("COMMIT;").unwrap();
        assert_eq!(generation(), 2);
        assert_eq!(
            query.execute_on(&reader, "p").unwrap().rows,
            [
                [b"one".to_vec(), vec![]],
                [b"two".to_vec(), b"three".to_vec()]
            ]
        );
        drop(reader);
        drop(writer);
        std::fs::remove_dir_all(path).unwrap();
    }
}
