import json
import os
from dataclasses import replace
from types import SimpleNamespace
from typing import cast

import pytest
from telethon import TelegramClient, errors, functions, types

from telegram_mcp.broker import Broker, BrokerError, bridge, frame, listen
from telegram_mcp import fill as fill_module
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
async def test_capacity_snapshot_counts_groups_after_two_thousand_direct_dialogs(
    monkeypatch, tmp_path
):
    class FakeClient:
        async def get_me(self):
            return SimpleNamespace(id=42, bot=False, premium=True)

        async def iter_dialogs(self, *, limit):
            dialogs = [SimpleNamespace(entity=object())] * 2100
            dialogs.extend(
                [
                    SimpleNamespace(entity=types.ChatForbidden(id=90, title="Old basic group")),
                    SimpleNamespace(
                        entity=types.Channel(
                            91, "Left channel", types.ChatPhotoEmpty(), None, left=True
                        )
                    ),
                    SimpleNamespace(
                        entity=types.Channel(92, "Joined channel", types.ChatPhotoEmpty(), None)
                    ),
                ]
            )
            for dialog in dialogs[:limit]:
                yield dialog

    state = fixture_state(monkeypatch, tmp_path)
    transport = PoolTransport(cast(TelegramClient, FakeClient()), state, "42", b"")
    result = await transport.snapshot()
    assert result["groupCount"] == 1
    assert result["telegramPremium"] is True


@pytest.mark.asyncio
async def test_capacity_scan_resumes_after_the_recorded_wait(monkeypatch, tmp_path):
    now = 1_800_000_000_000
    slept = []

    async def sleep(seconds):
        nonlocal now
        slept.append(seconds)
        now += int(seconds * 1000)

    monkeypatch.setattr(fill_module, "now_ms", lambda: now)
    monkeypatch.setattr(fill_module.asyncio, "sleep", sleep)

    class Dialogs:
        calls = 0

        def __aiter__(self):
            return self

        async def __anext__(self):
            self.calls += 1
            if self.calls == 2:
                raise errors.FloodWaitError(request=None, capture=2)
            if self.calls > 3:
                raise StopAsyncIteration
            return SimpleNamespace(
                entity=types.Channel(self.calls, "Group", types.ChatPhotoEmpty(), None)
            )

    class FakeClient:
        async def get_me(self):
            return SimpleNamespace(id=42, bot=False)

        def iter_dialogs(self, *, limit):
            return Dialogs()

    transport = PoolTransport(
        cast(TelegramClient, FakeClient()), fixture_state(monkeypatch, tmp_path), "42", b""
    )
    assert (await transport.snapshot())["groupCount"] == 2
    assert slept == [33.0]


@pytest.mark.asyncio
async def test_lost_create_stays_uncertain_and_never_sends_twice(monkeypatch, tmp_path):
    state = fixture_state(monkeypatch, tmp_path)
    sends = []
    dialog_scans = 0

    class FakeClient:
        async def get_me(self):
            return SimpleNamespace(id=42, bot=False)

        async def iter_dialogs(self, *, limit):
            nonlocal dialog_scans
            dialog_scans += 1
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
        snapshot = {k: request[k] for k in ("v", "id", "accountId")} | {"op": "account.snapshot"}
        assert json.loads(await bridge(state, frame(snapshot)))["result"]["groupCount"] == 0
        assert json.loads(await bridge(state, frame(request)))["result"] == {
            "outcome": "uncertain"
        }
        assert dialog_scans == 1
        assert json.loads(await bridge(state, frame(snapshot)))["result"] == {
            "accountId": "42",
            "groupCount": 0,
            "telegramPremium": False,
            "addsToday": 0,
            "holds": [],
        }
        assert dialog_scans == 2
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


@pytest.mark.asyncio
async def test_configuration_writes_use_verified_room_and_read_back(monkeypatch, tmp_path):
    from telegram_mcp.bot_identity import BotIdentity
    from telegram_mcp.fill import RIGHTS, TITLE

    class RoomClient:
        def __init__(self):
            self.member = False
            self.rights = None
            self.hidden = True
            self.photo = False
            self.operations = []

        async def get_me(self):
            return SimpleNamespace(id=42, bot=False)

        async def get_input_entity(self, selector):
            assert selector == -1000000001234
            return types.InputPeerChannel(1234, 12)

        async def get_entity(self, username):
            assert username == "FixtureBot"
            return types.User(id=55, bot=True, access_hash=56)

        async def upload_file(self, stream, *, file_name):
            assert stream.read() == b"fixture-photo"
            assert file_name == "sai-chat-photo.jpg"
            return types.InputFile(1, 1, file_name, "fixture")

        async def __call__(self, request):
            channel = functions.channels
            if isinstance(request, channel.GetFullChannelRequest):
                room = types.Channel(
                    1234, TITLE, types.ChatPhotoEmpty(), None, creator=True, megagroup=True
                )
                full = SimpleNamespace(
                    about="Chat with the Sai team. Ref: abcdef01",
                    hidden_prehistory=self.hidden,
                    chat_photo=(
                        types.PhotoEmpty(1)
                        if not self.photo
                        else types.Photo(1, 1, b"", None, [], 1)
                    ),
                )
                return SimpleNamespace(chats=[room], full_chat=full)
            if isinstance(request, channel.GetParticipantRequest):
                assert request.participant.user_id == 55
                if not self.member:
                    raise errors.UserNotParticipantError(request)
                return SimpleNamespace(
                    participant=(
                        types.ChannelParticipant(55, None)
                        if self.rights is None
                        else types.ChannelParticipantAdmin(55, 42, None, self.rights)
                    )
                )
            self.operations.append(type(request).__name__)
            if isinstance(request, channel.InviteToChannelRequest):
                assert request.users[0].user_id == 55
                self.member = True
            elif isinstance(request, channel.EditAdminRequest):
                assert request.user_id.user_id == 55
                self.rights = request.admin_rights
            elif isinstance(request, channel.TogglePreHistoryHiddenRequest):
                self.hidden = request.enabled
            elif isinstance(request, channel.EditPhotoRequest):
                self.photo = True
            else:
                raise AssertionError("unexpected request")
            return object()

    state = fixture_state(monkeypatch, tmp_path)
    client = RoomClient()
    owner = PoolTransport(
        cast(TelegramClient, client), state, "42", b"fixture-photo", BotIdentity(55, "FixtureBot")
    )
    action = Action.parse(create_request())
    for index, step in enumerate(("add_bot", "promote_bot", "show_history", "set_photo"), 10):
        result = await owner.execute(
            replace(action, action=str(index), chat="-1000000001234", step=step)
        )
        assert result["outcome"] == "succeeded", (step, result)
    result = await owner.observe(
        replace(action, action="450", chat="-1000000001234", step="verify")
    )
    assert result["outcome"] == "succeeded"
    assert result["botRights"] == dict.fromkeys(RIGHTS, True)
    assert result["photoSet"] is True
    assert client.operations == [
        "InviteToChannelRequest",
        "EditAdminRequest",
        "TogglePreHistoryHiddenRequest",
        "EditPhotoRequest",
    ]
    assert owner.journal.snapshot(now_ms())["addsToday"] == 1
