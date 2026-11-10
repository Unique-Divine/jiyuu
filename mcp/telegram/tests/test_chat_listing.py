"""Chat discovery exposes dialog checkpoints without per-chat history reads."""

import json
from datetime import datetime, timezone
from types import SimpleNamespace

import pytest

from telegram_mcp.tools import chats


def configured_client(monkeypatch, dialogs):
    class Client:
        def __init__(self):
            self.calls = []

        async def get_dialogs(self, **kwargs):
            self.calls.append(kwargs)
            limit = kwargs.get("limit")
            return dialogs if limit is None else dialogs[:limit]

        async def __call__(self, request):
            raise AssertionError("Listing metadata must not fetch per-chat details")

    async def connected(client):
        pass

    client = Client()
    monkeypatch.setattr(chats, "get_client", lambda account=None: client)
    monkeypatch.setattr(chats, "ensure_connected", connected)
    monkeypatch.setattr(chats, "get_marked_id", lambda entity: -entity.id)
    monkeypatch.setattr(chats, "get_entity_filter_type", lambda entity: "group")
    monkeypatch.setattr(chats, "get_entity_type", lambda entity: "group")
    return client


def dialog(number, message=None, unread=0):
    return SimpleNamespace(
        entity=SimpleNamespace(id=number, title=f"Sai {number}"),
        message=message,
        unread_count=unread,
        archived=False,
    )


def decode(output):
    value, _ = json.JSONDecoder().raw_decode(output)
    return value["results"]


@pytest.mark.asyncio
async def test_list_chats_reuses_latest_message_metadata_even_when_account_read(monkeypatch):
    date = datetime(2026, 11, 4, 6, 30, tzinfo=timezone.utc)
    client = configured_client(
        monkeypatch,
        [dialog(1, SimpleNamespace(id=312, date=date)), dialog(2)],
    )
    rows = decode(await chats.list_chats(chat_type="group", limit=10))

    assert client.calls == [{"limit": 10, "archived": None}]
    assert rows[0]["chat_id"] == -1
    assert rows[0]["last_message_id"] == 312
    assert rows[0]["last_message_date"] == date.isoformat()
    assert rows[0]["unread"] == 0
    assert rows[1]["last_message_id"] is None
    assert rows[1]["last_message_date"] is None
    assert "last_message" not in rows[0]


@pytest.mark.asyncio
async def test_get_chats_metadata_preserves_pagination_and_handles_missing_dialog_message(
    monkeypatch,
):
    date = datetime(2026, 11, 4, 7, tzinfo=timezone.utc)
    missing = SimpleNamespace(entity=SimpleNamespace(id=3, title="Sai 3"))
    client = configured_client(
        monkeypatch,
        [dialog(1), dialog(2, SimpleNamespace(id=99, date=date)), missing],
    )
    page_two = decode(await chats.get_chats(page=2, page_size=1))
    page_three = decode(await chats.get_chats(page=3, page_size=1))

    assert client.calls == [{"limit": 2}, {"limit": 3}]
    assert [row["chat_id"] for row in page_two] == [-2]
    assert page_two[0]["last_message_id"] == 99
    assert page_two[0]["last_message_date"] == date.isoformat()
    assert page_three[0]["last_message_id"] is None
    assert page_three[0]["last_message_date"] is None


@pytest.mark.asyncio
async def test_get_chats_reports_page_beyond_available_dialogs(monkeypatch):
    client = configured_client(monkeypatch, [dialog(1)])

    assert await chats.get_chats(page=2, page_size=1) == "Page out of range."
    assert client.calls == [{"limit": 2}]


@pytest.mark.asyncio
async def test_get_chats_bounds_a_stalled_dialog_read(monkeypatch):
    import asyncio

    real_wait_for = asyncio.wait_for
    cancelled = asyncio.Event()

    class Client:
        async def get_dialogs(self, **kwargs):
            try:
                await asyncio.Event().wait()
            finally:
                cancelled.set()

    configured_client(monkeypatch, [])
    monkeypatch.setattr(chats, "get_client", lambda account=None: Client())

    async def short_wait_for(awaitable, *, timeout):
        assert timeout == 30.0
        return await real_wait_for(awaitable, timeout=0.01)

    monkeypatch.setattr(chats.asyncio, "wait_for", short_wait_for)

    result = await chats.get_chats()

    assert cancelled.is_set()
    assert result == "Telegram chat listing timed out. Retry with a smaller page_size."


@pytest.mark.asyncio
async def test_unread_filter_remains_account_state_and_does_not_change_metadata(monkeypatch):
    client = configured_client(
        monkeypatch,
        [dialog(1, SimpleNamespace(id=100)), dialog(2, SimpleNamespace(id=200), unread=2)],
    )
    rows = decode(await chats.list_chats(unread_only=True))

    assert len(client.calls) == 1
    assert len(rows) == 1
    assert rows[0]["chat_id"] == -2
    assert rows[0]["unread"] == 2
    assert rows[0]["last_message_id"] == 200
    assert rows[0]["last_message_date"] is None


@pytest.mark.asyncio
@pytest.mark.parametrize("predecessor,max_id", [(123456, 27), (None, None)])
async def test_full_chat_reuses_migration_metadata_without_extra_api_requests(
    monkeypatch, predecessor, max_id
):
    entity = SimpleNamespace(id=100, title="Sai migrated chat")
    requests = []

    class Client:
        async def __call__(self, request):
            requests.append(request)
            return SimpleNamespace(
                chats=[entity],
                full_chat=SimpleNamespace(
                    about="",
                    migrated_from_chat_id=predecessor,
                    migrated_from_max_id=max_id,
                ),
            )

    async def connected(client):
        pass

    async def resolve(chat_id, client):
        return entity

    monkeypatch.setattr(chats, "get_client", lambda account=None: Client())
    monkeypatch.setattr(chats, "ensure_connected", connected)
    monkeypatch.setattr(chats, "resolve_entity", resolve)
    monkeypatch.setattr(chats, "get_marked_id", lambda entity: -1000000000000 - entity.id)

    result = json.loads(await chats.get_full_chat(chat_id=-1000000000100))

    assert len(requests) == 1
    assert isinstance(requests[0], chats.functions.channels.GetFullChannelRequest)
    assert result["migrated_from_chat_id"] == predecessor
    assert result["migrated_from_chat_marked_id"] == (-predecessor if predecessor else None)
    assert result["migrated_from_max_id"] == max_id
