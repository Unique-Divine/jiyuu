#!/usr/bin/env python3
"""Mori parity harness: index the corpus with two builds and diff L0-L4.

Zero dependencies (stdlib only). Run with `python3 -I`. Adapted from the
experiments in the Jiyuu Mori epic (Boku
`epics/boku/26-11-19-jiyuu-mori/rust-port/experiments/`).

Commands:
  run    Index every pinned corpus repo with --bin-a and --bin-b (default:
         both the C build), diff L0-L3 per repo, replay calls.jsonl (L4),
         apply allowlist.jsonl, print a per-layer summary. Exit 1 on any
         unlisted difference, 2 on a setup problem (moved checkout, failed
         index).
  ci     The same on small generated fixture repos and calls-ci.jsonl.
  pin    Rewrite corpus.json from the dotfiles repo manifest: current HEAD
         per repo, `gh` by tree hash, fresh .cbmignore copies.
  calls  Rebuild calls.jsonl from Claude Code session logs.
  seed   Write allow-list entries for every difference in a report.

Each side gets its own scratch CBM_CACHE_DIR under --work and a short
CBM_RUNTIME_DIR (`<rt-prefix>-a`, `-b`; long paths break the Unix socket).
Subprocesses get PATH, HOME, and those two variables only, so the live
cache (~/.cache/codebase-memory-mcp) and daemon are never touched.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import sqlite3
import subprocess
import sys
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
CALLS = os.path.join(HERE, "calls.jsonl")
CALLS_CI = os.path.join(HERE, "calls-ci.jsonl")
MARKER = ".mori-parity"
ALL_LAYERS = LAYERS + ("L4",)
INDEX_TIMEOUT_S = 3600
CALL_TIMEOUT_S = 300
RETRY_PAUSE_S = 10


def log(msg: str) -> None:
  print(msg, file=sys.stderr, flush=True)


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


def fresh_dir(path: str, marker: bool) -> None:
  """Create an empty mode-700 directory. An existing one is removed only
  if it carries the harness marker (or is an empty/socket-only runtime
  dir we own), so a typo never deletes someone's data."""
  if os.path.lexists(path):
    if os.path.islink(path) or not os.path.isdir(path):
      raise RuntimeError(f"{path} exists and is not a directory")
    st = os.stat(path)
    if st.st_uid != os.getuid():
      raise RuntimeError(f"{path} is not owned by this user")
    if marker and not os.path.exists(os.path.join(path, MARKER)):
      raise RuntimeError(f"{path} exists without {MARKER}; remove it by "
                         "hand or pick another --work")
    shutil.rmtree(path)
  os.makedirs(path, mode=0o700)
  os.chmod(path, 0o700)
  if marker:
    open(os.path.join(path, MARKER), "w").close()


def make_sides(bin_a: str, bin_b: str, work: str, rt_prefix: str
               ) -> list[Side]:
  fresh_dir(work, marker=True)
  sides = []
  for name, b in (("a", bin_a), ("b", bin_b)):
    if not os.access(b, os.X_OK):
      raise RuntimeError(f"binary {b} not found or not executable "
                         "(run `just mori-build`)")
    cache = os.path.join(work, name, "cache")
    os.makedirs(cache, mode=0o700)
    rt = f"{rt_prefix}-{name}"
    fresh_dir(rt, marker=False)
    sides.append(Side(name, os.path.abspath(b), cache, rt))
  return sides


def parse_timeouts(db: str) -> set[str]:
  """Files the index dropped with `parse timeout`. The C parser has a
  5 s CPU budget plus a 60 s wall-clock ceiling: a huge generated file
  (nibiru's statik.go) trips it every run, and a loaded or swapping machine
  trips it on different files each run."""
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


def index_sides(sides: list[Side], root: str, retries: int
                ) -> dict[str, dict[str, Any]]:
  """Index on every side, then reindex from scratch any side whose
  parse-timeout set differs from the others, up to `retries` times."""
  out: dict[str, dict[str, Any]] = {}
  for s in sides:
    log(f"  index side {s.name} ...")
    out[s.name] = run_index(s, root)
    out[s.name]["timeout_retries"] = 0
  by_name = {s.name: s for s in sides}

  def timeouts() -> dict[str, set[str]]:
    return {n: parse_timeouts(os.path.join(by_name[n].cache,
                                           f"{ix['project']}.db"))
            if ix["project"] else set() for n, ix in out.items()}

  sets = timeouts()
  for attempt in range(1, retries + 1):
    redo = timeout_sides(sets)
    if not redo:
      break
    for n in redo:
      extra = sorted(sets[n] - set.intersection(*sets.values()))
      log(f"    side {n}: {len(extra)} files hit the parse timeout only "
          f"there ({', '.join(extra[:3])}); reindexing")
      remove_project_db(by_name[n].cache, out[n]["project"])
      out[n] = run_index(by_name[n], root)
      out[n]["timeout_retries"] = attempt
    sets = timeouts()
  for n, ix in out.items():
    ix["parse_timeouts"] = sorted(sets[n])
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
  daemon can miss its 30 s rendezvous, and a checkout that changed while
  indexing aborts the run."""
  for attempt in range(1, attempts + 1):
    t0 = time.monotonic()
    code, out, err = side.cli("index_repository", {"repo_path": root},
                              INDEX_TIMEOUT_S)
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
    e = allowlist.find(entries, d.layer, d.key, d.group, repo)
    out.append({"repo": repo, "layer": d.layer, "key": d.key,
                "group": d.group, "count": d.count,
                "examples": d.examples,
                "allowed": e.reason if e else None})
  return out


def compare_repo(sides: list[Side], dir_: str, root: str,
                 entries: list[allowlist.Entry], retries: int = 2
                 ) -> RepoResult:
  r = RepoResult(dir_, root)
  before = corpus.fingerprint(root)
  r.index = index_sides(sides, root, retries)
  for n, ix in r.index.items():
    log(f"    side {n}: {ix['status']} in {ix['wall_s']} s, {ix['nodes']} "
        f"nodes, {ix['edges']} edges, parse timeouts "
        f"{ix['parse_timeouts'] or 'none'}")
  if corpus.fingerprint(root) != before:
    r.moved = True
    r.error = "checkout changed during the run; rerun this repo"
    return r
  ia, ib = r.index["a"], r.index["b"]
  if timeout_sides({n: set(ix["parse_timeouts"])
                    for n, ix in r.index.items()}):
    r.error = "sides dropped different files to the parse timeout " \
      "(machine load); rerun on a quieter machine"
    return r
  bad = [s for s, ix in r.index.items()
         if ix["exit"] != 0 or not ix["project"]]
  if bad:
    r.error = "index failed on side " + ", ".join(bad) + ": " + \
      json.dumps({s: r.index[s] for s in bad})[:600]
    return r
  t0 = time.monotonic()
  ea = export_db(os.path.join(sides[0].cache, f"{ia['project']}.db"))
  eb = export_db(os.path.join(sides[1].cache, f"{ib['project']}.db"))
  r.counts = {"nodes": len(ea["nodes"]), "edges": len(ea["edges"])}
  rep = diff_exports(ea, eb)
  del ea, eb
  r.export_s = round(time.monotonic() - t0, 2)
  r.diffs = classify(rep.diffs(), dir_, entries)
  return r


def replay(sides: list[Side], call_list: list[dict[str, Any]],
           roots: dict[str, str], projects: list[dict[str, str]],
           entries: list[allowlist.Entry]
           ) -> tuple[list[dict[str, Any]], dict[str, Any]]:
  """L4: run each call on both sides and diff normalized output."""
  rep_by_repo: dict[str, DiffReport] = {}
  stats = {"calls": 0, "skipped": 0, "differing": 0, "errors_a": 0,
           "wall_s": 0.0}
  norms = [calls_mod.OutputNormalizer(
    roots, projects[i], {"cache": s.cache, "rt": s.rt})
    for i, s in enumerate(sides)]
  t0 = time.monotonic()
  pool = ThreadPoolExecutor(max_workers=len(sides))
  for n, c in enumerate(call_list):
    need = calls_mod.placeholders(c["args"])
    dirs = sorted({d for _, d in need})
    if any(d not in roots for d in dirs):
      stats["skipped"] += 1
      continue
    repo = ",".join(dirs) or "-"
    def one(i: int) -> dict[str, Any]:
      args = calls_mod.resolve(c["args"], roots, projects[i])
      try:
        code, out, err = sides[i].cli(c["tool"], args, CALL_TIMEOUT_S)
      except subprocess.TimeoutExpired:
        code, out, err = -1, "", "timeout"
      return norms[i].output(code, out, err)
    # Both sides run at once: they share nothing but the machine.
    outs = list(pool.map(one, range(len(sides))))
    stats["calls"] += 1
    if outs[0]["exit"] != 0:
      stats["errors_a"] += 1
    rep = rep_by_repo.setdefault(repo, DiffReport())
    where = f"#{n} {c['tool']} {json.dumps(c['args'], sort_keys=True)}"
    group = calls_mod.call_id(c["tool"], c["args"])
    if calls_mod.diff_outputs(rep, c["tool"], group, where[:300], outs[0],
                              outs[1]):
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


def print_summary(results: list[RepoResult], l4: list[dict[str, Any]],
                  l4_stats: dict[str, Any] | None) -> None:
  head = ["repo", "index a (s)", "index b (s)", "nodes", "edges"]
  head += list(LAYERS)
  print("| " + " | ".join(head) + " |")
  print("|" + "---|" * len(head))
  for r in results:
    ia, ib = r.index.get("a", {}), r.index.get("b", {})
    row = [r.dir, str(ia.get("wall_s", "-")), str(ib.get("wall_s", "-")),
           str(r.counts.get("nodes", "-")), str(r.counts.get("edges", "-"))]
    if r.error:
      row += [r.error[:60]] + [""] * (len(LAYERS) - 1)
    else:
      s = layer_summary(r.diffs)
      row += [cell(s[l]) for l in LAYERS]
    print("| " + " | ".join(row) + " |")
  if l4_stats is not None:
    s = layer_summary(l4)["L4"]
    print(f"\nL4: {l4_stats['calls']} calls replayed, {l4_stats['skipped']} "
          f"skipped (repo not in run), {l4_stats['errors_a']} error "
          f"results, {l4_stats['differing']} differing, "
          f"{l4_stats['wall_s']} s: {cell(s)}")
  unlisted = [d for r in results for d in r.diffs if not d["allowed"]]
  unlisted += [d for d in l4 if not d["allowed"]]
  if unlisted:
    print(f"\nUnlisted differences ({len(unlisted)} keys):")
    for d in unlisted[:60]:
      print(f"  [{d['layer']}] {d['repo']} {d['key']} ({d['group']}): "
            f"{d['count']}")
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
    "repos": [{"dir": r.dir, "index": r.index, "moved": r.moved,
               "error": r.error, "counts": r.counts,
               "export_s": r.export_s, "diffs": r.diffs}
              for r in results],
    "l4": {"stats": l4_stats, "diffs": l4},
  }
  path = os.path.join(work, "report.json")
  with open(path, "w", encoding="utf-8") as f:
    json.dump(report, f, indent=1)
  print(f"\nreport: {path}")
  if any(r.error for r in results):
    return 2
  unlisted = any(not d["allowed"] for r in results for d in r.diffs)
  unlisted = unlisted or any(not d["allowed"] for d in l4)
  return 1 if unlisted else 0


def run(repos: list[tuple[str, str]], call_list: list[dict[str, Any]] | None,
        a: argparse.Namespace, meta: dict[str, Any]) -> int:
  entries = allowlist.load(a.allowlist)
  sides = make_sides(a.bin_a, a.bin_b, a.work, a.rt_prefix)
  log(f"side a: {sides[0].bin}\nside b: {sides[1].bin}\nwork: {a.work}")
  results = []
  t0 = time.monotonic()
  for dir_, root in repos:
    log(f"repo {dir_} ({root})")
    results.append(compare_repo(sides, dir_, root, entries,
                                a.timeout_retries))
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
    finally:
      for s, ok in zip(sides, started):
        if ok:
          s.daemon("stop")
  meta = {**meta, "bin_a": sides[0].bin, "bin_b": sides[1].bin,
          "total_s": round(time.monotonic() - t0, 1)}
  return finish(results, l4, l4_stats, a.work, meta)


# ---------------------------------------------------------------------------
# Commands


def cmd_run(a: argparse.Namespace) -> int:
  repos = corpus.load(a.corpus)
  if a.repos:
    want = set(a.repos.split(","))
    unknown = want - {r.dir for r in repos}
    if unknown:
      log(f"unknown corpus repos: {', '.join(sorted(unknown))}")
      return 2
    repos = [r for r in repos if r.dir in want]
  bad = []
  for r in repos:
    chk = corpus.verify(r, a.home, a.allow_dirty)
    if chk.dirty and chk.ok:
      log(f"warning: {r.dir} is dirty (--allow-dirty)")
    if not chk.ok:
      bad.append(f"  {r.dir}: " + "; ".join(chk.problems))
  if bad:
    log("corpus does not match its pins:\n" + "\n".join(bad))
    return 2
  snap = set(a.snapshot.split(",")) if a.snapshot else set()
  if "all" in snap:
    snap = {r.dir for r in repos}
  roots = []
  if snap:
    base = a.work + "-snap"
    fresh_dir(base, marker=True)
  for r in repos:
    if r.dir in snap:
      dest = os.path.join(base, r.dir)
      t0 = time.monotonic()
      n = corpus.snapshot(r, a.home, dest)
      log(f"snapshot {r.dir}: {n} files in "
          f"{time.monotonic() - t0:.1f} s -> {dest}")
      roots.append((r.dir, dest))
    else:
      roots.append((r.dir, r.root(a.home)))
  call_list = None if a.skip_l4 else calls_mod.load_calls(a.calls)
  return run(roots, call_list, a,
             {"mode": "corpus", "allow_dirty": a.allow_dirty,
              "snapshot": sorted(snap)})


def cmd_ci(a: argparse.Namespace) -> int:
  base = os.path.join(a.work + "-fixtures")
  fresh_dir(base, marker=True)
  repos = fixtures.write_all(base)
  call_list = None if a.skip_l4 else calls_mod.load_calls(a.calls)
  return run(repos, call_list, a, {"mode": "fixtures"})


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
        f"with secret-like args, {stats['other']} other tools; wrote "
        f"{len(out)} unique calls ({failed} failed at least once) to "
        f"{a.out}")
  return 0


def cmd_seed(a: argparse.Namespace) -> int:
  diffs = []
  for path in a.reports:
    with open(path, encoding="utf-8") as f:
      rep = json.load(f)
    # A repo whose run was invalid (parse timeouts, moved checkout) measured
    # nothing; leave out its diffs and the calls that touch it.
    bad = {r["dir"] for r in rep["repos"] if r["error"]}
    if bad:
      log(f"{path}: leaving out invalid repos {', '.join(sorted(bad))}")
    diffs += [d for r in rep["repos"] if r["dir"] not in bad
              for d in r["diffs"]]
    if rep["l4"]["stats"] and not bad:
      diffs += rep["l4"]["diffs"]
    elif rep["l4"]["stats"]:
      log(f"{path}: leaving out L4, which saw the invalid repos")
  entries = allowlist.seed_entries(diffs, a.measured)
  with open(a.out, "w", encoding="utf-8") as f:
    for e in entries:
      f.write(json.dumps(e, ensure_ascii=False) + "\n")
  print(f"wrote {len(entries)} entries to {a.out}")
  return 0


def main(argv: list[str]) -> int:
  home = os.path.expanduser("~")
  ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
  sub = ap.add_subparsers(dest="cmd", required=True)
  for name in ("run", "ci"):
    p = sub.add_parser(name)
    p.add_argument("--bin-a", default=DEFAULT_BIN)
    p.add_argument("--bin-b", default=DEFAULT_BIN)
    p.add_argument("--work", default="/tmp/mori-parity" +
                   ("" if name == "run" else "-ci"))
    p.add_argument("--rt-prefix", default="/tmp/mp-rt" +
                   ("" if name == "run" else "-ci"))
    p.add_argument("--allowlist", default=ALLOWLIST)
    p.add_argument("--calls", default=CALLS if name == "run" else CALLS_CI)
    p.add_argument("--skip-l4", action="store_true")
    p.add_argument("--timeout-retries", type=int, default=4,
                   help="reindexes when files hit the parse timeout")
    if name == "run":
      p.add_argument("--corpus", default=corpus.CORPUS_JSON)
      p.add_argument("--home", default=home)
      p.add_argument("--repos", help="comma-separated corpus dirs")
      p.add_argument("--allow-dirty", action="store_true")
      p.add_argument("--snapshot", default="",
                     help="comma-separated dirs (or 'all') to index from a "
                     "frozen copy under <work>-snap; for checkouts that "
                     "other work edits during the run")
  p = sub.add_parser("pin")
  p.add_argument("--manifest", default=os.path.join(
    home, "boku", "dotfiles", "cbm", "repos.json"))
  p.add_argument("--ignore-dir", default=os.path.join(
    home, "boku", "dotfiles", "cbm", "repos"))
  p.add_argument("--home", default=home)
  p.add_argument("--corpus", default=corpus.CORPUS_JSON)
  p = sub.add_parser("calls")
  p.add_argument("--logs", default=os.path.join(home, ".claude",
                                                "projects"))
  p.add_argument("--out", default=CALLS)
  p.add_argument("--home", default=home)
  p.add_argument("--corpus", default=corpus.CORPUS_JSON)
  p = sub.add_parser("seed")
  p.add_argument("reports", nargs="+", help="report.json of measured runs")
  p.add_argument("--out", default=ALLOWLIST)
  p.add_argument("--measured", required=True,
                 help="what was measured, e.g. 'C-vs-C 2026-11-20'")
  a = ap.parse_args(argv)
  try:
    return {"run": cmd_run, "ci": cmd_ci, "pin": cmd_pin,
            "calls": cmd_calls, "seed": cmd_seed}[a.cmd](a)
  except (RuntimeError, OSError, ValueError) as e:
    log(f"error: {e}")
    return 2


if __name__ == "__main__":
  sys.exit(main(sys.argv[1:]))
