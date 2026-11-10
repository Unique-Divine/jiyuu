---
name: ud-scripts
description: Choose the language for a repo script, Python or Bun TypeScript with few or no dependencies, with Bash for a few lines of glue, and make scripts testable. Use when writing, converting, testing, or reviewing repo scripts, shell or Bash scripts, CI scripts, release or install scripts, or justfile recipes that outgrow a few lines, or when choosing between Bash, Python, and Bun.
---

# UD Scripts

Write repo scripts in Python or Bun TypeScript. Keep Bash for short glue.
This applies to new scripts and to existing Bash scripts when they next need
real changes. The pages below are preferences and patterns that have worked
well, not a checklist. Fit them to the script.

- To make a script testable (plans, injected runners, behavior and golden
  tests, fake tools on `PATH`), read [testable-scripts.md](testable-scripts.md).
- For Bash you keep or write, read [bash.md](bash.md).

## Pick the Language

Neither Python nor Bun is the default. Choose per script:

- **Bun TypeScript** when the repo already uses Bun or TypeScript, when the
  script shares types or code with the app, or when it parses a lot of JSON.
  Bun's built-ins cover most needs: `fetch`, `Bun.file`, `Bun.spawn`,
  `bun:sqlite`, `Bun.Glob`, and `bun test`. Typecheck with `bun check`.
- **Python** when the script runs where only system tools exist (a VM, a
  GitHub-hosted runner, or a fresh machine). `python3` ships on macOS, most
  Linux distros, and those runners, but not on slim or Alpine container
  images. The standard library covers most needs: `argparse`, `json`,
  `pathlib`, `subprocess`, `sqlite3`, `tomllib`, and `urllib`. Type hints
  need a separate checker (pyright, mypy, or ty).
- **Bash** for a few lines that run commands in order, with no loops over
  data, no JSON, and little branching. Justfile recipes count. Once a script
  needs an `if` with real logic, a data loop, or JSON, Python or Bun is
  usually the better home.

## Habits

- **Few dependencies.** Prefer the standard library or Bun built-ins. A
  small package the repo already uses (an argument parser such as
  `commander`, or `@uniquedivine/bash`) is fine. Ask before adding a new
  one.
- **Prefer argument lists.** `subprocess.run(["gh", "pr", "list"])` and
  `Bun.spawnSync(["gh", "pr", "list"])` start one process.
  `shell=True` and `["bash", "-c", "..."]` start a shell and then the
  command, which doubles the cost and lets a filename with spaces or `;` be
  read as shell syntax. A shell string is fine for a real pipe, a glob, or a
  command sent over `ssh`.
- **Fail loudly.** Check exit codes (`check=True` in Python; test
  `exitCode` in Bun, since `Bun.spawnSync` does not throw) and exit non-zero
  with a message that names the step.
- **Keep tests next to the script** for any non-trivial logic, named with a
  `test` suffix: `foo.test.ts` with `bun test`, or `foo_test.py` with pytest.
  See [Python Tests With pytest](#python-tests-with-pytest).
- **Wrap it in a justfile recipe** when people run it often, and keep the
  logic in the script, not the recipe.

## Python Tests With pytest

Use pytest for Python tests: plain `assert` functions, fixtures such as
`tmp_path` and `monkeypatch`, and readable failure diffs. pytest finds
`foo_test.py` and `test_foo.py` without config. It is a development tool, so
a script that imports only the standard library stays dependency-free when it
runs.

Run it through uv so nothing is installed globally:

- **Standalone script** (no `pyproject.toml`): `uvx pytest -q`. The test
  imports the script from its own directory.
- **uv project**: add pytest to the dev group with
  `uv add --dev 'pytest>=8,<10'` and run `uv run pytest -q`. A `uvx` pytest
  can't import the project's packages.
- **CI**: install uv with `astral-sh/setup-uv`, then run the same command.

To test a script with a hyphen in its name, load it with `importlib` in the
test, or name the script with underscores. Fall back to the standard
library's `unittest` only where nothing can be installed, and then run
`python3 -m unittest discover -p '*_test.py'`, since its default pattern only
finds files that start with `test`.

## Why Not Bash by Default

Measured on one Linux laptop on 2026-11-10 with `hyperfine`:

| Test | Bash | Python | Bun |
| --- | --- | --- | --- |
| Start and exit | 1.2 ms | 14 ms | 3.7 ms |
| Loop of 1 million additions | 1,767 ms | 76 ms | 10 ms |
| 200 commands, started directly | 179 ms | 159 ms | 135 ms |
| 200 commands, through a shell | 390 ms | 300 ms | 366 ms |
| Parse and filter 13 MB of JSON | 576 ms (`jq`) | 276 ms | 96 ms |

Starting an external command costs about the same in all three, so glue
between tools like `gh`, `git`, and `rg` runs at the same speed in any of
them. Bash only wins on startup, by a few milliseconds. Logic inside the
script is far slower in Bash. The bigger gain is maintenance: Python and
Bun have real data structures, exceptions, type checks, and test runners,
while Bash carries on after errors unless every script remembers strict
mode and careful quoting.
