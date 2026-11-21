"""Row-level gating for tools that return result tables (L4).

`search_graph` and `query_graph` answer with tables. In text form a table is

  results: 3  (cols: qn label file lines rank)
    proj.lib.stats.mean Function lib/stats.ts 3-5 -42.67
    ...

and a table with enough shared prefixes gets a sibling `<key>_refs` table
(`@N+suffix` cells mean `prefix N` + suffix). In JSON form it is an object
with `cols` (or `columns`) and `rows`.

Every row is compared, after refs are expanded. Rows are grouped into tie
classes, consecutive rows the tool may order freely among themselves:

- ranked (`search_graph` with a `rank` or `score` column): rows with the
  same score;
- keyed (`query_graph` with ORDER BY on returned columns): rows with the
  same ORDER BY values;
- strict (`query_graph` with ORDER BY on anything else): every row alone;
- unordered (`search_graph` without a rank, `query_graph` without ORDER
  BY): all rows in one class.

Inside a class the rows are compared as a multiset; the classes must come
in the same order. When the output was cut (a `limit`, a Cypher `LIMIT`),
the last class on each side is the boundary: the cut may keep different
members of a tie. Boundary rows are returned to the caller, which checks
each against a replay of the same call without the limit.
"""

from __future__ import annotations

import json
import re
from collections import Counter
from dataclasses import dataclass, field
from typing import Any

ROW_TOOLS = {"search_graph", "query_graph", "list_projects"}
# Columns whose values vary between identical index runs. They are
# compared on their own, under a labeled key the allow-list can name, and
# left out of the row text.
VOLATILE_COLS = {"size_bytes"}
REF_RULE_RE = re.compile(r"^[A-Za-z_][\w.-]*_ref_rule: ")
HEADER_RE = re.compile(r"^([A-Za-z_][\w.-]*): (\d+)  \(cols: (.*)\)$")
# search_graph's grouped form: rows under `prefix (file):` group lines.
GROUPED_RE = re.compile(
  r"^([A-Za-z_][\w.-]*): (\d+)  \(rows: name label lines in out([^;)]*);"
  r".*\)$")
GROUPED_COLS = ["qn", "label", "file", "lines", "in", "out"]
REF_RE = re.compile(r"^@(\d+)\+(.*)$", re.DOTALL)
RANK_COLS = ("rank", "score")
LIMIT_RE = re.compile(r"\s+LIMIT\s+(\d+)\s*;?\s*$", re.IGNORECASE)
ORDER_RE = re.compile(
  r"\bORDER\s+BY\s+(.+?)(?=\s+(?:SKIP|LIMIT)\b|\s*;?\s*$)",
  re.IGNORECASE | re.DOTALL)
SKIP_RE = re.compile(r"\bSKIP\s+\d+", re.IGNORECASE)
RETURN_RE = re.compile(r"\bRETURN\s+(?:DISTINCT\s+)?(.+?)"
                       r"(?=\s+(?:ORDER\s+BY|SKIP|LIMIT)\b|\s*;?\s*$)",
                       re.IGNORECASE | re.DOTALL)
TRUE_LABELS = ("has_more", "truncated")


@dataclass
class Cell:
  text: str
  quoted: bool = False


@dataclass
class Table:
  key: str
  cols: list[str]
  rows: list[list[Any]] = field(default_factory=list)


@dataclass
class Parsed:
  """A tool's text output split into tables and the remaining lines."""
  tables: dict[str, Table] = field(default_factory=dict)
  other: list[str] = field(default_factory=list)


_DEC = json.JSONDecoder()


def read_cell(s: str, i: int, stop: str = " ") -> tuple[Cell, int]:
  """One cell starting at s[i]: a JSON-quoted string or a run of
  characters up to `stop`."""
  if s[i:i + 1] == '"':
    try:
      val, j = _DEC.raw_decode(s, i)
    except ValueError:
      val, j = None, -1
    if isinstance(val, str):
      return Cell(val, True), j
  j = s.find(stop, i)
  j = len(s) if j < 0 else j
  return Cell(s[i:j]), j


def tokenize_row(line: str) -> list[Cell]:
  """Cells of one table row: JSON-quoted strings or runs of non-spaces."""
  s = line[2:] if line.startswith("  ") else line.lstrip()
  out: list[Cell] = []
  i = 0
  while i < len(s):
    if s[i] == " ":
      i += 1
      continue
    c, i = read_cell(s, i)
    out.append(c)
  return out


def group_header(line: str) -> tuple[str, str] | None:
  """(prefix, file) of a grouped-form line `prefix (file):`."""
  if line.startswith(" ") or not line.endswith("):"):
    return None
  prefix, i = read_cell(line, 0)
  if line[i:i + 2] != " (":
    return None
  f, j = read_cell(line, i + 2, ")")
  if line[j:] != "):":
    return None
  pre = "" if prefix.text == "-" and not prefix.quoted else prefix.text
  return pre, f.text


def parse_text(lines: list[str]) -> Parsed:
  """Split text output into tables and other lines. A table header line
  stays among the other lines as `key: N`, so the flat and the grouped
  form of the same rows compare equal (search_graph picks the shorter)."""
  p = Parsed()
  cur: Table | None = None
  group: tuple[str, str] | None = None
  grouped = False
  for line in lines:
    if cur is not None and line.startswith("  "):
      cells = tokenize_row(line)
      if grouped and group is not None and cells:
        pre, file = group
        name = cells[0].text
        qn = Cell(f"{pre}.{name}" if pre else name)
        cells = [qn] + cells[1:2] + [Cell(file)] + cells[2:]
      cur.rows.append(cells)
      continue
    if cur is not None and grouped:
      g = group_header(line)
      if g is not None:
        group = g
        continue
    cur, group, grouped = None, None, False
    m = HEADER_RE.match(line)
    g = GROUPED_RE.match(line)
    if m:
      cur = Table(m.group(1), m.group(3).split())
    elif g:
      cur = Table(g.group(1), GROUPED_COLS + g.group(3).split())
      grouped = True
    if cur is not None:
      p.tables[cur.key] = cur
      line = f"{cur.key}: {(m or g).group(2)}"
    p.other.append(line)
  return p


def expand_refs(p: Parsed) -> None:
  """Replace `@N+suffix` cells with their prefix and drop `<key>_refs`
  tables and `<key>_ref_rule` lines, whose presence and content depend on
  which rows were kept."""
  p.other = [l for l in p.other if not REF_RULE_RE.match(l)]
  for key in [k for k in p.tables if k.endswith("_refs")]:
    refs = p.tables.pop(key)
    base = key[:-len("_refs")]
    table = {r[0].text: r[1].text for r in refs.rows if len(r) >= 2}
    p.other = [l for l in p.other if not l.startswith(key + ":")]
    t = p.tables.get(base)
    if t is None:
      continue
    for row in t.rows:
      for c in row:
        m = REF_RE.match(c.text) if not c.quoted else None
        if m and m.group(1) in table:
          c.text = table[m.group(1)] + m.group(2)


def row_text(row: list[Any]) -> str:
  vals = [c.text if isinstance(c, Cell) else c for c in row]
  return json.dumps(vals, ensure_ascii=False, sort_keys=True)


def cells(row: list[Any]) -> list[Any]:
  return [c.text if isinstance(c, Cell) else c for c in row]


# ---------------------------------------------------------------------------
# Ordering modes


def split_top(s: str) -> list[str]:
  """Split on commas outside brackets and quotes."""
  out, depth, quote, cur = [], 0, "", ""
  for ch in s:
    if quote:
      cur += ch
      if ch == quote:
        quote = ""
      continue
    if ch in "'\"":
      quote = ch
    elif ch in "([{":
      depth += 1
    elif ch in ")]}":
      depth -= 1
    elif ch == "," and depth == 0:
      out.append(cur.strip())
      cur = ""
      continue
    cur += ch
  if cur.strip():
    out.append(cur.strip())
  return out


def order_mode(tool: str, args: dict[str, Any], cols: list[str]
               ) -> tuple[str, list[int]]:
  """("ranked", [i]) | ("keyed", [i...]) | ("strict", []) |
  ("unordered", [])."""
  if tool == "list_projects":
    return "strict", []
  if tool == "search_graph":
    if cols and cols[-1] in RANK_COLS:
      return "ranked", [len(cols) - 1]
    return "unordered", []
  q = str(args.get("query", ""))
  m = ORDER_RE.search(q)
  if not m:
    return "unordered", []
  aliases: dict[str, str] = {}
  r = RETURN_RE.search(q)
  if r:
    for item in split_top(r.group(1)):
      a = re.match(r"(.+?)\s+AS\s+([\w.]+)$", item, re.IGNORECASE)
      if a:
        aliases[a.group(1).strip()] = a.group(2)
  idx = []
  for item in split_top(m.group(1)):
    expr = re.sub(r"\s+(ASC|DESC|ASCENDING|DESCENDING)$", "", item,
                  flags=re.IGNORECASE).strip()
    name = expr if expr in cols else aliases.get(expr, "")
    if name not in cols:
      return "strict", []
    idx.append(cols.index(name))
  return "keyed", idx


def classes(rows: list[list[Any]], mode: str, idx: list[int]
            ) -> list[tuple[str, list[str]]]:
  """Tie classes in output order: (class key, row texts)."""
  out: list[tuple[str, list[str]]] = []
  for row in rows:
    vals = cells(row)
    text = row_text(row)
    if mode == "unordered":
      key = ""
    elif mode == "strict":
      key = text
    else:
      key = json.dumps([vals[i] if i < len(vals) else None for i in idx])
    if out and out[-1][0] == key and mode != "strict":
      out[-1][1].append(text)
    else:
      out.append((key, [text]))
  return out


# ---------------------------------------------------------------------------
# Truncation and the unlimited replay


def labeled(other: list[str]) -> dict[str, str]:
  out = {}
  for line in other:
    m = re.match(r"^([A-Za-z_][\w.-]*): (.*)$", line)
    if m:
      out[m.group(1)] = m.group(2).strip()
  return out


def cypher_limit(args: dict[str, Any]) -> int | None:
  m = LIMIT_RE.search(str(args.get("query", "")))
  return int(m.group(1)) if m else None


def is_truncated(tool: str, args: dict[str, Any], fields: dict[str, Any],
                 nrows: int) -> bool:
  """The tool says it cut the list, or a Cypher LIMIT may have (C reports
  `has_more: false` after a Cypher LIMIT)."""
  for k in TRUE_LABELS:
    if str(fields.get(k, "")).lower() == "true":
      return True
  if tool == "query_graph":
    n = cypher_limit(args)
    return n is not None and nrows >= n
  return False


SEARCH_PAGE = 500          # search_graph clamps limit to 500
QUERY_MAX_ROWS = 99998     # query_graph's largest max_rows
MAX_OUTPUT_TOKENS = 1000000  # both tools' largest output budget
MAX_PAGES = 200


def unlimited_args(tool: str, args: dict[str, Any], fields: dict[str, Any]
                   ) -> dict[str, Any] | None:
  """The first page of the same call without its limit and with the
  largest output budget, or None when it can't be built (an offset,
  cursor, or SKIP, which page instead of cutting). Later pages follow
  each side's own `next_offset` (next_page)."""
  if args.get("offset") or args.get("cursor"):
    return None
  big = {"max_output_tokens": MAX_OUTPUT_TOKENS}
  if tool == "search_graph":
    return {**args, **big, "limit": SEARCH_PAGE, "offset": 0}
  if tool == "query_graph":
    q = str(args.get("query", ""))
    if SKIP_RE.search(q):
      return None
    return {**args, **big, "query": LIMIT_RE.sub("", q),
            "max_rows": QUERY_MAX_ROWS}
  return None


def next_page(args: dict[str, Any], fields: dict[str, Any]
              ) -> dict[str, Any] | None:
  """The next page for one side: its own `next_offset` while it reports
  more rows. The byte budget can cut two sides at different rows, so each
  side pages by itself."""
  if str(fields.get("has_more", "")).lower() != "true":
    return None
  try:
    nxt = int(str(fields.get("next_offset", "")))
  except ValueError:
    return None
  if nxt <= int(args.get("offset", 0)):
    return None
  return {**args, "offset": nxt}


# ---------------------------------------------------------------------------
# Comparison


@dataclass
class RowDiff:
  kind: str        # only_in_a, only_in_b, order, boundary_only_in_a, ...
  row: str = ""
  a: Any = None
  b: Any = None


def compare_classes(ca: list[tuple[str, list[str]]],
                    cb: list[tuple[str, list[str]]]) -> list[RowDiff]:
  out: list[RowDiff] = []
  ma = Counter((k, r) for k, rows in ca for r in rows)
  mb = Counter((k, r) for k, rows in cb for r in rows)
  for (_, r), n in sorted((ma - mb).items()):
    out.extend(RowDiff("only_in_a", r, r, "<absent>") for _ in range(n))
  for (_, r), n in sorted((mb - ma).items()):
    out.extend(RowDiff("only_in_b", r, "<absent>", r) for _ in range(n))
  if not out and [k for k, _ in ca] != [k for k, _ in cb]:
    out.append(RowDiff("order", "", [k for k, _ in ca][:5],
                       [k for k, _ in cb][:5]))
  return out


@dataclass
class TableResult:
  diffs: list[RowDiff]
  # Boundary rows that differ between sides; each must be confirmed
  # against the side's own unlimited replay.
  boundary: dict[str, list[tuple[str, str]]]


def compare_tables(ta: Table, tb: Table, mode: str, idx: list[int],
                   truncated: bool) -> TableResult:
  ca = classes(ta.rows, mode, idx)
  cb = classes(tb.rows, mode, idx)
  res = TableResult([], {"a": [], "b": []})
  if truncated and ca and cb and mode != "strict":
    (ka, ra), (kb, rb) = ca[-1], cb[-1]
    res.diffs = compare_classes(ca[:-1], cb[:-1])
    if ka != kb:
      res.diffs.append(RowDiff("boundary_key", "", ka, kb))
      return res
    extra_a = Counter(ra) - Counter(rb)
    extra_b = Counter(rb) - Counter(ra)
    res.boundary["a"] = [(ka, r) for r in sorted(extra_a.elements())]
    res.boundary["b"] = [(kb, r) for r in sorted(extra_b.elements())]
    return res
  res.diffs = compare_classes(ca, cb)
  return res


def tables_of(body: Any) -> tuple[dict[str, Table], Any]:
  """(tables, rest) of a JSON output body: its cols/columns and rows as
  one table named `rows`; rest is what is left for the generic diff."""
  if isinstance(body, dict):
    cols = body.get("cols", body.get("columns"))
    rows = body.get("rows")
    if isinstance(cols, list) and isinstance(rows, list):
      t = Table("rows", [str(c) for c in cols],
                [r if isinstance(r, list) else [r] for r in rows])
      rest = {k: v for k, v in body.items() if k != "rows"}
      return {"rows": t}, rest
  return {}, body


def fields_of(rest: Any) -> dict[str, Any]:
  if isinstance(rest, dict):
    return rest
  if isinstance(rest, list):
    return labeled(rest)
  return {}
