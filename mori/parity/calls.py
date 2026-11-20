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
import json
import os
import re
import shlex
from typing import Any, Callable, Iterable, Iterator

from graph_export import DiffReport
from allowlist import generalize_key

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
  stats = {"mcp": 0, "cli": 0, "mutating": 0, "secret": 0, "other": 0}
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


def load_calls(path: str) -> list[dict[str, Any]]:
  with open(path, encoding="utf-8") as f:
    return [json.loads(l) for l in f if l.strip()]


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

  def lines(self, text: str) -> list[str]:
    """Text output as lines; a `key: value` line whose key is a duration
    loses its value, and inline durations become `<duration>`."""
    out = []
    for line in self.text(text).splitlines():
      m = TEXT_KEY_RE.match(line)
      if m and TIMING_KEY_RE.match(m.group(1)):
        line = line[:m.end()] + "<duration>"
      out.append(TEXT_TIMING_RE.sub("<duration>", line))
    return out

  def output(self, exit_code: int, stdout: str, stderr: str
             ) -> dict[str, Any]:
    try:
      body: Any = self.value(json.loads(stdout))
    except ValueError:
      body = TextLines(self.lines(stdout))
    return {"exit": exit_code, "out": body,
            "err": TextLines(self.lines(stderr))}


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


def diff_outputs(rep: DiffReport, tool: str, where: str,
                 a: dict[str, Any], b: dict[str, Any]) -> int:
  """Add L4 differences for one call; returns how many paths differ."""
  n = 0
  for path, va, vb in diff_json(a, b):
    rep.add("L4", generalize_key(f"{tool}:{path}"), "-", va, vb,
            f"{where} {path}")
    n += 1
  return n


Runner = Callable[[str, list[str], dict[str, str]], tuple[int, str, str]]
