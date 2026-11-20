"""Tests for the runner's pure parts: run validity, safety, summaries."""

from __future__ import annotations

import os
import sqlite3
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import allowlist  # noqa: E402
import parity  # noqa: E402
from graph_export import Diff  # noqa: E402


class ParityTest(unittest.TestCase):
  def setUp(self) -> None:
    tmp = tempfile.TemporaryDirectory()
    self.addCleanup(tmp.cleanup)
    self.tmp = tmp.name

  def test_parse_timeouts_counts_dropped_files(self) -> None:
    db = os.path.join(self.tmp, "p.db")
    c = sqlite3.connect(db)
    c.execute("CREATE TABLE index_coverage (project TEXT, rel_path TEXT, "
              "kind TEXT, detail TEXT)")
    c.executemany("INSERT INTO index_coverage VALUES ('p', ?, ?, ?)", [
      ("a.rs", "extract", "parse timeout"),
      ("b.rs", "extract", "parse failed"),
      ("c.rs", "unresolved_calls", "[]"),
    ])
    c.commit()
    c.close()
    self.assertEqual(parity.parse_timeouts(db), {"a.rs"})
    self.assertEqual(parity.parse_timeouts(os.path.join(self.tmp, "x")),
                     set())

  def test_timeout_sides_keeps_files_both_drop(self) -> None:
    self.assertEqual(parity.timeout_sides({"a": {"big.go"},
                                           "b": {"big.go"}}), [])
    self.assertEqual(parity.timeout_sides({"a": {"big.go", "x.rs"},
                                           "b": {"big.go"}}), ["a"])
    self.assertEqual(parity.timeout_sides({"a": {"x.rs"}, "b": {"y.rs"}}),
                     ["a", "b"])
    self.assertEqual(parity.timeout_sides({"a": set(), "b": set()}), [])

  def test_fresh_dir_needs_marker(self) -> None:
    work = os.path.join(self.tmp, "work")
    parity.fresh_dir(work, marker=True)
    open(os.path.join(work, "old"), "w").close()
    parity.fresh_dir(work, marker=True)
    self.assertEqual(os.listdir(work), [parity.MARKER])
    other = os.path.join(self.tmp, "data")
    os.makedirs(other)
    with self.assertRaises(RuntimeError):
      parity.fresh_dir(other, marker=True)
    self.assertTrue(os.path.isdir(other))

  def test_classify_and_summary(self) -> None:
    entries = allowlist.parse([
      '{"layer": "L3", "key": "projects.indexed_at", "reason": "clock"}'])
    diffs = [Diff("L3", "projects.indexed_at", "{{project}}", 1),
             Diff("L2", "edges.properties.line", "CALLS", 4)]
    got = parity.classify(diffs, "repo", entries)
    self.assertEqual([d["allowed"] for d in got], ["clock", None])
    s = parity.layer_summary(got)
    self.assertEqual(s["L3"], {"allowed": 1, "unlisted": 0})
    self.assertEqual(parity.cell(s["L2"]), "UNLISTED 4")
    self.assertEqual(parity.cell(s["L0"]), "same")


if __name__ == "__main__":
  unittest.main()
