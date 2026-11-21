//! Read tools implemented against the existing graph database.

use serde::Serialize;
use serde_json::Value;

use crate::store::{SchemaGroup, Store};

const ADR_HINT: &str = concat!(
    "No ADR found. Use manage_adr(mode='update') to persist architectural ",
    "decisions across sessions. Run get_architecture(aspects=['all']) first."
);

#[derive(Serialize)]
struct NodeLabel {
    label: String,
    count: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    properties: Option<Vec<String>>,
}

#[derive(Serialize)]
struct EdgeType {
    r#type: String,
    count: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    properties: Option<Vec<String>>,
}

// Field order also determines the order in the compact tree projection.
#[derive(Serialize)]
struct GraphSchema {
    node_labels: Vec<NodeLabel>,
    edge_types: Vec<EdgeType>,
    total: usize,
    returned: usize,
    has_more: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_offset: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    adr_present: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    adr_hint: Option<&'static str>,
}

fn int_arg(args: &Value, key: &str, default: i32) -> i32 {
    args[key]
        .as_i64()
        .and_then(|value| i32::try_from(value).ok())
        .unwrap_or(default)
}

pub(crate) fn graph_schema(
    store: &Store,
    project: &str,
    args: &Value,
    adr_present: Option<bool>,
) -> Option<String> {
    let include_properties = args["diagnostics"] == "full";
    let limit = int_arg(args, "limit", 50).clamp(1, 500) as usize;
    let offset = int_arg(args, "offset", 0).max(0) as usize;

    let labels = store.schema_counts(project, SchemaGroup::NodeLabel).ok()?;
    let types = store.schema_counts(project, SchemaGroup::EdgeType).ok()?;
    let total = labels.len() + types.len();
    let start = offset.min(total);
    let end = (start + limit).min(total);
    let mut answer = GraphSchema {
        node_labels: Vec::new(),
        edge_types: Vec::new(),
        total,
        returned: end - start,
        has_more: end < total,
        next_offset: (end < total).then_some(end),
        adr_present,
        adr_hint: (include_properties && adr_present == Some(false))
            .then_some(ADR_HINT),
    };

    let label_page = labels
        .into_iter()
        .map(|entry| (SchemaGroup::NodeLabel, entry));
    let type_page = types
        .into_iter()
        .map(|entry| (SchemaGroup::EdgeType, entry));
    for (group, (name, count)) in
        label_page.chain(type_page).skip(start).take(end - start)
    {
        let properties = if include_properties {
            Some(store.schema_properties(project, group, &name).ok()?)
        } else {
            None
        };
        match group {
            SchemaGroup::NodeLabel => answer.node_labels.push(NodeLabel {
                label: name,
                count,
                properties,
            }),
            SchemaGroup::EdgeType => answer.edge_types.push(EdgeType {
                r#type: name,
                count,
                properties,
            }),
        }
    }
    serde_json::to_string(&answer).ok()
}
