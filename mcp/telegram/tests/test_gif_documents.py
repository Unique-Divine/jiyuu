"""GIF search retains real document metadata without changing integer tool IDs."""

import gc
import json
import weakref
from datetime import datetime, timezone
from types import SimpleNamespace

import pytest
from telethon import errors, types, utils

from telegram_mcp.gif_documents import GifDocuments, MAX_GIF_DOCUMENTS
from telegram_mcp.tools import media


def document(number=1):
    return types.Document(
        id=number,
        access_hash=123,
        file_reference=b"example-reference",
        date=datetime(2000, 1, 1, tzinfo=timezone.utc),
        mime_type="video/mp4",
        size=1,
        dc_id=1,
        attributes=[types.DocumentAttributeAnimated()],
    )


class Client:
    def __init__(self, results, error=None):
        self.results = results
        self.error = error
        self.sent = []
        self.queries = []

    async def inline_query(self, bot, query):
        self.queries.append((bot, query))
        return self.results

    async def send_file(self, entity, file):
        # Full metadata must survive conversion into Telegram's input object.
        assert utils.get_input_document(file)._bytes()
        self.sent.append((entity, file))
        if self.error:
            raise self.error


def configure(monkeypatch, clients):
    async def connected(client):
        pass

    async def resolve(identifier, client):
        return types.InputPeerChat(chat_id=42)

    monkeypatch.setattr(media, "_gif_documents", GifDocuments())
    monkeypatch.setattr(media, "get_client", lambda account=None: clients[account])
    monkeypatch.setattr(media, "ensure_connected", connected)
    monkeypatch.setattr(media, "resolve_entity", resolve)


@pytest.mark.asyncio
async def test_search_then_send_keeps_full_document_metadata(monkeypatch):
    gif = document()
    client = Client([SimpleNamespace(document=gif)])
    configure(monkeypatch, {None: client})
    assert json.loads(await media.get_gif_search("example")) == [gif.id]
    assert client.queries == [("gif", "example")]
    assert await media.send_gif(chat_id=-42, gif_id=gif.id) == "GIF sent to chat -42."
    assert client.sent[0][1] is gif


@pytest.mark.asyncio
async def test_account_isolation_and_unknown_id_refuse_network_send(monkeypatch):
    first = Client([SimpleNamespace(document=document())])
    second = Client([])
    configure(monkeypatch, {"first": first, "second": second})
    await media.get_gif_search("example", account="first")
    assert "get_gif_search again" in await media.send_gif(-42, 1, account="second")
    assert "get_gif_search again" in await media.send_gif(-42, 999, account="first")
    assert first.sent == [] and second.sent == []


@pytest.mark.parametrize("results", [[], [SimpleNamespace(document=None)], [object()]])
@pytest.mark.asyncio
async def test_empty_and_non_document_results_are_not_sendable(monkeypatch, results):
    client = Client(results)
    configure(monkeypatch, {None: client})
    assert json.loads(await media.get_gif_search("example")) == []


@pytest.mark.parametrize(
    "error_class", [errors.FileReferenceExpiredError, errors.FileReferenceInvalidError]
)
@pytest.mark.asyncio
async def test_expired_reference_requires_search_without_replaying_write(monkeypatch, error_class):
    client = Client([SimpleNamespace(document=document())], error_class(request=None))
    configure(monkeypatch, {None: client})
    await media.get_gif_search("example")
    assert "get_gif_search again" in await media.send_gif(-42, 1)
    assert len(client.sent) == 1
    assert "get_gif_search again" in await media.send_gif(-42, 1)
    assert len(client.sent) == 1


def test_metadata_is_bounded_per_client_and_released_with_client():
    store = GifDocuments()
    client = Client([])
    store.remember(client, [document(number) for number in range(MAX_GIF_DOCUMENTS + 1)])
    assert store.get(client, 0) is None
    assert store.get(client, MAX_GIF_DOCUMENTS) is not None
    ref = weakref.ref(client)
    del client
    gc.collect()
    assert ref() is None


@pytest.mark.asyncio
async def test_new_process_state_requires_search_again(monkeypatch):
    client = Client([SimpleNamespace(document=document())])
    configure(monkeypatch, {None: client})
    await media.get_gif_search("example")
    monkeypatch.setattr(media, "_gif_documents", GifDocuments())
    assert "get_gif_search again" in await media.send_gif(-42, 1)
    assert client.sent == []


@pytest.mark.asyncio
async def test_evicted_result_requires_search_before_network_send(monkeypatch):
    client = Client([])
    configure(monkeypatch, {None: client})
    for number in range(MAX_GIF_DOCUMENTS + 1):
        client.results = [SimpleNamespace(document=document(number))]
        await media.get_gif_search("example")
    assert "get_gif_search again" in await media.send_gif(-42, 0)
    assert client.sent == []
