"""Tests for allowlist parsing, matching, and seeding."""

from __future__ import annotations

import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import allowlist  # noqa: E402


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
      '{"layer": "L3", "key": "store_meta.v", "group": "db_uid", '
      '"reason": "random per file"}',
      '{"layer": "L4", "key": "index_status:*", "reason": "r", '
      '"repos": ["boku"]}',
    ])
    self.assertEqual(len(entries), 2)
    self.assertIsNotNone(allowlist.find(entries, "L3", "store_meta.v",
                                        "db_uid", "x"))
    self.assertIsNone(allowlist.find(entries, "L3", "store_meta.v",
                                     "schema_version", "x"))
    self.assertIsNone(allowlist.find(entries, "L2", "store_meta.v",
                                     "db_uid", "x"))
    self.assertIsNotNone(allowlist.find(entries, "L4", "index_status:$",
                                        "-", "boku"))
    self.assertIsNone(allowlist.find(entries, "L4", "index_status:$",
                                     "-", "nibiru"))

  def test_parse_requires_reason(self) -> None:
    with self.assertRaises(ValueError):
      allowlist.parse(['{"layer": "L3", "key": "x"}'])

  def test_seed_collapses_many_groups(self) -> None:
    diffs = [{"layer": "L3", "key": "store_meta.v", "group": "db_uid",
              "repo": "a", "count": 1},
             {"layer": "L3", "key": "store_meta.v", "group": "db_uid",
              "repo": "b", "count": 1}]
    diffs += [{"layer": "L4", "key": "q:$.out[3]", "group": f"g{i}",
               "repo": "a", "count": 2} for i in range(5)]
    seeded = allowlist.seed_entries(diffs, "test run")
    keys = [(e["layer"], e["key"], e["group"]) for e in seeded]
    self.assertEqual(keys, [("L3", "store_meta.v", "db_uid"),
                            ("L4", "q:$.out[*]", "*")])
    self.assertIn("2 repos", seeded[0]["reason"])
    self.assertEqual(len(allowlist.parse(
      [__import__("json").dumps(e) for e in seeded])), 2)


if __name__ == "__main__":
  unittest.main()
