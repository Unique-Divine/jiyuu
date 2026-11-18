"""List every tool this server registers, for docs and site manifests.

Importing ``telegram_mcp.runtime`` needs ``TELEGRAM_API_ID``,
``TELEGRAM_API_HASH`` and a session variable in the environment. Synthetic
values work: listing tools never connects to Telegram.
"""

from mcp.types import Tool


async def list_tools() -> list[Tool]:
    """Return all registered tools, before any exposure-mode pruning."""
    from telegram_mcp import tools  # noqa: F401  (registers the tools)
    from telegram_mcp.runtime import mcp

    return await mcp.list_tools()


__all__ = ["list_tools"]
