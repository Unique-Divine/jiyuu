"""Leased Deal Desk fill operations through the existing Telethon client."""

import asyncio
from collections.abc import Generator
from contextlib import closing, contextmanager
from dataclasses import dataclass
from datetime import datetime, timezone
import io
import os
import re
import sqlite3
import stat
from typing import Any

from telethon import TelegramClient, errors, functions, hints, types, utils

from telegram_mcp.broker import BrokerError, PeerConnected
from telegram_mcp.bot_identity import BotIdentity
from telegram_mcp.request_policy import raise_flood_waits, single_request
from telegram_mcp.session_store import State, validate_identity

TITLE = "Sai deal chat (available)"
REFERENCE = re.compile(r"[a-f0-9]{8}")
POSITIVE = re.compile(r"[1-9][0-9]{0,18}")
ROOM = re.compile(r"-[1-9][0-9]{12,18}")
STEPS = {
    "create": ("channels.createChannel", "create"),
    "add_bot": ("channels.inviteToChannel", "invite"),
    "promote_bot": ("channels.editAdmin", "method:channels.editAdmin"),
    "show_history": ("channels.togglePreHistoryHidden", "method:channels.togglePreHistoryHidden"),
    "set_photo": ("channels.editPhoto", "method:channels.editPhoto"),
    "verify": ("", ""),
}
RIGHTS = {
    "changeInfo": "change_info",
    "deleteMessages": "delete_messages",
    "banUsers": "ban_users",
    "inviteUsers": "invite_users",
    "pinMessages": "pin_messages",
    "addAdmins": "add_admins",
    "manageCall": "manage_call",
    "other": "other",
}
MAX_RPC_SECONDS = 30
DAY_MS = 86_400_000
ADD_GAP_MS = 1_200_000


def now_ms() -> int:
    return int(datetime.now(timezone.utc).timestamp() * 1000)


def timestamp(value: int) -> str:
    return datetime.fromtimestamp(value / 1000, timezone.utc).isoformat()


def retry(code: str, until: int | None = None) -> dict[str, Any]:
    return {
        "outcome": "retry",
        "errorCode": code,
        "retryAt": timestamp(until or now_ms() + 300_000),
    }


def positive(value: object) -> bool:
    return isinstance(value, str) and POSITIVE.fullmatch(value) is not None and int(value) < 2**63


@dataclass(frozen=True)
class Action:
    fill: str
    action: str
    ref: str
    step: str
    chat: str | None
    reconcile: bool
    expires: int
    cap: int
    groups: int

    @classmethod
    def parse(cls, body: dict[str, Any]) -> "Action":
        action = body.get("action")
        limits = body.get("limits")
        try:
            if (
                set(body) != {"v", "id", "op", "accountId", "action", "limits"}
                or not isinstance(action, dict)
                or set(action)
                != {"fillId", "actionId", "ref", "step", "chatId", "reconcile", "leaseExpiresAt"}
                or not isinstance(limits, dict)
                or set(limits) != {"cap", "groupStopAt"}
                or not positive(action["fillId"])
                or not positive(action["actionId"])
                or not isinstance(action["ref"], str)
                or REFERENCE.fullmatch(action["ref"]) is None
                or action["step"] not in STEPS
                or type(action["reconcile"]) is not bool
                or type(limits["cap"]) is not int
                or not 0 <= limits["cap"] <= 100_000
                or type(limits["groupStopAt"]) is not int
                or not 0 < limits["groupStopAt"] <= 100_000
            ):
                raise ValueError
            chat = action["chatId"]
            if action["step"] == "create":
                if chat is not None:
                    raise ValueError
            elif (
                not isinstance(chat, str)
                or ROOM.fullmatch(chat) is None
                or int(chat) >= -(10**12)
                or int(chat) <= -(2**63)
            ):
                raise ValueError
            expires = datetime.fromisoformat(action["leaseExpiresAt"].replace("Z", "+00:00"))
            if expires.tzinfo is None:
                raise ValueError
            deadline = int(expires.timestamp() * 1000)
            if deadline > now_ms() + 10 * 60_000:
                raise ValueError
        except (TypeError, ValueError, OverflowError, AttributeError):
            raise BrokerError("invalid_request") from None
        return cls(
            action["fillId"],
            action["actionId"],
            action["ref"],
            action["step"],
            chat,
            action["reconcile"],
            deadline,
            limits["cap"],
            limits["groupStopAt"],
        )


class Journal:
    def __init__(self, state: State, account: str):
        self.account = account
        self.path = state.tool / "run" / "managed-fills.sqlite"
        fd = os.open(self.path, os.O_NOFOLLOW | os.O_CREAT | os.O_RDWR, 0o600)
        try:
            metadata = os.fstat(fd)
            if (
                not stat.S_ISREG(metadata.st_mode)
                or metadata.st_uid != os.getuid()
                or stat.S_IMODE(metadata.st_mode) & 0o077
            ):
                raise BrokerError("budget_unavailable")
        finally:
            os.close(fd)
        with self.connect() as db:
            db.executescript(
                """
                CREATE TABLE IF NOT EXISTS dispatch_v1 (
                    account TEXT NOT NULL, fill TEXT NOT NULL, action TEXT NOT NULL,
                    step TEXT NOT NULL, at_ms INTEGER NOT NULL,
                    PRIMARY KEY (account,fill,action)
                );
                CREATE INDEX IF NOT EXISTS dispatch_v1_usage ON dispatch_v1(account,step,at_ms);
                CREATE TABLE IF NOT EXISTS hold_v1 (
                    account TEXT NOT NULL, grp TEXT NOT NULL, until_ms INTEGER NOT NULL,
                    source TEXT NOT NULL, method TEXT NOT NULL, wait_s INTEGER NOT NULL,
                    PRIMARY KEY (account,grp)
                );
            """
            )

    @contextmanager
    def connect(self) -> Generator[sqlite3.Connection, None, None]:
        with closing(sqlite3.connect(self.path, timeout=1)) as db:
            yield db

    def snapshot(self, now: int) -> dict[str, Any]:
        with self.connect() as db:
            row = db.execute(
                "SELECT COUNT(*) FROM dispatch_v1 WHERE account=? AND step='add_bot' AND at_ms>=?",
                (self.account, now // DAY_MS * DAY_MS),
            ).fetchone()
            holds = [
                {
                    "group": group,
                    "untilMs": until,
                    "source": source,
                    "method": method,
                    "waitS": wait,
                }
                for group, until, source, method, wait in db.execute(
                    "SELECT grp,until_ms,source,method,wait_s FROM hold_v1 WHERE account=? AND until_ms>? ORDER BY grp",
                    (self.account, now),
                )
            ]
        return {"addsToday": row[0], "holds": holds}

    def guard(self, action: Action, now: int, *, reserve: bool) -> dict[str, Any] | None:
        if now >= action.expires:
            return retry("lease_unavailable")
        with self.connect() as db:
            db.execute("BEGIN IMMEDIATE")
            group = STEPS[action.step][1]
            blocked = db.execute(
                "SELECT MAX(until_ms) FROM hold_v1 WHERE account=? AND until_ms>? AND (grp=? OR (?='create' AND grp='invite'))",
                (self.account, now, group, group),
            ).fetchone()[0]
            if blocked is not None:
                return retry("hold", blocked)
            if action.step in ("create", "add_bot"):
                daily, latest = db.execute(
                    "SELECT COALESCE(SUM(at_ms>=?), 0), MAX(at_ms) FROM dispatch_v1 WHERE account=? AND step='add_bot'",
                    (now // DAY_MS * DAY_MS, self.account),
                ).fetchone()
                if daily >= action.cap:
                    return retry("daily_cap", (now // DAY_MS + 1) * DAY_MS)
                if latest is not None and latest + ADD_GAP_MS > now:
                    return retry("add_spacing", latest + ADD_GAP_MS)
            if not reserve:
                return None
            inserted = db.execute(
                "INSERT OR IGNORE INTO dispatch_v1 VALUES (?,?,?,?,?)",
                (self.account, action.fill, action.action, action.step, now),
            ).rowcount
            if not inserted:
                return {"outcome": "uncertain"}
            db.commit()
        return None if now_ms() < action.expires else retry("lease_unavailable")

    def held(self, group: str) -> int | None:
        with self.connect() as db:
            row = db.execute(
                "SELECT until_ms FROM hold_v1 WHERE account=? AND grp=? AND until_ms>?",
                (self.account, group, now_ms()),
            ).fetchone()
        return None if row is None else row[0]

    def wait(self, method: str, group: str, error: Exception) -> dict[str, Any] | None:
        match error:
            case errors.FloodWaitError() | errors.FloodPremiumWaitError():
                seconds = max(1, error.seconds)
                source = (
                    "flood_premium_wait"
                    if isinstance(error, errors.FloodPremiumWaitError)
                    else "flood_wait"
                )
            case errors.PeerFloodError():
                seconds = 86_400
                source = "peer_flood"
            case _:
                return None
        until = now_ms() + (seconds + 30) * 1000
        with self.connect() as db:
            db.execute(
                """INSERT INTO hold_v1 VALUES (?,?,?,?,?,?)
                   ON CONFLICT(account,grp) DO UPDATE SET until_ms=excluded.until_ms,
                     source=excluded.source, method=excluded.method, wait_s=excluded.wait_s
                   WHERE hold_v1.until_ms < excluded.until_ms""",
                (self.account, group, until, source, method, seconds),
            )
            db.commit()
        return retry("hold", until)


class PoolTransport:
    def __init__(
        self,
        client: TelegramClient,
        state: State,
        account: str,
        photo: bytes,
        bot: BotIdentity = BotIdentity(),
    ):
        self.client = client
        self.bot = bot
        self.account = account
        self.journal = Journal(state, account)
        self.photo = photo

    async def identity(self) -> None:
        with raise_flood_waits():
            user = await self.client.get_me()
        validate_identity(user, int(self.account))

    async def dialogs(self, limit: int):
        method = "messages.getDialogs"
        group = f"method:{method}"
        if self.journal.held(group) is not None:
            raise BrokerError("account_held")
        try:
            async for dialog in self.client.iter_dialogs(limit=limit + 1):
                if limit == 0:
                    raise BrokerError("group_count_unavailable")
                limit -= 1
                yield dialog
        except Exception as error:
            if self.journal.wait(method, group, error) is not None:
                raise BrokerError("account_held") from None
            raise

    async def count(self) -> int:
        count = 0
        with raise_flood_waits():
            async for dialog in self.dialogs(2000):
                if isinstance(
                    dialog.entity,
                    (types.Channel, types.Chat, types.ChannelForbidden, types.ChatForbidden),
                ):
                    count += 1
        return count

    async def dispatch(
        self, body: dict[str, Any], peer_connected: PeerConnected | None = None
    ) -> dict[str, Any]:
        action = Action.parse(body)
        if body["op"] == "fill.observe":
            return await self.observe(action)
        return await self.execute(action, peer_connected)

    async def snapshot(self) -> dict[str, Any]:
        await self.identity()
        groups = await self.count()
        observed = now_ms()
        return {
            "accountId": self.account,
            "groupCount": groups,
            **self.journal.snapshot(observed),
        }

    async def read_room(self, selector: hints.EntityLike, ref: str):
        peer = await self.client.get_input_entity(selector)
        if not isinstance(peer, types.InputPeerChannel):
            return None
        channel = types.InputChannel(peer.channel_id, peer.access_hash)
        full = await self.client(functions.channels.GetFullChannelRequest(channel))
        rooms = [
            chat
            for chat in full.chats
            if isinstance(chat, types.Channel) and chat.id == peer.channel_id
        ]
        if len(rooms) != 1:
            return None
        room = rooms[0]
        if (
            not room.creator
            or not room.megagroup
            or room.forum
            or room.broadcast
            or room.title != TITLE
        ):
            return None
        if (
            not full.full_chat.about.endswith(f"Ref: {ref}")
            or re.search(r"(?<![\w])Ref: [a-f0-9]{8}$", full.full_chat.about) is None
        ):
            return None
        return channel, full.full_chat

    async def read_bot(self, room):
        bot = await self.client.get_entity(self.bot.username)
        if not isinstance(bot, types.User) or bot.id != self.bot.id or not bot.bot:
            raise BrokerError("invalid_bot")
        if bot.access_hash is None:
            raise BrokerError("invalid_bot")
        peer = types.InputUser(bot.id, bot.access_hash)
        try:
            result = await self.client(
                functions.channels.GetParticipantRequest(
                    room, types.InputPeerUser(bot.id, bot.access_hash)
                )
            )
        except errors.UserNotParticipantError:
            return peer, False, {}
        data = result.participant
        present = not isinstance(
            data, (types.ChannelParticipantLeft, types.ChannelParticipantBanned)
        )
        rights = (
            {
                name: True
                for name, attr in RIGHTS.items()
                if getattr(data.admin_rights, attr, False)
            }
            if isinstance(data, types.ChannelParticipantAdmin)
            else {}
        )
        return peer, present, rights

    async def observe(self, action: Action) -> dict[str, Any]:
        try:
            await self.identity()
            with raise_flood_waits():
                return await self._observe(action)
        except Exception:
            return retry("room_observation_unavailable")

    async def _observe(self, action: Action) -> dict[str, Any]:
        if action.step == "create":
            matches = set()
            async for dialog in self.dialogs(1000):
                entity = dialog.entity
                if (
                    isinstance(entity, types.Channel)
                    and entity.title == TITLE
                    and entity.creator
                    and await self.read_room(entity, action.ref)
                ):
                    matches.add(str(utils.get_peer_id(entity)))
            if len(matches) > 1:
                return retry("duplicate_pool_reference")
            if matches:
                return {"outcome": "succeeded", "chatId": matches.pop()}
            return {"outcome": "not_found"}
        if action.chat is None:
            return retry("invalid_action_binding")
        room = await self.read_room(int(action.chat), action.ref)
        if room is None:
            return {"outcome": "room_failed", "errorCode": "pool_chat_mismatch"}
        peer, full = room
        history = not full.hidden_prehistory
        photo = isinstance(full.chat_photo, types.Photo)
        if action.step == "show_history":
            return {"outcome": "succeeded" if history else "not_found"}
        if action.step == "set_photo":
            return {"outcome": "succeeded" if photo else "not_found", "photoSet": photo}
        _, present, rights = await self.read_bot(peer)
        if action.step == "add_bot":
            return {"outcome": "succeeded" if present else "not_found"}
        promoted = set(RIGHTS).issubset(rights)
        if action.step == "promote_bot":
            return {"outcome": "succeeded" if promoted else "not_found"}
        if not history or not promoted:
            return {"outcome": "room_failed", "errorCode": "room_verification_failed"}
        return {
            "outcome": "succeeded",
            "botRights": rights,
            "photoSet": photo,
            "verifiedAt": timestamp(now_ms()),
        }

    async def execute(
        self,
        action: Action,
        peer_connected: PeerConnected | None = None,
    ) -> dict[str, Any]:
        if action.reconcile or action.step == "verify":
            return retry("invalid_action_binding")
        dispatched = False
        try:
            await self.identity()
            with raise_flood_waits():
                refusal = self.journal.guard(action, now_ms(), reserve=False)
                if refusal is not None:
                    return refusal
                if action.step == "create":
                    if await self.count() >= action.groups:
                        return retry("group_limit")
                    request = functions.channels.CreateChannelRequest(
                        TITLE, f"Chat with the Sai team. Ref: {action.ref}", megagroup=True
                    )
                else:
                    evidence = await self._observe(action)
                    if evidence["outcome"] != "not_found":
                        return evidence
                    if action.chat is None:
                        return retry("invalid_action_binding")
                    room = await self.read_room(int(action.chat), action.ref)
                    if room is None:
                        return retry("room_preflight_unavailable")
                    peer, _ = room
                    if action.step in ("add_bot", "promote_bot"):
                        bot, present, _ = await self.read_bot(peer)
                        if action.step == "add_bot":
                            if present:
                                return {"outcome": "succeeded"}
                            request = functions.channels.InviteToChannelRequest(peer, [bot])
                        elif not present:
                            return retry("bot_not_visible")
                        else:
                            request = functions.channels.EditAdminRequest(
                                peer,
                                bot,
                                types.ChatAdminRights(
                                    **{field: True for field in RIGHTS.values()},
                                    manage_topics=True,
                                    post_stories=True,
                                    edit_stories=True,
                                    delete_stories=True,
                                ),
                                "",
                            )
                    elif action.step == "show_history":
                        request = functions.channels.TogglePreHistoryHiddenRequest(peer, False)
                    else:
                        if not self.photo:
                            return retry("photo_upload_unavailable")
                        if not await still_connected(peer_connected):
                            return retry("broker_peer_gone")
                        uploaded = await asyncio.wait_for(
                            self.client.upload_file(
                                io.BytesIO(self.photo), file_name="sai-chat-photo.jpg"
                            ),
                            timeout=MAX_RPC_SECONDS,
                        )
                        request = functions.channels.EditPhotoRequest(
                            peer, types.InputChatUploadedPhoto(file=uploaded)
                        )
                if not await still_connected(peer_connected):
                    return retry("broker_peer_gone")
                refusal = self.journal.guard(action, now_ms(), reserve=True)
                if refusal is not None:
                    return refusal
                if not await still_connected(peer_connected):
                    return retry("broker_peer_gone")
                dispatched = True
                try:
                    with single_request():
                        answer = await asyncio.wait_for(
                            self.client(request),
                            timeout=min(MAX_RPC_SECONDS, (action.expires - now_ms()) / 1000),
                        )
                except Exception as error:
                    method, group = STEPS[action.step]
                    return self.journal.wait(method, group, error) or {"outcome": "uncertain"}
                if action.step == "create":
                    rooms = [
                        room
                        for room in getattr(answer, "chats", ())
                        if isinstance(room, types.Channel) and room.title == TITLE and room.creator
                    ]
                    if len(rooms) == 1 and await self.read_room(rooms[0], action.ref):
                        return {"outcome": "succeeded", "chatId": str(utils.get_peer_id(rooms[0]))}
                else:
                    evidence = await self._observe(action)
                    if evidence["outcome"] == "succeeded":
                        return evidence
        except Exception:
            if not dispatched:
                return retry("room_preflight_unavailable")
        return {"outcome": "uncertain"}


async def still_connected(peer_connected: PeerConnected | None) -> bool:
    if peer_connected is None:
        return True
    # Give the Unix transport a turn to consume EOF delivered during a
    # synchronous journal operation.
    await asyncio.sleep(0)
    return peer_connected()
