---
name: ud-scripts
description: Choose the language for a repo script, zero-dependency Python or Bun TypeScript, with Bash only for a few lines of glue. Use when writing, converting, or reviewing repo scripts, shell or Bash scripts, CI scripts, or justfile recipes that outgrow a few lines, or when choosing between Bash, Python, and Bun.
---

# UD Scripts

Write repo scripts in Python or Bun TypeScript, with no third-party
dependencies. Keep Bash for short glue. This applies to new scripts and to
existing Bash scripts when they next need real changes.

## Pick the Language

Neither Python nor Bun is the default. Choose per script:

- **Bun TypeScript** when the repo already uses Bun or TypeScript, when the
  script shares types or code with the app, or when it parses a lot of JSON.
  Bun's built-ins cover most needs: `fetch`, `Bun.file`, `Bun.spawn`,
  `bun:sqlite`, `Bun.Glob`, and `bun test`. Typecheck with `bun check`.
- **Python** when the script runs where only system tools exist (a VM, a
  GitHub-hosted runner, or a fresh machine). `python3` ships on macOS, most
  Linux distros, and those runners, but not on slim or Alpine container
  images. The
  standard library covers most needs: `argparse`, `json`, `pathlib`,
  `subprocess`, `sqlite3`, `urllib`, and `unittest`. Type hints need a
  separate checker (pyright or mypy).
- **Bash** for a few lines that run commands in order, with no loops over
  data, no JSON, and little branching. Justfile recipes count. Once a script
  needs an `if` with real logic, a data loop, or JSON, move it to Python or
  Bun. For Bash you keep or write, read [bash.md](bash.md).

## Rules

- **No dependencies.** Use the standard library or Bun built-ins. Ask before
  adding a pip or npm package to a script.
- **Run commands as argument lists.** `subprocess.run(["gh", "pr", "list"])`
  and `Bun.spawnSync(["gh", "pr", "list"])` start one process.
  `shell=True` and `["bash", "-c", "..."]` start a shell and then the
  command, which doubles the cost and lets a filename with spaces or `;` be
  read as shell syntax. Use a shell only for a real pipe or glob.
- **Fail loudly.** Check exit codes (`check=True` in Python; test
  `exitCode` in Bun) and exit non-zero with a message that names the step.
- **Put tests next to the script** (`foo.test.ts` or `test_foo.py`) for any
  non-trivial logic. Keep parsing and decisions in pure functions so tests
  need no live tools.
- **Dry-run by default** for scripts that change remote state; apply with
  `--run`.

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
while Bash quietly carries on after errors unless every script remembers
strict mode and careful quoting.
