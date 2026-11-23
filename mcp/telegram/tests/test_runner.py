import pytest

from telegram_mcp import runner


class _FakeClient:
    def __init__(self, *, authorized: bool):
        self.authorized = authorized
        self.connected = False
        self.started = False

    async def connect(self):
        self.connected = True

    async def is_user_authorized(self):
        return self.authorized

    async def start(self):
        self.started = True


@pytest.mark.asyncio
async def test_connect_authorized_client_uses_existing_session_without_interactive_start():
    client = _FakeClient(authorized=True)

    await runner._connect_authorized_client("default", client)

    assert client.connected is True
    assert client.started is False


@pytest.mark.asyncio
async def test_connect_authorized_client_rejects_unauthorized_session():
    client = _FakeClient(authorized=False)

    with pytest.raises(RuntimeError, match="Interactive phone login is disabled"):
        await runner._connect_authorized_client("default", client)

    assert client.connected is True
    assert client.started is False


@pytest.mark.asyncio
async def test_legacy_saved_authorization_uses_same_broker_lock(monkeypatch, tmp_path):
    import os
    from contextlib import AsyncExitStack
    from types import SimpleNamespace
    from telethon.crypto.authkey import AuthKey
    from telethon.sessions import StringSession
    from telegram_mcp.session_store import State, StoreError, lock_authorization

    monkeypatch.setenv("BOKU_HOME", os.fspath(tmp_path / "boku"))
    session = StringSession()
    session.auth_key = AuthKey(b"f" * 256)
    monkeypatch.setattr(runner, "clients", {"legacy": SimpleNamespace(session=session)})
    async with AsyncExitStack() as stack:
        runner._lock_saved_authorizations(stack, None)
        with pytest.raises(StoreError, match="already in use"):
            with lock_authorization(State.discover(), session):
                pytest.fail("broker acquired the legacy session lock twice")
    with lock_authorization(State.discover(), session):
        pass
