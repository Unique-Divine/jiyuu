"""Serialize generated Telethon requests without sending Telegram writes."""

from datetime import datetime, timezone
from types import SimpleNamespace

import pytest
from telethon import errors, functions, types, utils

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


def invited_users(missing_user_ids=()):
    updates = types.Updates(updates=[], users=[], chats=[], date=None, seq=0)
    missing = [types.MissingInvitee(user_id=user_id) for user_id in missing_user_ids]
    return types.messages.InvitedUsers(updates=updates, missing_invitees=missing)


def example_user(user_id):
    return types.User(id=user_id, access_hash=user_id * 10, first_name="Example")


def example_channel():
    return types.Channel(
        id=77,
        title="Example Channel",
        photo=types.ChatPhotoEmpty(),
        date=None,
        access_hash=770,
        megagroup=True,
    )


class InviteClient(Client):
    """Answer each request from a per-user plan: a reply or an error to raise."""

    def __init__(self, replies):
        super().__init__(None)
        self.replies = replies

    async def __call__(self, request):
        await request.resolve(self, utils)
        assert request._bytes()
        self.requests.append(request)
        user_id = getattr(getattr(request, "user_id", None), "user_id", None)
        reply = self.replies.get(user_id, self.replies.get(None))
        if isinstance(reply, Exception):
            raise reply
        return reply


def configure_invite(monkeypatch, group, users, replies):
    client = InviteClient(replies)
    entities = {"group": group, **{user.id: user for user in users}}

    async def resolve(identifier, client):
        return entities[identifier]

    monkeypatch.setattr(groups, "get_client", lambda account=None: client)
    monkeypatch.setattr(groups, "resolve_entity", resolve)
    return client


def test_missing_invitee_ids_reads_invited_users_and_tolerates_old_replies():
    assert groups._missing_invitee_ids(invited_users([7, 8])) == [7, 8]
    assert groups._missing_invitee_ids(invited_users()) == []
    old_reply = types.Updates(updates=[], users=[], chats=[], date=None, seq=0)
    assert groups._missing_invitee_ids(old_reply) == []
    assert groups._missing_invitee_ids(None) == []


def test_not_added_note_names_users_and_reason():
    assert groups._not_added_note([], groups.PRIVACY_REASON) == ""
    assert groups._not_added_note([7], "a reason") == (
        "Telegram did not add 1 user because a reason: 7."
    )
    assert groups._not_added_note([7, 8], "a reason") == (
        "Telegram did not add 2 users because a reason: 7, 8."
    )


def test_invite_report_counts_only_users_that_were_added():
    report = groups._invite_report("Example", 5, missing=[8], already=[9], not_mutual=[10])
    assert report.startswith("Invited 2 of 5 users to Example.")
    assert "Already members: 9." in report
    assert f"because {groups.PRIVACY_REASON}: 8." in report
    assert f"because {groups.MUTUAL_CONTACT_REASON}: 10." in report
    assert "Stopped early" not in report


@pytest.mark.asyncio
async def test_basic_group_invite_sends_one_add_chat_user_request_per_user(monkeypatch):
    users = [example_user(7), example_user(8)]
    client = configure_invite(monkeypatch, basic_chat(), users, {None: invited_users()})

    result = await groups.invite_to_group(group_id="group", user_ids=[7, 8])

    assert result == "Invited 2 of 2 users to Example."
    assert [type(request) for request in client.requests] == [
        functions.messages.AddChatUserRequest,
        functions.messages.AddChatUserRequest,
    ]
    assert [request.chat_id for request in client.requests] == [42, 42]
    assert [request.user_id.user_id for request in client.requests] == [7, 8]
    assert all(request.fwd_limit == 100 for request in client.requests)


@pytest.mark.asyncio
async def test_basic_group_invite_reports_missing_and_existing_members(monkeypatch):
    users = [example_user(7), example_user(8), example_user(9)]
    replies = {
        None: invited_users(),
        7: errors.UserAlreadyParticipantError(request=None),
        8: invited_users([8]),
    }
    client = configure_invite(monkeypatch, basic_chat(), users, replies)

    result = await groups.invite_to_group(group_id="group", user_ids=[7, 8, 9])

    assert len(client.requests) == 3
    assert result.startswith("Invited 1 of 3 users to Example.")
    assert "Already members: 7." in result
    assert f"Telegram did not add 1 user because {groups.PRIVACY_REASON}: 8." in result


@pytest.mark.asyncio
async def test_basic_group_invite_reports_partial_success_when_stopped(monkeypatch):
    users = [example_user(7), example_user(8), example_user(9)]
    replies = {None: invited_users(), 8: RuntimeError("chat is gone")}
    client = configure_invite(monkeypatch, basic_chat(), users, replies)

    result = await groups.invite_to_group(group_id="group", user_ids=[7, 8, 9])

    assert len(client.requests) == 2
    assert result.startswith("Invited 1 of 3 users to Example.")
    assert "Stopped early: An error occurred" in result
    assert result.endswith("Not added: 8, 9.")


@pytest.mark.asyncio
async def test_basic_group_invite_first_failure_is_an_error(monkeypatch):
    users = [example_user(7), example_user(8)]
    replies = {7: RuntimeError("chat is gone")}
    client = configure_invite(monkeypatch, basic_chat(), users, replies)

    result = await groups.invite_to_group(group_id="group", user_ids=[7, 8])

    assert len(client.requests) == 1
    assert result.startswith("An error occurred")


@pytest.mark.asyncio
async def test_channel_invite_sends_one_request_and_reports_missing(monkeypatch):
    users = [example_user(7), example_user(8)]
    client = configure_invite(monkeypatch, example_channel(), users, {None: invited_users([8])})

    result = await groups.invite_to_group(group_id="group", user_ids=[7, 8])

    assert len(client.requests) == 1
    request = client.requests[0]
    assert isinstance(request, functions.channels.InviteToChannelRequest)
    assert isinstance(request.channel, types.InputChannel)
    assert [user.user_id for user in request.users] == [7, 8]
    assert result.startswith("Invited 1 of 2 users to Example Channel.")
    assert f"because {groups.PRIVACY_REASON}: 8." in result


@pytest.mark.asyncio
async def test_create_group_warns_about_missing_invitees(monkeypatch):
    created = basic_chat()
    updates = types.Updates(updates=[], users=[], chats=[created], date=None, seq=0)
    reply = types.messages.InvitedUsers(
        updates=updates, missing_invitees=[types.MissingInvitee(user_id=8)]
    )
    users = [example_user(7), example_user(8)]
    client = configure_invite(monkeypatch, None, users, {None: reply})

    result = await groups.create_group(title="Example", user_ids=[7, 8])

    assert isinstance(client.requests[0], functions.messages.CreateChatRequest)
    assert result.startswith("Group created with ID: -42\nWarning: ")
    assert f"Telegram did not add 1 user because {groups.PRIVACY_REASON}: 8." in result
