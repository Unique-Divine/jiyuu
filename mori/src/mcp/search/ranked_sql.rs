// Keep the inner FTS scan independent of the outer node filters so SQLite can
// order and cap the candidate set before joining it to nodes. Use the same
// weights and filters for the page and count. The label weight is zero when
// MATCH already requires that label. Older four-column FTS tables ignore the
// unused body weight.
macro_rules! weights {
    () => {
        "bm25(nodes_fts, 1.0, 1.0, ?11, 1.0, 0.3)"
    };
}

macro_rules! node_filters {
    () => {
        r" JOIN nodes n ON n.id = fts.rowid
           WHERE n.project = ?2
             AND (?7 IS NOT NULL OR n.label NOT IN ('File','Folder','Variable','Project'))
             AND (?6 IS NULL OR n.file_path LIKE ?6)
             AND (?7 IS NULL OR n.label = ?7)
             AND (?9 IS NULL OR (CASE WHEN json_valid(n.properties)
                  THEN json_extract(n.properties,'$.kind') END) = ?9)
             AND (?10 IS NULL OR (CASE WHEN json_valid(n.properties)
                  THEN json_extract(n.properties,'$.obj_key') END)
                  LIKE ('%/%/' || replace(replace(replace(?10,'\','\\'),'%','\%'),
                  '_','\_') || '/%') ESCAPE '\')"
    };
}

pub(super) const PAGE: &str = concat!(
    "SELECT n.id, n.label, n.name, n.qualified_name, n.file_path, n.start_line, n.end_line,
            (fts.base_rank
             - CASE WHEN n.name = ?8 THEN 30.0
                    WHEN lower(n.name) = lower(?8) THEN 20.0 ELSE 0.0 END
             - CASE WHEN n.label IN ('Function','Method') THEN 10.0
                    WHEN n.label = 'Route' THEN 8.0
                    WHEN n.label IN ('Class','Struct','Interface','Enum','Type','Trait',
                                     'Table','View','Model') THEN 5.0
                    ELSE 0.0 END) AS rank
     FROM (SELECT rowid, ",
    weights!(),
    " AS base_rank FROM nodes_fts WHERE nodes_fts MATCH ?1
      ORDER BY base_rank, rowid LIMIT ?5) fts",
    node_filters!(),
    " ORDER BY rank, n.id LIMIT ?3 OFFSET ?4"
);

pub(super) const COUNT: &str = concat!(
    "SELECT COUNT(*) FROM (
         SELECT fts.rowid FROM (
           SELECT rowid FROM nodes_fts WHERE nodes_fts MATCH ?1 ORDER BY ",
    weights!(),
    " LIMIT ?3
       ) fts",
    node_filters!(),
    ")"
);

// The probe needs only the existence of an extra match. It intentionally
// precedes the project and node filters.
pub(super) const PROBE: &str =
    "SELECT rowid FROM nodes_fts WHERE nodes_fts MATCH ?1
    ORDER BY bm25(nodes_fts), rowid LIMIT 1 OFFSET ?2";
