"""Allow-list of accepted parity differences (`allowlist.jsonl`).

One JSON object per line:

  {"layer": "L3", "key": "projects.indexed_at", "group": "*",
   "reason": "...", "measured": "C-vs-C 2026-11-20, 12 repos"}

`layer` is exact; `key` and `group` are patterns over a diff's
`table.field` key and its group (label, edge type, row key, or `-`) in
which `*` matches any run of characters and everything else, including
`[` and `?`, is literal. An optional
`repos` list limits the entry to those corpus dirs. Every entry needs a
reason. Seed entries come only from a measured run (`parity.py seed`).
"""

from __future__ import annotations

import json
import re
from dataclasses import dataclass
from typing import Any, Iterable

REQUIRED = ("layer", "key", "reason")


@dataclass(frozen=True)
class Entry:
  layer: str
  key: str
  group: str
  reason: str
  repos: tuple[str, ...] = ()

  def matches(self, layer: str, key: str, group: str, repo: str) -> bool:
    if layer != self.layer:
      return False
    if self.repos and repo not in self.repos:
      return False
    return star_match(self.key, key) and star_match(self.group, group)


def star_match(pattern: str, s: str) -> bool:
  """Match with `*` as the only wildcard."""
  rx = ".*".join(re.escape(p) for p in pattern.split("*"))
  return re.fullmatch(rx, s, re.DOTALL) is not None


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
    out.append(Entry(d["layer"], d["key"], d.get("group", "*"),
                     d["reason"], tuple(d.get("repos", ()))))
  return out


def load(path: str) -> list[Entry]:
  with open(path, encoding="utf-8") as f:
    return parse(f)


def find(entries: list[Entry], layer: str, key: str, group: str,
         repo: str) -> Entry | None:
  for e in entries:
    if e.matches(layer, key, group, repo):
      return e
  return None


_INDEX = re.compile(r"\[\d+\]")


def generalize_key(key: str) -> str:
  """Turn list indices into wildcards so one entry covers every row."""
  return _INDEX.sub("[*]", key)


def seed_entries(diffs: list[dict[str, Any]], measured: str,
                 max_groups: int = 3) -> list[dict[str, Any]]:
  """One entry per distinct (layer, generalized key, group) seen in a
  measured run, with the repos and counts that produced it. Reasons start
  as a measurement note; edit them to say why the key varies."""
  groups: dict[tuple[str, str], set[str]] = {}
  for d in diffs:
    groups.setdefault((d["layer"], generalize_key(d["key"])), set()).add(
      d["group"])
  agg: dict[tuple[str, str, str], dict[str, Any]] = {}
  for d in diffs:
    lk = (d["layer"], generalize_key(d["key"]))
    # Many groups (rows, labels) for one key: accept the key, not a list.
    group = d["group"] if len(groups[lk]) <= max_groups else "*"
    a = agg.setdefault((*lk, group), {"repos": set(), "count": 0})
    a["repos"].add(d["repo"])
    a["count"] += d["count"]
  out = []
  for (layer, key, group), a in sorted(agg.items()):
    repos = sorted(a["repos"])
    out.append({
      "layer": layer, "key": key, "group": group,
      "reason": f"measured C-vs-C noise: {a['count']} differences in "
                f"{len(repos)} repos",
      "measured": f"{measured}; repos: {', '.join(repos)}",
    })
  return out
