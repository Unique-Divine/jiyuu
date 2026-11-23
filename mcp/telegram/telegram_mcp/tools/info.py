"""The ``mcp_info`` tool: what this server is and where it runs from."""

from telegram_mcp import server_info
from telegram_mcp.runtime import *


@mcp.tool(annotations=ToolAnnotations(title="MCP Info", readOnlyHint=True))
async def mcp_info() -> str:
    """Report this server's version, install provenance and update status.

    Call this to learn where the Telegram MCP runs from, which version it is,
    whether a newer release exists, and which accounts it serves. Never read
    MCP client config files for this: they hold secrets. The report holds
    names, paths and short fingerprints only.
    """
    identity = server_info.current_identity()
    env = dict(os.environ)
    # PyPI on every call, so a long-running server never reports a stale answer.
    status_task = asyncio.to_thread(server_info.live_update_check, identity, env)
    if identity.source_root:
        status, git = await asyncio.gather(
            status_task, asyncio.to_thread(server_info.git_state, identity.source_root)
        )
    else:
        status, git = await status_task, None
    report = server_info.build_mcp_info(
        identity,
        status=status,
        git=git,
        clients=clients,
        env=os.environ,
        stored_accounts=STORED_PROFILES,
    )
    return json.dumps(report, indent=2)


__all__ = ["mcp_info"]
