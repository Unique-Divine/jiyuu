"""Layer L4: replayable tool calls (`calls.jsonl`) and their output diff.

`calls.jsonl` holds one `{"tool", "args", "seen", "failed"}` per line,
aggregated from Claude Code session logs: `mcp__codebase-memory-mcp__*`
tool_use inputs and `codebase-memory-mcp cli <tool>` Bash commands. Only
tool names and arguments are kept, never conversation text. Corpus paths
become `{{root:<dir>}}` and project names `{{project:<dir>}}`. `failed`
records that some recorded call returned an error; those calls stay in on
purpose, since error text is part of the tool contract.

Only read-only tools are kept: the runner does the indexing itself, so
`index_repository`, `delete_project`, `manage_adr`, and `ingest_traces`
calls are counted and dropped.
"""

from __future__ import annotations

import difflib
import hashlib
import json
import os
import re
import shlex
from typing import Any, Callable, Iterable, Iterator

import rows as rows_mod
from allowlist import generalize_key
from graph_export import DiffReport

MCP_PREFIX = "mcp__codebase-memory-mcp__"
READ_ONLY = {
  "query_graph", "search_graph", "list_projects", "index_status",
  "search_code", "check_index_coverage", "get_graph_schema", "trace_path",
  "get_code_snippet", "get_file_outline", "get_architecture",
  "compare_graphs", "detect_changes",
}
MUTATING = {"index_repository", "delete_project", "manage_adr",
            "ingest_traces"}
CLI_GLOBAL_FLAGS = {"--quiet", "--json", "--progress", "--verbose"}
CLI_RE = re.compile(r"codebase-memory-mcp(?:\.prev)?\s+cli\b")
REDIRECT_RE = re.compile(r"\d*>&\d+|\d*>>?\s*\S+|<\s*\S+")
# Strings that look like credentials; a call carrying one is dropped.
SECRET_RE = re.compile(
  r"(gh[pousr]_[A-Za-z0-9]{20,}|github_pat_|sk-[A-Za-z0-9_-]{20,}|"
  r"xox[abpr]-|AKIA[0-9A-Z]{16}|-----BEGIN|password\s*[=:]|"
  r"secret\s*[=:]|token\s*[=:])", re.IGNORECASE)


# ---------------------------------------------------------------------------
# Extraction from session logs


def iter_tool_events(lines: Iterable[str]) -> Iterator[tuple[str, Any]]:
  """Yield ("use", block) and ("error", tool_use_id) from transcript
  lines. Lines that are not JSON or carry no content list are skipped."""
  for line in lines:
    if "codebase-memory-mcp" not in line and '"is_error":true' not in \
        line.replace(" ", ""):
      continue
    try:
      d = json.loads(line)
    except ValueError:
      continue
    msg = d.get("message") if isinstance(d, dict) else None
    content = msg.get("content") if isinstance(msg, dict) else None
    if not isinstance(content, list):
      continue
    for b in content:
      if not isinstance(b, dict):
        continue
      if b.get("type") == "tool_use":
        yield "use", b
      elif b.get("type") == "tool_result" and b.get("is_error"):
        yield "error", b.get("tool_use_id")


def _scalar(v: str) -> Any:
  """Type a CLI flag value the way the C schema-typed parser would for
  the shapes seen in logs: integers, booleans, JSON arrays/objects."""
  if re.fullmatch(r"-?\d+", v):
    return int(v)
  if v in ("true", "false"):
    return v == "true"
  if v[:1] in ("[", "{"):
    try:
      return json.loads(v)
    except ValueError:
      pass
  return v


def parse_cli_tail(tokens: list[str]) -> tuple[str, dict[str, Any]] | None:
  """Parse the tokens after `cli`: global flags, the tool, then either one
  JSON object or `--flag value` pairs. None when the call cannot be
  replayed as written (help, shell variables, unknown shape)."""
  i = 0
  while i < len(tokens) and tokens[i] in CLI_GLOBAL_FLAGS | {"--format"}:
    i += 2 if tokens[i] == "--format" else 1
  if i >= len(tokens) or not re.fullmatch(r"[a-z_]+", tokens[i]):
    return None
  tool, rest = tokens[i], tokens[i + 1:]
  if any("$" in t or "`" in t for t in rest) or "--help" in rest:
    return None
  if not rest:
    return tool, {}
  if rest[0].startswith("{"):
    try:
      args = json.loads(rest[0])
    except ValueError:
      return None
    return (tool, args) if isinstance(args, dict) else None
  args: dict[str, Any] = {}
  j = 0
  while j < len(rest):
    t = rest[j]
    if not t.startswith("--"):
      return None
    body = t[2:]
    if "=" in body:
      k, v = body.split("=", 1)
      args[k.replace("-", "_")] = _scalar(v)
    elif j + 1 < len(rest) and not rest[j + 1].startswith("--"):
      args[body.replace("-", "_")] = _scalar(rest[j + 1])
      j += 1
    else:
      args[body.replace("-", "_")] = True
    j += 1
  return tool, args


def parse_bash_command(cmd: str) -> list[tuple[str, dict[str, Any]]]:
  """Every replayable `codebase-memory-mcp cli` call in a Bash command."""
  out = []
  for m in CLI_RE.finditer(cmd):
    tail = REDIRECT_RE.sub(" ", cmd[m.end():])
    lex = shlex.shlex(tail, posix=True, punctuation_chars=";&|()<>")
    lex.whitespace_split = True
    tokens: list[str] = []
    try:
      for tok in lex:
        if tok and all(c in ";&|()<>" for c in tok):
          break
        tokens.append(tok)
    except ValueError:
      continue
    parsed = parse_cli_tail(tokens)
    if parsed:
      out.append(parsed)
  return out


class Templater:
  """Replaces corpus roots and project names with placeholders."""

  def __init__(self, home: str, dirs: Iterable[str]) -> None:
    home = home.rstrip("/")
    alt = "|".join(re.escape(d) for d in sorted(dirs, key=len,
                                                 reverse=True))
    prefix = home.strip("/").replace("/", "-")
    self.root_re = re.compile(re.escape(home) + r"/(" + alt +
                              r")(?![\w.-])")
    self.proj_re = re.compile(r"(?<![\w-])" + re.escape(prefix) + r"-(" +
                              alt + r")(?![\w-])")

  def text(self, s: str) -> str:
    s = self.root_re.sub(lambda m: "{{root:" + m.group(1) + "}}", s)
    return self.proj_re.sub(lambda m: "{{project:" + m.group(1) + "}}", s)

  def value(self, v: Any) -> Any:
    if isinstance(v, str):
      return self.text(v)
    if isinstance(v, list):
      return [self.value(x) for x in v]
    if isinstance(v, dict):
      return {k: self.value(x) for k, x in v.items()}
    return v


def has_secret(v: Any) -> bool:
  return bool(SECRET_RE.search(json.dumps(v)))


def canonical(tool: str, args: dict[str, Any]) -> str:
  return json.dumps([tool, args], sort_keys=True, separators=(",", ":"))


def collect(files: Iterable[str], templater: Templater
            ) -> tuple[list[dict[str, Any]], dict[str, int]]:
  """Aggregate and deduplicate calls from transcript files."""
  calls: dict[str, dict[str, Any]] = {}
  stats = {"mcp": 0, "cli": 0, "mutating": 0, "secret": 0, "other": 0,
           "not_corpus": 0}
  for path in files:
    uses: dict[str, list[tuple[str, dict[str, Any]]]] = {}
    errors: set[str] = set()
    try:
      with open(path, encoding="utf-8", errors="replace") as f:
        for kind, x in iter_tool_events(f):
          if kind == "error":
            errors.add(x)
            continue
          name = x.get("name", "")
          inp = x.get("input") or {}
          if name.startswith(MCP_PREFIX):
            found = [(name[len(MCP_PREFIX):], inp)]
            stats["mcp"] += 1
          elif name == "Bash" and isinstance(inp.get("command"), str):
            found = parse_bash_command(inp["command"])
            stats["cli"] += len(found)
          else:
            continue
          uses.setdefault(x.get("id", ""), []).extend(found)
    except OSError:
      continue
    for use_id, found in uses.items():
      for tool, args in found:
        if tool in MUTATING:
          stats["mutating"] += 1
          continue
        if tool not in READ_ONLY:
          stats["other"] += 1
          continue
        if has_secret(args):
          stats["secret"] += 1
          continue
        targs = templater.value(args)
        if not project_is_placeholder(targs):
          stats["not_corpus"] += 1
          continue
        k = canonical(tool, targs)
        c = calls.setdefault(k, {"tool": tool, "args": targs, "seen": 0,
                                 "failed": False})
        c["seen"] += 1
        c["failed"] = c["failed"] or use_id in errors
  out = sorted(calls.values(), key=lambda c: canonical(c["tool"],
                                                       c["args"]))
  return out, stats


def transcript_files(root: str) -> list[str]:
  out = []
  for dirpath, _, filenames in os.walk(root):
    out.extend(os.path.join(dirpath, n) for n in filenames
               if n.endswith(".jsonl"))
  return sorted(out)


PROJECT_PLACEHOLDER_RE = re.compile(r"^\{\{project:[^}]+\}\}$")


def project_is_placeholder(args: dict[str, Any]) -> bool:
  """True when the call names no project or a corpus placeholder. A call
  on any other project (a worktree, a scratch index) can't be replayed
  and may carry a private name."""
  p = args.get("project")
  return p is None or (isinstance(p, str) and
                       PROJECT_PLACEHOLDER_RE.match(p) is not None)


def load_calls(path: str) -> tuple[list[dict[str, Any]], int]:
  """(calls, dropped): calls whose project isn't a corpus placeholder are
  dropped."""
  with open(path, encoding="utf-8") as f:
    raw = [json.loads(l) for l in f if l.strip()]
  keep = [c for c in raw if project_is_placeholder(c.get("args") or {})]
  return keep, len(raw) - len(keep)


# ---------------------------------------------------------------------------
# Replay


PLACEHOLDER_RE = re.compile(r"\{\{(root|project):([^}]+)\}\}")


def placeholders(v: Any) -> set[tuple[str, str]]:
  return set(PLACEHOLDER_RE.findall(json.dumps(v)))


def resolve(v: Any, roots: dict[str, str], projects: dict[str, str]
            ) -> Any:
  """Fill placeholders. Raises KeyError for a dir not in this run."""
  if isinstance(v, str):
    def sub(m: re.Match[str]) -> str:
      table = roots if m.group(1) == "root" else projects
      return table[m.group(2)]
    return PLACEHOLDER_RE.sub(sub, v)
  if isinstance(v, list):
    return [resolve(x, roots, projects) for x in v]
  if isinstance(v, dict):
    return {k: resolve(x, roots, projects) for k, x in v.items()}
  return v


# Durations only. Wall-clock timestamps (`indexed_at`) stay in the output
# and are accepted through the measured allow-list.
TIMING_KEY_RE = re.compile(
  r"^(elapsed.*|.*_ms|.*_us|.*_ns|.*_secs?|.*_seconds|duration.*|took.*|"
  r"wall_.*)$", re.IGNORECASE)
TEXT_TIMING_RE = re.compile(r"\b\d+(?:\.\d+)?\s?(?:ms|µs|us|ns|s)\b")
TEXT_KEY_RE = re.compile(r"^\s*([A-Za-z_][\w.-]*)\s*[:=]\s")


class OutputNormalizer:
  """Parses tool output and removes what differs by construction between
  sides: timings, the side's cache and runtime paths, and repo roots and
  project names (which become the same placeholders as in calls.jsonl)."""

  def __init__(self, roots: dict[str, str], projects: dict[str, str],
               scratch: dict[str, str]) -> None:
    self.subs: list[tuple[str, str]] = []
    for name, path in scratch.items():
      self.subs.append((path.rstrip("/"), "{{" + name + "}}"))
    for d, r in roots.items():
      self.subs.append((r.rstrip("/"), "{{root:" + d + "}}"))
    for d, p in projects.items():
      self.subs.append((p, "{{project:" + d + "}}"))
    self.subs.sort(key=lambda s: len(s[0]), reverse=True)

  def text(self, s: str) -> str:
    for old, new in self.subs:
      if old and old in s:
        s = s.replace(old, new)
    return s

  def value(self, v: Any) -> Any:
    if isinstance(v, dict):
      return {k: self.value(x) for k, x in sorted(v.items())
              if not TIMING_KEY_RE.match(k)}
    if isinstance(v, list):
      return [self.value(x) for x in v]
    if isinstance(v, str):
      inner = v.strip()
      if inner[:1] in ("{", "[") and inner[-1:] in ("}", "]"):
        try:
          return {"__json__": self.value(json.loads(inner))}
        except ValueError:
          pass
      return self.text(v)
    if isinstance(v, float):
      return round(v, 6)
    return v

  def lines(self, text: str, inline: bool = False) -> list[str]:
    """Text output as lines; a `key: value` line whose key is a duration
    (TIMING_KEY_RE) loses its value. Inline durations (`took 3 ms`)
    become `<duration>` only when `inline` is set, which is for stderr:
    in stdout a `10s` can be a file name or a source literal. Row order
    is kept: tie handling belongs to the row comparison (rows.py)."""
    out = []
    for line in self.text(text).splitlines():
      m = TEXT_KEY_RE.match(line)
      if m and TIMING_KEY_RE.match(m.group(1)):
        line = line[:m.end()] + "<duration>"
      elif inline:
        line = TEXT_TIMING_RE.sub("<duration>", line)
      out.append(line)
    return out

  def output(self, exit_code: int, stdout: str, stderr: str
             ) -> dict[str, Any]:
    try:
      body: Any = self.value(json.loads(stdout))
    except ValueError:
      body = TextLines(self.lines(stdout))
    return {"exit": exit_code, "out": body,
            "err": TextLines(self.lines(stderr, inline=True))}


class TextLines(list):
  """Text output; diffed by line alignment, not by position."""


def line_label(line: str | None) -> str:
  m = TEXT_KEY_RE.match(line or "")
  return m.group(1) if m else "*"


def diff_lines(a: list[str], b: list[str], path: str
               ) -> Iterator[tuple[str, Any, Any]]:
  """Aligned line diff: one difference per changed line pair, inserted
  line, or deleted line, keyed by the line's `key:` label when it has
  one (`$.out{indexed_at}`), else `$.out{*}`."""
  sm = difflib.SequenceMatcher(None, a, b, autojunk=False)
  for op, i1, i2, j1, j2 in sm.get_opcodes():
    if op == "equal":
      continue
    # Inside a changed hunk, pair lines that carry the same label, then
    # unlabeled lines in order; whatever is left was added or removed.
    left = list(a[i1:i2])
    right = list(b[j1:j2])
    while left:
      la = left.pop(0)
      lab = line_label(la)
      k = next((n for n, lb in enumerate(right)
                if line_label(lb) == lab), None)
      lb = right.pop(k) if k is not None else "<absent>"
      yield f"{path}{{{lab}}}", la, lb
    for lb in right:
      yield f"{path}{{{line_label(lb)}}}", "<absent>", lb


def diff_json(a: Any, b: Any, path: str = "$"
              ) -> Iterator[tuple[str, Any, Any]]:
  if isinstance(a, TextLines) and isinstance(b, TextLines):
    yield from diff_lines(a, b, path)
  elif isinstance(a, dict) and isinstance(b, dict):
    for k in sorted(set(a) | set(b)):
      yield from diff_json(a.get(k, "<absent>"), b.get(k, "<absent>"),
                           f"{path}.{k}")
  elif isinstance(a, list) and isinstance(b, list):
    for i in range(min(len(a), len(b))):
      yield from diff_json(a[i], b[i], f"{path}[{i}]")
    if len(a) != len(b):
      yield f"{path}.length", len(a), len(b)
  elif a != b:
    yield path, a, b


def call_id(tool: str, args: dict[str, Any]) -> str:
  """Stable id of a templated call, the L4 group, so the allow-list can
  accept one nondeterministic call (an unordered LIMIT) by itself."""
  digest = hashlib.sha256(canonical(tool, args).encode()).hexdigest()
  return "call:" + digest[:12]


def l4_key(tool: str, path: str, va: Any, vb: Any) -> str:
  """The L4 key of one value difference. A line or field present on only
  one side gets `{presence}`, so an entry for a changing value (a wall
  clock) never accepts a value that went missing."""
  key = generalize_key(f"{tool}:{path}")
  if ABSENT in (va, vb):
    key += "{presence}"
  return key


ABSENT = "<absent>"


def diff_outputs(rep: DiffReport, tool: str, group: str, where: str,
                 a: dict[str, Any], b: dict[str, Any]) -> int:
  """Add L4 differences for one call; returns how many paths differ."""
  n = 0
  for path, va, vb in diff_json(a, b):
    rep.add("L4", l4_key(tool, path, va, vb), group, va, vb,
            f"{where} {path}")
    n += 1
  return n


# Runs per-side arguments (None skips a side) and returns per-side outputs.
Rerun = Callable[[list[Any]], list[Any]]


def _side_rows(rep: DiffReport, k: str, group: str, where: str,
               kind: str, side: str, rows: list[str]) -> int:
  for r in rows:
    rep.add("L4", f"{k}{{{kind}_{side}}}", group,
            r if side == "a" else ABSENT, r if side == "b" else ABSENT,
            where, row=r)
  return len(rows)


def diff_call(rep: DiffReport, tool: str, args: dict[str, Any],
              group: str, where: str, a: dict[str, Any], b: dict[str, Any],
              rerun: Rerun | None = None,
              names: Any = (None, None)) -> int:
  """Add L4 differences for one call; returns how many were added.

  Tools without result tables get the line or JSON diff. For the table
  tools (rows.ROW_TOOLS), every row is compared (rows.py): each differing
  row is its own difference, keyed `<tool>:$.<table>{only_in_a}` with the
  row text, so an allow-list entry must name it. A cut output is replayed
  without its limit (`rerun`, paging as needed); the full sets are gated
  under `$.full`, and boundary rows that differ between the cut outputs
  must be members of their side's full set. `names` maps each side's
  qualified names to node names, for search_graph's name order."""
  if tool not in rows_mod.ROW_TOOLS:
    return diff_outputs(rep, tool, group, where, a, b)
  n = _diff_table_call(rep, tool, args, group, where, a, b, rerun, names)
  if rerun is not None and _ranked_text(tool, args, a):
    # Text prints ranks rounded (-15.3), so rows whose real ranks differ
    # look tied and may swap unseen. The JSON form carries the full rank:
    # replay it once and gate its rows too (under `$.rows`).
    jargs = {**args, "format": "json"}
    ja, jb = rerun([jargs, jargs])
    n += _diff_table_call(rep, tool, jargs, group, f"{where} (json)", ja,
                          jb, rerun, names)
  return n


def _ranked_text(tool: str, args: dict[str, Any], a: dict[str, Any]) -> bool:
  if tool != "search_graph" or args.get("format") == "json" or \
      not isinstance(a["out"], TextLines):
    return False
  tables, _ = split_tables(a["out"])
  return any(t.cols and t.cols[-1] in rows_mod.RANK_COLS
             for t in tables.values())


def _diff_table_call(rep: DiffReport, tool: str, args: dict[str, Any],
                     group: str, where: str, a: dict[str, Any],
                     b: dict[str, Any], rerun: Rerun | None,
                     names: Any) -> int:
  pa, pb = split_tables(a["out"]), split_tables(b["out"])
  n = _diff_rest(rep, tool, group, where, "$", a, b, pa[1], pb[1])
  fa, fb = rows_mod.fields_of(pa[1]), rows_mod.fields_of(pb[1])
  ua = rows_mod.unlimited_args(tool, args, fa)
  if unstable(fa) or unstable(fb):
    return n + _diff_unstable(rep, tool, args, group, where, pa[0], pb[0],
                              ua, rerun, names)
  gated = _gate(rep, tool, args, group, where, "$", pa[0], pb[0],
                rows_mod.is_truncated(tool, args, fa, _nrows(pa[0])) or
                rows_mod.is_truncated(tool, args, fb, _nrows(pb[0])), names)
  n += gated[0]
  need = gated[1]
  if not need:
    return n
  full = _fetch_full(tool, ua, rerun, rep, group, where) \
    if ua is not None and rerun is not None else None
  if full is None:
    for k, res, _, _ in need:
      for side in ("a", "b"):
        n += _side_rows(rep, k, group, where, "boundary_unverified", side,
                        [r for _, r in res.boundary[side]])
    return n
  (fta, ftb), more, dn = full
  n += dn
  if fta or ftb:
    fn, fneed = _gate(rep, tool, ua, group, f"{where} (unlimited)",
                      "$.full", fta, ftb, more, names)
    n += fn
    if more:
      # The last class of the full sets was never seen to its end.
      for fk, _, _, _ in fneed:
        rep.add("L4", f"{fk}{{full_incomplete}}", group, True, True,
                f"{where} (unlimited, {rows_mod.MAX_PAGES} pages)")
        n += 1
  for k, res, mode, idx in need:
    key = k.rsplit(".", 1)[1]
    for side, tf, name_of in (("a", fta, names[0]), ("b", ftb, names[1])):
      have = set()
      if key in tf:
        have = {(ck, r) for ck, rs in
                rows_mod.classes(tf[key].rows, mode, idx, name_of)
                for r in rs}
      n += _side_rows(rep, k, group, where, "boundary_only_in", side,
                      [r for ck, r in res.boundary[side]
                       if (ck, r) not in have])
  return n


def _diff_unstable(rep: DiffReport, tool: str, args: dict[str, Any],
                   group: str, where: str, ta: dict[str, rows_mod.Table],
                   tb: dict[str, rows_mod.Table], ua: Any,
                   rerun: Rerun | None, names: Any = (None, None)) -> int:
  """The engine evaluated a capped candidate set (a Cypher engine limit,
  a BM25 cap reported as `total_relation: gte`), so which rows come back
  depends on scan order (row ids). Equal rows pass. Otherwise the call
  gets one `{unstable}` difference per table, which a call-scoped entry
  may accept, and the rows are still gated: the counts must match
  (`{unstable_count}`, never accepted), and each side's rows must appear
  in the other side's unlimited replay (`{unstable_only_in_a}` per row),
  and search pages must keep their order mode
  (`{unstable_order_a}`). Every ordered page must lead its own unlimited
  replay, ties allowed at the cut (`{unstable_prefix_a}`). The last three
  are never accepted."""
  n = 0
  diff_keys = []
  for key in sorted(set(ta) | set(tb)):
    ra = [rows_mod.row_text(r) for r in ta[key].rows] if key in ta else []
    rb = [rows_mod.row_text(r) for r in tb[key].rows] if key in tb else []
    if ra == rb:
      continue
    k = f"{tool}:$.{key}"
    rep.add("L4", k + "{unstable}", group, len(ra), len(rb), where)
    n += 1
    if len(ra) != len(rb):
      rep.add("L4", k + "{unstable_count}", group, len(ra), len(rb), where)
      n += 1
    diff_keys.append((key, k, ra, rb))
  if not diff_keys:
    return n
  full = _fetch_full(tool, ua, rerun, rep, group, where) \
    if ua is not None and rerun is not None else None
  for key, k, ra, rb in diff_keys:
    if full is None:
      n += _side_rows(rep, k, group, where, "unstable_unverified", "a",
                      sorted(set(ra) - set(rb)))
      n += _side_rows(rep, k, group, where, "unstable_unverified", "b",
                      sorted(set(rb) - set(ra)))
      continue
    (fta, ftb), _, dn = full
    n += dn
    in_b = {rows_mod.row_text(r) for r in ftb[key].rows} if key in ftb \
      else set()
    in_a = {rows_mod.row_text(r) for r in fta[key].rows} if key in fta \
      else set()
    n += _side_rows(rep, k, group, where, "unstable_only_in", "a",
                    sorted(r for r in set(ra) if r not in in_b))
    n += _side_rows(rep, k, group, where, "unstable_only_in", "b",
                    sorted(r for r in set(rb) if r not in in_a))
    for side, page, ft, name_of in (("a", ta, fta, names[0]),
                                    ("b", tb, ftb, names[1])):
      if key not in page:
        continue
      t = page[key]
      mode, idx = rows_mod.order_mode(tool, args, t.cols)
      bad = rows_mod.out_of_order(t, mode, idx, name_of)
      if bad:
        rep.add("L4", f"{k}{{unstable_order_{side}}}", group, len(bad),
                bad[0], where)
        n += 1
      full_t = ft.get(key)
      if mode != "unordered" and full_t is not None and \
          not rows_mod.is_leading(
            rows_mod.classes(t.rows, mode, idx, name_of),
            rows_mod.classes(full_t.rows, mode, idx, name_of)):
        rep.add("L4", f"{k}{{unstable_prefix_{side}}}", group, len(t.rows),
                len(full_t.rows), where)
        n += 1
  return n


def unstable(fields: dict[str, Any]) -> bool:
  """The tool says its rows came from a capped evaluation."""
  return fields.get("truncation_reason") == "engine_limit" or \
    str(fields.get("total_relation", "")) == "gte"


def _nrows(tables: dict[str, rows_mod.Table]) -> int:
  return max((len(t.rows) for t in tables.values()), default=0)


def _diff_rest(rep: DiffReport, tool: str, group: str, where: str,
               path: str, a: dict[str, Any], b: dict[str, Any],
               rest_a: Any, rest_b: Any) -> int:
  n = 0
  for p, va, vb in diff_json({**a, "out": rest_a}, {**b, "out": rest_b},
                             path):
    rep.add("L4", l4_key(tool, p, va, vb), group, va, vb,
            f"{where} {p}")
    n += 1
  return n


def _gate(rep: DiffReport, tool: str, args: dict[str, Any], group: str,
          where: str, path: str, ta: dict[str, rows_mod.Table],
          tb: dict[str, rows_mod.Table], trunc: bool,
          names: Any = (None, None)
          ) -> tuple[int, list[tuple[str, Any, str, list[int]]]]:
  """Compare tables row by row. Returns (differences added, tables that
  need the full replay: (key, result, mode, idx))."""
  n = 0
  need = []
  for key in sorted(set(ta) | set(tb)):
    k = f"{tool}:{path}.{key}"
    if key not in ta or key not in tb:
      rep.add("L4", k + "{table_presence}", group, key in ta, key in tb,
              where)
      n += 1
      continue
    t1, t2 = ta[key], tb[key]
    if t1.cols != t2.cols:
      rep.add("L4", k + "{cols}", group, t1.cols, t2.cols, where)
      n += 1
      continue
    n += _split_volatile(rep, k, group, where, t1, t2)
    mode, idx = rows_mod.order_mode(tool, args, t1.cols)
    res = rows_mod.compare_tables(t1, t2, mode, idx, trunc, names[0],
                                  names[1])
    for d in res.diffs:
      rep.add("L4", f"{k}{{{d.kind}}}", group, d.a, d.b, where, row=d.row)
      n += 1
    if trunc or res.boundary["a"] or res.boundary["b"]:
      need.append((k, res, mode, idx))
  return n, need


def _split_volatile(rep: DiffReport, k: str, group: str, where: str,
                    t1: rows_mod.Table, t2: rows_mod.Table) -> int:
  """Compare volatile columns (rows.VOLATILE_COLS) row by row under
  `<key>{col:<name>}` and remove them from both tables, so a database
  size never turns a row into a row difference."""
  n = 0
  for col in [c for c in t1.cols if c in rows_mod.VOLATILE_COLS]:
    i = t1.cols.index(col)
    for r1, r2 in zip(t1.rows, t2.rows):
      v1 = rows_mod.cells(r1)[i] if i < len(r1) else None
      v2 = rows_mod.cells(r2)[i] if i < len(r2) else None
      if v1 != v2:
        rep.add("L4", f"{k}{{col:{col}}}", group, v1, v2, where)
        n += 1
    for t in (t1, t2):
      t.rows = [r[:i] + r[i + 1:] for r in t.rows]
      t.cols = t.cols[:i] + t.cols[i + 1:]
  return n


def _fetch_full(tool: str, ua: dict[str, Any], rerun: Rerun,
                rep: DiffReport, group: str, where: str
                ) -> tuple[tuple[dict[str, rows_mod.Table],
                                 dict[str, rows_mod.Table]], bool, int]:
  """Run the unlimited call on both sides, each following its own pages,
  and merge each side's tables. Returns ((tables a, tables b), whether a
  side still had more after MAX_PAGES, differences added). Only the rows
  and the first page's exit codes are compared: page sizes and offsets
  follow the byte budget and differ whenever row order does."""
  merged: list[dict[str, rows_mod.Table]] = [{}, {}]
  pending: list[dict[str, Any] | None] = [ua, ua]
  n = 0
  for page in range(rows_mod.MAX_PAGES):
    if pending == [None, None]:
      break
    outs = rerun(list(pending))
    if page == 0 and outs[0]["exit"] != outs[1]["exit"]:
      rep.add("L4", f"{tool}:$.full.exit", group, outs[0]["exit"],
              outs[1]["exit"], f"{where} (unlimited)")
      n += 1
    for i, o in enumerate(outs):
      if o is None or pending[i] is None:
        continue
      if o.get("timed_out"):
        rep.add("L4", f"{tool}:$.full.timed_out", group, i == 0, i == 1,
                f"{where} (unlimited p{page})")
        n += 1
        pending[i] = None
        continue
      tables, rest = split_tables(o["out"])
      for key, t in tables.items():
        m = merged[i].setdefault(key, rows_mod.Table(key, t.cols))
        m.rows.extend(t.rows)
      pending[i] = rows_mod.next_page(pending[i], rows_mod.fields_of(rest))
  more = pending != [None, None]
  return (merged[0], merged[1]), more, n


def split_tables(body: Any) -> tuple[dict[str, rows_mod.Table], Any]:
  """(tables, rest) of a normalized output body. Text tables lose their
  rows and `_refs` tables from `rest`; JSON tables lose `rows`."""
  if isinstance(body, TextLines):
    p = rows_mod.parse_text(list(body))
    rows_mod.expand_refs(p)
    return p.tables, TextLines(p.other)
  return rows_mod.tables_of(body)
