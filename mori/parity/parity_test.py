"""Tests for the runner's pure parts: run validity, safety, summaries."""

from __future__ import annotations

import argparse
import io
import json
import os
import socket
import sqlite3
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
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

  def test_graphql_mode_extends_only_the_requested_side_env(self) -> None:
    normal = parity.Side("a", "/bin/true", "/tmp/cache-a", "/tmp/rt-a")
    candidate = parity.Side("b", "/bin/true", "/tmp/cache-b", "/tmp/rt-b",
                            graphql_mode="rust")
    with patch.dict(os.environ, {"HOME": self.tmp, "MORI_IMPL_GRAPHQL": "c",
                                 "CBM_PROFILE": "ambient"}, clear=True):
      self.assertEqual(normal.env(), {
        "PATH": "/usr/local/bin:/usr/bin:/bin", "HOME": self.tmp,
        "CBM_CACHE_DIR": "/tmp/cache-a", "CBM_RUNTIME_DIR": "/tmp/rt-a",
      })
      self.assertEqual(candidate.env(), {
        "PATH": "/usr/local/bin:/usr/bin:/bin", "HOME": self.tmp,
        "CBM_CACHE_DIR": "/tmp/cache-b", "CBM_RUNTIME_DIR": "/tmp/rt-b",
        "MORI_IMPL_GRAPHQL": "rust", "CBM_PROFILE": "1",
      })

  def test_profile_retains_worker_logs_without_selecting_a_backend(self) -> None:
    side = parity.Side("a", "/bin/true", "/tmp/cache", "/tmp/rt", profile=True)
    with patch.dict(os.environ, {"MORI_IMPL_GRAPHQL": "c"}, clear=True):
      self.assertEqual(side.env()["CBM_PROFILE"], "1")
      self.assertNotIn("MORI_IMPL_GRAPHQL", side.env())

  def test_require_rust_query_requires_replay(self) -> None:
    for flags in (["--skip-l4"], ["--path-parity"]):
      with (self.subTest(flags=flags), redirect_stderr(io.StringIO()) as errors,
            self.assertRaises(SystemExit) as raised):
        parity.main(["ci", "--require-rust-query", "call:unknown", *flags])
      self.assertEqual(raised.exception.code, 2)
      self.assertIn("--require-rust-query requires tool-call replay",
                    errors.getvalue())

  def test_require_rust_query_rejects_corruption_mode(self) -> None:
    with (redirect_stderr(io.StringIO()) as errors,
          self.assertRaises(SystemExit) as raised):
      parity.main(["ci", "--corrupt", "rows", "--require-rust-query", "call:unknown"])
    self.assertEqual(raised.exception.code, 2)
    self.assertIn("--require-rust-query cannot be combined with --corrupt",
                  errors.getvalue())

  def test_disable_ui_configures_only_the_scratch_cache(self) -> None:
    side = parity.Side("b", "/bin/true", self.tmp + "/cache", self.tmp + "/rt")
    with patch.object(parity.subprocess, "run") as run:
      run.return_value.returncode = 0
      side.disable_ui()
      self.assertEqual(run.call_args.args[0],
                       ["/bin/true", "config", "set", "ui_enabled", "false"])
      self.assertEqual(run.call_args.kwargs["env"]["CBM_CACHE_DIR"], side.cache)
      run.return_value.returncode = 1
      with self.assertRaisesRegex(RuntimeError, "could not disable"):
        side.disable_ui()

  def test_rust_query_replay_checks_execution_even_when_replies_match(self) -> None:
    args = {"query": "MATCH (f) RETURN f.name", "format": "json"}
    call = {"tool": "query_graph", "args": args}
    call_id = calls.call_id("query_graph", args)
    sides = [parity.Side(n, "/bin/true", os.path.join(self.tmp, n), self.tmp)
             for n in ("a", "b")]
    log_dir = os.path.join(sides[1].cache, "logs")
    os.makedirs(log_dir)
    log_path = os.path.join(log_dir, "cbm-daemon.log")
    marker = parity.RUST_QUERY_LOG + b"\n"

    for events, required, expected_failures in (
        (0, False, 0), (1, True, 0), (0, True, 1), (2, True, 1)):
      with self.subTest(events=events, required=required):
        # A valid marker from an earlier request cannot prove this request.
        with open(log_path, "wb") as log_file:
          log_file.write(marker)

        reply = (0, '{"columns":["f.name"],"rows":[["one"]]}', "")

        def candidate(*_):
          with open(log_path, "ab") as log_file:
            log_file.write(marker * events)
          return reply

        with (patch.object(sides[0], "cli", return_value=reply),
              patch.object(sides[1], "cli", side_effect=candidate)):
          diffs, stats = parity.replay(
            sides, [call], {}, [{}, {}], [],
            frozenset({call_id}) if required else frozenset())
        self.assertEqual(len(diffs), expected_failures)
        self.assertEqual(stats.get("rust_query_checks"), 1 if required else None)
        self.assertEqual(stats.get("rust_query_failures"), expected_failures if required else None)
        self.assertEqual(stats["differing"], 0)
        if expected_failures:
          self.assertEqual(diffs[0]["key"], "query_graph:$.rust_dispatch")
          self.assertEqual(diffs[0]["group"], call_id)
          self.assertIsNone(diffs[0]["allowed"])
          self.assertNotIn(args["query"], json.dumps(diffs))

  def test_rust_query_replay_ignores_late_event_after_unselected_timeout(self) -> None:
    previous = {"tool": "query_graph", "args": {"query": "MATCH (a) RETURN a"}}
    args = {"query": "MATCH (b) RETURN b", "format": "json"}
    selected = {"tool": "query_graph", "args": args}
    call_id = calls.call_id("query_graph", args)
    prior_call_id = calls.call_id("query_graph", previous["args"])
    entries = [allowlist.Entry("L4", "query_graph:$.timed_out", prior_call_id,
                               "known timeout")]
    sides = [parity.Side(n, "/bin/true", os.path.join(self.tmp, n), self.tmp)
             for n in ("a", "b")]
    log_path = Path(sides[1].cache) / "logs/cbm-daemon.log"
    log_path.parent.mkdir(parents=True)
    log_path.touch()
    reply = (0, '{"columns":["b"],"rows":[["one"]]}', "")

    def candidate(tool, request, timeout):
      if request == previous["args"]:
        raise subprocess.TimeoutExpired(tool, timeout)
      with log_path.open("ab") as log_file:
        log_file.write(parity.RUST_QUERY_LOG + b"\n")
      return reply

    with (patch.object(sides[0], "cli", return_value=reply),
          patch.object(sides[1], "cli", side_effect=candidate)):
      diffs, stats = parity.replay(
        sides, [previous, selected], {}, [{}, {}], entries, frozenset({call_id}))

    self.assertEqual(stats["timeouts"], 1)
    self.assertEqual(stats["rust_query_checks"], 1)
    self.assertEqual(stats["rust_query_failures"], 1)
    self.assertEqual([d["key"] for d in diffs],
                     ["query_graph:$.timed_out", "query_graph:$.rust_dispatch"])
    self.assertEqual(diffs[0]["allowed"], "known timeout")
    self.assertEqual(diffs[1]["group"], call_id)
    self.assertIsNone(diffs[1]["allowed"])
    self.assertIn("evidence unavailable after side B timeout",
                  diffs[1]["examples"][0]["b"])

  def test_rust_query_replay_rejects_replaced_log(self) -> None:
    log_path = Path(self.tmp) / "cbm-daemon.log"
    marker = parity.RUST_QUERY_LOG + b"\n"
    log_path.write_bytes(marker)
    previous = parity.query_log_position(log_path)
    replacement = log_path.with_suffix(".new")
    replacement.write_bytes(marker * 2)
    replacement.replace(log_path)
    self.assertIsNone(parity.rust_query_events(log_path, previous))

  def test_path_parity_uses_one_worker_on_side_a(self) -> None:
    sides = [parity.Side(n, "/bin/true", self.tmp + "-cache-" + n,
                         self.tmp + "-rt-" + n) for n in ("a", "b")]
    with (patch.object(parity, "make_sides", return_value=sides),
          patch.object(parity, "finish", return_value=0)):
      work = self.tmp + "/work"
      with redirect_stderr(io.StringIO()):
        args = argparse.Namespace(allowlist=[], bin_a="/bin/true", bin_b="/bin/true",
                                  work=work, rt_prefix=work + "-rt", scratch_root=self.roots,
                                  export_a="sql", export_b="sql", graphql_a=None, graphql_b=None,
                                  verify_export=False, profile=True, path_parity=True,
                                  require_rust_query=[])
        self.assertEqual(parity.run([], None, 0, args, {}), 0)
      self.assertEqual(sides[0].env()["CBM_WORKERS"], "1")
      with patch.dict(os.environ, {"CBM_WORKERS": "2"}):
        self.assertNotIn("CBM_WORKERS", sides[1].env())

  def test_path_parity_checks_the_observed_worker_paths(self) -> None:
    side = parity.Side("b", "/bin/true", self.tmp, self.tmp)
    log_dir = os.path.join(side.cache, "logs")
    os.makedirs(log_dir)
    before = os.path.join(log_dir, ".worker-log-before")
    with open(before, "w", encoding="utf-8") as f:
      f.write("level=info msg=pipeline.mode mode=sequential files=10\n")
    previous = parity.worker_log_paths(side)
    after = os.path.join(log_dir, ".worker-log-after")
    with open(after, "w", encoding="utf-8") as f:
      f.write("level=info msg=pipeline.mode mode=parallel workers=12 files=62\n")
    observed = parity.observe_worker_paths(side, previous)
    self.assertEqual(observed, [{"mode": "parallel", "files": 62}])

    index = {"a": {"worker_paths": [{"mode": "sequential", "files": 62}]},
             "b": {"worker_paths": observed}}
    self.assertIsNone(parity.verify_worker_paths(index, fixture=True))
    index["b"]["worker_paths"] = [{"mode": "sequential", "files": 10}]
    self.assertIsNone(parity.verify_worker_paths(index, fixture=False))
    self.assertIn("side b expected parallel",
                  parity.verify_worker_paths(index, fixture=True) or "")
    index["b"]["worker_paths"] = [{"mode": "sequential", "files": 62}]
    self.assertIn("side b expected parallel",
                  parity.verify_worker_paths(index, fixture=False) or "")

  def test_path_parity_requires_a_parallel_repository(self) -> None:
    def compare(sides, name, root, entries, retries):
      for side in sides:
        log_dir = os.path.join(side.cache, "logs")
        os.makedirs(log_dir)
        with open(os.path.join(log_dir, ".worker-log-fixture"), "w",
                  encoding="utf-8") as f:
          f.write("level=info msg=pipeline.mode mode=sequential files=10\n")
      return parity.RepoResult(name, root)

    repo = parity.corpus.Repo("small", None, None, None, None)
    with (patch.object(parity.corpus, "load", return_value=[repo]),
          patch.object(parity.corpus, "verify", return_value=parity.corpus.Check(
            "small", True, False, [])),
          patch.object(parity, "compare_repo", side_effect=compare),
          redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO())):
      work = self.tmp + "/work"
      self.assertEqual(parity.main([
        "run", "--path-parity", "--work", work, "--rt-prefix", work + "-rt",
        "--scratch-root", self.tmp, "--bin-a", sys.executable, "--bin-b", sys.executable,
      ]), 2)
    with open(work + "/report.json", encoding="utf-8") as f:
      report = json.load(f)
    self.assertIn("requires at least one observed parallel", report["worker_path_error"])
    self.assertEqual(report["repos"][0]["index"]["b"]["worker_paths"],
                     [{"mode": "sequential", "files": 10}])

  def test_graphql_modes_are_reported_only_when_requested(self) -> None:
    for label, flags, requested in (
        ("default", [], None),
        ("single", ["--graphql-b", "rust"], {"b": "rust"}),
        ("both", ["--graphql-a", "c", "--graphql-b", "rust"], {"a": "c", "b": "rust"})):
      with self.subTest(label=label), patch.object(parity.fixtures, "write_all", return_value=[]):
        work = os.path.join(self.tmp, label)
        errors = io.StringIO()
        with redirect_stdout(io.StringIO()), redirect_stderr(errors):
          self.assertEqual(parity.main([
            "ci", "--work", work, "--rt-prefix", work + "-rt", "--scratch-root", self.tmp,
            "--skip-l4", "--bin-a", sys.executable, "--bin-b", sys.executable, *flags,
          ]), 0)
        with open(os.path.join(work, "report.json"), encoding="utf-8") as f:
          report = json.load(f)
        self.assertEqual(report.get("graphql_modes_requested"), requested)
        self.assertEqual("GraphQL modes requested:" in errors.getvalue(), requested is not None)

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
