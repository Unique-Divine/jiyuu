"""Serialize generated Telethon requests without sending Telegram writes."""

from datetime import datetime, timezone
from types import SimpleNamespace

import pytest
from telethon import functions, types, utils

from telegram_mcp.tools import chats, contacts, groups, profile


def basic_chat():
    return types.Chat(
        id=42,
        title="Example",
        photo=types.ChatPhotoEmpty(),
        participants_count=1,
        date=None,
        version=1,
    )


class Client:
    def __init__(self, result):
        self.result = result
        self.requests = []

    async def get_input_entity(self, entity):
        return utils.get_input_peer(entity)

    async def _get_input_notify(self, peer):
        return peer

    async def __call__(self, request):
        await request.resolve(self, utils)
        assert request._bytes()
        self.requests.append(request)
        return self.result

    async def get_dialogs(self):
        return []


def configure(monkeypatch, module, entity, result=True):
    client = Client(result)

    async def resolve(identifier, client):
        return entity

    async def resolve_input(identifier, client):
        return utils.get_input_peer(entity)

    async def connected(client):
        pass

    monkeypatch.setattr(module, "get_client", lambda account=None: client)
    monkeypatch.setattr(module, "resolve_entity", resolve)
    monkeypatch.setattr(module, "resolve_input_entity", resolve_input)
    monkeypatch.setattr(module, "ensure_connected", connected)
    return client


@pytest.mark.parametrize("muted", [True, False])
@pytest.mark.asyncio
async def test_notify_settings_use_input_notify_peer_and_utc_dates(monkeypatch, muted):
    client = configure(monkeypatch, chats, basic_chat())
    handler = chats.mute_chat if muted else chats.unmute_chat
    result = await handler(chat_id=-42)
    assert result == f"Chat -42 {'muted' if muted else 'unmuted'}."
    request = client.requests[0]
    assert isinstance(request, functions.account.UpdateNotifySettingsRequest)
    assert isinstance(request.peer, types.InputNotifyPeer)
    assert request.peer.peer.chat_id == 42
    assert request.settings.mute_until == datetime.fromtimestamp(
        2**31 - 1 if muted else 0, tz=timezone.utc
    )


@pytest.mark.asyncio
async def test_contact_import_uses_generated_contact_objects(monkeypatch):
    client = configure(monkeypatch, contacts, None, SimpleNamespace(imported=[object()]))
    assert (
        await contacts.import_contacts([{"phone": "+10000000000", "first_name": "Example"}])
        == "Imported 1 contacts."
    )
    request = client.requests[0]
    assert isinstance(request.contacts[0], types.InputPhoneContact)
    assert request.contacts[0].last_name == ""
    assert request.contacts[0].client_id == 0


@pytest.mark.asyncio
async def test_phone_add_serializes_generated_contact(monkeypatch):
    client = configure(monkeypatch, contacts, None, SimpleNamespace(imported=[object()]))
    result = await contacts.add_contact(phone="+10000000000", first_name="Example")
    assert "added successfully" in result
    assert len(client.requests) == 1
    assert isinstance(client.requests[0].contacts[0], types.InputPhoneContact)


@pytest.mark.asyncio
async def test_common_chats_uses_raw_api_with_input_user(monkeypatch):
    user = types.User(id=99, access_hash=123, first_name="Example")
    client = configure(monkeypatch, contacts, user, SimpleNamespace(chats=[basic_chat()]))
    result = await contacts.get_contact_chats(contact_id="@example")
    assert '"chat_id": -42' in result
    request = client.requests[0]
    assert isinstance(request, functions.messages.GetCommonChatsRequest)
    assert request.user_id.user_id == 99
    assert request.max_id == 0 and request.limit == 100


@pytest.mark.asyncio
async def test_leave_basic_chat_uses_self_input_user_once(monkeypatch):
    client = configure(monkeypatch, groups, basic_chat())
    assert "Left basic group" in await groups.leave_chat(chat_id=-42)
    assert len(client.requests) == 1
    request = client.requests[0]
    assert isinstance(request.user_id, types.InputUserSelf)
    assert request.chat_id == 42


@pytest.mark.parametrize("handler", [groups.get_invite_link, groups.export_chat_invite])
@pytest.mark.asyncio
async def test_invite_export_uses_existing_raw_request(monkeypatch, handler):
    client = configure(
        monkeypatch, groups, basic_chat(), SimpleNamespace(link="https://t.me/+example")
    )
    assert await handler(chat_id=-42) == "https://t.me/+example"
    assert len(client.requests) == 1
    assert isinstance(client.requests[0], functions.messages.ExportChatInviteRequest)
    assert isinstance(client.requests[0].peer, types.InputPeerChat)


@pytest.mark.asyncio
async def test_basic_chat_edits_use_resolved_positive_id(monkeypatch):
    client = configure(monkeypatch, groups, basic_chat())
    assert "updated" in await groups.edit_chat_title(chat_id="@example", title="Updated")
    assert client.requests[0].chat_id == 42
    assert "deleted" in await groups.delete_chat_photo(chat_id=-42)
    assert client.requests[1].chat_id == 42


@pytest.mark.asyncio
async def test_admin_log_resolves_input_channel(monkeypatch):
    entity = types.Channel(
        id=42, access_hash=123, title="Example", photo=types.ChatPhotoEmpty(), date=None
    )
    client = configure(monkeypatch, groups, entity, SimpleNamespace(events=[]))
    assert await groups.get_recent_actions(chat_id="@example") == "No recent admin actions found."
    request = client.requests[0]
    assert isinstance(request.channel, types.InputChannel)
    assert request.channel.channel_id == 42


@pytest.mark.asyncio
async def test_profile_photo_lookup_uses_self_input_user(monkeypatch):
    client = configure(monkeypatch, profile, None, SimpleNamespace(photos=[]))
    assert await profile.delete_profile_photo() == "No profile photo to delete."
    assert isinstance(client.requests[0].user_id, types.InputUserSelf)


@pytest.mark.asyncio
async def test_privacy_users_are_resolved_input_users(monkeypatch):
    user = types.User(id=99, access_hash=123)
    client = configure(monkeypatch, profile, user)
    assert "updated successfully" in await profile.set_privacy_settings(
        key="phone", allow_users=[99], disallow_users=[100]
    )
    request = client.requests[0]
    assert all(isinstance(rule.users[0], types.InputUser) for rule in request.rules)


@pytest.mark.asyncio
async def test_basic_chat_photo_upload_uses_resolved_id(monkeypatch, tmp_path):
    client = configure(monkeypatch, groups, basic_chat())

    async def readable(**kwargs):
        return tmp_path / "photo.jpg", None

    async def upload(path):
        return types.InputFile(id=1, parts=1, name="photo.jpg", md5_checksum="")

    monkeypatch.setattr(groups, "_resolve_readable_file_path", readable)
    monkeypatch.setattr(client, "upload_file", upload, raising=False)
    assert "updated" in await groups.edit_chat_photo(chat_id=-42, file_path="photo.jpg")
    assert client.requests[0].chat_id == 42


@pytest.mark.parametrize("timestamp", [0, 1_900_000_000])
@pytest.mark.asyncio
async def test_permission_expiry_preserves_wire_timestamp(monkeypatch, timestamp):
    client = configure(monkeypatch, groups, basic_chat())
    assert "updated" in await groups.set_default_chat_permissions(
        chat_id=-42, until_date=timestamp
    )
    request = client.requests[0]
    expected = datetime.fromtimestamp(timestamp, tz=timezone.utc) if timestamp else None
    assert request.banned_rights.until_date == expected
