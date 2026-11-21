#!/usr/bin/env python3
"""Export and diff Mori (Codebase Memory) project databases, layers L0-L3.

Zero dependencies (stdlib only). Run with `python3 -I`.

The schema is discovered from `sqlite_master` and `PRAGMA table_xinfo`, not
assumed. Layers follow the port design, section 4:

  schema  every `sqlite_master` entry, whitespace-normalized SQL
  L0      counts: nodes per label, edges per type
  L1      identity: node (qualified_name, label, file_path); edge (source
          qn, target qn, type, local_name_gen). Project prefix stripped, row
          ids never used.
  L2      properties: every other node and edge column; JSON columns
          parsed, keys sorted, floats to 6 places, root path -> {{root}}.
  L3      side tables keyed by primary key; FTS shadow tables and vector
          tables compared by row count only (layer `counts`), and FTS
          logical content through fts5vocab 'row' (layer `fts`).

Every difference is a `Diff` with a layer, a key (`table.field`), and a
group (label, edge type, or `-`). The allow-list matches on those; nothing
is ignored while diffing.

Usage:
  python3 -I graph_export.py counts DB
  python3 -I graph_export.py export DB OUTDIR
  python3 -I graph_export.py diff DB_A DB_B [--examples N] [--json-out F]
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sqlite3
import subprocess
import sys
import tempfile
from collections import Counter
from dataclasses import dataclass, field
from typing import Any, Iterable

FLOAT_PLACES = 6
# Tables compared by row count only (ids or opaque derived content).
COUNT_ONLY_PREFIXES = ("nodes_fts", "node_vectors", "token_vectors")
COUNT_ONLY_TABLES = {"sqlite_sequence"}
LAYERS = ("schema", "L0", "L1", "L2", "L3", "fts", "counts")


def open_ro(path: str) -> sqlite3.Connection:
  if not os.path.isfile(path):
    raise FileNotFoundError(f"no database at {path}")
  conn = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
  conn.row_factory = sqlite3.Row
  # The C store can hold invalid UTF-8 (a docstring cut mid-character);
  # keep the bad bytes visible and comparable instead of failing.
  conn.text_factory = lambda b: b.decode("utf-8", "backslashreplace")
  return conn


def table_names(conn: sqlite3.Connection) -> list[str]:
  rows = conn.execute(
    "SELECT name FROM sqlite_master WHERE type='table' ORDER BY name"
  )
  return [r[0] for r in rows]


def columns(conn: sqlite3.Connection, table: str) -> list[dict[str, Any]]:
  """Columns including generated ones (hidden 2 or 3)."""
  out = []
  for r in conn.execute(f'PRAGMA table_xinfo("{table}")'):
    out.append({
      "name": r["name"], "type": (r["type"] or "").upper(),
      "pk": r["pk"], "hidden": r["hidden"],
    })
  return out


def is_count_only(table: str) -> bool:
  return table in COUNT_ONLY_TABLES or table.startswith(COUNT_ONLY_PREFIXES)


class Normalizer:
  """Strips the project prefix and root path; canonicalizes values."""

  def __init__(self, project: str, root: str) -> None:
    self.project = project
    self.root = root.rstrip("/")

  def qn(self, qn: str) -> str:
    if qn == self.project:
      return "{{project}}"
    pre = self.project + "."
    return qn[len(pre):] if qn.startswith(pre) else qn

  def value(self, v: Any) -> Any:
    if isinstance(v, float):
      return round(v, FLOAT_PLACES)
    if isinstance(v, (bytes, memoryview)):
      b = bytes(v)
      return f"blob:sha256:{hashlib.sha256(b).hexdigest()[:16]}:{len(b)}"
    if isinstance(v, str):
      if self.root and self.root in v:
        v = v.replace(self.root, "{{root}}")
      if self.project and self.project in v:
        v = v.replace(self.project, "{{project}}")
      return v
    return v

  def json_value(self, v: Any) -> Any:
    if isinstance(v, dict):
      return {k: self.json_value(v[k]) for k in sorted(v)}
    if isinstance(v, list):
      return [self.json_value(x) for x in v]
    return self.value(v)

  def maybe_json(self, v: Any) -> Any:
    """Parse JSON object/array text; else normalize as a scalar."""
    if isinstance(v, str) and v[:1] in ("{", "["):
      try:
        return self.json_value(json.loads(v))
      except ValueError:
        pass
    return self.value(v)


def project_identity(conn: sqlite3.Connection) -> tuple[str, str]:
  """(name, root_path) of the database's own project. A database can hold a
  second `<project>::missed` row; the shortest name is the project."""
  tables = set(table_names(conn))
  if "projects" in tables:
    cols = {c["name"] for c in columns(conn, "projects")}
    if {"name", "root_path"} <= cols:
      rows = conn.execute(
        "SELECT name, root_path FROM projects ORDER BY length(name), name"
      ).fetchall()
      if rows:
        return rows[0][0], rows[0][1]
  return "", ""


def node_id_column(conn: sqlite3.Connection) -> str:
  cols = columns(conn, "nodes")
  pk = [c["name"] for c in cols if c["pk"]]
  return pk[0] if pk else "rowid"


def edge_endpoint_columns(conn: sqlite3.Connection) -> list[str]:
  """Edge columns that are foreign keys into nodes, in declaration order.
  PRAGMA foreign_key_list lists them in reverse, so sort by column
  position; the first is the source."""
  pos = {c["name"]: i for i, c in enumerate(columns(conn, "edges"))}
  out = []
  for r in conn.execute('PRAGMA foreign_key_list("edges")'):
    if r["table"] == "nodes":
      out.append(r["from"])
  return sorted(out, key=lambda n: pos.get(n, 0))


def export_db(path: str) -> dict[str, Any]:
  """Return the normalized export of one project database."""
  conn = open_ro(path)
  try:
    return _export(conn)
  finally:
    conn.close()


def new_export(project: str, root: str) -> dict[str, Any]:
  return {
    "project": project, "root": root, "schema": {}, "nodes": {},
    "edges": {}, "side": {}, "counts_only": {}, "l0": {},
  }


def set_graph_counts(result: dict[str, Any]) -> None:
  result["l0"]["nodes_by_label"] = dict(sorted(Counter(
    k[1] for k in result["nodes"]).items()))
  result["l0"]["edges_by_type"] = dict(sorted(Counter(
    k[2] for k in result["edges"]).items()))


def same_export_value(a: Any, b: Any) -> bool:
  """Compare normalized export values without Python's numeric coercions."""
  if type(a) is not type(b):
    return False
  if isinstance(a, dict):
    # Decoder keys are strings or tuples of strings.
    return a.keys() == b.keys() and all(same_export_value(a[k], b[k]) for k in a)
  if isinstance(a, (list, tuple)):
    return len(a) == len(b) and all(same_export_value(x, y) for x, y in zip(a, b))
  return a == b


EXPORT_RECORD_FIELDS = {
  "project": {"name": str, "root": str},
  "schema": {"key": str, "sql": str},
  "count": {"table": str, "count": int},
  "table": {"table": str, "key": list, "count": int},
  "row": {"table": str, "properties": dict},
  "fts": {"term": str, "doc": int, "count": int},
}


def read_export_record(line: str, number: int) -> dict[str, Any]:
  try:
    row = json.loads(line)
  except ValueError as e:
    raise ValueError(f"Mori export line {number}: {e}") from e
  if not isinstance(row, dict):
    raise ValueError(f"Mori export line {number}: expected a JSON object")
  kind = row.get("kind")
  if not isinstance(kind, str) or kind not in EXPORT_RECORD_FIELDS:
    raise ValueError(f"Mori export line {number}: unknown record kind {kind!r}")
  for name, want in EXPORT_RECORD_FIELDS[kind].items():
    if type(row.get(name)) is not want:
      raise ValueError(f"Mori export line {number}: {kind}.{name} must be {want.__name__}")
  return row


def decode_mori_export(lines: Iterable[str]) -> dict[str, Any]:
  """Read the sorted JSON Lines emitted by `mori dev export`."""
  stream = enumerate(lines, 1)
  _, first_line = next(stream, (1, ""))
  if not first_line:
    raise ValueError("Mori export line 1: missing project record")
  first = read_export_record(first_line, 1)
  if first["kind"] != "project":
    raise ValueError("Mori export line 1: expected a project record")
  result = new_export(first["name"], first["root"])
  for number, line in stream:
    row = read_export_record(line, number)
    kind = row["kind"]
    if kind == "schema":
      result["schema"][row["key"]] = row["sql"]
    elif kind == "count":
      result["counts_only"][row["table"]] = row["count"]
    elif kind == "table":
      if any(not isinstance(key, str) for key in row["key"]):
        raise ValueError(f"Mori export line {number}: table.key must contain strings")
      result["side"][row["table"]] = {
        "key": row["key"] or None, "rows": {}, "multiset": Counter(),
        "count": row["count"],
      }
    elif kind == "row":
      table = row["table"]
      props = row["properties"]
      key = row.get("key")
      if table in ("nodes", "edges"):
        want = 3 if table == "nodes" else 4
        if not isinstance(key, list) or len(key) != want or any(
            not isinstance(value, str) for value in key):
          raise ValueError(f"Mori export line {number}: {table} key must contain {want} strings")
        result[table][tuple(key)] = props
      else:
        side = result["side"].get(table)
        if side is None:
          raise ValueError(f"Mori export line {number}: no table record for {table!r}")
        if side["key"]:
          if not isinstance(key, list) or len(key) != len(side["key"]):
            raise ValueError(f"Mori export line {number}: invalid key for {table!r}")
          key = tuple(json.dumps(value, sort_keys=True) for value in key)
          side["rows"][key] = props
        else:
          side["multiset"][json.dumps(props, sort_keys=True)] += 1
    elif kind == "fts":
      result.setdefault("fts_vocab", {})[row["term"]] = (row["doc"], row["count"])
    else:
      raise ValueError(f"Mori export line {number}: unexpected {kind!r} record")
  if "nodes_fts" in result["counts_only"]:
    result.setdefault("fts_vocab", {})
  set_graph_counts(result)
  return result


def export_mori(path: str, binary: str, env: dict[str, str], timeout: int
                ) -> dict[str, Any]:
  with tempfile.TemporaryFile(mode="w+", encoding="utf-8") as rows:
    process = subprocess.run(
      [binary, "dev", "export", "--db", path], cwd="/", env=env,
      stdout=rows, stderr=subprocess.PIPE, text=True, timeout=timeout,
      stdin=subprocess.DEVNULL)
    if process.returncode:
      raise ValueError(f"{binary} dev export exited {process.returncode}: "
                       f"{process.stderr.strip()}")
    rows.seek(0)
    return decode_mori_export(rows)


def _export(conn: sqlite3.Connection) -> dict[str, Any]:
  project, root = project_identity(conn)
  norm = Normalizer(project, root)
  tables = table_names(conn)
  result = new_export(project, root)

  for r in conn.execute(
    "SELECT type, name, sql FROM sqlite_master ORDER BY type, name"
  ):
    result["schema"][f"{r[0]}:{r[1]}"] = " ".join((r[2] or "").split())

  # Nodes: identity key and properties.
  id_col = node_id_column(conn)
  ncols = columns(conn, "nodes")
  id_to_qn: dict[int, str] = {}
  skip = {id_col, "project", "qualified_name", "label", "file_path"}
  for row in conn.execute("SELECT * FROM nodes"):
    d = dict(row)
    qn = norm.qn(d["qualified_name"])
    id_to_qn[d[id_col]] = qn
    fp = (d.get("file_path") or "").replace("\\", "/")
    key = (qn, d.get("label", ""), fp)
    props = {c["name"]: norm.maybe_json(d.get(c["name"]))
             for c in ncols if c["name"] not in skip}
    if key in result["nodes"]:
      props["__duplicate_key__"] = True
    result["nodes"][key] = props

  # Edges: endpoints by qualified name.
  ecols = columns(conn, "edges")
  ep = edge_endpoint_columns(conn) or ["source_id", "target_id"]
  src_col, tgt_col = ep[0], ep[1]
  e_skip = {c["name"] for c in ecols if c["pk"]}
  e_skip |= {src_col, tgt_col, "project", "type", "local_name_gen"}
  has_lng = any(c["name"] == "local_name_gen" for c in ecols)
  names = [c["name"] for c in ecols]
  sel = ", ".join(f'"{n}"' for n in names)
  for row in conn.execute(f"SELECT {sel} FROM edges"):
    d = dict(zip(names, row))
    s = id_to_qn.get(d[src_col], f"<dangling:{d[src_col]}>")
    t = id_to_qn.get(d[tgt_col], f"<dangling:{d[tgt_col]}>")
    lng = d.get("local_name_gen") if has_lng else ""
    key = (s, t, d.get("type", ""), lng or "")
    props = {n: norm.maybe_json(d.get(n)) for n in names if n not in e_skip}
    if key in result["edges"]:
      props["__duplicate_key__"] = True
    result["edges"][key] = props

  set_graph_counts(result)

  # FTS logical content, independent of segment layout and rowids.
  if "nodes_fts" in tables:
    try:
      conn.execute("CREATE VIRTUAL TABLE temp.cbm_vocab USING "
                   "fts5vocab(main, nodes_fts, 'row')")
      result["fts_vocab"] = {r[0]: (r[1], r[2]) for r in conn.execute(
        "SELECT term, doc, cnt FROM temp.cbm_vocab")}
    except sqlite3.Error as e:
      result["fts_vocab_error"] = str(e)

  for t in tables:
    if t in ("nodes", "edges"):
      continue
    cnt = conn.execute(f'SELECT count(*) FROM "{t}"').fetchone()[0]
    if is_count_only(t):
      result["counts_only"][t] = cnt
      continue
    cols = columns(conn, t)
    cnames = [c["name"] for c in cols]
    pk = [c["name"] for c in sorted(cols, key=lambda c: c["pk"]) if c["pk"]]
    rows: dict[Any, dict[str, Any]] = {}
    multi: Counter = Counter()
    sel = ", ".join(f'"{n}"' for n in cnames)
    for row in conn.execute(f'SELECT {sel} FROM "{t}"'):
      nd = {n: norm.maybe_json(v) for n, v in zip(cnames, row)}
      if pk:
        key = tuple(json.dumps(nd[k], sort_keys=True) for k in pk)
        rows[key] = {n: v for n, v in nd.items() if n not in pk}
      else:
        multi[json.dumps(nd, sort_keys=True)] += 1
    result["side"][t] = {"key": pk or None, "rows": rows,
                         "multiset": multi, "count": cnt}
  return result


def short(v: Any, n: int = 160) -> str:
  s = v if isinstance(v, str) else json.dumps(v, sort_keys=True)
  return s if len(s) <= n else s[: n - 3] + "..."


@dataclass
class Diff:
  """All differences sharing (layer, key, group, row), with a few
  examples. `row` is set only for L4 result-table rows, so each differing
  row is classified by itself."""
  layer: str
  key: str
  group: str
  count: int = 0
  examples: list[dict[str, str]] = field(default_factory=list)
  row: str = ""


class DiffReport:
  def __init__(self, examples: int = 2) -> None:
    self.examples = examples
    self.items: dict[tuple[str, str, str, str], Diff] = {}

  def add(self, layer: str, key: str, group: str, a: Any, b: Any,
          where: str = "", row: str = "") -> None:
    k = (layer, key, group, row)
    d = self.items.get(k)
    if d is None:
      d = self.items[k] = Diff(layer, key, group, row=row)
    d.count += 1
    if len(d.examples) < self.examples:
      d.examples.append({"where": short(where, 200), "a": short(a),
                         "b": short(b)})

  def diffs(self) -> list[Diff]:
    return [self.items[k] for k in sorted(self.items)]


def row_group(key: tuple[str, ...]) -> str:
  """An L3 row's primary key as a group, so the allow-list can accept one
  row (`store_meta` row `db_uid`) without accepting the whole column."""
  parts = []
  for k in key:
    v = json.loads(k)
    parts.append(v if isinstance(v, str) else json.dumps(v))
  return "|".join(parts)


def diff_props(rep: DiffReport, layer: str, table: str, group: str,
               where: str, pa: dict[str, Any], pb: dict[str, Any]) -> None:
  """Diff column values; JSON object columns are diffed per top-level key
  so `edges.properties.args` and `edges.properties.line` stay apart."""
  for col in sorted(set(pa) | set(pb)):
    va, vb = pa.get(col, "<absent>"), pb.get(col, "<absent>")
    if isinstance(va, dict) and isinstance(vb, dict):
      for jk in sorted(set(va) | set(vb)):
        ja, jb = va.get(jk, "<absent>"), vb.get(jk, "<absent>")
        if ja != jb:
          rep.add(layer, f"{table}.{col}.{jk}", group, ja, jb, where)
    elif va != vb:
      rep.add(layer, f"{table}.{col}", group, va, vb, where)


def diff_exports(a: dict[str, Any], b: dict[str, Any],
                 examples: int = 2) -> DiffReport:
  rep = DiffReport(examples)
  for k in sorted(set(a["schema"]) | set(b["schema"])):
    sa, sb = a["schema"].get(k, "<absent>"), b["schema"].get(k, "<absent>")
    if sa != sb:
      rep.add("schema", f"sqlite_master.{k}", k.split(":")[0], sa, sb, k)

  for kind, table in (("nodes_by_label", "nodes"), ("edges_by_type",
                                                    "edges")):
    ca, cb = a["l0"][kind], b["l0"][kind]
    for g in sorted(set(ca) | set(cb)):
      if ca.get(g, 0) != cb.get(g, 0):
        rep.add("L0", f"{table}.count", g, ca.get(g, 0), cb.get(g, 0), g)

  for table, gi in (("nodes", 1), ("edges", 2)):
    ka, kb = set(a[table]), set(b[table])
    for k in sorted(ka - kb):
      rep.add("L1", f"{table}.only_in_a", k[gi], list(k), None, k[0])
    for k in sorted(kb - ka):
      rep.add("L1", f"{table}.only_in_b", k[gi], None, list(k), k[0])
    for k in sorted(ka & kb):
      pa, pb = a[table][k], b[table][k]
      if pa != pb:
        diff_props(rep, "L2", table, k[gi], " | ".join(map(str, k)), pa, pb)

  ea, eb = a.get("fts_vocab_error"), b.get("fts_vocab_error")
  if ea or eb:
    # FTS content went unchecked: a difference even when both failed.
    rep.add("fts", "nodes_fts.vocab_error", "-", ea, eb)
  va, vb = a.get("fts_vocab"), b.get("fts_vocab")
  if (va is None) != (vb is None):
    rep.add("fts", "nodes_fts.vocab_presence", "-", va is not None,
            vb is not None)
  elif va is not None and vb is not None:
    for term in sorted(set(va) | set(vb)):
      if va.get(term) != vb.get(term):
        rep.add("fts", "nodes_fts.term", "-", va.get(term), vb.get(term),
                term)

  for t in sorted(set(a["counts_only"]) | set(b["counts_only"])):
    ca, cb = a["counts_only"].get(t), b["counts_only"].get(t)
    if ca != cb:
      rep.add("counts", f"{t}.count", "-", ca, cb, t)

  for t in sorted(set(a["side"]) | set(b["side"])):
    sa, sb = a["side"].get(t), b["side"].get(t)
    if sa is None or sb is None:
      rep.add("L3", f"{t}.table_presence", "-", sa is not None,
              sb is not None, t)
      continue
    if sa["key"]:
      ra, rb = sa["rows"], sb["rows"]
      for k in sorted(set(ra) - set(rb)):
        rep.add("L3", f"{t}.only_in_a", row_group(k), k, None, str(k))
      for k in sorted(set(rb) - set(ra)):
        rep.add("L3", f"{t}.only_in_b", row_group(k), None, k, str(k))
      for k in sorted(set(ra) & set(rb)):
        if ra[k] != rb[k]:
          diff_props(rep, "L3", t, row_group(k), str(k), ra[k], rb[k])
    else:
      ma, mb = sa["multiset"], sb["multiset"]
      for r in sorted((ma - mb).elements()):
        rep.add("L3", f"{t}.row_only_in_a", "-", r, None)
      for r in sorted((mb - ma).elements()):
        rep.add("L3", f"{t}.row_only_in_b", "-", None, r)
  return rep


def print_counts(exp: dict[str, Any]) -> None:
  print(f"project: {exp['project']}")
  print(f"nodes: {len(exp['nodes'])}  edges: {len(exp['edges'])}")
  print("nodes by label:")
  for k, v in exp["l0"]["nodes_by_label"].items():
    print(f"  {k}: {v}")
  print("edges by type:")
  for k, v in exp["l0"]["edges_by_type"].items():
    print(f"  {k}: {v}")
  print("side tables (rows):")
  for t, s in sorted(exp["side"].items()):
    print(f"  {t}: {s['count']}")
  for t, c in sorted(exp["counts_only"].items()):
    print(f"  {t}: {c} (count only)")


def write_jsonl(path: str, rows: Iterable[Any]) -> None:
  with open(path, "w", encoding="utf-8") as f:
    for r in rows:
      f.write(json.dumps(r, sort_keys=True, ensure_ascii=False) + "\n")


def cmd_export(db: str, outdir: str) -> None:
  exp = export_db(db)
  os.makedirs(outdir, mode=0o700, exist_ok=True)
  write_jsonl(os.path.join(outdir, "nodes.jsonl"), (
    {"qn": k[0], "label": k[1], "file_path": k[2], **exp["nodes"][k]}
    for k in sorted(exp["nodes"])))
  write_jsonl(os.path.join(outdir, "edges.jsonl"), (
    {"src": k[0], "tgt": k[1], "type": k[2], "local_name_gen": k[3],
     **exp["edges"][k]} for k in sorted(exp["edges"])))
  for t, s in sorted(exp["side"].items()):
    if s["key"]:
      rows = ({**dict(zip(s["key"], map(json.loads, k))), **s["rows"][k]}
              for k in sorted(s["rows"]))
    else:
      rows = (json.loads(r) for r in sorted(s["multiset"].elements()))
    write_jsonl(os.path.join(outdir, f"{t}.jsonl"), rows)
  with open(os.path.join(outdir, "l0.json"), "w", encoding="utf-8") as f:
    json.dump({"project": exp["project"], **exp["l0"],
               "counts_only": exp["counts_only"],
               "schema": exp["schema"]}, f, indent=1, sort_keys=True)
  print(f"wrote {outdir}")


def print_diffs(diffs: list[Diff], examples: int = 2) -> None:
  for d in diffs:
    print(f"[{d.layer}] {d.key} ({d.group}): {d.count}")
    for ex in d.examples[:examples]:
      print(f"    e.g. {ex['where'][:120]}")
      print(f"      a: {ex['a']}")
      print(f"      b: {ex['b']}")


def main(argv: list[str]) -> int:
  ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
  sub = ap.add_subparsers(dest="cmd", required=True)
  p = sub.add_parser("counts")
  p.add_argument("db")
  p = sub.add_parser("export")
  p.add_argument("db")
  p.add_argument("outdir")
  p = sub.add_parser("diff")
  p.add_argument("db_a")
  p.add_argument("db_b")
  p.add_argument("--examples", type=int, default=2)
  p.add_argument("--json-out")
  args = ap.parse_args(argv)
  try:
    if args.cmd == "counts":
      print_counts(export_db(args.db))
    elif args.cmd == "export":
      cmd_export(args.db, args.outdir)
    else:
      rep = diff_exports(export_db(args.db_a), export_db(args.db_b),
                         args.examples)
      diffs = rep.diffs()
      print_diffs(diffs, args.examples)
      if args.json_out:
        with open(args.json_out, "w", encoding="utf-8") as f:
          json.dump([d.__dict__ for d in diffs], f, indent=1)
      return 1 if diffs else 0
  except (sqlite3.Error, FileNotFoundError) as e:
    print(f"error: {e}", file=sys.stderr)
    return 2
  return 0


if __name__ == "__main__":
  sys.exit(main(sys.argv[1:]))
