"""Runtime wiring raises flood waits, keeps the connection, and shares cooldowns."""

import asyncio
import json
from types import SimpleNamespace

import pytest
import telethon.client.users as telethon_users
from telethon import errors, functions, types
from telethon.sessions import StringSession
from telethon.tl.types import PeerChannel

from telegram_mcp import request_policy, runtime
from telegram_mcp.tools import chats, groups, messages


def flood(seconds):
    return errors.FloodWaitError(request=None, capture=seconds)


def test_clients_absorb_short_waits_by_default(monkeypatch):
    created = []

    def fake_client(*args, **kwargs):
        created.append(kwargs)
        return SimpleNamespace()

    monkeypatch.setattr(runtime, "PolicyTelegramClient", fake_client)
    monkeypatch.setattr(runtime, "_build_proxy_for_label", lambda label: (None, None))

    runtime._build_client("session", "default")

    assert created[0]["flood_sleep_threshold"] == request_policy.CLIENT_FLOOD_SLEEP_THRESHOLD
    assert 0 < request_policy.CLIENT_FLOOD_SLEEP_THRESHOLD <= 10


class _Sender:
    """Stands in for Telethon's MTProto sender, failing with queued errors."""

    def __init__(self, *failures):
        self.failures = list(failures)

    def send(self, request, ordered=False):
        async def reply():
            if self.failures:
                raise self.failures.pop(0)
            return types.NearestDc(country="US", this_dc=1, nearest_dc=1)

        return reply()


@pytest.fixture
def telethon_sleeps(monkeypatch):
    slept = []

    async def sleep(seconds):
        slept.append(seconds)

    monkeypatch.setattr(telethon_users, "asyncio", SimpleNamespace(sleep=sleep))
    return slept


@pytest.mark.asyncio
async def test_unbounded_requests_sleep_through_short_waits(telethon_sleeps):
    client = request_policy.PolicyTelegramClient(
        StringSession(), 12345, "hash", flood_sleep_threshold=10
    )

    # A mid-iteration page or transfer chunk that waits 3 s still succeeds.
    result = await client._call(_Sender(flood(3)), functions.help.GetNearestDcRequest())

    assert isinstance(result, types.NearestDc)
    assert telethon_sleeps == [3]


@pytest.mark.asyncio
async def test_bounded_requests_raise_waits_in_their_task_only(telethon_sleeps):
    client = request_policy.PolicyTelegramClient(
        StringSession(), 12345, "hash", flood_sleep_threshold=10
    )
    seen = {}

    async def concurrent_tool():
        seen["threshold"] = client.flood_sleep_threshold

    async def bounded():
        task = asyncio.get_running_loop().create_task(concurrent_tool())
        with request_policy.raise_flood_waits():
            seen["inside"] = client.flood_sleep_threshold
            await task
            return await client._call(_Sender(flood(3)), functions.help.GetNearestDcRequest())

    with pytest.raises(errors.FloodWaitError):
        await bounded()

    assert seen == {"inside": 0, "threshold": 10}
    assert client.flood_sleep_threshold == 10
    assert telethon_sleeps == []


@pytest.mark.asyncio
async def test_get_chats_timeout_cannot_cancel_a_short_wait(monkeypatch, telethon_sleeps):
    client = request_policy.PolicyTelegramClient(
        StringSession(), 12345, "hash", flood_sleep_threshold=10
    )
    sender = _Sender(flood(3))

    async def get_dialogs(**kwargs):
        return await client._call(sender, functions.help.GetNearestDcRequest())

    async def connected(cl):
        pass

    monkeypatch.setattr(client, "get_dialogs", get_dialogs)
    monkeypatch.setattr(chats, "get_client", lambda account=None: client)
    monkeypatch.setattr(chats, "ensure_connected", connected)

    result = json.loads(await chats.get_chats())["error"]

    assert result["type"] == "flood_wait"
    assert result["wait_seconds"] == 3
    assert telethon_sleeps == []


@pytest.mark.asyncio
async def test_wait_on_connection_ping_neither_reconnects_nor_fails(monkeypatch):
    class Client:
        def __init__(self):
            self.calls = []

        def is_connected(self):
            return True

        async def __call__(self, request):
            self.calls.append("ping")
            raise flood(30)

        async def disconnect(self):
            self.calls.append("disconnect")

        async def connect(self):
            self.calls.append("connect")

    client = Client()
    monkeypatch.setattr(runtime, "_last_conn_verified", {})

    await runtime.ensure_connected(client)
    await runtime.ensure_connected(client)

    # The wait proved the connection works, so the next call skips the ping.
    assert client.calls == ["ping"]
    assert id(client) in runtime._last_conn_verified


def test_unrecorded_tool_wait_names_method_and_does_not_promise_refusal():
    request = functions.messages.GetHistoryRequest(
        peer=types.InputPeerEmpty(),
        offset_id=0,
        offset_date=None,
        add_offset=0,
        limit=1,
        max_id=0,
        min_id=0,
        hash=0,
    )
    error = errors.FloodWaitError(request=request, capture=90)
    result = json.loads(runtime.log_and_format_error("get_history", error))["error"]

    assert result["type"] == "flood_wait"
    assert result["request"] == "get_history"
    assert result["method"] == "GetHistoryRequest"
    assert result["wait_seconds"] == 90
    assert result["recorded"] is False
    assert result["retry_at"].endswith("Z")
    assert "refuses" not in result["message"]
    assert "check state before retrying a write" in result["message"]


def test_unrecorded_slow_mode_names_the_tool_chat():
    error = errors.SlowModeWaitError(request=None, capture=20)
    result = json.loads(runtime.log_and_format_error("send_file", error, chat_id=-42))["error"]

    assert result["type"] == "slow_mode"
    assert result["chat_id"] == -42
    assert "chat -42" in result["message"]

    without_chat = json.loads(runtime.log_and_format_error("forward_message", error))["error"]
    assert "None" not in without_chat["message"]
    assert "this chat" in without_chat["message"]


@pytest.mark.asyncio
async def test_cache_warming_flood_wait_becomes_get_chats_cooldown(monkeypatch):
    class Client:
        def __init__(self):
            self.dialog_reads = 0

        async def get_dialogs(self, **kwargs):
            self.dialog_reads += 1
            raise flood(300)

    async def connected(client):
        raise AssertionError("A cooldown must refuse before connection checks")

    client = Client()
    with pytest.raises(runtime.RequestLimited):
        await runtime.warm_dialogs(client)

    monkeypatch.setattr(chats, "get_client", lambda account=None: client)
    monkeypatch.setattr(chats, "ensure_connected", connected)
    result = json.loads(await chats.get_chats())["error"]

    assert result["source"] == "cooldown"
    assert result["request"] == "get_dialogs"
    assert client.dialog_reads == 1


@pytest.mark.asyncio
async def test_send_message_slow_mode_blocks_only_that_chat(monkeypatch):
    slow_chat = PeerChannel(100)
    other_chat = PeerChannel(200)

    class Client:
        def __init__(self):
            self.sent = []

        async def send_message(self, entity, message, parse_mode=None):
            if entity == slow_chat:
                raise errors.SlowModeWaitError(request=None, capture=60)
            self.sent.append((entity.channel_id, message))

    async def resolve(chat_id, client):
        return slow_chat if chat_id == -1000000000100 else other_chat

    client = Client()
    monkeypatch.setattr(messages, "get_client", lambda account=None: client)
    monkeypatch.setattr(messages, "resolve_entity", resolve)

    first = json.loads(await messages.send_message(chat_id=-1000000000100, message="a"))
    again = json.loads(await messages.send_message(chat_id=-1000000000100, message="b"))
    elsewhere = await messages.send_message(chat_id=-1000000000200, message="c")

    assert first["error"]["type"] == "slow_mode"
    assert first["error"]["chat_id"] == -1000000000100
    assert first["error"]["wait_seconds"] == 60
    assert again["error"]["source"] == "cooldown"
    assert elsewhere == "Message sent successfully."
    assert client.sent == [(200, "c")]


class _LookupClient:
    """Resolves only the identifiers it knows; dialogs are under a wait."""

    def __init__(self, known):
        self.known = known
        self.dialog_reads = 0

    def is_connected(self):
        return True

    async def get_dialogs(self, **kwargs):
        self.dialog_reads += 1
        raise flood(300)

    async def get_entity(self, identifier):
        if identifier in self.known:
            return self.known[identifier]
        raise ValueError(f"unknown {identifier}")

    async def get_input_entity(self, identifier):
        return await self.get_entity(identifier)


@pytest.mark.asyncio
@pytest.mark.parametrize("resolver", ["resolve_entity", "resolve_input_entity"])
async def test_dialog_cooldown_still_tries_marked_ids(monkeypatch, resolver):
    channel = SimpleNamespace(id=100)
    client = _LookupClient({-1000000000100: channel})
    monkeypatch.setattr(runtime, "_last_conn_verified", {id(client): runtime.time.time()})

    assert await getattr(runtime, resolver)(100, client) is channel
    assert client.dialog_reads == 1

    # With a cooldown active, nothing resolves, so the limit explains why.
    with pytest.raises(runtime.RequestLimited) as caught:
        await getattr(runtime, resolver)(555, client)
    assert caught.value.limit.source == "cooldown"
    assert client.dialog_reads == 1


@pytest.mark.asyncio
async def test_create_group_reports_the_new_chat_without_reading_dialogs(monkeypatch):
    created = types.Chat(
        id=4321,
        title="Ops",
        photo=types.ChatPhotoEmpty(),
        participants_count=2,
        date=None,
        version=1,
    )

    class Client:
        async def __call__(self, request):
            assert isinstance(request, functions.messages.CreateChatRequest)
            updates = types.Updates(updates=[], users=[], chats=[created], date=None, seq=0)
            return types.messages.InvitedUsers(updates=updates, missing_invitees=[])

        async def get_dialogs(self, **kwargs):
            raise flood(120)

    async def resolve(user_id, client):
        return SimpleNamespace(id=user_id)

    monkeypatch.setattr(groups, "get_client", lambda account=None: Client())
    monkeypatch.setattr(groups, "resolve_entity", resolve)

    result = await groups.create_group(title="Ops", user_ids=[7])

    assert result == "Group created with ID: -4321"


@pytest.mark.asyncio
async def test_create_group_without_chat_in_reply_still_reports_success(monkeypatch):
    class Client:
        async def __call__(self, request):
            updates = types.Updates(updates=[], users=[], chats=[], date=None, seq=0)
            return types.messages.InvitedUsers(updates=updates, missing_invitees=[])

        async def get_dialogs(self, **kwargs):
            raise flood(120)

    async def resolve(user_id, client):
        return SimpleNamespace(id=user_id)

    monkeypatch.setattr(groups, "get_client", lambda account=None: Client())
    monkeypatch.setattr(groups, "resolve_entity", resolve)

    result = await groups.create_group(title="Ops", user_ids=[7])

    assert result.startswith("Group created, but")
    assert "before creating it again" in result
    assert "wait" not in result.lower()


@pytest.mark.asyncio
async def test_send_message_refuses_known_slow_mode_before_resolving(monkeypatch):
    client = object()
    request_policy.request_cooldowns.record(
        id(client),
        "send_message",
        errors.SlowModeWaitError(request=None, capture=60),
        chat_id=-1000000000100,
    )

    async def resolve(chat_id, cl):
        raise AssertionError("A known cooldown must refuse before network lookups")

    monkeypatch.setattr(messages, "get_client", lambda account=None: client)
    monkeypatch.setattr(messages, "resolve_entity", resolve)

    result = json.loads(await messages.send_message(chat_id=-1000000000100, message="a"))

    assert result["error"]["type"] == "slow_mode"
    assert result["error"]["source"] == "cooldown"


def _policy_client_with_sender(monkeypatch, sender):
    client = request_policy.PolicyTelegramClient(
        StringSession(), 12345, "hash", flood_sleep_threshold=10
    )
    client._sender = sender
    monkeypatch.setattr(client, "is_connected", lambda: True)
    reconnects = []

    async def force_reconnect(cl):
        reconnects.append(cl)

    monkeypatch.setattr(runtime, "_force_reconnect", force_reconnect)
    return client, reconnects


@pytest.mark.asyncio
async def test_short_wait_on_ping_raises_instead_of_sleeping_into_a_reconnect(
    monkeypatch, telethon_sleeps
):
    # 7 s is under the 10 s client threshold and over the 5 s ping bound.
    client, reconnects = _policy_client_with_sender(monkeypatch, _Sender(flood(7)))
    monkeypatch.setattr(runtime, "_last_conn_verified", {})

    await runtime.ensure_connected(client)

    assert telethon_sleeps == []
    assert reconnects == []
    assert id(client) in runtime._last_conn_verified


@pytest.mark.asyncio
async def test_short_wait_on_profile_read_reaches_caller_without_reconnect(
    monkeypatch, telethon_sleeps
):
    client, reconnects = _policy_client_with_sender(monkeypatch, _Sender(flood(7)))
    monkeypatch.setattr(runtime, "_last_conn_verified", {id(client): runtime.time.time()})

    with pytest.raises(errors.FloodWaitError):
        await runtime.read_account_profile(client)

    assert telethon_sleeps == []
    assert reconnects == []


@pytest.mark.asyncio
async def test_connect_inside_bounded_request_starts_tasks_with_default_threshold(
    monkeypatch,
):
    client = request_policy.PolicyTelegramClient(
        StringSession(), 12345, "hash", flood_sleep_threshold=10
    )
    seen = []

    async def base_connect(self):
        # Stands in for Telethon starting _update_loop and _keepalive_loop.
        async def background():
            seen.append(self.flood_sleep_threshold)

        await asyncio.get_running_loop().create_task(background())

    monkeypatch.setattr(request_policy.TelegramClient, "connect", base_connect)

    with request_policy.raise_flood_waits():
        await client.connect()
        assert client.flood_sleep_threshold == 0

    assert seen == [10]
