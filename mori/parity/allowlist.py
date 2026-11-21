"""Allow-list of accepted parity differences (`allowlist.jsonl`).

One JSON object per line:

  {"layer": "L3", "key": "projects.indexed_at", "group": "{{project}}",
   "reason": "...", "measured": "C-vs-C 2026-11-21"}

`layer` is exact. `key`, `group`, and `row` are patterns in which `*`
matches any run of characters and everything else, including `[` and `?`,
is literal. An optional `repos` list limits the entry to those corpus
dirs. Every entry needs a reason.

L4 result-table rows (`search_graph`, `query_graph`) are classified one
row at a time: a differing row matches only an entry that names the call
(`group: "call:<id>"`), one repo, and a `row` pattern. `lint_entry`
rejects entries broad enough to hide a wrong result, and `parse` runs it.
"""

from __future__ import annotations

import json
import re
from dataclasses import dataclass
from typing import Any, Iterable

REQUIRED = ("layer", "key", "reason")
ROW_KEY_RE = re.compile(r"\{(only_in_[ab]|boundary_\w+|unstable_\w+_[ab])\}$")
TABLE_KEY_RE = re.compile(r"\{(only_in_[ab]|order|boundary_\w+|"
                          r"table_presence|cols|unstable\w*|"
                          r"full_incomplete)\}$")
# Differences no entry may accept.
NEVER_KEY_RE = re.compile(r"\{unstable_(count|order_[ab]|prefix_[ab])\}$")
CALL_GROUP_RE = re.compile(r"^call:[0-9a-f]{12}$")
MIN_ROW_LITERAL = 8


@dataclass(frozen=True)
class Entry:
  layer: str
  key: str
  group: str
  reason: str
  repos: tuple[str, ...] = ()
  row: str = ""

  def matches(self, layer: str, key: str, group: str, repo: str,
              row: str = "") -> bool:
    if layer != self.layer:
      return False
    if self.repos and repo not in self.repos:
      return False
    # A row difference needs an entry written for rows, and the reverse.
    if bool(row) != bool(self.row):
      return False
    if row and not star_match(self.row, row):
      return False
    return star_match(self.key, key) and star_match(self.group, group)


def star_match(pattern: str, s: str) -> bool:
  """Match with `*` as the only wildcard."""
  rx = ".*".join(re.escape(p) for p in pattern.split("*"))
  return re.fullmatch(rx, s, re.DOTALL) is not None


def lint_entry(d: dict[str, Any]) -> list[str]:
  """Problems that make an entry too broad to keep; empty when fine."""
  out = []
  layer, key = d.get("layer", ""), d.get("key", "")
  group, row = d.get("group", "*"), d.get("row", "")
  repos = d.get("repos") or []
  if not key.replace("*", "").strip(".:$[]{} "):
    out.append("key is only wildcards")
  if NEVER_KEY_RE.search(key):
    out.append("a capped call's count, order, or prefix change can't be "
               "accepted")
  if "{*}" in key:
    out.append("key accepts every unlabeled output line ({*})")
  if layer == "L4":
    if "*" in key.replace("[*]", ""):
      out.append("L4 key has a wildcard outside a list index")
    if TABLE_KEY_RE.search(key):
      if not CALL_GROUP_RE.match(group):
        out.append("result-table entry must name one call "
                   "(group call:<id>)")
      if len(repos) != 1:
        out.append("result-table entry must name exactly one repo")
    if ROW_KEY_RE.search(key):
      if len(row.replace("*", "")) < MIN_ROW_LITERAL:
        out.append(f"row pattern needs at least {MIN_ROW_LITERAL} "
                   "literal characters")
    elif row:
      out.append("row given on a key that has no rows")
  elif row:
    out.append("row is only for L4 result-table keys")
  if layer in ("L0", "L1") and group == "*" and not repos:
    out.append(f"{layer} entry with group * must name repos")
  return out


def parse(lines: Iterable[str]) -> list[Entry]:
  out = []
  for n, line in enumerate(lines, 1):
    line = line.strip()
    if not line or line.startswith("#"):
      continue
    try:
      d = json.loads(line)
    except ValueError as e:
      raise ValueError(f"allow-list line {n}: bad JSON: {e}") from e
    missing = [k for k in REQUIRED if not d.get(k)]
    if missing:
      raise ValueError(f"allow-list line {n}: missing {', '.join(missing)}")
    problems = lint_entry(d)
    if problems:
      raise ValueError(f"allow-list line {n}: " + "; ".join(problems))
    out.append(Entry(d["layer"], d["key"], d.get("group", "*"),
                     d["reason"], tuple(d.get("repos", ())),
                     d.get("row", "")))
  return out


def load(path: str) -> list[Entry]:
  with open(path, encoding="utf-8") as f:
    try:
      return parse(f)
    except ValueError as e:
      raise ValueError(f"{path}: {e}") from e


def load_all(paths: Iterable[str]) -> list[Entry]:
  out: list[Entry] = []
  for p in paths:
    out.extend(load(p))
  return out


def find(entries: list[Entry], layer: str, key: str, group: str,
         repo: str, row: str = "") -> Entry | None:
  for e in entries:
    if e.matches(layer, key, group, repo, row):
      return e
  return None


_INDEX = re.compile(r"\[\d+\]")


def generalize_key(key: str) -> str:
  """Turn list indices into wildcards so one entry covers every row."""
  return _INDEX.sub("[*]", key)


def seed_entries(diffs: list[dict[str, Any]], measured: str
                 ) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
  """(entries, rejected): one narrow entry per distinct (layer, key,
  group, row) seen in a measured run, limited to the repos that produced
  it. Reasons start as a measurement note; edit them to say why the key
  varies. Entries the lint rejects come back in `rejected`: fix their
  cause in the harness instead of accepting them."""
  agg: dict[tuple[str, str, str, str], dict[str, Any]] = {}
  for d in diffs:
    k = (d["layer"], generalize_key(d["key"]), d["group"],
         d.get("row", ""))
    a = agg.setdefault(k, {"repos": set(), "count": 0})
    a["repos"].add(d["repo"])
    a["count"] += d["count"]
  out, rejected = [], []
  for (layer, key, group, row), a in sorted(agg.items()):
    repos = sorted(a["repos"])
    e: dict[str, Any] = {
      "layer": layer, "key": key, "group": group,
      "reason": f"measured noise: {a['count']} differences in "
                f"{len(repos)} repos",
      "measured": measured,
    }
    if layer in ("L0", "L1", "L4") or row:
      e["repos"] = repos
    if row:
      e["row"] = row
    (rejected if lint_entry(e) else out).append(e)
  return out, rejected
