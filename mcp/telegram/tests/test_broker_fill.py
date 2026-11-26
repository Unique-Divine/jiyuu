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
            "leaseExpiresAt": timestamp(fill_module.now_ms() + 120_000),
        },
        "limits": {"cap": 3, "groupStopAt": 100},
    }


def fixture_state(monkeypatch, tmp_path):
    monkeypatch.setenv("BOKU_HOME", os.fspath(tmp_path / "boku"))
    return State.discover()


def trial_config(now):
    return {
        "id": "2d0a9c4b-32a4-42c3-9568-cc934bbc77d0",
        "state": "running",
        "generation": 1,
        "cap": 32,
        "activateAtMs": now - 1,
        "expiresAtMs": now + 3 * fill_module.DAY_MS,
        "attemptLimit": 120,
    }


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


def test_trial_records_a_short_premium_wait_and_resumes_at_deadline(monkeypatch, tmp_path):
    now = 1_800_000_000_000
    monkeypatch.setattr(fill_module, "now_ms", lambda: now)
    state = fixture_state(monkeypatch, tmp_path)
    journal = Journal(state, "42")
    trial = trial_config(now)
    journal.configure_trial(trial)
    action = Action.parse(create_request())
    event_id = journal.trial_attempt(action, "channels.createChannel")
    now += 10
    hold = journal.wait(
        "channels.createChannel",
        "create",
        errors.FloodPremiumWaitError(request=None, capture=2),
        origin="managed",
        event_id=event_id,
    )
    assert hold and hold["errorCode"] == "hold"
    deadline = now + 2_000
    assert hold["retryAt"] == timestamp(deadline)
    restarted = Journal(state, "42")
    blocked = restarted.guard(action, deadline - 1, reserve=False)
    assert blocked == hold
    unrelated = replace(action, step="add_bot", chat="-1001234567890")
    assert restarted.guard(unrelated, now, reserve=False) is None
    now = deadline
    assert restarted.guard(action, now, reserve=False) is None
    assert restarted.snapshot(now)["holds"] == []
    events = restarted.trial_events(trial["id"], 0)
    assert len(events) == 1
    assert events[0]["id"] == event_id
    assert events[0]["outcome"] == "flood_premium_wait"
    assert events[0]["waitS"] == 2
    assert events[0]["holdUntilMs"] == deadline
    assert restarted.trial_events(trial["id"], events[0]["sequence"]) == []
    assert restarted.trial_attempt(action, "channels.createChannel") is not None
    assert restarted.trial_events(trial["id"], 0)[-1]["generation"] == trial["generation"]

    assert restarted.guard(unrelated, now, reserve=True) is None
    wait = restarted.wait("channels.inviteToChannel", "invite", errors.FloodWaitError(None, 3))
    assert wait and wait["retryAt"] == timestamp(now + 3_000)
    assert restarted.guard(unrelated, now + 3_000, reserve=False) == fill_module.retry(
        "add_spacing", now + fill_module.ADD_GAP_MS
    )


def test_upgrade_ignores_legacy_day_stop_but_keeps_scoped_holds_and_manual_pause(
    monkeypatch, tmp_path
):
    now = 1_800_000_000_000
    monkeypatch.setattr(fill_module, "now_ms", lambda: now)
    state = fixture_state(monkeypatch, tmp_path)
    journal = Journal(state, "42")
    trial = trial_config(now)
    journal.configure_trial(trial)
    with journal.connect() as db:
        db.execute(
            "CREATE TABLE pool_trial_stop_v1 (account TEXT, trial_id TEXT, generation INTEGER, until_ms INTEGER)"
        )
        db.execute(
            "INSERT INTO pool_trial_stop_v1 VALUES(?,?,?,?)",
            ("42", trial["id"], 1, now + fill_module.DAY_MS),
        )
        db.execute("UPDATE pool_trial_v1 SET next_resume_ms=?", (now + fill_module.DAY_MS,))
        db.execute(
            "INSERT INTO hold_v1 VALUES(?,?,?,?,?,?)",
            ("42", "create", now + 3_000, "flood_wait", "channels.createChannel", 3),
        )
        db.commit()
    restarted = Journal(state, "42")
    action = Action.parse(create_request())
    assert restarted.guard(action, now, reserve=False) == fill_module.retry("hold", now + 3_000)
    now += 3_000
    assert restarted.guard(action, now, reserve=False) is None
    assert restarted.manual_trial_blocked()
    restarted.configure_trial(trial | {"state": "paused"})
    restarted.configure_trial(trial)
    blocked = restarted.guard(action, now, reserve=False)
    assert blocked and blocked["errorCode"] == "trial_not_running"
    restarted.configure_trial(trial | {"state": "ended"})
    assert restarted.guard(action, now, reserve=False) is None
    assert not restarted.manual_trial_blocked()


def test_peer_flood_uses_a_scoped_synthetic_deadline(monkeypatch, tmp_path):
    now = 1_800_000_000_000
    monkeypatch.setattr(fill_module, "now_ms", lambda: now)
    journal = Journal(fixture_state(monkeypatch, tmp_path), "42")
    trial = trial_config(now)
    journal.configure_trial(trial)
    action = Action.parse(create_request())
    wait = journal.wait("channels.createChannel", "create", errors.PeerFloodError(None))
    deadline = now + fill_module.DAY_MS
    assert wait == fill_module.retry("hold", deadline)
    assert journal.trial_events(trial["id"], 0)[0]["holdUntilMs"] == deadline
    assert (
        journal.guard(replace(action, expires=deadline + 1_000), deadline, reserve=False) is None
    )


@pytest.mark.asyncio
async def test_trial_socket_replays_flood_and_correlates_dispatch_history(monkeypatch, tmp_path):
    from uuid import uuid4

    from telegram_mcp import call_history

    class FakeClient:
        async def get_me(self):
            return SimpleNamespace(id=42, bot=False)

        async def iter_dialogs(self, *, limit):
            if False:
                yield

        async def __call__(self, request):
            assert isinstance(request, functions.channels.CreateChannelRequest)
            raise errors.FloodWaitError(request=request, capture=5)

    state = fixture_state(monkeypatch, tmp_path)
    history = call_history.History(state.tool / "logs")
    monkeypatch.setattr(call_history, "_history", history)
    owner = PoolTransport(cast(TelegramClient, FakeClient()), state, "42", b"")
    broker = Broker({"42": owner.broker_snapshot}, {"42": owner.dispatch})
    trial_id = str(uuid4())
    request_id = str(uuid4())
    now = now_ms()
    envelope = {"v": 1, "accountId": "42"}

    async def call(operation, **fields):
        request = envelope | {"id": str(uuid4()), "op": operation} | fields
        return json.loads(await bridge(state, frame(request)))

    async with listen(state, broker):
        assert (await call("account.snapshot"))["error"]["code"] == "inventory_refresh_pending"
        await owner.snapshot()
        assert (
            await call(
                "trial.configure",
                trial={
                    "id": trial_id,
                    "state": "running",
                    "generation": 1,
                    "cap": 32,
                    "activateAtMs": now - 1,
                    "expiresAtMs": now + fill_module.DAY_MS,
                    "attemptLimit": 120,
                },
            )
        )["result"]["configured"]
        create = create_request() | {"id": request_id}
        assert (
            await call("fill.execute", **{key: create[key] for key in ("id", "action", "limits")})
        )["result"]["errorCode"] == "hold"
        result = await call("trial.events", trialId=trial_id, after=0)
        assert result["result"]["events"][0]["outcome"] == "flood_wait"
        assert result["result"]["events"][0]["fillId"] == create["action"]["fillId"]
        assert owner.journal.manual_trial_blocked()
    assert history.flush()
    trace = [
        row
        for row in call_history.read_rows(state.tool / "logs")
        if row.get("requestId") == request_id
    ]
    assert [event["event"] for event in trace] == ["call.start", "call.dispatch", "call.end"]
    assert trace[-1]["fillId"] == create["action"]["fillId"]
    assert trace[-1]["code"] == "hold"


@pytest.mark.asyncio
async def test_required_snapshot_wait_is_replayable(monkeypatch, tmp_path):
    now = 1_800_000_000_000
    monkeypatch.setattr(fill_module, "now_ms", lambda: now)

    class Client:
        identity_reads = 0

        async def get_me(self):
            self.identity_reads += 1
            if self.identity_reads == 1:
                raise errors.FloodPremiumWaitError(
                    request=functions.users.GetUsersRequest([]), capture=2
                )
            return SimpleNamespace(id=42, bot=False)

        async def iter_dialogs(self, *, limit):
            if False:
                yield

    client = Client()
    owner = PoolTransport(
        cast(TelegramClient, client), fixture_state(monkeypatch, tmp_path), "42", b""
    )
    trial = trial_config(now)
    owner.journal.configure_trial(trial)
    await owner.read_inventory()
    with pytest.raises(BrokerError, match="hold"):
        await owner.broker_snapshot()
    now += 1_999
    with pytest.raises(BrokerError, match="hold"):
        await owner.broker_snapshot()
    assert client.identity_reads == 1
    now += 1
    assert (await owner.broker_snapshot())["groupCount"] == 0
    replay = owner.journal.trial_events(trial["id"], 0)
    assert len(replay) == 1
    assert replay[0]["method"] == "users.getUsers"
    assert replay[0]["source"] == "required_read"
    assert replay[0]["outcome"] == "flood_premium_wait"
    assert replay[0]["holdUntilMs"] == now


@pytest.mark.asyncio
async def test_participant_wait_only_defers_steps_that_read_it(monkeypatch, tmp_path):
    now = 1_800_000_000_000
    monkeypatch.setattr(fill_module, "now_ms", lambda: now)

    class Client:
        async def get_me(self):
            return SimpleNamespace(id=42, bot=False)

        async def iter_dialogs(self, *, limit):
            if False:
                yield

    owner = PoolTransport(
        cast(TelegramClient, Client()), fixture_state(monkeypatch, tmp_path), "42", b""
    )
    owner.journal.configure_trial(trial_config(now))
    hold = owner.journal.wait(
        "channels.getParticipant", "method:channels.getParticipant", errors.FloodWaitError(None, 2)
    )
    create = Action.parse(create_request(reconcile=True))
    add = replace(create, step="add_bot", chat="-1001234567890")
    assert await owner.observe(add) == hold == fill_module.retry("hold", now + 2_000)
    assert (await owner.snapshot())["groupCount"] == 0
    assert await owner.observe(create) == {"outcome": "not_found"}


@pytest.mark.asyncio
async def test_interrupted_inventory_hold_is_not_an_observation_failure(monkeypatch, tmp_path):
    now = 1_800_000_000_000
    monkeypatch.setattr(fill_module, "now_ms", lambda: now)
    monkeypatch.setattr(fill_module, "CAPACITY_MAX_WAIT_SECONDS", 0)

    class Client:
        scans = 0

        async def get_me(self):
            return SimpleNamespace(id=42, bot=False)

        async def iter_dialogs(self, *, limit):
            self.scans += 1
            if self.scans == 1:
                raise errors.FloodPremiumWaitError(None, 2)
            if False:
                yield

    owner = PoolTransport(
        cast(TelegramClient, Client()), fixture_state(monkeypatch, tmp_path), "42", b""
    )
    owner.journal.configure_trial(trial_config(now))
    with pytest.raises(BrokerError, match="account_held"):
        await owner.read_inventory()
    action = Action.parse(create_request(reconcile=True))
    assert await owner.observe(action) == fill_module.retry("hold", now + 2_000)
    now += 2_000
    assert (await owner.observe(action))["errorCode"] == "inventory_refresh_pending"
    await owner.read_inventory()
    assert await owner.observe(action) == {"outcome": "not_found"}


@pytest.mark.asyncio
async def test_capacity_snapshot_counts_groups_after_two_thousand_direct_dialogs(
    monkeypatch, tmp_path
):
    scans = 0

    class FakeClient:
        async def get_me(self):
            return SimpleNamespace(id=42, bot=False, premium=True)

        async def iter_dialogs(self, *, limit):
            nonlocal scans
            scans += 1
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
                        entity=types.Channel(
                            92,
                            fill_module.TITLE,
                            types.ChatPhotoEmpty(),
                            None,
                            creator=True,
                            megagroup=True,
                        )
                    ),
                ]
            )
            for dialog in dialogs[:limit]:
                yield dialog

        async def get_input_entity(self, entity):
            return types.InputPeerChannel(entity.id, 1)

        async def __call__(self, request):
            assert isinstance(request, functions.channels.GetFullChannelRequest)
            return SimpleNamespace(
                chats=[
                    types.Channel(
                        92,
                        fill_module.TITLE,
                        types.ChatPhotoEmpty(),
                        None,
                        creator=True,
                        megagroup=True,
                    )
                ],
                full_chat=SimpleNamespace(about="Chat with the Sai team. Ref: abcdef01"),
            )

    state = fixture_state(monkeypatch, tmp_path)
    transport = PoolTransport(cast(TelegramClient, FakeClient()), state, "42", b"")
    result = await transport.snapshot()
    assert result["groupCount"] == 1
    assert result["telegramPremium"] is True
    assert await transport.observe(Action.parse(create_request(reconcile=True))) == {
        "outcome": "succeeded",
        "chatId": "-1000000000092",
    }
    assert scans == 1


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
    assert slept == [32.0]


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
        await restarted.snapshot()
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
    await account.snapshot()
    result = await account.dispatch(create_request())
    assert result["outcome"] == "uncertain"
    assert result["errorCode"] == "hold"
    assert account.journal.snapshot(now_ms())["holds"][0]["source"] == "flood_wait"


@pytest.mark.asyncio
async def test_cold_create_defers_before_dispatch_and_misses_use_distinct_scans(
    monkeypatch, tmp_path
):
    import asyncio

    release = asyncio.Event()
    scans = 0

    class FakeClient:
        async def get_me(self):
            return SimpleNamespace(id=42, bot=False)

        async def iter_dialogs(self, *, limit):
            nonlocal scans
            scans += 1
            await release.wait()
            if False:
                yield

    owner = PoolTransport(
        cast(TelegramClient, FakeClient()), fixture_state(monkeypatch, tmp_path), "42", b""
    )
    result = await asyncio.wait_for(owner.dispatch(create_request()), 1)
    assert result["outcome"] == "retry"
    assert result["errorCode"] == "inventory_refresh_pending"
    with owner.journal.connect() as db:
        assert db.execute("SELECT count(*) FROM dispatch_v1").fetchone()[0] == 0
    release.set()
    await owner.snapshot()
    action = Action.parse(create_request(reconcile=True))
    assert await owner.observe(action) == {"outcome": "not_found"}
    assert (await owner.observe(action))["errorCode"] == "inventory_refresh_pending"
    await owner.snapshot()
    assert await owner.observe(action) == {"outcome": "not_found"}
    assert scans == 2


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
