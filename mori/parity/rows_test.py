"""Tests for row-level gating of search_graph and query_graph output."""

from __future__ import annotations

import json
import os
import re
import sys
import unittest
from typing import Any

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import calls  # noqa: E402
import rows  # noqa: E402
from graph_export import DiffReport  # noqa: E402

NORM = calls.OutputNormalizer({}, {}, {})

RANKED = """results: 4  (cols: qn label file lines rank)
  p.lib.stats.mean Function lib/stats.ts 3-5 -42.67
  p.lib.stats.Summary.mean Method lib/stats.ts 9-11 -12.5
  p.docs.guide.Stats Section docs/guide.md 3-4 -12.5
  p.docs.guide.Math Section docs/guide.md 6-7 -0.64
total: 4
total_relation: eq
search_mode: bm25
returned: 4
has_more: false
truncated: false
"""


def both(fn: Any) -> Any:
  """A per-side rerun from a function of one argument dict that returns
  (a, b) outputs."""
  def run(per_side: list[Any]) -> list[Any]:
    args = next(a for a in per_side if a is not None)
    pair = fn(args)
    return [pair[i] if per_side[i] is not None else None for i in (0, 1)]
  return run


def out(text: str, code: int = 0) -> dict[str, Any]:
  return NORM.output(code, text, "")


def keys(rep: DiffReport) -> list[tuple[str, str]]:
  return sorted((d.key, d.row) for d in rep.diffs())


def diff(tool: str, args: dict[str, Any], a: str, b: str,
         rerun: Any = None) -> DiffReport:
  rep = DiffReport()
  calls.diff_call(rep, tool, args, "call:000000000000", "#0", out(a),
                  out(b), both(rerun) if rerun else None)
  return rep


def corrupt(text: str) -> str:
  """What a broken build might return: same counts, wrong rows."""
  text = text.replace(" Function ", " Variable ").replace(".ts ", ".WRONG ")
  return re.sub(r" (\d+)-(\d+) ", lambda m: f" {m.group(1)}-"
                f"{int(m.group(2)) + 99} ", text)


class ParseTest(unittest.TestCase):
  def test_refs_are_expanded_and_dropped(self) -> None:
    text = ("results_refs: 2  (cols: id prefix)\n"
            "  0 p.web.client.\n"
            "  1 web/client.ts\n"
            "results_ref_rule: @N+suffix=prefix+suffix\n"
            "results: 2  (cols: qn label file lines in out)\n"
            "  @0+Item Interface @1+ 1-3 1 0\n"
            "  \"@0+literal\" Variable x.ts 1-1 0 0\n"
            "total: 2\n")
    tables, rest = calls.split_tables(out(text)["out"])
    self.assertEqual(list(tables), ["results"])
    self.assertEqual(rows.cells(tables["results"].rows[0]),
                     ["p.web.client.Item", "Interface", "web/client.ts",
                      "1-3", "1", "0"])
    self.assertEqual(rows.cells(tables["results"].rows[1])[0], "@0+literal")
    self.assertEqual(list(rest), ["results: 2", "total: 2"])

  def test_grouped_and_flat_forms_compare_equal(self) -> None:
    grouped = ('results: 3  (rows: name label lines in out; group prefix '
               '"-" means empty; qn = prefix empty ? name : prefix + "." + '
               'name)\n'
               'p.docs.guide (docs/guide.md):\n'
               '  Guide Section 1-2 0 0\n'
               '  Math Section 7-8 0 0\n'
               '- ("a b.md"):\n'
               '  Top Section 1-1 0 0\n'
               'total: 3\n')
    flat = ("results: 3  (cols: qn label file lines in out)\n"
            "  p.docs.guide.Guide Section docs/guide.md 1-2 0 0\n"
            "  p.docs.guide.Math Section docs/guide.md 7-8 0 0\n"
            '  Top Section "a b.md" 1-1 0 0\n'
            "total: 3\n")
    args = {"label": "Section"}
    self.assertEqual(keys(diff("search_graph", args, grouped, flat)), [])
    got = keys(diff("search_graph", args, grouped,
                    flat.replace("7-8", "7-9")))
    self.assertEqual([k for k, _ in got], [
      "search_graph:$.results{only_in_a}",
      "search_graph:$.results{only_in_b}"])

  def test_order_modes(self) -> None:
    cols = ["n", "c"]
    q = {"query": "MATCH (f) RETURN f.name AS n, count(*) AS c "
                  "ORDER BY c DESC, n LIMIT 5"}
    self.assertEqual(rows.order_mode("query_graph", q, cols),
                     ("keyed", [1, 0]))
    q = {"query": "MATCH (f) RETURN f.name AS n ORDER BY f.start_line"}
    self.assertEqual(rows.order_mode("query_graph", q, ["n"]),
                     ("strict", []))
    self.assertEqual(rows.order_mode("query_graph", {"query": "MATCH (f) "
                                     "RETURN f.name LIMIT 3"}, ["f.name"]),
                     ("unordered", []))
    self.assertEqual(rows.order_mode("search_graph", {}, ["qn", "rank"]),
                     ("ranked", [1]))
    self.assertEqual(rows.order_mode("search_graph", {}, ["qn", "in"]),
                     ("named", [0]))

  def test_unlimited_args(self) -> None:
    self.assertEqual(rows.unlimited_args(
      "query_graph", {"query": "MATCH (n) RETURN n.name LIMIT 20"}, {}),
      {"query": "MATCH (n) RETURN n.name", "max_rows": 99998,
       "max_output_tokens": 1000000})
    self.assertIsNone(rows.unlimited_args(
      "query_graph", {"query": "MATCH (n) RETURN n SKIP 5 LIMIT 5"}, {}))
    self.assertEqual(rows.unlimited_args(
      "search_graph", {"query": "x", "limit": 3}, {"total": "40"}),
      {"query": "x", "limit": 500, "offset": 0,
       "max_output_tokens": 1000000})
    self.assertIsNone(rows.unlimited_args(
      "query_graph", {"query": "MATCH (n) RETURN n", "cursor": "c"}, {}))
    self.assertIsNone(rows.unlimited_args(
      "search_graph", {"query": "x", "offset": 3}, {}))


class GateTest(unittest.TestCase):
  def test_identical_output_has_no_diff(self) -> None:
    self.assertEqual(keys(diff("search_graph", {"query": "mean"}, RANKED,
                               RANKED)), [])

  def test_corrupted_rows_are_each_reported(self) -> None:
    rep = diff("search_graph", {"query": "mean"}, RANKED, corrupt(RANKED))
    got = keys(rep)
    self.assertEqual(len(got), 8)  # 4 rows gone, 4 wrong rows instead
    self.assertTrue(all(k.startswith("search_graph:$.results{only_in_")
                        for k, _ in got))
    self.assertTrue(any("Variable" in r and "WRONG" in r for _, r in got))

  def test_ties_may_reorder_but_scores_may_not(self) -> None:
    lines = RANKED.splitlines()
    swapped = "\n".join(lines[:2] + [lines[3], lines[2]] + lines[4:])
    self.assertEqual(keys(diff("search_graph", {"query": "m"}, RANKED,
                               swapped)), [])
    moved = "\n".join([lines[0], lines[4], lines[1], lines[2], lines[3]]
                      + lines[5:])
    # Moving a row changes its tie class: the row text is equal, but the
    # rank sequence is not.
    rep = diff("search_graph", {"query": "m"}, RANKED, moved)
    self.assertEqual([k for k, _ in keys(rep)],
                     ["search_graph:$.results{order}"])

  def test_cut_ties_are_checked_against_the_unlimited_replay(self) -> None:
    head = "results: 2  (cols: qn label file lines rank)\n"
    tail = "total: 3\nreturned: 2\nhas_more: true\ntruncated: true\n"
    r1 = "  p.a Function a.ts 1-2 -5\n"
    r2 = "  p.b Function b.ts 1-2 -1\n"
    r3 = "  p.c Function c.ts 1-2 -1\n"
    a, b = head + r1 + r2 + tail, head + r1 + r3 + tail
    full = ("results: 3  (cols: qn label file lines rank)\n" + r1 + r2 +
            r3 + "total: 3\nreturned: 3\nhas_more: false\n"
            "truncated: false\n")
    asked: list[dict[str, Any]] = []

    def rerun(args: dict[str, Any]) -> tuple[Any, Any]:
      asked.append(args)
      return out(full), out(full)

    args = {"query": "x", "limit": 2}
    self.assertEqual(keys(diff("search_graph", args, a, b, rerun)), [])
    self.assertEqual(asked[0], {"query": "x", "limit": 500, "offset": 0,
                                "max_output_tokens": 1000000})
    self.assertEqual(asked[-1], {"query": "x", "limit": 2,
                                 "format": "json"})

    # A boundary row the build made up is not in its unlimited answer.
    bad = head + r1 + "  p.zzz Function z.ts 1-2 -1\n" + tail
    got = keys(diff("search_graph", args, a, bad, rerun))
    self.assertEqual(got, [("search_graph:$.results{boundary_only_in_b}",
                            '["p.zzz", "Function", "z.ts", "1-2", "-1"]')])

    # Without a replay the differing boundary rows stay unverified.
    got = keys(diff("search_graph", args, a, b, None))
    self.assertEqual([k for k, _ in got], [
      "search_graph:$.results{boundary_unverified_a}",
      "search_graph:$.results{boundary_unverified_b}"])

  def test_rows_above_the_boundary_must_match(self) -> None:
    head = "results: 2  (cols: qn label file lines rank)\n"
    tail = "total: 9\nreturned: 2\nhas_more: true\ntruncated: true\n"
    a = head + "  p.a Function a.ts 1-2 -5\n  p.b Function b.ts 1-2 -1\n"
    b = head + "  p.x Function a.ts 1-2 -5\n  p.b Function b.ts 1-2 -1\n"
    full = out(a + tail)
    got = keys(diff("search_graph", {"query": "x", "limit": 2}, a + tail,
                    b + tail, lambda _: (full, full)))
    self.assertEqual([k for k, _ in got], [
      "search_graph:$.results{only_in_a}",
      "search_graph:$.results{only_in_b}"])

  def test_a_cut_output_always_gets_the_unlimited_replay(self) -> None:
    q = {"query": "MATCH (f) RETURN f.name LIMIT 1"}
    a = "rows: 1  (cols: f.name)\n  add\nreturned: 1\n"
    full_a = out("rows: 2  (cols: f.name)\n  add\n  sum\nreturned: 2\n")
    full_b = out("rows: 2  (cols: f.name)\n  add\n  WRONG\nreturned: 2\n")
    got = keys(diff("query_graph", q, a, a, lambda _: (full_a, full_b)))
    self.assertEqual(got, [
      ("query_graph:$.full.rows{only_in_a}", '["sum"]'),
      ("query_graph:$.full.rows{only_in_b}", '["WRONG"]')])

  def test_cypher_limit_without_order_replays_the_full_set(self) -> None:
    q = {"query": "MATCH (f:Function) RETURN f.name LIMIT 2"}
    tail = "returned: 2\ntotal: 2\nhas_more: false\ntruncated: false\n"
    a = "rows: 2  (cols: f.name)\n  add\n  mean\n" + tail
    b = "rows: 2  (cols: f.name)\n  sum\n  add\n" + tail
    full = out("rows: 3  (cols: f.name)\n  add\n  mean\n  sum\n"
               "returned: 3\ntotal: 3\nhas_more: false\ntruncated: false\n")
    asked = []

    def rerun(args: dict[str, Any]) -> tuple[Any, Any]:
      asked.append(args["query"])
      return full, full

    self.assertEqual(keys(diff("query_graph", q, a, b, rerun)), [])
    self.assertEqual(asked, ["MATCH (f:Function) RETURN f.name"])
    # The unlimited sets themselves are gated in full.
    broken = out("rows: 3  (cols: f.name)\n  add\n  mean\n  WRONG\n"
                 "returned: 3\ntotal: 3\nhas_more: false\n"
                 "truncated: false\n")
    got = keys(diff("query_graph", q, a, b, lambda _: (full, broken)))
    self.assertIn(("query_graph:$.full.rows{only_in_b}", '["WRONG"]'), got)
    self.assertIn(("query_graph:$.rows{boundary_only_in_b}", '["sum"]'),
                  got)

  def test_unlimited_search_graph_follows_pages(self) -> None:
    head = "results: 1  (cols: qn label file lines in out)\n"
    cut = head + "  p.a Function a.ts 1-2 0 0\ntotal: 3\nhas_more: true\n"
    pages = {
      0: head + "  p.a Function a.ts 1-2 0 0\n  p.b Function b.ts 1-2 0 0\n"
         "has_more: true\nnext_offset: 500\n",
      500: head + "  p.c Function c.ts 1-2 0 0\nhas_more: false\n"}
    bad = dict(pages)
    bad[500] = pages[500].replace("p.c", "p.WRONG")
    asked = []

    def rerun(args: dict[str, Any]) -> tuple[Any, Any]:
      asked.append(args["offset"])
      return out(pages[args["offset"]]), out(bad[args["offset"]])

    got = keys(diff("search_graph", {"name_pattern": ".*", "limit": 1},
                    cut, cut, rerun))
    self.assertEqual(asked, [0, 500])
    self.assertEqual([k for k, _ in got], [
      "search_graph:$.full.results{only_in_a}",
      "search_graph:$.full.results{only_in_b}"])

  def test_engine_limit_rows_are_still_gated(self) -> None:
    q = {"query": "MATCH (f)-[:CALLS]->(g) RETURN f.name, g.name "
                  "ORDER BY f.name, g.name LIMIT 2"}
    tail = ("returned: 2\ntotal: 2\ntotal_relation: gte\nhas_more: true\n"
            "truncated: true\ntruncation_reason: engine_limit\n")
    head = "rows: 2  (cols: f.name g.name)\n"
    a = head + "  a x\n  b y\n" + tail
    b = head + "  a x\n  c z\n" + tail
    full = out("rows: 3  (cols: f.name g.name)\n  a x\n  b y\n  c z\n"
               + tail)
    # Side b's capped candidates put b y after its cut: one acceptable key.
    full_b = out("rows: 3  (cols: f.name g.name)\n  a x\n  c z\n  b y\n"
                 + tail)
    got = keys(diff("query_graph", q, a, b, lambda _: (full, full_b)))
    self.assertEqual(got, [("query_graph:$.rows{unstable}", "")])
    self.assertEqual(keys(diff("query_graph", q, a, a)), [])
    # A row the other side never returns, even unlimited, is reported.
    bad = head + "  a x\n  WRONG z\n" + tail
    got = keys(diff("query_graph", q, a, bad, lambda _: (full, full)))
    self.assertIn(
      ("query_graph:$.rows{unstable_only_in_b}", '["WRONG", "z"]'), got)
    # A different count is never an unstable-only difference.
    short = "rows: 1  (cols: f.name g.name)\n  a x\n" + tail
    got = [k for k, _ in keys(diff("query_graph", q, a, short,
                                   lambda _: (full, full)))]
    self.assertIn("query_graph:$.rows{unstable_count}", got)
    # Without a replay the differing rows stay unverified.
    got = [k for k, _ in keys(diff("query_graph", q, a, b))]
    self.assertIn("query_graph:$.rows{unstable_unverified_b}", got)

  def test_capped_bm25_total_is_gated_against_the_replay(self) -> None:
    a = RANKED.replace("total_relation: eq", "total_relation: gte")
    b = a.replace("p.docs.guide.Math", "p.docs.guide.Other")
    other = "  p.docs.guide.Other Section docs/guide.md 6-7 -0.64\n"
    math = "  p.docs.guide.Math Section docs/guide.md 6-7 -0.64\n"
    full_a = a.replace("total: 4", other.rstrip("\n") + "\ntotal: 4", 1)
    full_b = b.replace("total: 4", math.rstrip("\n") + "\ntotal: 4", 1)
    got = keys(diff("search_graph", {"query": "m"}, a, b,
                    lambda _: (out(full_a), out(full_b))))
    self.assertEqual(got, [("search_graph:$.results{unstable}", "")])

  def test_capped_cypher_uses_the_replay_order_for_mixed_values(self) -> None:
    q = {"query": "MATCH (n) RETURN n.name, n.file_path ORDER BY n.name DESC LIMIT 3",
         "format": "json"}
    rows_ = [["a", "a.ts"], ["2", "b.ts"], ["10", "c.ts"], ["10", "d.ts"]]

    def result(page: list[list[str]]) -> str:
      return json.dumps({"columns": ["n.name", "n.file_path"], "rows": page,
                         "truncation_reason": "engine_limit"})

    full = out(result(rows_))
    got = keys(diff("query_graph", q, result(rows_[:3]),
                    result(rows_[:2] + rows_[3:]), lambda _: (full, full)))
    self.assertEqual(got, [("query_graph:$.rows{unstable}", "")])
    swapped = result([rows_[0], rows_[2], rows_[1]])
    got = keys(diff("query_graph", q, result(rows_[:3]), swapped,
                    lambda _: (full, full)))
    self.assertIn(("query_graph:$.rows{unstable_prefix_b}", ""), got)

  def test_ranked_text_is_also_gated_in_json(self) -> None:
    """Two rows print the same rank in text but differ in JSON: a swap
    between them is caught by the JSON replay."""
    text = ("results: 2  (cols: qn label file lines rank)\n"
            "  p.a Function a.ts 1-2 -15.3\n  p.b Function b.ts 1-2 -15.3\n"
            "total: 2\ntotal_relation: eq\nhas_more: false\n")
    swapped = text.replace("p.a Function a.ts", "p.x Function x.ts") \
      .replace("p.b Function b.ts", "p.a Function a.ts") \
      .replace("p.x Function x.ts", "p.b Function b.ts")
    def js(order: list[tuple[str, float]]) -> dict[str, Any]:
      return out(json.dumps({"cols": ["qn", "label", "file", "lines",
                                      "rank"],
                             "rows": [[q, "Function", f"{q[-1]}.ts", "1-2", r]
                                      for q, r in order],
                             "total": 2, "has_more": False}))
    good = js([("p.a", -15.31), ("p.b", -15.29)])
    bad = js([("p.b", -15.29), ("p.a", -15.31)])
    asked = []

    def rerun(args: dict[str, Any]) -> tuple[Any, Any]:
      asked.append(args.get("format"))
      return good, bad

    got = [k for k, _ in keys(diff("search_graph", {"query": "x"}, text,
                                   swapped, rerun))]
    self.assertEqual(asked, ["json"])
    self.assertEqual(got, ["search_graph:$.rows{order}"])

  def test_capped_page_must_lead_its_own_replay(self) -> None:
    """Side b answers a capped BM25 call with the bottom of its candidate
    list, in ascending order: every row exists on both sides, the counts
    match, and it still fails."""
    def page(rows_: list[str]) -> str:
      return (f"results: {len(rows_)}  (cols: qn label file lines rank)\n"
              + "".join(f"  {r}\n" for r in rows_) +
              "total: 4\ntotal_relation: gte\nreturned: 2\n"
              "has_more: true\ntruncated: true\n")
    full = ["p.top1 Function a.ts 1-2 -40.1",
            "p.top2 Function a.ts 3-4 -30.2",
            "p.low1 Function a.ts 5-6 -2.1",
            "p.low2 Function a.ts 7-8 -1.05"]
    a, b = page(full[:2]), page(list(reversed(full[2:])))
    got = [k for k, _ in keys(diff(
      "search_graph", {"query": "x", "limit": 2}, a, b,
      lambda _: (out(page(full)), out(page(full)))))]
    self.assertIn("search_graph:$.results{unstable_order_b}", got)
    self.assertIn("search_graph:$.results{unstable_prefix_b}", got)
    # In order but still the bottom rows: not the leading rows.
    got = [k for k, _ in keys(diff(
      "search_graph", {"query": "x", "limit": 2}, a, page(full[2:]),
      lambda _: (out(page(full)), out(page(full)))))]
    self.assertNotIn("search_graph:$.results{unstable_order_b}", got)
    self.assertIn("search_graph:$.results{unstable_prefix_b}", got)

  def test_unranked_search_graph_follows_name_order(self) -> None:
    """The C store returns unranked search_graph rows ORDER BY name, id:
    a build that returns another page of the same set must fail."""
    head = "results: 2  (cols: qn label file lines in out)\n"
    tail = "total: 4\nreturned: 2\nhas_more: true\nnext_offset: 2\n"
    rows_ = ["  p.a.add Function a.ts 1-2 0 0\n",
             "  p.b.mean Function b.ts 1-2 0 0\n",
             "  p.c.sum Function c.ts 1-2 0 0\n",
             "  p.d.zip Function d.ts 1-2 0 0\n"]
    names = {"p.a.add": "add", "p.b.mean": "mean", "p.c.sum": "sum",
             "p.d.zip": "zip"}
    full = out("results: 4  (cols: qn label file lines in out)\n" +
               "".join(rows_) + "total: 4\nhas_more: false\n")
    first = head + rows_[0] + rows_[1] + tail
    last = head + rows_[3] + rows_[2] + tail  # the end of the order
    rep = DiffReport()
    calls.diff_call(rep, "search_graph", {"name_pattern": ".*", "limit": 2},
                    "call:000000000000", "#0", out(first), out(last),
                    both(lambda _: (full, full)),
                    (names.get, names.get))
    got = sorted(d.key for d in rep.diffs())
    self.assertTrue(got)
    self.assertIn("search_graph:$.results{only_in_b}", got)
    # Equal names may come in either order (C breaks the tie by node id).
    twin = head + "  p.x.add Function x.ts 1-2 0 0\n" + rows_[0] + tail
    swap = head + rows_[0] + "  p.x.add Function x.ts 1-2 0 0\n" + tail
    names["p.x.add"] = "add"
    rep = DiffReport()
    calls.diff_call(rep, "search_graph", {"name_pattern": "add"},
                    "call:000000000000", "#0", out(twin), out(swap),
                    both(lambda _: (out(twin), out(swap))),
                    (names.get, names.get))
    self.assertEqual([d.key for d in rep.diffs()], [])

  def test_unchecked_pages_are_reported(self) -> None:
    q = {"name_pattern": ".*", "limit": 1}
    head = "results: 1  (cols: qn label file lines in out)\n"
    cut = head + "  p.a Function a.ts 1-2 0 0\nhas_more: true\n"

    def endless(args: dict[str, Any]) -> tuple[Any, Any]:
      o = out(head + f"  p.r{args['offset']} Function a.ts 1-2 0 0\n"
              f"has_more: true\nnext_offset: {args['offset'] + 500}\n")
      return o, o

    got = [k for k, _ in keys(diff("search_graph", q, cut, cut, endless))]
    self.assertIn("search_graph:$.full.results{full_incomplete}", got)

  def test_order_by_ties_may_reorder(self) -> None:
    q = {"query": "MATCH (f) RETURN f.label AS l, count(*) AS c "
                  "ORDER BY c DESC"}
    tail = "returned: 3\ntotal: 3\n"
    a = "rows: 3  (cols: l c)\n  Function 5\n  Method 2\n  Class 2\n" + tail
    b = "rows: 3  (cols: l c)\n  Function 5\n  Class 2\n  Method 2\n" + tail
    self.assertEqual(keys(diff("query_graph", q, a, b)), [])
    c = "rows: 3  (cols: l c)\n  Method 2\n  Function 5\n  Class 2\n" + tail
    self.assertEqual([k for k, _ in keys(diff("query_graph", q, a, c))],
                     ["query_graph:$.rows{order}"])

  def test_order_by_unreturned_key_is_strict(self) -> None:
    q = {"query": "MATCH (f) RETURN f.name ORDER BY f.start_line"}
    a = "rows: 2  (cols: f.name)\n  add\n  sum\nreturned: 2\n"
    b = "rows: 2  (cols: f.name)\n  sum\n  add\nreturned: 2\n"
    self.assertEqual([k for k, _ in keys(diff("query_graph", q, a, b))],
                     ["query_graph:$.rows{order}"])

  def test_json_rows_are_gated(self) -> None:
    q = {"query": "MATCH (f) RETURN f.name", "format": "json"}
    a = json.dumps({"columns": ["f.name"], "rows": [["add"], ["sum"]],
                    "returned": 2, "has_more": False})
    b = json.dumps({"columns": ["f.name"], "rows": [["sum"], ["adx"]],
                    "returned": 2, "has_more": False})
    self.assertEqual(keys(diff("query_graph", q, a, a)), [])
    self.assertEqual(keys(diff("query_graph", q, a, b)), [
      ("query_graph:$.rows{only_in_a}", '["add"]'),
      ("query_graph:$.rows{only_in_b}", '["adx"]')])

  def test_nested_json_tables_are_gated_as_tables(self) -> None:
    def js(rows_: list[list[Any]]) -> str:
      return json.dumps({"cols": ["qn", "rank"], "rows": [],
                         "semantic": {"cols": ["qn", "score"],
                                      "rows": rows_}})
    a = js([["p.a", 0.9], ["p.b", 0.5], ["p.c", 0.5]])
    b = js([["p.a", 0.9], ["p.c", 0.5], ["p.b", 0.5]])
    q = {"semantic_query": ["x"], "format": "json"}
    self.assertEqual(keys(diff("search_graph", q, a, b)), [])
    c = js([["p.b", 0.5], ["p.a", 0.9], ["p.c", 0.5]])
    self.assertEqual([k for k, _ in keys(diff("search_graph", q, a, c))],
                     ["search_graph:$.semantic{order}"])

  def test_labeled_lines_stay_gated(self) -> None:
    b = RANKED.replace("total: 4", "total: 5")
    self.assertEqual(keys(diff("search_graph", {"query": "m"}, RANKED, b)),
                     [("search_graph:$.out{total}", "")])

  def test_ref_rule_line_follows_the_rows(self) -> None:
    a = ("rows_ref_rule: @N+suffix=prefix+suffix\n"
         "rows: 1  (cols: f.name)\n  add\nreturned: 1\n")
    b = "rows: 1  (cols: f.name)\n  add\nreturned: 1\n"
    self.assertEqual(keys(diff("query_graph", {"query": "MATCH (f) "
                                               "RETURN f.name"}, a, b)), [])

  def test_volatile_column_is_compared_by_itself(self) -> None:
    head = ("projects: 1  (cols: name root_path branch nodes edges "
            "size_bytes)\n")
    a = head + "  p /r - 10 20 4096\ntotal: 1\n"
    got = keys(diff("list_projects", {"detail": "stats"}, a,
                    a.replace("4096", "8192")))
    self.assertEqual(got, [("list_projects:$.projects{col:size_bytes}", "")])
    got = keys(diff("list_projects", {"detail": "stats"}, a,
                    a.replace(" 20 ", " 21 ")))
    self.assertEqual([k for k, _ in got], [
      "list_projects:$.projects{only_in_a}",
      "list_projects:$.projects{only_in_b}"])

  def test_other_tools_keep_row_order(self) -> None:
    a = "callers: 2  (cols: qn)\n  p.a 2\n  p.b 2\n"
    b = "callers: 2  (cols: qn)\n  p.b 2\n  p.a 2\n"
    rep = diff("trace_path", {}, a, b)
    self.assertTrue(rep.diffs())


if __name__ == "__main__":
  unittest.main()
