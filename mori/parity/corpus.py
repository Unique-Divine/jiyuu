"""A pinned parity corpus file: load, pin, verify, fingerprint, fetch.

Each repo is `{dir, remote, commit, cbmignore, source}`.

- `source: "checkout"` (the default) indexes the checkout at `<home>/<dir>`,
  which must sit at `commit` (`git rev-parse HEAD` at pin time). A
  directory that is not a Git checkout carries `tree` instead: a SHA-256
  over every file's relative path, mode, and content.
- `source: "archive"` fetches `commit` from `remote` (or from
  `fetch_from`, a local clone, when given) into a bare clone under the
  fetch cache and indexes an extracted copy, so a moving checkout never
  matters. The public corpus uses this.

`cbmignore` is a path, relative to the corpus file, to the `.cbmignore`
copy used when indexing, or null.
"""

from __future__ import annotations

import hashlib
import json
import os
import io
import shutil
import stat
import subprocess
import tarfile
from dataclasses import dataclass
from typing import Any

SOURCES = ("checkout", "archive")


@dataclass(frozen=True)
class Repo:
  dir: str
  remote: str | None
  commit: str | None
  tree: str | None
  cbmignore: str | None
  source: str = "checkout"
  fetch_from: str | None = None

  def root(self, home: str) -> str:
    return os.path.join(home, self.dir)

  def cbmignore_path(self) -> str | None:
    """Absolute path of the pinned copy (load() resolves it against the
    corpus.json directory)."""
    return self.cbmignore


def load(path: str) -> list[Repo]:
  with open(path, encoding="utf-8") as f:
    d = json.load(f)
  base = os.path.dirname(os.path.abspath(path))
  out = []
  for r in d["repos"]:
    src = r.get("source", "checkout")
    if src not in SOURCES:
      raise ValueError(f"{path}: repo {r['dir']}: unknown source {src}")
    if src == "archive" and not (r.get("remote") and r.get("commit")):
      raise ValueError(f"{path}: repo {r['dir']}: an archive source needs "
                       "remote and commit")
    out.append(Repo(r["dir"], r.get("remote"), r.get("commit"),
                    r.get("tree"),
                    os.path.join(base, r["cbmignore"])
                    if r.get("cbmignore") else None, src,
                    os.path.expanduser(r["fetch_from"])
                    if r.get("fetch_from") else None))
  return out


def git(root: str, *args: str) -> str:
  p = subprocess.run(["git", "-C", root, *args], capture_output=True,
                     text=True, check=False)
  if p.returncode != 0:
    raise RuntimeError(f"git {' '.join(args)} in {root}: "
                       f"{p.stderr.strip()}")
  return p.stdout


def is_git(root: str) -> bool:
  """True when root is the top of a Git work tree. `~/gh` holds an empty
  `.git` directory, so the directory's presence is not enough."""
  if not os.path.exists(os.path.join(root, ".git")):
    return False
  p = subprocess.run(["git", "-C", root, "rev-parse", "--show-toplevel"],
                     capture_output=True, text=True, check=False)
  return p.returncode == 0 and os.path.realpath(p.stdout.strip()) == \
    os.path.realpath(root)


def tree_hash(root: str) -> str:
  """SHA-256 over sorted (relative path, kind, content hash). Symlinks
  hash their target; directories are implied by their files."""
  h = hashlib.sha256()
  entries = []
  for dirpath, dirnames, filenames in os.walk(root):
    dirnames.sort()
    for name in filenames:
      p = os.path.join(dirpath, name)
      rel = os.path.relpath(p, root)
      st = os.lstat(p)
      if stat.S_ISLNK(st.st_mode):
        entries.append((rel, "l", os.readlink(p).encode()))
      elif stat.S_ISREG(st.st_mode):
        fh = hashlib.sha256()
        with open(p, "rb") as f:
          for chunk in iter(lambda: f.read(1 << 20), b""):
            fh.update(chunk)
        kind = "x" if st.st_mode & 0o111 else "f"
        entries.append((rel, kind, fh.hexdigest().encode()))
  for rel, kind, digest in sorted(entries):
    h.update(rel.encode() + b"\0" + kind.encode() + b"\0" + digest + b"\n")
  return "sha256:" + h.hexdigest()


def dirty_lines(root: str) -> list[str]:
  """`git status --porcelain` lines, without the `.cbmignore` copy that
  setup drops into each checkout untracked."""
  out = git(root, "status", "--porcelain=v1", "--untracked-files=normal")
  return [l for l in out.splitlines() if l[3:] != ".cbmignore"]


def fingerprint(root: str) -> str:
  """Cheap identity of a checkout's current content: HEAD, status, and the
  size and mtime of every dirty path, recursing into dirty submodules. Two
  equal fingerprints mean nothing moved between them."""
  if not is_git(root):
    return tree_hash(root)
  h = hashlib.sha256()
  h.update(git(root, "rev-parse", "HEAD").encode())
  for line in dirty_lines(root):
    h.update(line.encode() + b"\n")
    p = os.path.join(root, line[3:].split(" -> ")[-1].strip('"'))
    if os.path.isdir(p) and is_git(p):
      h.update(fingerprint(p).encode())
    elif os.path.lexists(p):
      st = os.lstat(p)
      h.update(f"{st.st_size}:{st.st_mtime_ns}".encode())
  return h.hexdigest()


@dataclass
class Check:
  dir: str
  ok: bool
  dirty: bool
  problems: list[str]


def verify(repo: Repo, home: str, allow_dirty: bool) -> Check:
  """Compare a checkout against its pin. A moved HEAD always fails; dirty
  content fails unless allow_dirty."""
  root = repo.root(home)
  problems: list[str] = []
  dirty = False
  if not os.path.isdir(root):
    return Check(repo.dir, False, False, [f"missing checkout {root}"])
  if repo.commit:
    if not is_git(root):
      return Check(repo.dir, False, False, [f"{root} is not a Git checkout"])
    head = git(root, "rev-parse", "HEAD").strip()
    if head != repo.commit:
      problems.append(f"HEAD {head[:12]} differs from pin "
                      f"{repo.commit[:12]} (run `parity.py pin`)")
    lines = dirty_lines(root)
    if lines:
      dirty = True
      if not allow_dirty:
        problems.append(f"{len(lines)} dirty paths (use --allow-dirty)")
  elif repo.tree:
    now = tree_hash(root)
    if now != repo.tree:
      dirty = True
      if not allow_dirty:
        problems.append(f"tree {now[:19]} differs from pin "
                        f"{repo.tree[:19]} (pin again or --allow-dirty)")
  else:
    problems.append("no commit or tree pin")
  copy = repo.cbmignore_path()
  live = os.path.join(root, ".cbmignore")
  if copy:
    if not os.path.isfile(copy):
      problems.append(f"missing .cbmignore copy {copy}")
    elif not os.path.isfile(live) or not same_file(copy, live):
      problems.append(f"{live} differs from the pinned copy {repo.cbmignore}")
  return Check(repo.dir, not problems, dirty, problems)


def same_file(a: str, b: str) -> bool:
  with open(a, "rb") as fa, open(b, "rb") as fb:
    return fa.read() == fb.read()


def pin(manifest: str, ignore_dir: str | None, home: str,
        out: str) -> dict[str, Any]:
  """Write corpus.json from a repos manifest (dotfiles `cbm/repos.json`):
  current HEAD (or tree hash), and a fresh `.cbmignore` copy for each repo,
  taken from `ignore_dir/<dir>.cbmignore` or else the checkout's own."""
  with open(manifest, encoding="utf-8") as f:
    entries = json.load(f)["repos"]
  copies = os.path.join(os.path.dirname(out), "cbmignore")
  os.makedirs(copies, exist_ok=True)
  repos = []
  for e in entries:
    d = e["dir"]
    root = os.path.join(home, d)
    if not os.path.isdir(root):
      raise RuntimeError(f"pin: missing checkout {root}")
    r: dict[str, Any] = {"dir": d, "remote": e.get("remote")}
    if is_git(root):
      r["commit"] = git(root, "rev-parse", "HEAD").strip()
    else:
      r["tree"] = tree_hash(root)
    src = None
    if ignore_dir and os.path.isfile(os.path.join(ignore_dir,
                                                  f"{d}.cbmignore")):
      src = os.path.join(ignore_dir, f"{d}.cbmignore")
    elif os.path.isfile(os.path.join(root, ".cbmignore")):
      src = os.path.join(root, ".cbmignore")
    if src:
      shutil.copyfile(src, os.path.join(copies, f"{d}.cbmignore"))
      r["cbmignore"] = f"cbmignore/{d}.cbmignore"
    else:
      r["cbmignore"] = None
    repos.append(r)
  doc = {"note": "Pinned by `parity.py pin`; checked before every run.",
         "repos": repos}
  with open(out, "w", encoding="utf-8") as f:
    json.dump(doc, f, indent=2)
    f.write("\n")
  return doc


def submodule_paths(root: str) -> list[str]:
  """Initialized submodules of a checkout, relative to it."""
  if not os.path.isfile(os.path.join(root, ".gitmodules")):
    return []
  out = git(root, "submodule", "status")
  paths = []
  for line in out.splitlines():
    parts = line[1:].split()
    if line[:1] != "-" and len(parts) >= 2:
      paths.append(parts[1])
  return paths


def checkout_files(root: str) -> list[str]:
  """Files an index of the checkout can see: tracked plus untracked files
  that no ignore rule hides, recursing into initialized submodules.
  Nested repos that are not submodules are skipped."""
  out = git(root, "ls-files", "-z", "--cached", "--others",
            "--exclude-standard")
  subs = submodule_paths(root)
  files = []
  for rel in sorted(set(filter(None, out.split("\0")))):
    if rel in subs or rel.rstrip("/") in subs:
      continue
    p = os.path.join(root, rel)
    if os.path.isdir(p) and not os.path.islink(p):
      continue  # an untracked nested repo shows up as a directory
    files.append(rel)
  for sub in subs:
    files.extend(os.path.join(sub, f)
                 for f in checkout_files(os.path.join(root, sub)))
  return files


def snapshot(repo: Repo, home: str, dest: str) -> int:
  """Copy a checkout's indexable content to dest (mtimes kept) with the
  pinned .cbmignore, so a repo that other work keeps editing holds still
  while both sides index it. Returns the file count."""
  root = repo.root(home)
  if is_git(root):
    files = checkout_files(root)
  else:
    files = []
    for dirpath, _, names in os.walk(root):
      files.extend(os.path.relpath(os.path.join(dirpath, n), root)
                   for n in names)
  os.makedirs(dest, mode=0o700, exist_ok=True)
  n = 0
  for rel in files:
    src = os.path.join(root, rel)
    dst = os.path.join(dest, rel)
    if not os.path.lexists(src):
      continue  # deleted from the working tree but still tracked
    os.makedirs(os.path.dirname(dst), exist_ok=True)
    shutil.copy2(src, dst, follow_symlinks=False)
    n += 1
  copy = repo.cbmignore_path()
  if copy:
    shutil.copyfile(copy, os.path.join(dest, ".cbmignore"))
  return n


def fetch(repo: Repo, cache: str) -> str:
  """A bare clone under `cache` holding `repo.commit`, fetched from
  `repo.fetch_from` or `repo.remote` only when missing. Returns the
  clone's path."""
  bare = os.path.join(cache, f"{repo.dir}.git")
  if not os.path.isdir(bare):
    os.makedirs(cache, mode=0o700, exist_ok=True)
    subprocess.run(["git", "init", "-q", "--bare", bare], check=True,
                   capture_output=True)
  have = subprocess.run(
    ["git", "--git-dir", bare, "cat-file", "-e", f"{repo.commit}^{{commit}}"],
    capture_output=True)
  if have.returncode != 0:
    p = subprocess.run(["git", "--git-dir", bare, "fetch", "-q", "--depth",
                        "1", str(repo.fetch_from or repo.remote),
                        str(repo.commit)],
                       capture_output=True, text=True)
    if p.returncode != 0:
      raise RuntimeError(f"fetch {repo.dir} {repo.commit}: "
                         f"{p.stderr.strip()[-300:]}")
  return bare


def materialize(repo: Repo, dest: str, cache: str) -> int:
  """Extract `repo.commit` into dest (plus the pinned .cbmignore, if any)
  and return the file count."""
  bare = fetch(repo, cache)
  p = subprocess.run(["git", "--git-dir", bare, "archive", "--format=tar",
                      str(repo.commit)], capture_output=True, check=True)
  os.makedirs(dest, mode=0o700, exist_ok=True)
  n = 0
  with tarfile.open(fileobj=io.BytesIO(p.stdout)) as tar:
    members = [m for m in tar.getmembers() if m.isfile() or m.issym()
               or m.isdir()]
    tar.extractall(dest, members=members, filter="data")
    n = sum(1 for m in members if not m.isdir())
  if repo.cbmignore:
    shutil.copyfile(repo.cbmignore, os.path.join(dest, ".cbmignore"))
  return n
