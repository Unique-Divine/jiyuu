"""Tests for corpus pinning and verification on temp Git repos."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import corpus  # noqa: E402
from fixtures import GIT_ENV  # noqa: E402


def git(root: str, *args: str) -> None:
  env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": root,
         **GIT_ENV}
  subprocess.run(["git", "-C", root, *args], env=env, check=True,
                 capture_output=True)


def write(path: str, text: str) -> None:
  os.makedirs(os.path.dirname(path), exist_ok=True)
  with open(path, "w") as f:
    f.write(text)


class CorpusTest(unittest.TestCase):
  def setUp(self) -> None:
    tmp = tempfile.TemporaryDirectory()
    self.addCleanup(tmp.cleanup)
    self.home = os.path.join(tmp.name, "home")
    self.out = os.path.join(tmp.name, "parity", "corpus.json")
    os.makedirs(os.path.dirname(self.out))
    repo = os.path.join(self.home, "repo")
    write(os.path.join(repo, "a.py"), "x = 1\n")
    git(repo, "init", "-q", "-b", "main")
    git(repo, "add", "-A")
    git(repo, "commit", "-q", "-m", "one")
    write(os.path.join(repo, ".git", "info", "exclude"), ".cbmignore\n")
    write(os.path.join(repo, ".cbmignore"), "*.json\n")
    self.repo = repo
    self.plain = os.path.join(self.home, "notes")
    write(os.path.join(self.plain, "r.md"), "# R\n")
    os.makedirs(os.path.join(self.plain, ".git"))  # empty, like ~/gh
    self.ignores = os.path.join(tmp.name, "ignores")
    write(os.path.join(self.ignores, "notes.cbmignore"), "tmp/\n")
    write(os.path.join(self.plain, ".cbmignore"), "tmp/\n")
    manifest = os.path.join(tmp.name, "repos.json")
    write(manifest, json.dumps({"repos": [
      {"dir": "repo", "remote": "git@x:repo.git"}, {"dir": "notes"}]}))
    corpus.pin(manifest, self.ignores, self.home, self.out)
    self.repos = {r.dir: r for r in corpus.load(self.out)}

  def verify(self, d: str, allow_dirty: bool = False) -> corpus.Check:
    return corpus.verify(self.repos[d], self.home, allow_dirty)

  def test_pin_shapes(self) -> None:
    with open(self.out) as f:
      doc = json.load(f)
    repo, notes = doc["repos"]
    self.assertEqual(len(repo["commit"]), 40)
    self.assertEqual(repo["cbmignore"], "cbmignore/repo.cbmignore")
    self.assertTrue(notes["tree"].startswith("sha256:"))
    self.assertNotIn("commit", notes)

  def test_clean_checkout_passes(self) -> None:
    self.assertTrue(self.verify("repo").ok)
    self.assertTrue(self.verify("notes").ok)

  def test_dirty_needs_allow_dirty(self) -> None:
    write(os.path.join(self.repo, "a.py"), "x = 2\n")
    self.assertFalse(self.verify("repo").ok)
    chk = self.verify("repo", allow_dirty=True)
    self.assertTrue(chk.ok)
    self.assertTrue(chk.dirty)

  def test_moved_head_always_fails(self) -> None:
    write(os.path.join(self.repo, "b.py"), "y = 1\n")
    git(self.repo, "add", "-A")
    git(self.repo, "commit", "-q", "-m", "two")
    chk = self.verify("repo", allow_dirty=True)
    self.assertFalse(chk.ok)
    self.assertIn("differs from pin", chk.problems[0])

  def test_cbmignore_must_match_copy(self) -> None:
    write(os.path.join(self.repo, ".cbmignore"), "*.yaml\n")
    self.assertFalse(self.verify("repo", allow_dirty=True).ok)

  def test_tree_change_is_dirty(self) -> None:
    write(os.path.join(self.plain, "s.md"), "# S\n")
    self.assertFalse(self.verify("notes").ok)
    self.assertTrue(self.verify("notes", allow_dirty=True).ok)

  def test_fingerprint_tracks_edits(self) -> None:
    write(os.path.join(self.repo, "a.py"), "x = 2\n")
    f1 = corpus.fingerprint(self.repo)
    self.assertEqual(f1, corpus.fingerprint(self.repo))
    write(os.path.join(self.repo, "a.py"), "x = 33\n")
    self.assertNotEqual(f1, corpus.fingerprint(self.repo))

  def test_snapshot_copies_indexable_files(self) -> None:
    write(os.path.join(self.repo, ".gitignore"), "build/\n")
    write(os.path.join(self.repo, "build", "out.bin"), "x")
    write(os.path.join(self.repo, "new.py"), "z = 1\n")
    dest = os.path.join(self.home, "..", "snap", "repo")
    n = corpus.snapshot(self.repos["repo"], self.home, dest)
    got = sorted(os.path.relpath(os.path.join(d, f), dest)
                 for d, _, fs in os.walk(dest) for f in fs)
    self.assertEqual(got, [".cbmignore", ".gitignore", "a.py", "new.py"])
    self.assertEqual(n, 3)
    with open(os.path.join(dest, ".cbmignore")) as f:
      self.assertEqual(f.read(), "*.json\n")
    self.assertEqual(
      os.stat(os.path.join(dest, "a.py")).st_mtime_ns,
      os.stat(os.path.join(self.repo, "a.py")).st_mtime_ns)


if __name__ == "__main__":
  unittest.main()
