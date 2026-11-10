"""Runtime wiring raises flood waits, keeps the connection, and shares cooldowns."""

import json
from types import SimpleNamespace

import pytest
from telethon import errors
from telethon.tl.types import PeerChannel

from telegram_mcp import runtime
from telegram_mcp.tools import chats, messages


def flood(seconds):
    return errors.FloodWaitError(request=None, capture=seconds)


def test_clients_raise_flood_waits_instead_of_sleeping(monkeypatch):
    created = []

    def fake_client(*args, **kwargs):
        created.append(kwargs)
        return SimpleNamespace()

    monkeypatch.setattr(runtime, "TelegramClient", fake_client)
    monkeypatch.setattr(runtime, "_build_proxy_for_label", lambda label: (None, None))

    runtime._build_client("session", "default")

    assert created[0]["flood_sleep_threshold"] == 0


@pytest.mark.asyncio
async def test_flood_wait_on_connection_ping_does_not_reconnect(monkeypatch):
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

    with pytest.raises(errors.FloodWaitError):
        await runtime.ensure_connected(client)

    assert client.calls == ["ping"]


def test_any_tool_error_reports_flood_wait_seconds():
    result = json.loads(runtime.log_and_format_error("get_history", flood(90)))["error"]

    assert result["type"] == "flood_wait"
    assert result["request"] == "get_history"
    assert result["wait_seconds"] == 90
    assert result["retry_at"].endswith("Z")


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
