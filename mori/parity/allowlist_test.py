"""Tests for allow-list parsing, matching, lint, and seeding, and for the
committed allow-list against a corrupted build."""

from __future__ import annotations

import json
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import allowlist  # noqa: E402
import calls  # noqa: E402
import parity  # noqa: E402
from graph_export import DiffReport  # noqa: E402
from rows_test import RANKED, both, corrupt, out  # noqa: E402

CALL = "call:0123456789ab"


def entry(**kw: object) -> str:
  return json.dumps({"reason": "r", **kw})


class AllowlistTest(unittest.TestCase):
  def test_star_is_the_only_wildcard(self) -> None:
    self.assertTrue(allowlist.star_match("q:$.out[*]", "q:$.out[*]"))
    self.assertTrue(allowlist.star_match("q:$.out[*]", "q:$.out[12]"))
    self.assertTrue(allowlist.star_match("*.id", "nodes.id"))
    self.assertFalse(allowlist.star_match("a?c", "abc"))
    self.assertFalse(allowlist.star_match("projects.indexed_at",
                                          "projects.indexed_at.x"))

  def test_parse_and_find(self) -> None:
    entries = allowlist.parse([
      "# comment",
      entry(layer="L3", key="store_meta.v", group="db_uid"),
      entry(layer="L4", key="index_status:$.out{indexed_at}",
            repos=["alpha"]),
    ])
    self.assertEqual(len(entries), 2)
    self.assertIsNotNone(allowlist.find(entries, "L3", "store_meta.v",
                                        "db_uid", "x"))
    self.assertIsNone(allowlist.find(entries, "L3", "store_meta.v",
                                     "schema_version", "x"))
    self.assertIsNone(allowlist.find(entries, "L2", "store_meta.v",
                                     "db_uid", "x"))
    self.assertIsNotNone(allowlist.find(
      entries, "L4", "index_status:$.out{indexed_at}", "-", "alpha"))
    self.assertIsNone(allowlist.find(
      entries, "L4", "index_status:$.out{indexed_at}", "-", "beta"))

  def test_parse_requires_reason(self) -> None:
    with self.assertRaises(ValueError):
      allowlist.parse(['{"layer": "L3", "key": "x"}'])

  def test_lint_rejects_broad_entries(self) -> None:
    broad = [
      # The two entries that let corrupted rows through before.
      {"layer": "L4", "key": "search_graph:$.out{*}", "group": "*"},
      {"layer": "L4", "key": "query_graph:$.out{*}", "group": "*"},
      {"layer": "L4", "key": "*", "group": "*"},
      {"layer": "L4", "key": "search_graph:*", "group": CALL},
      # Row entries must name one call, one repo, and a real row pattern.
      {"layer": "L4", "key": "search_graph:$.results{only_in_b}",
       "group": "*", "repos": ["alpha"], "row": '["p.mean", "Function"'},
      {"layer": "L4", "key": "search_graph:$.results{only_in_b}",
       "group": CALL, "row": '["p.mean", "Function"'},
      {"layer": "L4", "key": "search_graph:$.results{only_in_b}",
       "group": CALL, "repos": ["alpha"], "row": "*"},
      {"layer": "L4", "key": "query_graph:$.rows{order}", "group": "*",
       "repos": ["alpha"]},
      {"layer": "L1", "key": "nodes.only_in_b", "group": "*"},
      {"layer": "L3", "key": "projects.indexed_at", "row": "abcdefghij"},
      {"layer": "L4", "key": "search_graph:$.results{unstable_count}",
       "group": CALL, "repos": ["alpha"]},
      {"layer": "L4", "key": "search_graph:$.results{unstable_only_in_b}",
       "group": CALL, "repos": ["alpha"]},
    ]
    for d in broad:
      self.assertTrue(allowlist.lint_entry(d), d)
      with self.assertRaises(ValueError):
        allowlist.parse([entry(**d)])
    narrow = {"layer": "L4", "key": "search_graph:$.results{only_in_b}",
              "group": CALL, "repos": ["alpha"],
              "row": '["p.lib.stats.mean", "Function", *'}
    self.assertEqual(allowlist.lint_entry(narrow), [])

  def test_row_diffs_need_row_entries(self) -> None:
    key = "search_graph:$.results{only_in_b}"
    (e,) = allowlist.parse([entry(layer="L4", key=key, group=CALL,
                                  repos=["alpha"], row='["p.lib.a", *')])
    self.assertTrue(e.matches("L4", key, CALL, "alpha", '["p.lib.a", "F"]'))
    self.assertFalse(e.matches("L4", key, CALL, "alpha", '["p.lib.b"]'))
    self.assertFalse(e.matches("L4", key, CALL, "alpha", ""))
    self.assertFalse(e.matches("L4", key, CALL, "beta", '["p.lib.a"]'))
    (plain,) = allowlist.parse([entry(layer="L4", key="t:$.out{x}")])
    self.assertFalse(plain.matches("L4", "t:$.out{x}", CALL, "a", "row"))

  def test_seed_is_narrow(self) -> None:
    diffs = [{"layer": "L3", "key": "store_meta.v", "group": "db_uid",
              "repo": r, "count": 1} for r in ("a", "b")]
    diffs += [{"layer": "L4", "key": "q:$.out[3]", "group": f"g{i}",
               "repo": "a", "count": 2} for i in range(5)]
    diffs += [{"layer": "L4", "key": "search_graph:$.results{only_in_a}",
               "group": CALL, "repo": "a", "count": 1,
               "row": '["p.lib.stats.mean", "Function"]'}]
    diffs += [{"layer": "L4", "key": "search_graph:$.out{*}",
               "group": CALL, "repo": "a", "count": 1}]
    seeded, rejected = allowlist.seed_entries(diffs, "test run")
    got = [(e["layer"], e["key"], e["group"]) for e in seeded]
    self.assertIn(("L3", "store_meta.v", "db_uid"), got)
    # One entry per group: nothing collapses to `*`.
    self.assertEqual(sum(1 for g in got if g[1] == "q:$.out[*]"), 5)
    self.assertIn(("L4", "search_graph:$.results{only_in_a}", CALL), got)
    self.assertEqual([e["key"] for e in rejected],
                     ["search_graph:$.out{*}"])
    self.assertEqual(len(allowlist.parse(json.dumps(e) for e in seeded)),
                     len(seeded))


class CommittedAllowlistTest(unittest.TestCase):
  """The committed allow-list must pass the lint and must not accept a
  changed result row from either row tool."""

  def setUp(self) -> None:
    self.entries = allowlist.load(parity.ALLOWLIST)

  def classify(self, tool: str, args: dict, a: str, b: str
               ) -> list[dict]:
    rep = DiffReport()
    group = calls.call_id(tool, args)
    calls.diff_call(rep, tool, args, group, "#0", out(a), out(b),
                    both(lambda _: (out(a), out(b))))
    return parity.classify(rep.diffs(), "fx-lib", self.entries)

  def test_changed_search_graph_row_is_unlisted(self) -> None:
    args = {"project": "{{project:fx-lib}}", "query": "mean", "limit": 5}
    got = self.classify("search_graph", args, RANKED, corrupt(RANKED))
    self.assertTrue(got)
    self.assertTrue(all(d["allowed"] is None for d in got), got)

  def test_changed_query_graph_row_is_unlisted(self) -> None:
    args = {"project": "{{project:fx-polyglot}}",
            "query": "MATCH (f:Function) RETURN f.name ORDER BY f.name "
                     "LIMIT 20"}
    a = ("rows: 2  (cols: f.name)\n  add\n  mean\nreturned: 2\n"
         "total: 2\n")
    b = a.replace("mean", "meen")
    got = self.classify("query_graph", args, a, b)
    self.assertEqual(sorted(d["key"] for d in got), [
      "query_graph:$.rows{only_in_a}", "query_graph:$.rows{only_in_b}"])
    self.assertTrue(all(d["allowed"] is None for d in got), got)

  def test_no_entry_names_a_corpus_repo(self) -> None:
    for e in self.entries:
      self.assertEqual(e.repos, (), e)


if __name__ == "__main__":
  unittest.main()
