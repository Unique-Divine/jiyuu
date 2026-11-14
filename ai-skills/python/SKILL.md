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
- Prefer Astral's [ty](https://docs.astral.sh/ty/) for new type-checking setups
  and [Ruff](https://docs.astral.sh/ruff/) for new linting and formatting setups.
  Keep existing repository tooling unless changing it is part of the task.
- Prefer [pytest](https://docs.pytest.org/en/stable/) with plain assertions and
  fixtures such as `tmp_path` and `monkeypatch`. Run it through the project's uv
  environment. Keep tests near the code they exercise.

For script language selection and standalone-script testing details, use
[ud-scripts](../ud-scripts/SKILL.md). Read that skill when working on scripts;
its fuller guidance stays there.
