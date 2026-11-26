"""Apply one existing MCP invite under the managed account's add budget."""

import asyncio

from telethon import errors, functions, types, utils

from telegram_mcp.fill import MAX_RPC_SECONDS, TITLE, PoolTransport, now_ms
from telegram_mcp.request_policy import raise_flood_waits, single_request

# The runner registers only the clients it has verified and locked.
active: dict[int, PoolTransport] = {}


async def member(owner: PoolTransport, chat: types.Channel | types.Chat, user: types.User) -> bool:
    try:
        permissions = await owner.client.get_permissions(chat, user)
    except errors.UserNotParticipantError:
        return False
    if permissions is None or permissions.is_banned:
        raise ValueError("membership unavailable")
    return not permissions.has_left


async def invite(
    owner: PoolTransport,
    chat: types.Channel | types.Chat,
    user: types.User,
    history: int,
) -> str:
    async with owner.mutation_lock:
        return await invite_locked(owner, chat, user, history)


async def invite_locked(
    owner: PoolTransport,
    chat: types.Channel | types.Chat,
    user: types.User,
    history: int,
) -> str:
    if chat.title == TITLE or not user.id or str(user.id) == owner.account:
        return "target_unavailable"
    chat_id = utils.get_peer_id(chat)
    with raise_flood_waits():
        try:
            await owner.identity()
            if await member(owner, chat, user):
                owner.journal.observe_manual_member(chat_id, user.id)
                return "already_member"
            peer = await owner.client.get_input_entity(user)
            if not isinstance(peer, types.InputPeerUser) or peer.user_id != user.id:
                return "target_unavailable"
            target = types.InputUser(peer.user_id, peer.access_hash)
            if isinstance(chat, types.Channel):
                channel = await owner.client.get_input_entity(chat)
                if (
                    not isinstance(channel, types.InputPeerChannel)
                    or channel.channel_id != chat.id
                ):
                    return "target_unavailable"
                request = functions.channels.InviteToChannelRequest(
                    types.InputChannel(channel.channel_id, channel.access_hash), [target]
                )
                method = "channels.inviteToChannel"
            else:
                request = functions.messages.AddChatUserRequest(chat.id, target, history)
                method = "messages.addChatUser"
            budget = owner.latest_budget
            if budget is None:
                return "budget_unavailable"
            cap, expires = budget
            row, reason = owner.journal.reserve_manual(chat_id, user.id, cap, expires)
            if row is None:
                return reason
        except Exception:
            return "preflight_unavailable"

        try:
            with single_request():
                result = await asyncio.wait_for(
                    owner.client(request),
                    timeout=min(MAX_RPC_SECONDS, (expires - now_ms()) / 1000),
                )
        except errors.UserAlreadyParticipantError:
            return await reconcile(owner, chat, user, chat_id, "already_member")
        except (errors.UserPrivacyRestrictedError, errors.UserNotMutualContactError):
            owner.journal.refuse_manual(row)
            return "privacy_restricted"
        except Exception as error:
            blocked = owner.journal.wait(method, "invite", error, origin="manual")
            if blocked is not None:
                owner.journal.refuse_manual(row)
                return "hold"
            return "uncertain"

        missed = getattr(result, "missing_invitees", ())
        if any(getattr(entry, "user_id", None) == user.id for entry in missed):
            owner.journal.refuse_manual(row)
            return "privacy_restricted"
        return await reconcile(owner, chat, user, chat_id, "added")


async def reconcile(
    owner: PoolTransport,
    chat: types.Channel | types.Chat,
    user: types.User,
    chat_id: int,
    outcome: str,
) -> str:
    try:
        if await member(owner, chat, user):
            owner.journal.observe_manual_member(chat_id, user.id)
            return outcome
    except Exception:
        pass
    return "uncertain"
