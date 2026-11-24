import pytest
from telethon.extensions import BinaryReader
from telethon.tl import functions, types

from telegram_mcp.tools import messages


@pytest.mark.asyncio
async def test_create_poll_sends_input_answers_with_zero_hash(monkeypatch):
    requests = []
    peer = types.InputPeerChat(chat_id=42)

    async def send(request):
        with BinaryReader(bytes(request)) as reader:
            requests.append(reader.tgread_object())

    async def resolve(chat_id, client):
        return peer

    monkeypatch.setattr(messages, "get_client", lambda account=None: send)
    monkeypatch.setattr(messages, "resolve_entity", resolve)

    result = await messages.create_poll(-42, "Choose a day", ["Monday", "Tuesday"])

    assert result == "Poll created successfully in chat -42."
    (request,) = requests
    assert isinstance(request, functions.messages.SendMediaRequest)
    assert isinstance(request.media, types.InputMediaPoll)
    assert isinstance(request.media.poll, types.Poll)
    poll = request.media.poll
    assert poll.hash == 0
    assert poll.question.text == "Choose a day"
    assert all(isinstance(answer, types.InputPollAnswer) for answer in poll.answers)
    assert [answer.text.text for answer in poll.answers] == ["Monday", "Tuesday"]
