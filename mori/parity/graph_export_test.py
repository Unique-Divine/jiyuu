"""Tests for graph_export: tiny project databases built in temp dirs."""

from __future__ import annotations

import json
import os
import sqlite3
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from graph_export import decode_mori_export, diff_exports, export_db  # noqa: E402
from parity import Side  # noqa: E402

SCHEMA = """
CREATE TABLE projects (name TEXT PRIMARY KEY, indexed_at TEXT,
  root_path TEXT);
CREATE TABLE nodes (id INTEGER PRIMARY KEY, project TEXT, label TEXT,
  name TEXT, qualified_name TEXT, file_path TEXT, start_line INTEGER,
  properties TEXT);
CREATE TABLE edges (id INTEGER PRIMARY KEY, project TEXT,
  source_id INTEGER REFERENCES nodes(id),
  target_id INTEGER REFERENCES nodes(id), type TEXT, properties TEXT);
CREATE TABLE store_meta (k TEXT PRIMARY KEY, v TEXT);
"""

PROJECT = "home-u-repo"
ROOT = "/home/u/repo"


def make_db(path: str, nodes: list[tuple], edges: list[tuple],
            meta: dict[str, str], indexed_at: str = "t0",
            id_offset: int = 0) -> None:
  """nodes: (label, name, file, line, props); edges: (src name, tgt name,
  type, props). Ids start at id_offset + 1 so two databases can hold the
  same graph under different row ids."""
  c = sqlite3.connect(path)
  c.executescript(SCHEMA)
  c.execute("INSERT INTO projects VALUES (?, ?, ?)",
            (PROJECT, indexed_at, ROOT))
  ids = {}
  for i, (label, name, fp, line, props) in enumerate(nodes):
    nid = id_offset + i + 1
    ids[name] = nid
    c.execute("INSERT INTO nodes VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
              (nid, PROJECT, label, name, f"{PROJECT}.{name}", fp, line,
               json.dumps(props)))
  for i, (s, t, typ, props) in enumerate(edges):
    c.execute("INSERT INTO edges VALUES (?, ?, ?, ?, ?, ?)",
              (id_offset + i + 1, PROJECT, ids[s], ids[t], typ,
               json.dumps(props)))
  for k, v in meta.items():
    c.execute("INSERT INTO store_meta VALUES (?, ?)", (k, v))
  c.commit()
  c.close()


NODES = [
  ("Function", "main", "main.go", 1, {"path": f"{ROOT}/main.go"}),
  ("Function", "helper", "util.go", 3, {"importance": 0.1234567}),
]
EDGES = [("main", "helper", "CALLS", {"line": 2, "args": []})]


class ExportDiffTest(unittest.TestCase):
  def setUp(self) -> None:
    self.tmp = tempfile.TemporaryDirectory()
    self.addCleanup(self.tmp.cleanup)

  def db(self, name: str, **kw) -> str:
    path = os.path.join(self.tmp.name, name)
    args = {"nodes": NODES, "edges": EDGES, "meta": {"db_uid": "x"}}
    args.update(kw)
    make_db(path, **args)
    return path

  def keys(self, a: str, b: str) -> set[tuple[str, str, str]]:
    rep = diff_exports(export_db(a), export_db(b))
    return {(d.layer, d.key, d.group) for d in rep.diffs()}

  def test_export_normalizes(self) -> None:
    exp = export_db(self.db("a.db"))
    self.assertEqual(exp["project"], PROJECT)
    props = exp["nodes"][("main", "Function", "main.go")]["properties"]
    self.assertEqual(props["path"], "{{root}}/main.go")
    helper = exp["nodes"][("helper", "Function", "util.go")]["properties"]
    self.assertEqual(helper["importance"], 0.123457)
    self.assertIn(("main", "helper", "CALLS", ""), exp["edges"])
    self.assertEqual(exp["l0"]["edges_by_type"], {"CALLS": 1})

  def test_row_ids_do_not_matter(self) -> None:
    a = self.db("a.db")
    b = self.db("b.db", id_offset=100)
    self.assertEqual(self.keys(a, b), set())

  def test_property_change_is_l2(self) -> None:
    a = self.db("a.db")
    b = self.db("b.db", edges=[("main", "helper", "CALLS",
                                {"line": 9, "args": []})])
    self.assertEqual(self.keys(a, b),
                     {("L2", "edges.properties.line", "CALLS")})

  def test_identity_change_is_l0_and_l1(self) -> None:
    a = self.db("a.db")
    b = self.db("b.db", edges=EDGES + [("helper", "main", "CALLS", {})])
    self.assertEqual(self.keys(a, b), {
      ("L0", "edges.count", "CALLS"),
      ("L1", "edges.only_in_b", "CALLS"),
    })

  def test_side_table_row_is_grouped_by_key(self) -> None:
    a = self.db("a.db", meta={"db_uid": "x", "v": "1"})
    b = self.db("b.db", meta={"db_uid": "y", "v": "1"}, indexed_at="t1")
    self.assertEqual(self.keys(a, b), {
      ("L3", "store_meta.v", "db_uid"),
      ("L3", "projects.indexed_at", "{{project}}"),
    })

  def test_invalid_utf8_is_compared_not_fatal(self) -> None:
    a = self.db("a.db")
    b = self.db("b.db")
    c = sqlite3.connect(b)
    c.execute("UPDATE nodes SET name = CAST(X'4D6172E2' AS TEXT) "
              "WHERE name = 'main'")
    c.commit()
    c.close()
    self.assertEqual(self.keys(a, b), {("L2", "nodes.name", "Function")})

  def test_fts_vocab_failure_is_a_difference_on_both_sides(self) -> None:
    ea = export_db(self.db("a.db"))
    eb = export_db(self.db("b.db"))
    ea["fts_vocab_error"] = eb["fts_vocab_error"] = "no such module"
    got = {(d.layer, d.key) for d in diff_exports(ea, eb).diffs()}
    self.assertIn(("fts", "nodes_fts.vocab_error"), got)

  def test_strict_export_distinguishes_nested_booleans_from_numbers(self) -> None:
    path = self.db("types.db", nodes=[
      ("Function", "main", "main.go", 1, {"options": [{"active": True}]}),
      *NODES[1:],
    ])
    exported = export_db(path)
    node = exported["nodes"][("main", "Function", "main.go")]
    side = Side("b", "unused", "unused", "unused", "rust", True)
    with patch("parity.export_mori", return_value=exported):
      self.assertEqual(side.export(path), exported)
      node["properties"]["options"][0]["active"] = 1
      with self.assertRaisesRegex(ValueError, r"export differs from SQL.*nodes"):
        side.export(path)

  def test_malformed_export_records_raise_value_error_with_line(self) -> None:
    project = '{"kind":"project","name":"p","root":"/p"}'
    cases = [
      (["[]"], "line 1: expected a JSON object"),
      ([project, "null"], "line 2: expected a JSON object"),
      ([project, '{"kind":"count","table":"x","count":true}'], "line 2: count.count"),
      ([project, '{"kind":"row","table":"nodes","key":[{}],"properties":{}}'],
       "line 2: nodes key"),
      ([project, '{"kind":"row","table":"other","properties":{}}'],
       "line 2: no table record"),
    ]
    for lines, error in cases:
      with self.subTest(lines=lines), self.assertRaisesRegex(ValueError, error):
        decode_mori_export(lines)


if __name__ == "__main__":
  unittest.main()
