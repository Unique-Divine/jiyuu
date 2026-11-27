//! Parity export for the SQLite graph and its side tables.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::ffi::OsStr;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::store::{Cell, Cells, Store};

const USAGE: &str = "Usage: mori dev export --db <path> [--tables <table,...>]";
type ExportResult<T> = Result<T, Box<dyn Error>>;
type Properties = Map<String, Value>;

struct Normalizer {
    project: String,
    root: String,
}

impl Normalizer {
    fn qualified_name(&self, name: String) -> String {
        if name == self.project {
            return "{{project}}".into();
        }
        name.strip_prefix(&format!("{}.", self.project))
            .map(str::to_owned)
            .unwrap_or(name)
    }

    fn text(&self, mut text: String) -> String {
        if !self.root.is_empty() {
            text = text.replace(&self.root, "{{root}}");
        }
        if !self.project.is_empty() {
            text = text.replace(&self.project, "{{project}}");
        }
        text
    }

    fn real(value: f64) -> Value {
        // Formatting rounds the binary64 value directly to six decimal
        // places. Scaling first can move the value onto the wrong tie.
        match format!("{value:.6}").parse::<f64>() {
            Ok(value) => json!(value),
            Err(_) => Value::Null,
        }
    }

    fn json_value(&self, value: Value) -> Value {
        match value {
            Value::String(text) => self.text(text).into(),
            Value::Array(array) => array
                .into_iter()
                .map(|value| self.json_value(value))
                .collect(),
            Value::Object(object) => object
                .into_iter()
                .map(|(key, value)| (key, self.json_value(value)))
                .collect(),
            Value::Number(number) if number.is_f64() => {
                Self::real(number.as_f64().unwrap())
            }
            other => other,
        }
    }

    fn cell(&self, cell: Cell) -> Value {
        match cell {
            Cell::Null => Value::Null,
            Cell::Integer(value) => value.into(),
            Cell::Real(value) => Self::real(value),
            Cell::Text(text) if text.starts_with(['{', '[']) => {
                match serde_json::from_str::<Value>(&text) {
                    Ok(parsed) => self.json_value(parsed),
                    Err(_) => self.text(text).into(),
                }
            }
            Cell::Text(text) => self.text(text).into(),
            Cell::Blob(value) => {
                let sha = format!("{:x}", Sha256::digest(&value));
                format!("blob:sha256:{}:{}", &sha[..16], value.len()).into()
            }
        }
    }

    fn properties(&self, cells: Cells) -> Properties {
        cells
            .into_iter()
            .map(|(key, value)| (key, self.cell(value)))
            .collect()
    }
}

fn write_jsonl(out: &mut impl Write, value: &Value) -> ExportResult<()> {
    serde_json::to_writer(&mut *out, value)?;
    writeln!(out)?;
    Ok(())
}

fn put_graph_row<K: Ord>(
    rows: &mut BTreeMap<K, Properties>,
    key: K,
    mut properties: Properties,
) {
    if rows.contains_key(&key) {
        properties.insert("__duplicate_key__".into(), true.into());
    }
    rows.insert(key, properties);
}

fn is_count_only(table: &str) -> bool {
    table == "sqlite_sequence"
        || ["nodes_fts", "node_vectors", "token_vectors"]
            .iter()
            .any(|prefix| table.starts_with(prefix))
}

pub(crate) fn export(
    path: &Path,
    selected: &BTreeSet<String>,
    out: &mut impl Write,
) -> ExportResult<()> {
    let store = Store::open(path)?;
    let tables = store.tables()?;
    for table in selected {
        if !tables.contains(table) {
            return Err(format!("unknown table {table:?}").into());
        }
    }
    let include = |table| selected.is_empty() || selected.contains(table);
    let (project, root) = store.identity()?;
    let norm = Normalizer {
        project,
        root: root.trim_end_matches('/').into(),
    };
    if selected.is_empty() {
        write_jsonl(
            out,
            &json!({"kind": "project", "name": norm.project, "root": root}),
        )?;
        for (kind, name, sql) in store.schema()? {
            let key = format!("{kind}:{name}");
            let sql = sql.split_whitespace().collect::<Vec<_>>().join(" ");
            write_jsonl(
                out,
                &json!({"kind": "schema", "key": key, "sql": sql}),
            )?;
        }
    }

    let mut qualified_names = BTreeMap::new();
    if include("nodes") || include("edges") {
        let mut nodes = BTreeMap::new();
        for node in store.nodes()? {
            let name = norm.qualified_name(node.qualified_name);
            qualified_names.insert(node.id, name.clone());
            if include("nodes") {
                put_graph_row(
                    &mut nodes,
                    (name, node.label, node.file_path.replace('\\', "/")),
                    norm.properties(node.properties),
                );
            }
        }
        for ((name, label, path), properties) in nodes {
            write_jsonl(
                out,
                &json!({
                    "kind": "row", "table": "nodes", "key": [name, label, path],
                    "properties": properties,
                }),
            )?;
        }
    }
    if include("edges") {
        let mut edges = BTreeMap::new();
        let endpoint = |id| {
            qualified_names
                .get(&id)
                .cloned()
                .unwrap_or_else(|| format!("<dangling:{id}>"))
        };
        for edge in store.edges()? {
            put_graph_row(
                &mut edges,
                (
                    endpoint(edge.source_id),
                    endpoint(edge.target_id),
                    edge.kind,
                    edge.local_name,
                ),
                norm.properties(edge.properties),
            );
        }
        for ((source, target, kind, local_name), properties) in edges {
            write_jsonl(
                out,
                &json!({
                    "kind": "row", "table": "edges", "key": [source, target, kind, local_name],
                    "properties": properties,
                }),
            )?;
        }
    }

    for table in tables.iter().filter(|table| include(table.as_str())) {
        if table == "nodes" || table == "edges" {
            continue;
        }
        let count = store.count(table)?;
        if is_count_only(table) {
            write_jsonl(
                out,
                &json!({"kind": "count", "table": table, "count": count}),
            )?;
            continue;
        }
        let mut primary_key: Vec<_> = store
            .columns(table)?
            .into_iter()
            .filter(|column| column.pk != 0)
            .collect();
        primary_key.sort_by_key(|column| column.pk);
        let key_columns: Vec<_> =
            primary_key.into_iter().map(|column| column.name).collect();
        write_jsonl(
            out,
            &json!({
                "kind": "table", "table": table, "key": key_columns, "count": count,
            }),
        )?;
        let mut rows = BTreeMap::new();
        let mut unkeyed = Vec::new();
        for cells in store.rows(table)? {
            let mut properties = norm.properties(cells);
            if key_columns.is_empty() {
                unkeyed.push(serde_json::to_string(&properties)?);
            } else {
                let key: Vec<_> = key_columns
                    .iter()
                    .map(|name| properties.remove(name).unwrap_or(Value::Null))
                    .collect();
                let order = key.iter().map(Value::to_string).collect::<Vec<_>>();
                rows.insert(order, (key, properties));
            }
        }
        for (key, properties) in rows.into_values() {
            write_jsonl(
                out,
                &json!({
                    "kind": "row", "table": table, "key": key, "properties": properties,
                }),
            )?;
        }
        unkeyed.sort_unstable();
        for row in unkeyed {
            write_jsonl(
                out,
                &json!({
                    "kind": "row", "table": table, "properties": serde_json::from_str::<Value>(&row)?,
                }),
            )?;
        }
    }
    if tables.iter().any(|table| table == "nodes_fts") && include("nodes_fts") {
        for (term, doc, count) in store.fts_vocabulary()? {
            write_jsonl(
                out,
                &json!({"kind": "fts", "term": term, "doc": doc, "count": count}),
            )?;
        }
    }
    Ok(())
}

fn parse<'a>(
    mut args: impl Iterator<Item = &'a OsStr>,
) -> Result<(PathBuf, BTreeSet<String>), String> {
    if args.next() != Some(OsStr::new("export")) {
        return Err("expected dev export".into());
    }
    let mut db = None;
    let mut tables = BTreeSet::new();
    while let Some(arg) = args.next() {
        let value = args.next().ok_or_else(|| {
            format!("missing value for {}", arg.to_string_lossy())
        })?;
        match arg.to_str() {
            Some("--db") if db.is_none() => db = Some(PathBuf::from(value)),
            Some("--tables") => {
                let value = value.to_str().ok_or("table names must be UTF-8")?;
                for name in value.split(',') {
                    if name.is_empty() {
                        return Err("empty table name".into());
                    }
                    tables.insert(name.to_owned());
                }
            }
            _ => {
                return Err(format!(
                    "unexpected argument {}",
                    arg.to_string_lossy()
                ));
            }
        }
    }
    Ok((db.ok_or("--db is required")?, tables))
}

pub(crate) fn run<'a>(args: impl Iterator<Item = &'a OsStr>) -> i32 {
    let (path, tables) = match parse(args) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("mori: {error}\n{USAGE}");
            return 2;
        }
    };
    let mut out = BufWriter::new(io::stdout().lock());
    match export(&path, &tables, &mut out).and_then(|_| Ok(out.flush()?)) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("mori: export {}: {error}", path.display());
            1
        }
    }
}
