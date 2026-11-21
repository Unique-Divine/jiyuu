"""Small generated repos for `parity.py ci`.

Each fixture is a Git repo with one commit, written fresh under a scratch
directory, so CI needs neither the private corpus nor network access. The
files cover calls across files, methods, imports, HTTP and GraphQL routes,
tests, and Markdown that mentions code.
"""

from __future__ import annotations

import os
import subprocess

FIXTURES: dict[str, dict[str, str]] = {
  "fx-polyglot": {
    "README.md": (
      "# Polyglot Fixture\n\n"
      "`load_config` in `app/config.py` feeds `Server.start`.\n"
      "The Go service serves `/api/items`; `web/client.ts` calls it.\n"),
    "app/__init__.py": "",
    "app/config.py": (
      "import json\n\n\n"
      "def load_config(path):\n"
      "    with open(path) as f:\n"
      "        return parse(f.read())\n\n\n"
      "def parse(text):\n"
      "    return json.loads(text)\n"),
    "app/server.py": (
      "from app.config import load_config\n\n\n"
      "class Server:\n"
      "    def __init__(self, path):\n"
      "        self.config = load_config(path)\n\n"
      "    def start(self):\n"
      "        return self.handle('start')\n\n"
      "    def handle(self, event):\n"
      "        return {'event': event, 'config': self.config}\n"),
    "tests/test_server.py": (
      "from app.server import Server\n\n\n"
      "def test_start(tmp_path):\n"
      "    p = tmp_path / 'c.json'\n"
      "    p.write_text('{}')\n"
      "    assert Server(str(p)).start()['event'] == 'start'\n"),
    "svc/main.go": (
      "package main\n\n"
      "import (\n\t\"encoding/json\"\n\t\"net/http\"\n)\n\n"
      "type Item struct {\n\tName string `json:\"name\"`\n}\n\n"
      "func listItems() []Item {\n"
      "\treturn []Item{{Name: \"a\"}, {Name: \"b\"}}\n}\n\n"
      "func itemsHandler(w http.ResponseWriter, r *http.Request) {\n"
      "\tjson.NewEncoder(w).Encode(listItems())\n}\n\n"
      "func main() {\n"
      "\thttp.HandleFunc(\"/api/items\", itemsHandler)\n"
      "\thttp.ListenAndServe(\":8080\", nil)\n}\n"),
    "svc/main_test.go": (
      "package main\n\nimport \"testing\"\n\n"
      "func TestListItems(t *testing.T) {\n"
      "\tif len(listItems()) != 2 {\n\t\tt.Fatal(\"want 2\")\n\t}\n}\n"),
    "gqlgen.yml": (
      "schema:\n  - graph/schema.graphqls\n"
      "resolver:\n  layout: follow-schema\n  dir: graph\n"),
    "graph/schema.graphqls": "type Query { fixtureItem: Int }\n",
    "graph/handlers.go": (
      "package graph\n\n"
      "type queryResolver struct{}\n\n"
      "func (q *queryResolver) FixtureItem() int { return 1 }\n"),
    "web/client.ts": (
      "export interface Item {\n  name: string\n}\n\n"
      "export async function fetchItems(): Promise<Item[]> {\n"
      "  const res = await fetch(\"/api/items\")\n"
      "  return res.json()\n}\n\n"
      "export async function itemNames(): Promise<string[]> {\n"
      "  const items = await fetchItems()\n"
      "  return items.map((i) => i.name)\n}\n"),
  },
  "fx-lib": {
    "lib/math.ts": (
      "export function add(a: number, b: number): number {\n"
      "  return a + b\n}\n\n"
      "export function sum(xs: number[]): number {\n"
      "  return xs.reduce(add, 0)\n}\n"),
    "lib/stats.ts": (
      "import { sum } from \"./math\"\n\n"
      "export function mean(xs: number[]): number {\n"
      "  return xs.length ? sum(xs) / xs.length : 0\n}\n\n"
      "export class Summary {\n"
      "  constructor(private xs: number[]) {}\n"
      "  mean(): number {\n    return mean(this.xs)\n  }\n}\n"),
    "lib/stats.test.ts": (
      "import { mean } from \"./stats\"\n\n"
      "test(\"mean\", () => {\n  expect(mean([1, 2, 3])).toBe(2)\n})\n"),
    "docs/guide.md": (
      "# Guide\n\n## Stats\n\nUse `mean` from `lib/stats.ts`.\n\n"
      "## Math\n\n`sum` folds with `add`.\n"),
  },
}

GIT_ENV = {
  "GIT_AUTHOR_NAME": "parity", "GIT_AUTHOR_EMAIL": "parity@localhost",
  "GIT_COMMITTER_NAME": "parity", "GIT_COMMITTER_EMAIL": "parity@localhost",
  "GIT_AUTHOR_DATE": "2026-01-01T00:00:00Z",
  "GIT_COMMITTER_DATE": "2026-01-01T00:00:00Z",
}


def write_repo(root: str, files: dict[str, str]) -> None:
  for rel, text in files.items():
    p = os.path.join(root, rel)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, "w", encoding="utf-8") as f:
      f.write(text)
  env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"),
         "HOME": root, **GIT_ENV}
  for cmd in (["git", "init", "-q", "-b", "main"], ["git", "add", "-A"],
              ["git", "commit", "-q", "-m", "fixture"]):
    subprocess.run(cmd, cwd=root, env=env, check=True,
                   capture_output=True)


def write_all(base: str) -> list[tuple[str, str]]:
  out = []
  for name, files in FIXTURES.items():
    root = os.path.join(base, name)
    write_repo(root, files)
    out.append((name, root))
  return out
