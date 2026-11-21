"""Tests for call extraction, templating, and L4 output diffing."""

from __future__ import annotations

import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import calls  # noqa: E402
from graph_export import DiffReport  # noqa: E402

HOME = "/home/u"
DIRS = ["shop-api", "notes", "docs"]


def tool_use(uid: str, name: str, inp: dict) -> str:
  return json.dumps({"type": "assistant", "message": {"content": [
    {"type": "text", "text": "conversation text that must not leak"},
    {"type": "tool_use", "id": uid, "name": name, "input": inp}]}})


def tool_error(uid: str) -> str:
  return json.dumps({"type": "user", "message": {"content": [
    {"type": "tool_result", "tool_use_id": uid, "is_error": True,
     "content": "boom"}]}})


class ParseBashTest(unittest.TestCase):
  def test_json_form(self) -> None:
    cmd = ("codebase-memory-mcp cli --quiet query_graph "
           "'{\"project\":\"p\",\"query\":\"MATCH (n) RETURN n\"}' | head")
    self.assertEqual(calls.parse_bash_command(cmd), [
      ("query_graph", {"project": "p", "query": "MATCH (n) RETURN n"})])

  def test_flag_form_is_typed(self) -> None:
    cmd = ("~/.local/bin/codebase-memory-mcp cli --quiet search_graph "
           "--project p --name-pattern '.*x.*' --limit 5 2>&1; echo done")
    self.assertEqual(calls.parse_bash_command(cmd), [
      ("search_graph", {"project": "p", "name_pattern": ".*x.*",
                        "limit": 5})])

  def test_unreplayable_forms_are_skipped(self) -> None:
    for cmd in ("codebase-memory-mcp cli --quiet \"$@\"",
                "codebase-memory-mcp cli index_status --help",
                "codebase-memory-mcp cli --quiet search_graph --project $P",
                "codebase-memory-mcp cli --quiet search_graph 'unclosed"):
      self.assertEqual(calls.parse_bash_command(cmd), [], cmd)

  def test_two_calls_in_one_command(self) -> None:
    cmd = ("codebase-memory-mcp cli list_projects && "
           "codebase-memory-mcp cli --json index_status --project p")
    self.assertEqual([t for t, _ in calls.parse_bash_command(cmd)],
                     ["list_projects", "index_status"])


class TemplaterTest(unittest.TestCase):
  def test_roots_and_projects(self) -> None:
    t = calls.Templater(HOME, DIRS)
    self.assertEqual(t.text("/home/u/shop-api/src"),
                     "{{root:shop-api}}/src")
    self.assertEqual(t.text("home-u-notes"), "{{project:notes}}")
    self.assertEqual(t.text("STARTS WITH 'home-u-notes.epics'"),
                     "STARTS WITH '{{project:notes}}.epics'")
    # Worktrees and other projects keep their literal names.
    self.assertEqual(t.text("home-u-shop-api-fix-branch"),
                     "home-u-shop-api-fix-branch")
    self.assertEqual(t.text("home-u-notes-jiyuu"), "home-u-notes-jiyuu")
    self.assertEqual(t.text("/home/u/ghost"), "/home/u/ghost")


class CollectTest(unittest.TestCase):
  def test_aggregates_without_text(self) -> None:
    lines = [
      tool_use("1", "mcp__codebase-memory-mcp__search_graph",
               {"project": "home-u-notes", "query": "x"}),
      tool_use("2", "mcp__codebase-memory-mcp__search_graph",
               {"query": "x", "project": "home-u-notes"}),
      tool_use("3", "mcp__codebase-memory-mcp__query_graph",
               {"project": "home-u-docs", "query": "bad("}),
      tool_error("3"),
      tool_use("4", "mcp__codebase-memory-mcp__index_repository",
               {"repo_path": "/home/u/notes"}),
      tool_use("5", "Bash", {"command":
               "codebase-memory-mcp cli --quiet index_status "
               "--project home-u-shop-api"}),
      tool_use("6", "mcp__codebase-memory-mcp__search_code",
               {"project": "p", "pattern": "token=abc"}),
      # A worktree's project is not a corpus placeholder: dropped.
      tool_use("7", "mcp__codebase-memory-mcp__search_graph",
               {"project": "home-u-notes-wt", "query": "y"}),
      "not json",
    ]
    with tempfile.TemporaryDirectory() as d:
      path = os.path.join(d, "s.jsonl")
      with open(path, "w") as f:
        f.write("\n".join(lines) + "\n")
      out, stats = calls.collect([path], calls.Templater(HOME, DIRS))
    self.assertEqual(stats["mutating"], 1)
    self.assertEqual(stats["secret"], 1)
    self.assertEqual(stats["not_corpus"], 1)
    by_tool = {c["tool"]: c for c in out}
    self.assertEqual(len(out), 3)
    self.assertEqual(by_tool["search_graph"]["seen"], 2)
    self.assertEqual(by_tool["search_graph"]["args"]["project"],
                     "{{project:notes}}")
    self.assertTrue(by_tool["query_graph"]["failed"])
    self.assertFalse(by_tool["index_status"]["failed"])
    self.assertNotIn("conversation", json.dumps(out))


class ReplayTest(unittest.TestCase):
  def test_resolve(self) -> None:
    args = {"project": "{{project:notes}}", "paths": ["{{root:notes}}/a"]}
    got = calls.resolve(args, {"notes": "/r/notes"}, {"notes": "r-notes"})
    self.assertEqual(got, {"project": "r-notes", "paths": ["/r/notes/a"]})
    self.assertEqual(calls.placeholders(args),
                     {("project", "notes"), ("root", "notes")})
    with self.assertRaises(KeyError):
      calls.resolve({"project": "{{project:gh}}"}, {}, {})

  def test_normalizer_json(self) -> None:
    n = calls.OutputNormalizer({"notes": "/r/notes"}, {"notes": "r-notes"},
                               {"cache": "/tmp/w/a/cache"})
    out = n.output(0, json.dumps({
      "elapsed_ms": 5, "indexed_at": "t", "db": "/tmp/w/a/cache/r-notes.db",
      "file": "/r/notes/x.go", "score": 0.12345678}), "")
    self.assertEqual(out["out"], {
      "indexed_at": "t", "db": "{{cache}}/{{project:notes}}.db",
      "file": "{{root:notes}}/x.go", "score": 0.123457})

  def test_text_diff_is_aligned_and_labeled(self) -> None:
    n = calls.OutputNormalizer({}, {}, {})
    a = n.output(0, "total: 2\nelapsed_ms: 4\nindexed_at: t0\nrow a\n", "")
    b = n.output(0, "total: 2\nelapsed_ms: 9\nnew row\nindexed_at: t1\n"
                 "row a\n", "")
    rep = DiffReport()
    calls.diff_outputs(rep, "index_status", "call:x", "#0", a, b)
    self.assertEqual(sorted(d.key for d in rep.diffs()), [
      "index_status:$.out{*}{presence}", "index_status:$.out{indexed_at}"])

  def test_call_id_ignores_key_order(self) -> None:
    self.assertEqual(calls.call_id("q", {"a": 1, "b": 2}),
                     calls.call_id("q", {"b": 2, "a": 1}))
    self.assertNotEqual(calls.call_id("q", {"a": 1}),
                        calls.call_id("q", {"a": 2}))

  def test_same_output_has_no_diff(self) -> None:
    n = calls.OutputNormalizer({}, {}, {})
    a = n.output(1, "error: x\nelapsed_ms: 3", "warn took 10ms")
    b = n.output(1, "error: x\nelapsed_ms: 7", "warn took 12ms")
    rep = DiffReport()
    self.assertEqual(calls.diff_outputs(rep, "t", "call:x", "#0", a, b), 0)

  def test_stdout_durations_are_data(self) -> None:
    n = calls.OutputNormalizer({}, {}, {})
    a = n.output(0, "  sleep(10s) in jobs/10s.txt", "")
    b = n.output(0, "  sleep(20s) in jobs/20s.txt", "")
    rep = DiffReport()
    self.assertEqual(calls.diff_outputs(rep, "t", "call:x", "#0", a, b), 1)

  def test_a_missing_value_line_is_not_a_changed_value(self) -> None:
    n = calls.OutputNormalizer({}, {}, {})
    a = n.output(0, "indexed_at: t0\ntotal: 1", "")
    b = n.output(0, "total: 1", "")
    rep = DiffReport()
    calls.diff_outputs(rep, "index_status", "call:x", "#0", a, b)
    self.assertEqual([d.key for d in rep.diffs()],
                     ["index_status:$.out{indexed_at}{presence}"])


if __name__ == "__main__":
  unittest.main()
