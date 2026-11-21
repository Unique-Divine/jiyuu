#!/usr/bin/env python3
"""Mori parity harness: index a corpus with two builds and diff L0-L4.

Zero dependencies (stdlib only). Run with `python3 -I`.

Commands:
  run    Index every corpus repo with --bin-a and --bin-b (default: both
         the C build), diff L0-L3 per repo, replay the calls (L4), apply
         the allow-lists, print a per-layer summary. Exit 1 on any
         unlisted difference, 2 on a setup problem (moved checkout, failed
         index, load-dependent parse timeouts).
  ci     The same on small generated fixture repos and calls-ci.jsonl.
  pin    Write a corpus file from a repos manifest: current HEAD per repo
         (or a tree hash), fresh .cbmignore copies.
  calls  Build a calls file from Claude Code session logs.
  seed   Print allow-list entries for every difference in a report.

Corpora. The committed public corpus (`corpus-public.json`, the default)
names public repos only; each is fetched at its pinned commit into a
local cache and indexed from an extracted copy. A private corpus, its
calls, and its own allow-list stay outside this repository; point the
harness at them with `--private-dir DIR` (reads `corpus.json`,
`calls.jsonl`, and, when present, `allowlist.jsonl`) or with `--corpus`,
`--calls`, and `--allowlist`. `pin`, `calls`, and `seed` write only where
`--out`/`--corpus` says, never into this directory by default.

Each side gets its own scratch CBM_CACHE_DIR under --work and a short
CBM_RUNTIME_DIR (`<rt-prefix>-a`, `-b`; long paths break the Unix
socket). Every directory the harness creates or deletes must sit inside a
scratch root (`--scratch-root`, default /tmp and the system temp dir).
Subprocesses get PATH, HOME, and those two variables only, so the live
cache and daemon are never touched.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shlex
import shutil
import sqlite3
import stat
import subprocess
import sys
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from typing import Any

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import allowlist  # noqa: E402
import calls as calls_mod  # noqa: E402
import corpus  # noqa: E402
import fixtures  # noqa: E402
from graph_export import LAYERS, DiffReport, diff_exports, export_db  # noqa

MORI = os.path.dirname(HERE)
DEFAULT_BIN = os.path.join(MORI, "build", "c", "codebase-memory-mcp")
ALLOWLIST = os.path.join(HERE, "allowlist.jsonl")
PUBLIC_CORPUS = os.path.join(HERE, "corpus-public.json")
PUBLIC_CALLS = os.path.join(HERE, "calls-public.jsonl")
CALLS_CI = os.path.join(HERE, "calls-ci.jsonl")
FETCH_CACHE = os.path.join(os.path.expanduser("~"), ".cache", "jiyuu-mori",
                           "parity")
MARKER = ".mori-parity"
ALL_LAYERS = ("index",) + LAYERS + ("L4",)
INDEX_TIMEOUT_S = 3600
CALL_TIMEOUT_S = 300
RETRY_PAUSE_S = 10
# What the C runtime leaves in CBM_RUNTIME_DIR: a `cbm-daemon-<uid>`
# directory of sockets, their sidecars, and lock files (src/daemon/ipc.c).
LOCK_SUFFIXES = (".lock", ".turn", ".rw", ".anc", ".sock.identity",
                 ".sock.pending")


def log(msg: str) -> None:
  print(msg, file=sys.stderr, flush=True)


# ---------------------------------------------------------------------------
# Scratch directories


def default_scratch_roots() -> list[str]:
  return sorted({os.path.realpath("/tmp"),
                 os.path.realpath(tempfile.gettempdir())})


def check_scratch(path: str, roots: list[str]) -> str:
  """The absolute path, if it lies strictly inside a scratch root."""
  real = os.path.realpath(path)
  for r in roots:
    r = os.path.realpath(r)
    if real.startswith(r.rstrip("/") + "/"):
      return real
  raise RuntimeError(f"{path} is outside the scratch roots "
                     f"({', '.join(roots)}); pass --scratch-root to add one")


def check_owned_dir(path: str) -> None:
  if os.path.islink(path) or not os.path.isdir(path):
    raise RuntimeError(f"{path} exists and is not a directory")
  if os.stat(path).st_uid != os.getuid():
    raise RuntimeError(f"{path} is not owned by this user")


def fresh_work_dir(path: str, roots: list[str]) -> None:
  """Create an empty mode-700 directory carrying the harness marker. An
  existing one is removed only if it carries the marker."""
  check_scratch(path, roots)
  if os.path.lexists(path):
    check_owned_dir(path)
    if not os.path.exists(os.path.join(path, MARKER)):
      raise RuntimeError(f"{path} exists without {MARKER}; remove it by "
                         "hand or pick another --work")
    shutil.rmtree(path)
  os.makedirs(path, mode=0o700)
  os.chmod(path, 0o700)
  open(os.path.join(path, MARKER), "w").close()


def runtime_leftovers(path: str) -> list[str]:
  """Entries under a runtime dir that are not sockets, `cbm-*` lock and
  socket-sidecar files, the harness marker, or `cbm-daemon-*`
  directories holding only those."""
  bad = []
  for name in os.listdir(path):
    p = os.path.join(path, name)
    st = os.lstat(p)
    if name == MARKER and stat.S_ISREG(st.st_mode):
      continue
    if stat.S_ISSOCK(st.st_mode):
      continue
    if stat.S_ISREG(st.st_mode) and name.startswith("cbm-") and \
        name.endswith(LOCK_SUFFIXES):
      continue
    if stat.S_ISDIR(st.st_mode) and name.startswith("cbm-daemon-"):
      bad.extend(os.path.join(name, b) for b in runtime_leftovers(p))
      continue
    bad.append(name)
  return bad


def fresh_runtime_dir(path: str, roots: list[str]) -> None:
  """Create an empty mode-700 runtime dir with the marker. An existing
  one is removed only when it holds nothing but sockets, lock files, or
  the marker, so a mistyped --rt-prefix never deletes data."""
  check_scratch(path, roots)
  if os.path.lexists(path):
    check_owned_dir(path)
    bad = runtime_leftovers(path)
    if bad:
      raise RuntimeError(f"{path} holds more than sockets and lock files "
                         f"({', '.join(sorted(bad)[:3])}); remove it by "
                         "hand or pick another --rt-prefix")
    shutil.rmtree(path)
  os.makedirs(path, mode=0o700)
  os.chmod(path, 0o700)
  open(os.path.join(path, MARKER), "w").close()


# ---------------------------------------------------------------------------
# Sides


@dataclass
class Side:
  name: str
  bin: str
  cache: str
  rt: str

  def env(self) -> dict[str, str]:
    return {
      "PATH": "/usr/local/bin:/usr/bin:/bin",
      "HOME": os.environ.get("HOME", "/tmp"),
      "CBM_CACHE_DIR": self.cache,
      "CBM_RUNTIME_DIR": self.rt,
    }

  def cli(self, tool: str, args: dict[str, Any], timeout: int
          ) -> tuple[int, str, str]:
    p = subprocess.run(
      [self.bin, "cli", "--quiet", tool, json.dumps(args)], cwd="/",
      env=self.env(), capture_output=True, text=True, timeout=timeout,
      stdin=subprocess.DEVNULL)
    return p.returncode, p.stdout, p.stderr

  def daemon(self, action: str) -> int:
    p = subprocess.run([self.bin, "daemon", action], cwd="/",
                       env=self.env(), capture_output=True, text=True,
                       timeout=120, stdin=subprocess.DEVNULL)
    return p.returncode


def make_sides(bin_a: str, bin_b: str, work: str, rt_prefix: str,
               roots: list[str]) -> list[Side]:
  for b in (bin_a, bin_b):
    if not os.access(b, os.X_OK):
      raise RuntimeError(f"binary {b} not found or not executable "
                         "(run `just mori-build`)")
  fresh_work_dir(work, roots)
  sides = []
  for name, b in (("a", bin_a), ("b", bin_b)):
    cache = os.path.join(work, name, "cache")
    os.makedirs(cache, mode=0o700)
    rt = f"{rt_prefix}-{name}"
    fresh_runtime_dir(rt, roots)
    sides.append(Side(name, os.path.abspath(b), cache, rt))
  return sides


# ---------------------------------------------------------------------------
# Indexing


def parse_timeouts(db: str) -> set[str]:
  """Files the index dropped with `parse timeout`. The C parser has a
  CPU budget plus a wall-clock ceiling: a huge generated file trips it
  every run, and a loaded or swapping machine trips it on different files
  each run."""
  if not os.path.isfile(db):
    return set()
  conn = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
  try:
    return {r[0] for r in conn.execute(
      "SELECT rel_path FROM index_coverage WHERE kind = 'extract' AND "
      "detail LIKE 'parse timeout%'")}
  except sqlite3.Error:
    return set()
  finally:
    conn.close()


def project_rows(db: str) -> list[list[str]]:
  """Every (name, root_path) the database stores, sorted."""
  conn = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
  try:
    return sorted([r[0], r[1]] for r in conn.execute(
      "SELECT name, root_path FROM projects"))
  finally:
    conn.close()


def remove_project_db(cache: str, project: str) -> None:
  base = os.path.join(cache, f"{project}.db")
  for suffix in ("", "-wal", "-shm"):
    if os.path.exists(base + suffix):
      os.remove(base + suffix)


def timeout_sides(sets: dict[str, set[str]]) -> list[str]:
  """Sides to reindex: those that dropped a file the other side parsed.
  Files every side drops are deterministic and stay; the comparison is
  valid once all sides drop the same files."""
  common = set.intersection(*sets.values()) if sets else set()
  return sorted(s for s, files in sets.items() if files - common)


def timeout_verdict(attempts: dict[str, list[set[str]]]
                    ) -> tuple[dict[str, list[str]], dict[str, list[str]]]:
  """(persistent, varying) per side, from every attempt's parse-timeout
  set. A file is persistent when one side dropped it on each of its
  attempts (at least two) and the other side never did: a build that is
  slower or faster on that file, which is a parity difference. Anything
  else left over after the retries depends on load."""
  final = {n: a[-1] for n, a in attempts.items()}
  common = set.intersection(*final.values()) if final else set()
  persistent: dict[str, list[str]] = {}
  varying: dict[str, list[str]] = {}
  for n, sets in attempts.items():
    others = [s for m, a in attempts.items() if m != n for s in a]
    for f in sorted(final[n] - common):
      steady = len(sets) >= 2 and all(f in s for s in sets)
      if steady and not any(f in s for s in others):
        persistent.setdefault(n, []).append(f)
      else:
        varying.setdefault(n, []).append(f)
  return persistent, varying


def index_sides(sides: list[Side], root: str, retries: int
                ) -> dict[str, dict[str, Any]]:
  """Index on every side, then reindex from scratch any side whose
  parse-timeout set differs from the others, up to `retries` times, or
  until each such side dropped the same extra files three times running.
  Records every attempt's timeout set and the total index attempts."""
  out: dict[str, dict[str, Any]] = {}
  for s in sides:
    log(f"  index side {s.name} ...")
    out[s.name] = run_index(s, root)
    out[s.name]["attempts_total"] = out[s.name]["attempts"]
  by_name = {s.name: s for s in sides}

  def timeouts() -> dict[str, set[str]]:
    return {n: parse_timeouts(os.path.join(by_name[n].cache,
                                           f"{ix['project']}.db"))
            if ix["project"] else set() for n, ix in out.items()}

  sets = timeouts()
  history = {n: [s] for n, s in sets.items()}
  for attempt in range(1, retries + 1):
    redo = timeout_sides(sets)
    if not redo:
      break
    common = set.intersection(*sets.values())
    if all(len(history[n]) >= 3 and
           all(h - common == sets[n] - common for h in history[n])
           for n in redo):
      break
    for n in redo:
      extra = sorted(sets[n] - common)
      log(f"    side {n}: {len(extra)} files hit the parse timeout only "
          f"there ({', '.join(extra[:3])}); reindexing")
      remove_project_db(by_name[n].cache, out[n]["project"] or "")
      total = out[n]["attempts_total"]
      out[n] = run_index(by_name[n], root)
      out[n]["attempts_total"] = total + out[n]["attempts"]
    sets = timeouts()
    for n in redo:
      history[n].append(sets[n])
  for n, ix in out.items():
    ix["parse_timeouts"] = sorted(sets[n])
    ix["parse_timeout_attempts"] = [sorted(h) for h in history[n]]
  return out


def parse_index_output(out: str, err: str) -> dict[str, Any]:
  """The index result JSON, from stdout or, on failure, from stderr."""
  for text in (out, err):
    for line in [text] + text.splitlines():
      try:
        res = json.loads(line)
      except ValueError:
        continue
      if isinstance(res, dict) and "status" in res:
        return res
  return {"raw": out[-400:]}


def run_index(side: Side, root: str, attempts: int = 3) -> dict[str, Any]:
  """Full index of one repo. Retries, after a pause, a run that exits
  non-zero or reports `aborted_previous_preserved`: under load a temporary
  daemon can miss its rendezvous, and a checkout that changed while
  indexing aborts the run. The summary flags a side that needed more
  attempts than the other."""
  for attempt in range(1, attempts + 1):
    t0 = time.monotonic()
    try:
      code, out, err = side.cli("index_repository", {"repo_path": root},
                                INDEX_TIMEOUT_S)
    except subprocess.TimeoutExpired:
      code, out, err = -1, "", "index timeout"
    wall = time.monotonic() - t0
    res = parse_index_output(out, err)
    status = res.get("status", "?")
    failed = code != 0 or status != "indexed"
    if failed and attempt < attempts:
      log(f"    attempt {attempt}: exit {code}, status {status}; retrying")
      time.sleep(RETRY_PAUSE_S)
      continue
    return {"exit": code, "status": status, "wall_s": round(wall, 2),
            "attempts": attempt, "project": res.get("project"),
            "nodes": res.get("nodes"), "edges": res.get("edges"),
            "stderr": err[-400:], "raw": res.get("raw")}
  raise RuntimeError("unreachable")


# ---------------------------------------------------------------------------
# Results


@dataclass
class RepoResult:
  dir: str
  root: str
  index: dict[str, dict[str, Any]] = field(default_factory=dict)
  moved: bool = False
  error: str | None = None
  diffs: list[dict[str, Any]] = field(default_factory=list)
  counts: dict[str, int] = field(default_factory=dict)
  export_s: float = 0.0


def classify(diffs: list[Any], repo: str, entries: list[allowlist.Entry]
             ) -> list[dict[str, Any]]:
  out = []
  for d in diffs:
    e = allowlist.find(entries, d.layer, d.key, d.group, repo, d.row)
    out.append({"repo": repo, "layer": d.layer, "key": d.key,
                "group": d.group, "row": d.row, "count": d.count,
                "examples": d.examples,
                "allowed": e.reason if e else None})
  return out


def identity_diffs(rep: DiffReport, index: dict[str, dict[str, Any]],
                   rows: dict[str, list[list[str]]]) -> None:
  """L3: both sides must report and store the same project names and root
  paths. Agents pass the name back to every tool, and the exporters
  normalize both away, so this is the only place they are compared."""
  pa, pb = index["a"].get("project"), index["b"].get("project")
  if pa != pb:
    rep.add("L3", "projects.identity", "index_result", pa, pb,
            "project name in the index_repository result")
  if rows["a"] != rows["b"]:
    rep.add("L3", "projects.identity", "projects", rows["a"], rows["b"],
            "projects (name, root_path) rows")


def compare_repo(sides: list[Side], dir_: str, root: str,
                 entries: list[allowlist.Entry], retries: int = 4
                 ) -> RepoResult:
  r = RepoResult(dir_, root)
  before = corpus.fingerprint(root)
  r.index = index_sides(sides, root, retries)
  for n, ix in r.index.items():
    log(f"    side {n}: {ix['status']} in {ix['wall_s']} s, {ix['nodes']} "
        f"nodes, {ix['edges']} edges, {ix['attempts_total']} attempts, "
        f"parse timeouts {ix['parse_timeouts'] or 'none'}")
  if corpus.fingerprint(root) != before:
    r.moved = True
    r.error = "checkout changed during the run; rerun this repo"
    return r
  bad = [s for s, ix in r.index.items()
         if ix["exit"] != 0 or not ix["project"]]
  if bad:
    r.error = "index failed on side " + ", ".join(bad) + ": " + \
      json.dumps({s: r.index[s] for s in bad})[:600]
    return r
  rep = DiffReport()
  persistent, varying = timeout_verdict(
    {n: [set(h) for h in ix["parse_timeout_attempts"]]
     for n, ix in r.index.items()})
  if varying:
    r.error = "parse timeouts varied between attempts (machine load): " + \
      "; ".join(f"side {n}: {', '.join(fs[:3])}"
                for n, fs in sorted(varying.items())) + \
      "; rerun on a quieter machine"
    return r
  for n, files in sorted(persistent.items()):
    for f in files:
      rep.add("index", f"index.parse_timeout_only_in_{n}", f, f,
              f"dropped on every attempt on side {n} only", f)
  ia, ib = r.index["a"], r.index["b"]
  dba = os.path.join(sides[0].cache, f"{ia['project']}.db")
  dbb = os.path.join(sides[1].cache, f"{ib['project']}.db")
  identity_diffs(rep, r.index, {"a": project_rows(dba),
                                "b": project_rows(dbb)})
  t0 = time.monotonic()
  ea = export_db(dba)
  eb = export_db(dbb)
  r.counts = {"nodes": len(ea["nodes"]), "edges": len(ea["edges"])}
  graph = diff_exports(ea, eb).diffs()
  del ea, eb
  r.export_s = round(time.monotonic() - t0, 2)
  r.diffs = classify(rep.diffs() + graph, dir_, entries)
  return r


def node_names(side: Side, projects: dict[str, str],
               norm: calls_mod.OutputNormalizer) -> dict[str, str]:
  """{normalized qualified name: node name} over the side's databases,
  for search_graph's `ORDER BY name, id` (a qualified name doesn't show
  where the name starts). A qualified name with two names maps to none."""
  out: dict[str, str] = {}
  dup: set[str] = set()
  for project in projects.values():
    db = os.path.join(side.cache, f"{project}.db")
    if not os.path.isfile(db):
      continue
    conn = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    try:
      for qn, name in conn.execute(
          "SELECT qualified_name, name FROM nodes"):
        k = norm.text(qn or "")
        if k in out and out[k] != name:
          dup.add(k)
        out[k] = name
    finally:
      conn.close()
  for k in dup:
    del out[k]
  return out


def replay(sides: list[Side], call_list: list[dict[str, Any]],
           roots: dict[str, str], projects: list[dict[str, str]],
           entries: list[allowlist.Entry]
           ) -> tuple[list[dict[str, Any]], dict[str, Any]]:
  """L4: run each call on both sides and diff normalized output. Result
  tables are gated row by row (calls.diff_call); a call whose output was
  cut is replayed once more without its limit."""
  rep_by_repo: dict[str, DiffReport] = {}
  stats = {"calls": 0, "skipped": 0, "differing": 0, "errors_a": 0,
           "unlimited_replays": 0, "timeouts": 0, "wall_s": 0.0}
  norms = [calls_mod.OutputNormalizer(
    roots, projects[i], {"cache": s.cache, "rt": s.rt})
    for i, s in enumerate(sides)]
  names = [node_names(s, projects[i], norms[i])
           for i, s in enumerate(sides)]
  t0 = time.monotonic()
  pool = ThreadPoolExecutor(max_workers=len(sides))

  def run_sides(per_side: list[dict[str, Any] | None], tool: str
                ) -> list[dict[str, Any] | None]:
    def one(i: int) -> dict[str, Any] | None:
      if per_side[i] is None:
        return None
      args = calls_mod.resolve(per_side[i], roots, projects[i])
      try:
        code, out, err = sides[i].cli(tool, args, CALL_TIMEOUT_S)
      except subprocess.TimeoutExpired:
        stats["timeouts"] += 1
        return {"exit": -1, "out": "", "err": "", "timed_out": True}
      return norms[i].output(code, out, err)
    # Both sides run at once: they share nothing but the machine.
    return list(pool.map(one, range(len(sides))))

  for n, c in enumerate(call_list):
    need = calls_mod.placeholders(c["args"])
    dirs = sorted({d for _, d in need})
    if any(d not in roots for d in dirs):
      stats["skipped"] += 1
      continue
    repo = ",".join(dirs) or "-"
    tool = c["tool"]
    outs = run_sides([c["args"], c["args"]], tool)
    stats["calls"] += 1
    if outs[0]["exit"] != 0:
      stats["errors_a"] += 1
    rep = rep_by_repo.setdefault(repo, DiffReport())
    where = f"#{n} {tool} {json.dumps(c['args'], sort_keys=True)}"
    group = calls_mod.call_id(tool, c["args"])
    if any(o.get("timed_out") for o in outs):
      # A call that times out compared nothing, so it is a difference
      # even when both sides time out.
      rep.add("L4", f"{tool}:$.timed_out", group,
              bool(outs[0].get("timed_out")),
              bool(outs[1].get("timed_out")), where[:300])
      stats["differing"] += 1
      continue

    def rerun(per_side: list[dict[str, Any] | None], tool: str = tool
              ) -> list[dict[str, Any] | None]:
      stats["unlimited_replays"] += 1
      return run_sides(per_side, tool)

    if calls_mod.diff_call(rep, tool, c["args"], group, where[:300],
                           outs[0], outs[1], rerun,
                           (names[0].get, names[1].get)):
      stats["differing"] += 1
  pool.shutdown()
  stats["wall_s"] = round(time.monotonic() - t0, 1)
  diffs = []
  for repo, rep in sorted(rep_by_repo.items()):
    diffs.extend(classify(rep.diffs(), repo, entries))
  return diffs, stats


# ---------------------------------------------------------------------------
# Summary


def layer_summary(diffs: list[dict[str, Any]]) -> dict[str, dict[str, int]]:
  """{layer: {"allowed": n, "unlisted": n}} counting differing rows."""
  out = {l: {"allowed": 0, "unlisted": 0} for l in ALL_LAYERS}
  for d in diffs:
    k = "allowed" if d["allowed"] else "unlisted"
    out[d["layer"]][k] += d["count"]
  return out


def cell(s: dict[str, int]) -> str:
  if s["unlisted"]:
    return f"UNLISTED {s['unlisted']}"
  if s["allowed"]:
    return f"allowed {s['allowed']}"
  return "same"


def attempt_flags(results: list[RepoResult]) -> list[str]:
  """One line per repo where one side needed more index attempts than the
  other (failed or aborted runs, parse-timeout reindexes)."""
  out = []
  for r in results:
    n = {s: ix.get("attempts_total", ix.get("attempts", 1))
         for s, ix in r.index.items()}
    if len(set(n.values())) > 1:
      out.append(f"{r.dir}: " + ", ".join(f"side {s} {k}"
                                          for s, k in sorted(n.items())))
  return out


TABLE_LAYERS = ("index",) + LAYERS


def print_summary(results: list[RepoResult], l4: list[dict[str, Any]],
                  l4_stats: dict[str, Any] | None) -> None:
  head = ["repo", "index a (s)", "index b (s)", "nodes", "edges"]
  head += list(TABLE_LAYERS)
  print("| " + " | ".join(head) + " |")
  print("|" + "---|" * len(head))
  for r in results:
    ia, ib = r.index.get("a", {}), r.index.get("b", {})
    row = [r.dir, str(ia.get("wall_s", "-")), str(ib.get("wall_s", "-")),
           str(r.counts.get("nodes", "-")), str(r.counts.get("edges", "-"))]
    if r.error:
      row += [r.error[:60]] + [""] * (len(TABLE_LAYERS) - 1)
    else:
      s = layer_summary(r.diffs)
      row += [cell(s[l]) for l in TABLE_LAYERS]
    print("| " + " | ".join(row) + " |")
  flags = attempt_flags(results)
  if flags:
    print("\nFLAG: sides needed different index attempts (a flaky build "
          "under test can hide behind retries):")
    for f in flags:
      print(f"  {f}")
  if l4_stats is not None:
    s = layer_summary(l4)["L4"]
    print(f"\nL4: {l4_stats['calls']} calls replayed, "
          f"{l4_stats['unlimited_replays']} unlimited replay pages, "
          f"{l4_stats['skipped']} skipped (repo not in run), "
          f"{l4_stats.get('dropped', 0)} dropped (project not a corpus "
          f"placeholder), {l4_stats['errors_a']} error results, "
          f"{l4_stats['timeouts']} call timeouts, {l4_stats['differing']} "
          f"differing, {l4_stats['wall_s']} s: {cell(s)}")
  unlisted = [d for r in results for d in r.diffs if not d["allowed"]]
  unlisted += [d for d in l4 if not d["allowed"]]
  if unlisted:
    print(f"\nUnlisted differences ({len(unlisted)} keys):")
    for d in unlisted[:60]:
      print(f"  [{d['layer']}] {d['repo']} {d['key']} ({d['group']}): "
            f"{d['count']}")
      if d.get("row"):
        print(f"      row: {d['row'][:110]}")
      for ex in d["examples"][:1]:
        print(f"      {ex['where'][:110]}")
        print(f"      a: {ex['a'][:110]}")
        print(f"      b: {ex['b'][:110]}")


def finish(results: list[RepoResult], l4: list[dict[str, Any]],
           l4_stats: dict[str, Any] | None, work: str, meta: dict[str, Any]
           ) -> int:
  print_summary(results, l4, l4_stats)
  report = {
    **meta,
    "attempt_flags": attempt_flags(results),
    "repos": [{"dir": r.dir, "index": r.index, "moved": r.moved,
               "error": r.error, "counts": r.counts,
               "export_s": r.export_s, "diffs": r.diffs}
              for r in results],
    "l4": {"stats": l4_stats, "diffs": l4},
  }
  path = os.path.join(work, "report.json")
  with open(path, "w", encoding="utf-8") as f:
    json.dump(report, f, indent=1)
  if any(r.error for r in results):
    return 2
  unlisted = any(not d["allowed"] for r in results for d in r.diffs)
  unlisted = unlisted or any(not d["allowed"] for d in l4)
  return 1 if unlisted else 0


def run(repos: list[tuple[str, str]], call_list: list[dict[str, Any]] | None,
        dropped: int, a: argparse.Namespace, meta: dict[str, Any]) -> int:
  entries = allowlist.load_all(a.allowlist)
  sides = make_sides(a.bin_a, a.bin_b, a.work, a.rt_prefix, a.scratch_root)
  log(f"side a: {sides[0].bin}\nside b: {sides[1].bin}\nwork: {a.work}")
  results = []
  t0 = time.monotonic()
  for dir_, root in repos:
    log(f"repo {dir_} ({root})")
    try:
      results.append(compare_repo(sides, dir_, root, entries,
                                  a.timeout_retries))
    except (sqlite3.Error, OSError, ValueError, KeyError) as e:
      log(f"  {dir_}: comparison failed: {e!r}")
      r = RepoResult(dir_, root)
      r.error = f"comparison failed: {e!r}"[:300]
      results.append(r)
  l4: list[dict[str, Any]] = []
  l4_stats = None
  if call_list is not None:
    roots, projects = {}, [{}, {}]
    for r in results:
      if r.error:
        continue
      roots[r.dir] = r.root
      projects[0][r.dir] = r.index["a"]["project"]
      projects[1][r.dir] = r.index["b"]["project"]
    log(f"L4: replaying {len(call_list)} calls")
    started = [s.daemon("start") == 0 for s in sides]
    try:
      l4, l4_stats = replay(sides, call_list, roots, projects, entries)
      l4_stats["dropped"] = dropped
    finally:
      for s, ok in zip(sides, started):
        if ok:
          s.daemon("stop")
  meta = {**meta, "bin_a": sides[0].bin, "bin_b": sides[1].bin,
          "allowlists": a.allowlist,
          "total_s": round(time.monotonic() - t0, 1)}
  return finish(results, l4, l4_stats, a.work, meta)


# ---------------------------------------------------------------------------
# Commands


def parse_snapshot(value: str, dirs: list[str]) -> set[str]:
  """`--snapshot` dirs; unknown names are an error, not a silent skip."""
  want = {s for s in value.split(",") if s} if value else set()
  if "all" in want:
    return set(dirs)
  unknown = want - set(dirs)
  if unknown:
    raise ValueError(f"unknown --snapshot repos: {', '.join(sorted(unknown))}"
                     f" (corpus has {', '.join(dirs)})")
  return want


def resolve_inputs(a: argparse.Namespace) -> None:
  """Fill --corpus, --calls, and --allowlist from --private-dir or the
  public defaults."""
  priv = a.private_dir
  if priv and not os.path.isdir(priv):
    raise RuntimeError(f"--private-dir {priv} is not a directory")
  if not a.corpus:
    a.corpus = os.path.join(priv, "corpus.json") if priv else PUBLIC_CORPUS
  if not a.calls:
    a.calls = os.path.join(priv, "calls.jsonl") if priv else PUBLIC_CALLS
  lists = [ALLOWLIST]
  if priv and os.path.isfile(os.path.join(priv, "allowlist.jsonl")):
    lists.append(os.path.join(priv, "allowlist.jsonl"))
  a.allowlist = lists + (a.allowlist or [])


def cmd_run(a: argparse.Namespace) -> int:
  resolve_inputs(a)
  repos = corpus.load(a.corpus)
  if a.repos:
    want = set(a.repos.split(","))
    unknown = want - {r.dir for r in repos}
    if unknown:
      log(f"unknown corpus repos: {', '.join(sorted(unknown))}")
      return 2
    repos = [r for r in repos if r.dir in want]
  try:
    snap = parse_snapshot(a.snapshot, [r.dir for r in repos])
  except ValueError as e:
    log(str(e))
    return 2
  bad = []
  for r in repos:
    if r.source == "archive":
      continue
    chk = corpus.verify(r, a.home, a.allow_dirty)
    if chk.dirty and chk.ok:
      log(f"warning: {r.dir} is dirty (--allow-dirty)")
    if not chk.ok:
      bad.append(f"  {r.dir}: " + "; ".join(chk.problems))
  if bad:
    log("corpus does not match its pins:\n" + "\n".join(bad))
    return 2
  roots = []
  base = a.work + "-snap"
  if snap or any(r.source == "archive" for r in repos):
    fresh_work_dir(base, a.scratch_root)
  for r in repos:
    dest = os.path.join(base, r.dir)
    t0 = time.monotonic()
    if r.source == "archive":
      n = corpus.materialize(r, dest, a.fetch_cache)
      log(f"archive {r.dir} @ {r.commit[:12]}: {n} files in "
          f"{time.monotonic() - t0:.1f} s -> {dest}")
      roots.append((r.dir, dest))
    elif r.dir in snap:
      n = corpus.snapshot(r, a.home, dest)
      log(f"snapshot {r.dir}: {n} files in "
          f"{time.monotonic() - t0:.1f} s -> {dest}")
      roots.append((r.dir, dest))
    else:
      roots.append((r.dir, r.root(a.home)))
  call_list, dropped = (None, 0) if a.skip_l4 else \
    calls_mod.load_calls(a.calls)
  if dropped:
    log(f"dropped {dropped} calls whose project is not a corpus placeholder")
  return run(roots, call_list, dropped, a,
             {"mode": "corpus", "corpus": a.corpus,
              "allow_dirty": a.allow_dirty, "snapshot": sorted(snap)})


def corrupt_bin(mode: str, real: str, path: str, roots: list[str]) -> str:
  """A side-b executable that runs `real` through corrupt.py MODE."""
  check_scratch(path, roots)
  with open(path, "w", encoding="utf-8") as f:
    f.write("#!/bin/sh\nexec python3 -I " +
            shlex.quote(os.path.join(HERE, "corrupt.py")) + f" {mode} " +
            shlex.quote(os.path.abspath(real)) + ' "$@"\n')
  os.chmod(path, 0o700)
  return path


# What each corrupt.py mode must produce among the unlisted differences:
# (description, key pattern, required text in the call's `where`, or "").
CORRUPT_EXPECT = {
  "rows": [("a changed row in search_graph or query_graph",
            re.compile(r"^(search_graph|query_graph):.*\{only_in_[ab]\}$"),
            "")],
  "page": [("an order or boundary difference in unranked search_graph",
            re.compile(r"^search_graph:.*\{(order|boundary_\w+)\}$"),
            "name_pattern")],
}
CORRUPT_EXPECT["all"] = CORRUPT_EXPECT["rows"] + CORRUPT_EXPECT["page"]


def corrupt_verdict(mode: str, rc: int, report_path: str) -> int:
  """0 when a corrupted run failed for the reasons its mode targets, so an
  unrelated difference (setup noise, a clock) can't pass the self-test."""
  if rc != 1:
    log(f"corrupt {mode}: run exited {rc}, want 1")
    return 1
  with open(report_path, encoding="utf-8") as f:
    rep = json.load(f)
  diffs = [d for d in rep["l4"]["diffs"] if not d["allowed"]]
  missing = []
  for what, rx, where in CORRUPT_EXPECT[mode]:
    if not any(rx.search(d["key"]) and
               (not where or any(where in ex["where"]
                                 for ex in d["examples"]))
               for d in diffs):
      missing.append(what)
  if missing:
    log(f"corrupt {mode}: no " + "; no ".join(missing))
    return 1
  print(f"corrupt {mode}: caught ({len(diffs)} unlisted L4 keys)")
  return 0


def cmd_ci(a: argparse.Namespace) -> int:
  a.allowlist = [ALLOWLIST] + (a.allowlist or [])
  if a.corrupt:
    a.bin_b = corrupt_bin(a.corrupt, a.bin_b, a.work + "-corrupt.sh",
                          a.scratch_root)
  base = a.work + "-fixtures"
  fresh_work_dir(base, a.scratch_root)
  repos = fixtures.write_all(base)
  call_list, dropped = (None, 0) if a.skip_l4 else \
    calls_mod.load_calls(a.calls or CALLS_CI)
  rc = run(repos, call_list, dropped, a, {"mode": "fixtures"})
  if a.corrupt:
    return corrupt_verdict(a.corrupt, rc,
                           os.path.join(a.work, "report.json"))
  return rc


def cmd_pin(a: argparse.Namespace) -> int:
  doc = corpus.pin(a.manifest, a.ignore_dir, a.home, a.corpus)
  for r in doc["repos"]:
    pin = r.get("commit") or r.get("tree")
    print(f"{r['dir']}: {pin[:19]} cbmignore={r['cbmignore']}")
  return 0


def cmd_calls(a: argparse.Namespace) -> int:
  dirs = [r.dir for r in corpus.load(a.corpus)]
  t = calls_mod.Templater(a.home, dirs)
  files = calls_mod.transcript_files(a.logs)
  out, stats = calls_mod.collect(files, t)
  with open(a.out, "w", encoding="utf-8") as f:
    for c in out:
      f.write(json.dumps(c, sort_keys=True, ensure_ascii=False) + "\n")
  failed = sum(1 for c in out if c["failed"])
  print(f"{len(files)} logs; {stats['mcp']} MCP and {stats['cli']} CLI "
        f"calls; dropped {stats['mutating']} mutating, {stats['secret']} "
        f"with secret-like args, {stats['not_corpus']} on projects outside "
        f"the corpus, {stats['other']} other tools; wrote {len(out)} unique "
        f"calls ({failed} failed at least once) to {a.out}")
  return 0


def cmd_seed(a: argparse.Namespace) -> int:
  diffs = []
  for path in a.reports:
    with open(path, encoding="utf-8") as f:
      rep = json.load(f)
    # A repo whose run was invalid (parse timeouts, moved checkout)
    # measured nothing; leave out its diffs and the calls that touch it.
    bad = {r["dir"] for r in rep["repos"] if r["error"]}
    if bad:
      log(f"{path}: leaving out invalid repos {', '.join(sorted(bad))}")
    diffs += [d for r in rep["repos"] if r["dir"] not in bad
              for d in r["diffs"]]
    if rep["l4"]["stats"] and not bad:
      diffs += rep["l4"]["diffs"]
    elif rep["l4"]["stats"]:
      log(f"{path}: leaving out L4, which saw the invalid repos")
  entries, rejected = allowlist.seed_entries(diffs, a.measured)
  with open(a.out, "w", encoding="utf-8") as f:
    for e in entries:
      f.write(json.dumps(e, ensure_ascii=False) + "\n")
  print(f"wrote {len(entries)} entries to {a.out}")
  if rejected:
    print(f"{len(rejected)} differences are too broad to allow-list; fix "
          "them in the harness or the build:")
    for e in rejected:
      print(f"  [{e['layer']}] {e['key']} ({e['group']}) "
            f"{e.get('row', '')[:80]}: "
            + "; ".join(allowlist.lint_entry(e)))
  return 0


def main(argv: list[str]) -> int:
  home = os.path.expanduser("~")
  ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
  sub = ap.add_subparsers(dest="cmd", required=True)
  for name in ("run", "ci"):
    p = sub.add_parser(name)
    p.add_argument("--bin-a", default=DEFAULT_BIN)
    p.add_argument("--bin-b", default=DEFAULT_BIN)
    p.add_argument("--work", help="work dir (default: a new "
                   "/tmp/mori-parity-XXXX/w per run, so runs never share "
                   "or delete each other's dirs)")
    p.add_argument("--rt-prefix", help="runtime dir prefix (default: "
                   "beside the default work dir; keep it short)")
    p.add_argument("--scratch-root", action="append",
                   help="directory the harness may create and delete "
                   "inside (repeatable; default /tmp and the system temp "
                   "dir)")
    p.add_argument("--allowlist", action="append",
                   help="extra allow-list, after the committed one "
                   "(repeatable)")
    p.add_argument("--calls")
    p.add_argument("--skip-l4", action="store_true")
    p.add_argument("--timeout-retries", type=int, default=4,
                   help="reindexes when files hit the parse timeout")
    if name == "ci":
      p.add_argument("--corrupt", choices=("rows", "page", "all"),
                     help="harness self-test: run side b through "
                     "corrupt.py; exit 0 only when the mode's corruption "
                     "was reported")
    if name == "run":
      p.add_argument("--private-dir",
                     help="directory holding corpus.json, calls.jsonl, and "
                     "optionally allowlist.jsonl")
      p.add_argument("--corpus")
      p.add_argument("--home", default=home)
      p.add_argument("--fetch-cache", default=FETCH_CACHE,
                     help="bare clones for archive-source corpus repos")
      p.add_argument("--repos", help="comma-separated corpus dirs")
      p.add_argument("--allow-dirty", action="store_true")
      p.add_argument("--snapshot", default="",
                     help="comma-separated dirs (or 'all') to index from a "
                     "frozen copy under <work>-snap; for checkouts that "
                     "other work edits during the run")
  p = sub.add_parser("pin")
  p.add_argument("--manifest", required=True,
                 help="repos manifest: {\"repos\": [{dir, remote}]}")
  p.add_argument("--ignore-dir",
                 help="directory of <dir>.cbmignore files to copy")
  p.add_argument("--home", default=home)
  p.add_argument("--corpus", required=True, help="corpus file to write")
  p = sub.add_parser("calls")
  p.add_argument("--logs", default=os.path.join(home, ".claude",
                                                "projects"))
  p.add_argument("--out", required=True)
  p.add_argument("--home", default=home)
  p.add_argument("--corpus", required=True)
  p = sub.add_parser("seed")
  p.add_argument("reports", nargs="+", help="report.json of measured runs")
  p.add_argument("--out", required=True)
  p.add_argument("--measured", required=True,
                 help="what was measured, e.g. 'C-vs-C 2026-11-21'")
  a = ap.parse_args(argv)
  if getattr(a, "scratch_root", None) is None and a.cmd in ("run", "ci"):
    a.scratch_root = default_scratch_roots()
  base = None
  if a.cmd in ("run", "ci") and (a.work is None or a.rt_prefix is None):
    base = tempfile.mkdtemp(prefix="mori-parity-", dir="/tmp")
    a.work = a.work or os.path.join(base, "w")
    a.rt_prefix = a.rt_prefix or os.path.join(base, "rt")
  try:
    rc = {"run": cmd_run, "ci": cmd_ci, "pin": cmd_pin,
          "calls": cmd_calls, "seed": cmd_seed}[a.cmd](a)
  except (RuntimeError, OSError, ValueError) as e:
    log(f"error: {e}")
    rc = 2
  report = os.path.join(a.work, "report.json") if a.cmd in ("run", "ci") \
    else ""
  if base and a.cmd == "ci" and rc == 0:
    # A passing fixture run (or caught corruption) leaves nothing worth
    # reading behind.
    shutil.rmtree(base, ignore_errors=True)
    print("\nfixture run passed; its work dir was removed")
  elif report and os.path.isfile(report):
    print(f"\nreport: {report}")
  return rc


if __name__ == "__main__":
  sys.exit(main(sys.argv[1:]))
