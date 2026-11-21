#!/usr/bin/env python3
"""A deliberately wrong build for testing the harness: runs the real
binary and corrupts result tables. `parity.py ci --corrupt MODE` uses it
as side b, and `just parity-corrupt` requires every mode to fail.

Usage: corrupt.py MODE REAL_BINARY ARGS...

Modes:
  rows  rewrite every search_graph and query_graph row (label, file
        extension, end line) and keep every count line.
  page  answer unranked search_graph with the end of the full
        `ORDER BY name, id` order instead of its start, with the same
        counts: a wrong page of the right set.
  all   page for unranked search_graph, rows for every other table call;
        one fixture run checks both.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import rows  # noqa: E402

MODES = ("rows", "page", "all")


def run(real: str, argv: list[str]) -> tuple[int, str, str]:
  p = subprocess.run([real] + argv, capture_output=True, text=True)
  return p.returncode, p.stdout, p.stderr


def corrupt_rows(out: str) -> str:
  out = out.replace(" Function ", " Variable ").replace(".ts ", ".WRONG ")
  return re.sub(r" (\d+)-(\d+) ",
                lambda m: f" {m.group(1)}-{int(m.group(2)) + 99} ", out)


def cell(c: rows.Cell) -> str:
  if c.quoted or " " in c.text or not c.text:
    return json.dumps(c.text)
  return c.text


def wrong_page(real: str, argv: list[str], out: str) -> str:
  """The last rows of the full order, rendered as a flat table."""
  p = rows.parse_text(out.split("\n"))
  rows.expand_refs(p)
  t = p.tables.get("results")
  if t is None or not t.rows:
    return out
  args = json.loads(argv[3])
  _, full, _ = run(real, argv[:3] + [json.dumps(
    {**args, "limit": 500, "offset": 0})])
  fp = rows.parse_text(full.split("\n"))
  rows.expand_refs(fp)
  new = list(reversed(fp.tables["results"].rows))[:len(t.rows)]
  lines = []
  for line in p.other:
    if line.startswith("results: "):
      lines.append(f"results: {len(new)}  (cols: {' '.join(t.cols)})")
      lines += ["  " + " ".join(cell(c) for c in r) for r in new]
    else:
      lines.append(line)
  return "\n".join(lines) + "\n"


def main(argv: list[str]) -> int:
  mode, real, rest = argv[0], argv[1], argv[2:]
  if mode not in MODES:
    print(f"corrupt.py: unknown mode {mode}", file=sys.stderr)
    return 2
  tool = rest[2] if len(rest) >= 4 and rest[0] == "cli" else ""
  if tool not in ("search_graph", "query_graph"):
    return subprocess.run([real] + rest).returncode
  code, out, err = run(real, rest)
  args = json.loads(rest[3])
  paged = tool == "search_graph" and "query" not in args and \
    args.get("format") != "json"
  if mode in ("page", "all") and paged:
    out = wrong_page(real, rest, out)
  elif mode == "rows" or (mode == "all" and not paged):
    out = corrupt_rows(out)
  sys.stdout.write(out)
  sys.stderr.write(err)
  return code


if __name__ == "__main__":
  sys.exit(main(sys.argv[1:]))
