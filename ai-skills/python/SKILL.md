---
name: python
description: Apply Python typing, uv dependency management, Astral tooling, and pytest conventions when writing, editing, or reviewing Python code and project configuration.
---

# Python

Keep Python work consistent with the repository's supported versions and existing
commands. Prefer the nearest relevant Just recipes when available.

- Follow [Typing Best Practices](https://typing.python.org/en/latest/reference/best_practices.html)
  and the [typing modernization guide](https://typing.python.org/en/latest/guides/modernizing.html).
  Prefer `T | None` and built-in generics such as `list[T]` when the declared
  minimum Python version supports them. These are defaults, not a reason to
  raise the project's minimum version or rewrite unrelated code.
- Use [uv](https://docs.astral.sh/uv/guides/projects/) for dependencies,
  environments, and execution. In uv projects, manage dependencies with `uv add`,
  run commands through `uv run`, and honor the lockfile. In CI, install with
  `uv sync --locked`, which fails when `uv.lock` no longer matches
  `pyproject.toml`; `--frozen` skips that check.
- Prefer [pytest](https://docs.pytest.org/en/stable/) with plain assertions and
  fixtures such as `tmp_path` and `monkeypatch`. Run it through the project's uv
  environment. Keep tests near the code they exercise.

## Lint, Format, and Type Check

Use Astral's [Ruff](https://docs.astral.sh/ruff/) for linting and formatting
and [ty](https://docs.astral.sh/ty/) for static type checking when choosing
tools for a Python project. Choose ty over Pyright for a new setup. Follow
the existing checks in repositories that have already chosen their tools.

In a uv project, put Ruff, ty, and pytest in the dev dependency group and
check in the lockfile. Prefer the repository's Just recipes. A typical
`just check` runs these commands, with the type check scoped to the source
paths the project supports.

```sh
uv run ruff check .
uv run ruff format --check .
uv run ty check src
```

Run `uv run ruff format` on edited paths when formatting is needed. Use
`uv run pytest` for tests. In CI, install the locked dev tools and run the
same checks as the local recipes. For a standalone script outside a project,
use `uvx ruff check <file>`, `uvx ruff format --check <file>`, and
`uvx ty check <file>` without creating a project just to run the tools.

For script language selection and standalone-script testing details, use
[ud-scripts](../ud-scripts/SKILL.md). Read that skill when working on scripts;
its fuller guidance stays there.
