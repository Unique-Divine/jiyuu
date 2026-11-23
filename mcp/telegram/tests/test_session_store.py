import json
import os
import stat
from types import SimpleNamespace

import pytest
from telethon.crypto.authkey import AuthKey
from telethon.sessions import StringSession

from telegram_mcp import session_store as store


def demo_profile(user_id=42):
    session = StringSession()
    session.set_dc(2, "192.0.2.25", 443)
    session.auth_key = AuthKey(b"f" * 256)
    return json.dumps(
        {"apiId": 123, "apiHash": "a" * 32, "sessionString": session.save(), "userId": user_id}
    ).encode()


def test_private_state_and_duplicate_authorization_lock(monkeypatch, tmp_path):
    monkeypatch.setenv("BOKU_HOME", os.fspath(tmp_path / "boku"))
    state = store.State.discover()
    for path in [state.tool, state.tool / "secrets", state.tool / "logs", state.socket.parent]:
        assert stat.S_IMODE(path.stat().st_mode) == 0o700
    profile = store.Profile.parse(demo_profile())
    with store.lock_authorization(state, profile.session()):
        with pytest.raises(store.StoreError, match="already in use"):
            with store.lock_authorization(state, profile.session()):
                pytest.fail("authorization used twice")
    with store.lock_authorization(state, profile.session()):
        pass


def test_stored_profile_refuses_custody_bots_and_bad_payloads():
    with pytest.raises(store.StoreError, match="Invalid encrypted"):
        store.Profile.parse(demo_profile(store.CUSTODY_ID))
    with pytest.raises(store.StoreError, match="Invalid encrypted"):
        store.Profile.parse(b"bad secret")
    for entry in [SimpleNamespace(id=12, bot=False), SimpleNamespace(id=42, bot=True), None]:
        with pytest.raises(store.StoreError, match="approved human"):
            store.validate_identity(entry, 42)


def test_helper_uses_pipes_and_discards_its_diagnostics(monkeypatch, tmp_path):
    monkeypatch.setenv("BOKU_HOME", os.fspath(tmp_path / "boku"))
    state = store.State.discover()
    seen = []

    def helper(arguments, **options):
        seen.append((arguments, options))
        return SimpleNamespace(stdout=demo_profile())

    monkeypatch.setattr(store.shutil, "which", lambda _: "/private/local-secret-store")
    monkeypatch.setattr(store.subprocess, "run", helper)
    account = store.read_profile(state, "test")
    assert account.user_id == 42
    store._helper(state, "put", "test", demo_profile())
    assert seen[0][0] == [
        "/private/local-secret-store",
        "--store",
        str(state.tool / "secrets"),
        "get",
        "telegram-test",
    ]
    assert seen[1][1]["input"] == demo_profile()
    assert seen[1][1]["env"] == {}
    assert "sessionString" not in " ".join(seen[1][0])


@pytest.mark.asyncio
async def test_import_only_stores_live_verified_account(monkeypatch, tmp_path):
    monkeypatch.setenv("BOKU_HOME", os.fspath(tmp_path / "boku"))
    state = store.State.discover()
    profile = store.Profile.parse(demo_profile())
    events = []
    user = SimpleNamespace(id=99, bot=False)

    class Client:
        def __init__(self, session, api_id, api_hash, **kwargs):
            assert api_id == 123
            assert kwargs == {"request_retries": 0}

        async def connect(self):
            events.append("connect")

        async def get_me(self):
            return user

        async def disconnect(self):
            events.append("disconnect")

    monkeypatch.setattr(store, "TelegramClient", Client)
    monkeypatch.setattr(store, "_helper", lambda *_: events.append("put"))
    with pytest.raises(store.StoreError, match="could not be verified"):
        await store.import_profile(state, "test", profile)
    assert events == ["connect", "disconnect"]
    user.id = 42
    await store.import_profile(state, "test", profile)
    assert events[2:] == ["connect", "disconnect", "put"]
