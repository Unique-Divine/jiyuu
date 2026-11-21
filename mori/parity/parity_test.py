"""Tests for the runner's pure parts: run validity, safety, summaries."""

from __future__ import annotations

import argparse
import io
import json
import os
import socket
import sqlite3
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from unittest.mock import patch

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import allowlist  # noqa: E402
import calls  # noqa: E402
import parity  # noqa: E402
from graph_export import Diff, DiffReport  # noqa: E402


class ParityTest(unittest.TestCase):
  def setUp(self) -> None:
    tmp = tempfile.TemporaryDirectory()
    self.addCleanup(tmp.cleanup)
    self.tmp = tmp.name
    self.roots = [self.tmp]

  def test_verify_export_requires_rust_before_allocating_scratch(self) -> None:
    for command in ("ci", "run"):
      with self.subTest(command=command), patch.object(parity.tempfile, "mkdtemp") as mkdtemp:
        errors = io.StringIO()
        with redirect_stderr(errors), self.assertRaises(SystemExit) as raised:
          parity.main([command, "--verify-export"])
        self.assertEqual(raised.exception.code, 2)
        self.assertIn("--verify-export requires --export-a rust or --export-b rust",
                      errors.getvalue())
        mkdtemp.assert_not_called()

  def test_ci_reports_only_work_that_survives_cleanup(self) -> None:
    reports: list[str] = []

    def fake_ci(args: argparse.Namespace) -> int:
      work = args.work
      os.mkdir(work)
      report = os.path.join(work, "report.json")
      with open(report, "w", encoding="utf-8") as f:
        json.dump({}, f)
      reports.append(report)
      return 0

    with patch.object(parity, "cmd_ci", fake_ci):
      kept = io.StringIO()
      with redirect_stdout(kept):
        self.assertEqual(parity.main(["ci", "--work", self.tmp + "/saved"]), 0)
      self.assertIn(f"report: {reports[0]}", kept.getvalue())
      self.assertTrue(os.path.isfile(reports[0]))
      removed = io.StringIO()
      with redirect_stdout(removed):
        self.assertEqual(parity.main(["ci"]), 0)
      self.assertNotIn(reports[1], removed.getvalue())
      self.assertFalse(os.path.exists(reports[1]))

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

  def test_repeated_one_sided_timeout_is_a_parity_difference(self) -> None:
    # Side a dropped gen.go on all three attempts; side b never did.
    persistent, varying = parity.timeout_verdict({
      "a": [{"big.go", "gen.go"}] * 3, "b": [{"big.go"}]})
    self.assertEqual(persistent, {"a": ["gen.go"]})
    self.assertEqual(varying, {})
    # A file that came and went is load, not parity.
    persistent, varying = parity.timeout_verdict({
      "a": [{"x.go"}, set(), {"x.go"}], "b": [set()]})
    self.assertEqual(varying, {"a": ["x.go"]})
    # One attempt proves nothing.
    _, varying = parity.timeout_verdict({"a": [{"x.go"}], "b": [set()]})
    self.assertEqual(varying, {"a": ["x.go"]})
    # Both sides agree: nothing to report.
    self.assertEqual(parity.timeout_verdict({"a": [{"x"}], "b": [{"x"}]}),
                     ({}, {}))

  def test_work_dir_needs_marker_and_scratch_root(self) -> None:
    work = os.path.join(self.tmp, "work")
    parity.fresh_work_dir(work, self.roots)
    open(os.path.join(work, "old"), "w").close()
    parity.fresh_work_dir(work, self.roots)
    self.assertEqual(os.listdir(work), [parity.MARKER])
    other = os.path.join(self.tmp, "data")
    os.makedirs(other)
    with self.assertRaises(RuntimeError):
      parity.fresh_work_dir(other, self.roots)
    self.assertTrue(os.path.isdir(other))
    with self.assertRaises(RuntimeError):
      parity.fresh_work_dir(os.path.join(self.tmp, "w"),
                            [os.path.join(self.tmp, "elsewhere")])
    with self.assertRaises(RuntimeError):
      parity.fresh_work_dir(self.tmp, self.roots)  # the root itself

  def test_runtime_dir_holds_only_sockets_and_locks(self) -> None:
    rt = os.path.join(self.tmp, "rt")
    parity.fresh_runtime_dir(rt, self.roots)
    daemon = os.path.join(rt, "cbm-daemon-1000")
    os.makedirs(daemon)
    for name in ("cbm-x.lock", "cbm-y.turn", "cbm-y.rw", "cbm-k.anc",
                 "cbm-k.sock.identity"):
      open(os.path.join(daemon, name), "w").close()
    sock = socket.socket(socket.AF_UNIX)
    self.addCleanup(sock.close)
    sock.bind(os.path.join(rt, "s"))
    parity.fresh_runtime_dir(rt, self.roots)
    self.assertEqual(os.listdir(rt), [parity.MARKER])
    # A directory holding anything else is not deleted.
    data = os.path.join(self.tmp, "x-a")
    os.makedirs(os.path.join(data, "cbm-daemon-1000"))
    open(os.path.join(data, "cbm-daemon-1000", "notes.txt"), "w").close()
    open(os.path.join(data, "cbm-daemon-1000", "cbm-x.lock"), "w").close()
    with self.assertRaises(RuntimeError) as cm:
      parity.fresh_runtime_dir(data, self.roots)
    self.assertIn("notes.txt", str(cm.exception))
    self.assertTrue(os.path.exists(os.path.join(data, "cbm-daemon-1000",
                                                "notes.txt")))
    empty_no_marker = os.path.join(self.tmp, "x-b")
    os.makedirs(empty_no_marker)
    parity.fresh_runtime_dir(empty_no_marker, self.roots)
    with self.assertRaises(RuntimeError):
      parity.fresh_runtime_dir("/var/tmp-not-scratch/rt", self.roots)

  def test_identity_check_compares_raw_names_and_roots(self) -> None:
    rep = DiffReport()
    parity.identity_diffs(
      rep, {"a": {"project": "tmp-x"}, "b": {"project": "tmp-x"}},
      {"a": [["tmp-x", "/tmp/x"]], "b": [["tmp-x", "/tmp/x"]]})
    self.assertEqual(rep.diffs(), [])
    parity.identity_diffs(
      rep, {"a": {"project": "tmp-x"}, "b": {"project": "x"}},
      {"a": [["tmp-x", "/tmp/x"]], "b": [["tmp-x", "/tmp/x/"]]})
    self.assertEqual([(d.layer, d.key, d.group) for d in rep.diffs()], [
      ("L3", "projects.identity", "index_result"),
      ("L3", "projects.identity", "projects")])
    entries = allowlist.load(parity.ALLOWLIST)
    got = parity.classify(rep.diffs(), "fx-lib", entries)
    self.assertTrue(all(d["allowed"] is None for d in got))

  def test_unknown_snapshot_is_rejected(self) -> None:
    dirs = ["alpha", "beta"]
    self.assertEqual(parity.parse_snapshot("", dirs), set())
    self.assertEqual(parity.parse_snapshot("all", dirs), {"alpha", "beta"})
    self.assertEqual(parity.parse_snapshot("beta", dirs), {"beta"})
    with self.assertRaises(ValueError):
      parity.parse_snapshot("beta,btea", dirs)

  def test_attempt_flags(self) -> None:
    r = parity.RepoResult("alpha", "/x", index={
      "a": {"attempts_total": 1}, "b": {"attempts_total": 3}})
    same = parity.RepoResult("beta", "/y", index={
      "a": {"attempts_total": 2}, "b": {"attempts_total": 2}})
    self.assertEqual(parity.attempt_flags([r, same]),
                     ["alpha: side a 1, side b 3"])

  def test_calls_off_the_corpus_are_dropped(self) -> None:
    path = os.path.join(self.tmp, "calls.jsonl")
    rows = [
      {"tool": "list_projects", "args": {}},
      {"tool": "search_graph", "args": {"project": "{{project:alpha}}"}},
      {"tool": "search_graph", "args": {"project": "home-u-alpha-wt"}},
      {"tool": "search_graph", "args": {"project": "x {{project:alpha}}"}},
    ]
    with open(path, "w") as f:
      f.write("\n".join(json.dumps(r) for r in rows) + "\n")
    got, dropped = calls.load_calls(path)
    self.assertEqual([c["args"] for c in got],
                     [{}, {"project": "{{project:alpha}}"}])
    self.assertEqual(dropped, 2)

  def test_corrupt_verdict_needs_the_targeted_keys(self) -> None:
    path = os.path.join(self.tmp, "report.json")

    def report(*diffs: dict) -> str:
      with open(path, "w") as f:
        json.dump({"l4": {"diffs": list(diffs)}}, f)
      return path

    rows_d = {"key": "query_graph:$.rows{only_in_b}", "allowed": None,
              "examples": [{"where": "#9 query_graph {}"}]}
    page_d = {"key": "search_graph:$.results{order}", "allowed": None,
              "examples": [{"where": '#4 search_graph {"name_pattern": ".*"}'}]}
    clock = {"key": "index_status:$.out{indexed_at}{presence}",
             "allowed": None, "examples": [{"where": "#1"}]}
    self.assertEqual(parity.corrupt_verdict("all", 1,
                                            report(rows_d, page_d)), 0)
    self.assertEqual(parity.corrupt_verdict("rows", 1, report(rows_d)), 0)
    # An unrelated difference does not pass the self-test.
    self.assertEqual(parity.corrupt_verdict("page", 1, report(clock)), 1)
    self.assertEqual(parity.corrupt_verdict("all", 1, report(rows_d)), 1)
    # Nor does a run that wasn't a clean "differences found" exit.
    self.assertEqual(parity.corrupt_verdict("rows", 2, report(rows_d)), 1)

  def test_classify_and_summary(self) -> None:
    entries = allowlist.parse([
      '{"layer": "L3", "key": "projects.indexed_at", "reason": "clock"}'])
    diffs = [Diff("L3", "projects.indexed_at", "{{project}}", 1),
             Diff("L2", "edges.properties.line", "CALLS", 4),
             Diff("index", "index.parse_timeout_only_in_b", "gen.go", 1)]
    got = parity.classify(diffs, "repo", entries)
    self.assertEqual([d["allowed"] for d in got], ["clock", None, None])
    s = parity.layer_summary(got)
    self.assertEqual(s["L3"], {"allowed": 1, "unlisted": 0})
    self.assertEqual(parity.cell(s["L2"]), "UNLISTED 4")
    self.assertEqual(parity.cell(s["index"]), "UNLISTED 1")
    self.assertEqual(parity.cell(s["L0"]), "same")


if __name__ == "__main__":
  unittest.main()
