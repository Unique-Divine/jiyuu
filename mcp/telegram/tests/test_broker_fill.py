import json
import os
from dataclasses import replace
from types import SimpleNamespace
from typing import cast

import pytest
from telethon import TelegramClient, errors, functions, types

from telegram_mcp.broker import Broker, BrokerError, bridge, frame, listen
from telegram_mcp.fill import Action, Journal, PoolTransport, now_ms, timestamp
from telegram_mcp.session_store import State


def create_request(*, reconcile=False):
    return {
        "v": 1,
        "id": "fixture",
        "op": "fill.execute",
        "accountId": "42",
        "action": {
            "fillId": "12",
            "actionId": "34",
            "ref": "abcdef01",
            "step": "create",
            "chatId": None,
            "reconcile": reconcile,
            "leaseExpiresAt": timestamp(now_ms() + 120_000),
        },
        "limits": {"cap": 3, "groupStopAt": 100},
    }


def fixture_state(monkeypatch, tmp_path):
    monkeypatch.setenv("BOKU_HOME", os.fspath(tmp_path / "boku"))
    return State.discover()


def test_journal_scopes_cap_and_dispatch_identity_to_the_owner(monkeypatch, tmp_path):
    state = fixture_state(monkeypatch, tmp_path)
    journal = Journal(state, "42")
    other = Journal(state, "55")
    action = replace(Action.parse(create_request()), step="add_bot", chat="-1001234567890")
    assert journal.guard(action, now_ms(), reserve=True) is None
    assert journal.snapshot(now_ms()) == {"addsToday": 1, "holds": []}
    assert other.snapshot(now_ms()) == {"addsToday": 0, "holds": []}
    blocked = journal.guard(replace(action, cap=1), now_ms(), reserve=True)
    assert blocked is not None and blocked["errorCode"] == "daily_cap"
    assert other.guard(action, now_ms(), reserve=True) is None
    assert other.snapshot(now_ms())["addsToday"] == 1


@pytest.mark.asyncio
async def test_lost_create_stays_uncertain_and_never_sends_twice(monkeypatch, tmp_path):
    state = fixture_state(monkeypatch, tmp_path)
    sends = []

    class FakeClient:
        async def get_me(self):
            return SimpleNamespace(id=42, bot=False)

        async def iter_dialogs(self, *, limit):
            if False:
                yield

        async def __call__(self, request):
            assert isinstance(request, functions.channels.CreateChannelRequest)
            sends.append(request)
            raise TimeoutError

    request = create_request()
    client = cast(TelegramClient, FakeClient())
    first = PoolTransport(client, state, "42", b"")
    broker = Broker({"42": first.snapshot}, {"42": first.dispatch})
    async with listen(state, broker):
        assert json.loads(await bridge(state, frame(request)))["result"] == {
            "outcome": "uncertain"
        }
        snapshot = {k: request[k] for k in ("v", "id", "accountId")} | {"op": "account.snapshot"}
        assert json.loads(await bridge(state, frame(snapshot)))["result"] == {
            "accountId": "42",
            "groupCount": 0,
            "addsToday": 0,
            "holds": [],
        }
    restarted = PoolTransport(client, state, "42", b"")
    broker = Broker({"42": restarted.snapshot}, {"42": restarted.dispatch})
    async with listen(state, broker):
        assert json.loads(await bridge(state, frame(request)))["result"] == {
            "outcome": "uncertain"
        }
        assert (
            json.loads(await bridge(state, frame(create_request(reconcile=True))))["result"][
                "errorCode"
            ]
            == "invalid_action_binding"
        )
    assert len(sends) == 1
    assert sends[0].megagroup and not sends[0].broadcast


def test_rejects_unbound_actions_and_secrets():
    request = create_request()
    request["action"]["leaseToken"] = "never-on-this-wire"
    with pytest.raises(BrokerError, match="invalid_request"):
        Action.parse(request)


@pytest.mark.asyncio
async def test_flood_on_readback_after_a_create_always_remains_uncertain(monkeypatch, tmp_path):
    class ResultClient:
        async def get_me(self):
            return SimpleNamespace(id=42, bot=False)

        async def iter_dialogs(self, *, limit):
            if False:
                yield

        async def __call__(self, request):
            if isinstance(request, functions.channels.CreateChannelRequest):
                room = types.Channel(
                    1234, request.title, types.ChatPhotoEmpty(), None, creator=True, megagroup=True
                )
                return SimpleNamespace(chats=[room])
            raise AssertionError("unexpected fixture request")

        async def get_input_entity(self, peer):
            raise errors.FloodWaitError(request=None, capture=17)

    state = fixture_state(monkeypatch, tmp_path)
    account = PoolTransport(cast(TelegramClient, ResultClient()), state, "42", b"")
    assert await account.dispatch(create_request()) == {"outcome": "uncertain"}
    assert account.journal.snapshot(now_ms())["holds"] == []


@pytest.mark.asyncio
async def test_disconnected_peer_is_fenced_after_delayed_preflight(monkeypatch, tmp_path):
    import asyncio

    entered = asyncio.Event()
    released = asyncio.Event()
    seen = []

    class DelayedClient:
        async def get_me(self):
            entered.set()
            await released.wait()
            return SimpleNamespace(id=42, bot=False)

        async def iter_dialogs(self, *, limit):
            if False:
                yield

        async def __call__(self, request):
            seen.append(request)
            return SimpleNamespace(chats=[])

    state = fixture_state(monkeypatch, tmp_path)
    owner = PoolTransport(cast(TelegramClient, DelayedClient()), state, "42", b"")
    broker = Broker({"42": owner.snapshot}, {"42": owner.dispatch})
    async with listen(state, broker):
        _, writer = await asyncio.open_unix_connection(state.socket)
        writer.write(frame(create_request()))
        await writer.drain()
        await asyncio.wait_for(entered.wait(), 1)
        writer.close()
        await writer.wait_closed()
        released.set()
    assert seen == []
    with owner.journal.connect() as db:
        assert db.execute("SELECT count(*) FROM dispatch_v1").fetchone()[0] == 0
