import asyncio
import os

import httpx
from mcp.server.fastmcp import FastMCP
import pytest

from telegram_mcp import runner
from telegram_mcp.http_service import serve_mcp
from telegram_mcp.session_store import State


@pytest.mark.asyncio
async def test_native_mcp_sessions_reconnect_without_stopping_the_shared_service(
    monkeypatch, tmp_path
):
    monkeypatch.setenv("BOKU_HOME", os.fspath(tmp_path / "boku"))
    state = State.discover()
    path = state.tool / "run" / "mcp.sock"
    app = FastMCP("fixture", json_response=True)
    count = 0

    @app.tool()
    def counter() -> int:
        nonlocal count
        count += 1
        return count

    stop = asyncio.Event()
    service = asyncio.create_task(serve_mcp(state, app, stop=stop))
    try:
        for _ in range(100):
            if path.exists():
                break
            await asyncio.sleep(0.01)
        assert path.stat().st_mode & 0o077 == 0

        async with httpx.AsyncClient(
            transport=httpx.AsyncHTTPTransport(uds=os.fspath(path)),
            base_url="http://localhost",
            headers={"accept": "application/json, text/event-stream"},
        ) as client:
            for value in (1, 2):

                async def send(method, params, request_id, headers=None):
                    request = {"jsonrpc": "2.0", "method": method, "params": params}
                    if request_id is not None:
                        request["id"] = request_id
                    response = await client.post("/mcp", json=request, headers=headers)
                    response.raise_for_status()
                    return response

                initialized = await send(
                    "initialize",
                    {
                        "protocolVersion": "2025-03-26",
                        "capabilities": {},
                        "clientInfo": {"name": "fixture", "version": "1"},
                    },
                    1,
                )
                session = initialized.headers["mcp-session-id"]
                headers = {
                    "mcp-session-id": session,
                    "mcp-protocol-version": initialized.json()["result"]["protocolVersion"],
                }
                await send("notifications/initialized", {}, None, headers)
                listed = await send("tools/list", {}, 2, headers)
                assert [tool["name"] for tool in listed.json()["result"]["tools"]] == ["counter"]
                result = await send("tools/call", {"name": "counter", "arguments": {}}, 3, headers)
                assert result.json()["result"]["structuredContent"] == {"result": value}
                done = await client.delete("/mcp", headers=headers)
                done.raise_for_status()
                assert not service.done()
    finally:
        stop.set()
        await asyncio.wait_for(service, 5)
    assert not path.exists()


def test_shared_mcp_keeps_non_quota_tools_and_gates_member_adds():
    app = FastMCP("fixture")

    def tool() -> None:
        pass

    for name in (
        "send_message",
        "promote_admin",
        "wait_for_new_message",
        "export_chat_invite",
        "create_group",
        "create_channel",
        "invite_to_group",
        "join_chat_by_link",
        "import_chat_invite",
    ):
        app.add_tool(tool, name=name)
    runner._configure_broker_tools(app)
    assert {tool.name for tool in app._tool_manager.list_tools()} == {
        "send_message",
        "promote_admin",
        "wait_for_new_message",
        "export_chat_invite",
        "invite_to_group",
    }


@pytest.mark.asyncio
async def test_existing_admin_changes_cannot_add_nonmembers(monkeypatch):
    from types import SimpleNamespace
    from telethon import types
    from telegram_mcp.tools.groups import _broker_admin_target_is_member

    calls = []
    peer = types.InputPeerUser(22, 0)
    channel = types.InputChannel(99, 0)

    async def permissions(request):
        calls.append(request)
        return SimpleNamespace(participant=types.ChannelParticipantLeft(peer=types.PeerUser(22)))

    assert await _broker_admin_target_is_member(permissions, channel, peer)
    assert calls == []
    monkeypatch.setenv("TELEGRAM_BROKER", "1")
    assert not await _broker_admin_target_is_member(permissions, channel, peer)
    assert len(calls) == 1
