import json
import os

import pytest

from telegram_mcp.broker import Broker, bridge, frame, listen
from telegram_mcp.session_store import State, StoreError


@pytest.mark.asyncio
async def test_private_socket_routes_only_explicit_account_and_redacts_errors(
    monkeypatch, tmp_path
):
    monkeypatch.setenv("BOKU_HOME", os.fspath(tmp_path / "boku"))
    state = State.discover()

    async def fixture_snapshot():
        return {"accountId": "42", "groupCount": 7}

    async def private_error():
        raise RuntimeError("do not leak this session")

    broker = Broker({"42": fixture_snapshot, "55": private_error})
    request = {"v": 1, "id": "fixture-ID", "op": "account.snapshot", "accountId": "42"}
    async with listen(state, broker):
        assert state.socket.stat().st_mode & 0o077 == 0
        with pytest.raises(StoreError, match="already running"):
            async with listen(state, broker):
                pytest.fail("second server opened the same socket")
        assert json.loads(await bridge(state, frame(request))) == {
            "v": 1,
            "id": "fixture-ID",
            "result": {"accountId": "42", "groupCount": 7},
        }
        assert json.loads(await bridge(state, frame(request | {"accountId": "99"})))["error"] == {
            "code": "account_unavailable"
        }
        failure = await bridge(state, frame(request | {"accountId": "55"}))
        assert json.loads(failure)["error"] == {"code": "broker_unavailable"}
        assert b"session" not in failure
        assert json.loads(await bridge(state, frame(request | {"op": "fill.execute"})))[
            "error"
        ] == {"code": "operation_unavailable"}
    assert not state.socket.exists()
