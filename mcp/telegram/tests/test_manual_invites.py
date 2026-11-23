import os
from types import SimpleNamespace
from typing import cast

import pytest
from telethon import TelegramClient, errors, functions, types

from telegram_mcp.fill import Action, PoolTransport, now_ms
from telegram_mcp.manual_invites import invite
from telegram_mcp.session_store import State
from test_broker_fill import create_request


class Client:
    def __init__(self):
        self.called = []
        self.added = False
        self.wait = False

    async def get_me(self):
        return SimpleNamespace(id=42, bot=False)

    async def get_permissions(self, chat, user):
        if not self.added:
            raise errors.UserNotParticipantError(None)
        return SimpleNamespace(is_banned=False, has_left=False)

    async def get_input_entity(self, target):
        if isinstance(target, types.User):
            return types.InputPeerUser(target.id, 123)
        return types.InputPeerChannel(target.id, 234)

    async def __call__(self, request):
        self.called.append(request)
        if self.wait:
            raise errors.FloodWaitError(request, 17)
        raise TimeoutError


@pytest.mark.asyncio
@pytest.mark.parametrize("channel", [False, True])
async def test_one_invite_shares_usage_and_reconciles_a_lost_result(
    monkeypatch, tmp_path, channel
):
    monkeypatch.setenv("BOKU_HOME", os.fspath(tmp_path / "boku"))
    state = State.discover()
    client = Client()
    owner = PoolTransport(cast(TelegramClient, client), state, "42", b"")
    target = types.User(id=55)
    chat = (
        types.Channel(17, "Fixture", types.ChatPhotoEmpty(), None)
        if channel
        else types.Chat(17, "Fixture", types.ChatPhotoEmpty(), 1, None, 0)
    )
    assert await invite(owner, chat, target, 0) == "budget_unavailable"
    assert client.called == []
    owner.latest_budget = (3, now_ms() - 1_000)
    assert await invite(owner, chat, target, 0) == "budget_unavailable"
    assert client.called == []
    owner.latest_budget = (3, now_ms() + 120_000)
    assert await invite(owner, chat, target, 0) == "uncertain"
    assert len(client.called) == 1
    assert isinstance(
        client.called[0],
        (
            functions.channels.InviteToChannelRequest
            if channel
            else functions.messages.AddChatUserRequest
        ),
    )
    assert owner.journal.snapshot(now_ms())["addsToday"] == 1
    baseline = Action.parse(create_request())
    block = owner.journal.guard(baseline, now_ms(), reserve=False)
    assert block is not None and block["errorCode"] == "add_spacing"

    restarted = PoolTransport(cast(TelegramClient, client), state, "42", b"")
    restarted.latest_budget = (3, now_ms() + 60_000)
    assert await invite(restarted, chat, target, 0) == "prior_outcome_unknown"
    assert len(client.called) == 1
    client.added = True
    assert await invite(restarted, chat, target, 0) == "already_member"
    assert len(client.called) == 1
    assert restarted.journal.snapshot(now_ms())["addsToday"] == 1


@pytest.mark.asyncio
async def test_manual_wait_blocks_managed_and_new_manual_adds(monkeypatch, tmp_path):
    monkeypatch.setenv("BOKU_HOME", os.fspath(tmp_path / "boku"))
    state = State.discover()
    client = Client()
    client.wait = True
    owner = PoolTransport(cast(TelegramClient, client), state, "42", b"")
    owner.latest_budget = (3, now_ms() + 120_000)
    room = types.Channel(17, "Fixture", types.ChatPhotoEmpty(), None)
    assert await invite(owner, room, types.User(id=55), 0) == "hold"
    assert await invite(owner, room, types.User(id=56), 0) == "hold"
    assert len(client.called) == 1
    holds = owner.journal.snapshot(now_ms())["holds"]
    assert [(hold["group"], hold["waitS"]) for hold in holds] == [("invite", 17)]
    baseline = Action.parse(create_request())
    block = owner.journal.guard(baseline, now_ms(), reserve=False)
    assert block is not None and block["errorCode"] == "hold"
