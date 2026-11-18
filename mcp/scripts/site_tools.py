"""Print a Python MCP server's tool list as JSON for site-manifest.ts.

Usage: python -I site_tools.py IMPORT_ROOT MODULE:FUNCTION

FUNCTION is an async function that returns MCP ``Tool`` models or plain
dicts. The caller runs this with a scrubbed environment and synthetic
credentials. ``dotenv.load_dotenv`` becomes a no-op before any server module
loads, so no ``.env`` file is read.
"""

from __future__ import annotations

import asyncio
import contextlib
import importlib
import json
import sys
from typing import Any


def disable_dotenv() -> None:
    """Make ``load_dotenv`` a no-op for modules imported after this call."""
    try:
        import dotenv
    except ImportError:
        return

    def _no_dotenv(*_args: Any, **_kwargs: Any) -> bool:
        return False

    setattr(dotenv, "load_dotenv", _no_dotenv)


def as_dict(tool: Any) -> dict[str, Any]:
    if isinstance(tool, dict):
        return tool
    return tool.model_dump(mode="json", by_alias=True, exclude_none=True)


def list_tools(import_root: str, spec: str) -> list[dict[str, Any]]:
    module_name, sep, function_name = spec.partition(":")
    if not sep or not module_name or not function_name:
        raise SystemExit(f"site_tools: expected MODULE:FUNCTION, got {spec!r}")
    sys.path.insert(0, import_root)
    disable_dotenv()
    function = getattr(importlib.import_module(module_name), function_name)
    return [as_dict(tool) for tool in asyncio.run(function())]


def main(argv: list[str]) -> int:
    if len(argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2
    # Server modules may print at import; keep stdout for the JSON alone.
    with contextlib.redirect_stdout(sys.stderr):
        tools = list_tools(argv[1], argv[2])
    json.dump(tools, sys.stdout)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
