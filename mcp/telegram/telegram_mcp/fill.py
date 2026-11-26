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
import sys
from time import monotonic
import traceback
from typing import Any
from uuid import UUID, uuid4

from telethon import TelegramClient, errors, functions, hints, types, utils
from telethon.requestiter import RequestIter

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
CAPACITY_OBSERVATION_SECONDS = 180
DIALOG_SCAN_LIMIT = 12000
DIALOG_PAGE_SPACING_SECONDS = 3
CAPACITY_READ_SECONDS = 480
CAPACITY_MAX_WAIT_SECONDS = 240
INVENTORY_RETRY_MS = 15_000
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
                CREATE TABLE IF NOT EXISTS manual_invite_v1 (
                    id INTEGER PRIMARY KEY,
                    account TEXT NOT NULL, chat INTEGER NOT NULL, target INTEGER NOT NULL,
                    at_ms INTEGER NOT NULL, outcome TEXT NOT NULL
                        CHECK(outcome IN ('uncertain','added','refused'))
                );
                CREATE INDEX IF NOT EXISTS manual_invite_v1_usage
                    ON manual_invite_v1(account,at_ms);
                CREATE UNIQUE INDEX IF NOT EXISTS manual_invite_v1_pending
                    ON manual_invite_v1(account,chat,target) WHERE outcome='uncertain';
                CREATE TABLE IF NOT EXISTS hold_v1 (
                    account TEXT NOT NULL, grp TEXT NOT NULL, until_ms INTEGER NOT NULL,
                    source TEXT NOT NULL, method TEXT NOT NULL, wait_s INTEGER NOT NULL,
                    PRIMARY KEY (account,grp)
                );
                CREATE TABLE IF NOT EXISTS pool_trial_v1 (
                    account TEXT PRIMARY KEY, trial_id TEXT NOT NULL,
                    generation INTEGER NOT NULL, state TEXT NOT NULL,
                    cap INTEGER NOT NULL, activate_ms INTEGER NOT NULL,
                    expires_ms INTEGER NOT NULL, attempt_limit INTEGER NOT NULL,
                    next_resume_ms INTEGER
                );
                CREATE TABLE IF NOT EXISTS pool_trial_stop_v1 (
                    account TEXT PRIMARY KEY, trial_id TEXT NOT NULL,
                    generation INTEGER NOT NULL, until_ms INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS pool_rpc_event_v1 (
                    sequence INTEGER PRIMARY KEY, id TEXT UNIQUE NOT NULL,
                    account TEXT NOT NULL, trial_id TEXT, generation INTEGER,
                    occurred_ms INTEGER NOT NULL, method TEXT NOT NULL,
                    origin TEXT NOT NULL, outcome TEXT NOT NULL, wait_s INTEGER,
                    hold_until_ms INTEGER, fill_id TEXT, action_id TEXT
                );
                CREATE INDEX IF NOT EXISTS pool_rpc_trial_events
                    ON pool_rpc_event_v1(account, trial_id, sequence);
            """
            )

    @contextmanager
    def connect(self) -> Generator[sqlite3.Connection, None, None]:
        with closing(sqlite3.connect(self.path, timeout=1)) as db:
            yield db

    def _usage(self, db: sqlite3.Connection, now: int) -> tuple[int, int | None]:
        return db.execute(
            """SELECT COALESCE(SUM(at_ms>=?), 0), MAX(at_ms) FROM (
                SELECT at_ms FROM dispatch_v1 WHERE account=? AND step='add_bot'
                UNION ALL SELECT at_ms FROM manual_invite_v1 WHERE account=?
            )""",
            (now // DAY_MS * DAY_MS, self.account, self.account),
        ).fetchone()

    def configure_trial(self, trial: object) -> None:
        if trial is None:
            with self.connect() as db:
                db.execute("DELETE FROM pool_trial_v1 WHERE account=?", (self.account,))
                db.commit()
            return
        if not isinstance(trial, dict):
            raise BrokerError("invalid_request")
        try:
            trial_id = str(UUID(trial["id"]))
            state = trial["state"]
            next_resume = trial.get("nextResumeAtMs")
            generation, cap, activate, expires, attempt_limit = (
                trial[key]
                for key in ("generation", "cap", "activateAtMs", "expiresAtMs", "attemptLimit")
            )
            if (
                state not in ("scheduled", "running", "paused", "ended")
                or any(
                    type(value) is not int
                    for value in (generation, cap, activate, expires, attempt_limit)
                )
                or not 0 <= generation < 2**31
                or cap not in (32, 16, 8, 4)
                or not 0 < activate < expires < 2**53
                or not 0 < attempt_limit <= 120
                or (
                    next_resume is not None
                    and (type(next_resume) is not int or not 0 < next_resume < 2**53)
                )
            ):
                raise ValueError
        except (KeyError, TypeError, AttributeError, ValueError):
            raise BrokerError("invalid_request") from None
        with self.connect() as db:
            db.execute(
                """INSERT INTO pool_trial_v1 VALUES (?,?,?,?,?,?,?,?,?)
                   ON CONFLICT(account) DO UPDATE SET trial_id=excluded.trial_id,
                     generation=excluded.generation, state=excluded.state, cap=excluded.cap,
                     activate_ms=excluded.activate_ms, expires_ms=excluded.expires_ms,
                     attempt_limit=excluded.attempt_limit,
                     next_resume_ms=CASE
                         WHEN pool_trial_v1.trial_id=excluded.trial_id
                          AND pool_trial_v1.generation=excluded.generation
                         THEN NULLIF(MAX(COALESCE(pool_trial_v1.next_resume_ms,0),
                                         COALESCE(excluded.next_resume_ms,0)),0)
                         ELSE excluded.next_resume_ms END
                   WHERE pool_trial_v1.trial_id != excluded.trial_id
                      OR pool_trial_v1.generation < excluded.generation
                      OR (pool_trial_v1.generation = excluded.generation
                          AND pool_trial_v1.state != 'ended'
                          AND (pool_trial_v1.state != 'paused' OR excluded.state IN ('paused','ended')))
                      OR (pool_trial_v1.trial_id=excluded.trial_id
                          AND pool_trial_v1.generation=excluded.generation
                          AND pool_trial_v1.state='ended' AND excluded.state='ended')""",
                (
                    self.account,
                    trial_id,
                    generation,
                    state,
                    cap,
                    activate,
                    expires,
                    attempt_limit,
                    next_resume,
                ),
            )
            db.commit()

    def trial_events(self, trial: str, after: int) -> list[dict[str, Any]]:
        with self.connect() as db:
            db.row_factory = sqlite3.Row
            rows = db.execute(
                """SELECT sequence, id, generation, occurred_ms AS occurredAtMs,
                          method, origin AS source, outcome, wait_s AS waitS,
                          hold_until_ms AS holdUntilMs, fill_id AS fillId, action_id AS actionId
                   FROM pool_rpc_event_v1 WHERE account=? AND trial_id=? AND sequence>?
                   ORDER BY sequence LIMIT 32""",
                (self.account, trial, after),
            ).fetchall()
        return [dict(row) for row in rows]

    def _trial_gate(
        self, db: sqlite3.Connection, now: int, *, manual: bool = False
    ) -> tuple[dict[str, Any] | None, int | None]:
        trial = db.execute(
            """SELECT trial_id,generation,state,cap,activate_ms,expires_ms,attempt_limit,next_resume_ms
               FROM pool_trial_v1 WHERE account=?""",
            (self.account,),
        ).fetchone()
        stop = db.execute(
            "SELECT trial_id,generation,until_ms FROM pool_trial_stop_v1 WHERE account=?",
            (self.account,),
        ).fetchone()
        if stop and (
            now < stop[2]
            or (trial and trial[2] != "ended" and trial[0] == stop[0] and trial[1] <= stop[1])
        ):
            return retry("trial_flood_stop", max(now + INVENTORY_RETRY_MS, stop[2])), None
        if trial and trial[7] is not None and now < trial[7]:
            return retry("trial_flood_stop", trial[7]), None
        if trial is None or trial[2] == "ended":
            return None, None
        trial_id, _, state, cap, activate, expires, attempt_limit, _ = trial
        if manual or state not in ("scheduled", "running") or not activate <= now < expires:
            return retry("trial_not_running", max(now + INVENTORY_RETRY_MS, activate)), None
        attempts = db.execute(
            """SELECT count(*) FROM pool_rpc_event_v1
               WHERE account=? AND trial_id=? AND origin='managed'
               AND method='channels.inviteToChannel' AND fill_id IS NOT NULL AND outcome!='reserved'""",
            (self.account, trial_id),
        ).fetchone()[0]
        if attempts >= attempt_limit:
            return retry("trial_attempt_limit"), None
        return None, cap

    def manual_trial_blocked(self) -> bool:
        with self.connect() as db:
            return self._trial_gate(db, now_ms(), manual=True)[0] is not None

    def trial_attempt(self, action: Action, method: str) -> str | None:
        now = now_ms()
        with self.connect() as db:
            trial = db.execute(
                """SELECT trial_id,generation FROM pool_trial_v1
                   WHERE account=? AND state IN ('scheduled','running')
                     AND activate_ms<=? AND ?<expires_ms""",
                (self.account, now, now),
            ).fetchone()
            if trial is None:
                return None
            event_id = str(uuid4())
            db.execute(
                """INSERT INTO pool_rpc_event_v1
                   (id,account,trial_id,generation,occurred_ms,method,origin,outcome,fill_id,action_id)
                   VALUES (?,?,?,?,?,?,'managed','uncertain',?,?)""",
                (event_id, self.account, *trial, now_ms(), method, action.fill, action.action),
            )
            db.commit()
            return event_id

    def finish_trial_attempt(self, event_id: str | None, outcome: str) -> None:
        if event_id is not None:
            with self.connect() as db:
                db.execute(
                    """UPDATE pool_rpc_event_v1 SET outcome=?
                       WHERE account=? AND id=? AND outcome='uncertain'""",
                    (outcome, self.account, event_id),
                )
                db.commit()

    def _add_block(self, db: sqlite3.Connection, now: int, cap: int) -> dict[str, Any] | None:
        daily, latest = self._usage(db, now)
        if daily >= cap:
            return retry("daily_cap", (now // DAY_MS + 1) * DAY_MS)
        if latest is not None and latest + ADD_GAP_MS > now:
            return retry("add_spacing", latest + ADD_GAP_MS)
        return None

    def reserve_manual(
        self, chat: int, target: int, cap: int, expires: int
    ) -> tuple[int | None, str]:
        with self.connect() as db:
            db.execute("BEGIN IMMEDIATE")
            now = now_ms()
            if now >= expires:
                return None, "budget_unavailable"
            trial_block, _ = self._trial_gate(db, now, manual=True)
            if trial_block is not None:
                return None, trial_block["errorCode"]
            if db.execute(
                "SELECT 1 FROM manual_invite_v1 WHERE account=? AND chat=? AND target=? AND outcome='uncertain'",
                (self.account, chat, target),
            ).fetchone():
                return None, "prior_outcome_unknown"
            if db.execute(
                "SELECT 1 FROM hold_v1 WHERE account=? AND until_ms>? AND grp='invite'",
                (self.account, now),
            ).fetchone():
                return None, "hold"
            block = self._add_block(db, now, cap)
            if block is not None:
                return None, block["errorCode"]
            cursor = db.execute(
                "INSERT INTO manual_invite_v1(account,chat,target,at_ms,outcome) VALUES (?,?,?,?,'uncertain')",
                (self.account, chat, target, now),
            )
            db.commit()
            return cursor.lastrowid, "reserved"

    def observe_manual_member(self, chat: int, target: int) -> None:
        with self.connect() as db:
            db.execute(
                "UPDATE manual_invite_v1 SET outcome='added' WHERE account=? AND chat=? AND target=? AND outcome='uncertain'",
                (self.account, chat, target),
            )
            db.commit()

    def refuse_manual(self, row: int) -> None:
        with self.connect() as db:
            db.execute(
                "UPDATE manual_invite_v1 SET outcome='refused' WHERE account=? AND id=? AND outcome='uncertain'",
                (self.account, row),
            )
            db.commit()

    def snapshot(self, now: int) -> dict[str, Any]:
        with self.connect() as db:
            count, _ = self._usage(db, now)
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
        return {"addsToday": count, "holds": holds}

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
                trial_block, trial_cap = self._trial_gate(db, now)
                if trial_block is not None:
                    return trial_block
                block = self._add_block(db, now, min(action.cap, trial_cap or action.cap))
                if block is not None:
                    return block
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

    def wait(
        self,
        method: str,
        group: str,
        error: Exception,
        *,
        origin: str = "required_read",
        event_id: str | None = None,
    ) -> dict[str, Any] | None:
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
        now = now_ms()
        until = now + (seconds + 30) * 1000
        with self.connect() as db:
            db.execute("BEGIN IMMEDIATE")
            db.execute(
                """INSERT INTO hold_v1 VALUES (?,?,?,?,?,?)
                   ON CONFLICT(account,grp) DO UPDATE SET until_ms=excluded.until_ms,
                     source=excluded.source, method=excluded.method, wait_s=excluded.wait_s
                   WHERE hold_v1.until_ms < excluded.until_ms""",
                (self.account, group, until, source, method, seconds),
            )
            trial = db.execute(
                """SELECT trial_id,generation FROM pool_trial_v1
                   WHERE account=? AND state IN ('scheduled','running')
                     AND activate_ms<=? AND ?<expires_ms""",
                (self.account, now, now),
            ).fetchone()
            if event_id is not None:
                db.execute(
                    """UPDATE pool_rpc_event_v1 SET outcome=?,wait_s=?,hold_until_ms=?,occurred_ms=?
                       WHERE account=? AND id=?""",
                    (source, seconds, until, now, self.account, event_id),
                )
            else:
                trial_id, generation = trial or (None, None)
                db.execute(
                    """INSERT INTO pool_rpc_event_v1
                       (id,account,trial_id,generation,occurred_ms,method,origin,outcome,wait_s,hold_until_ms)
                       VALUES (?,?,?,?,?,?,?,?,?,?)""",
                    (
                        str(uuid4()),
                        self.account,
                        trial_id,
                        generation,
                        now,
                        method,
                        origin,
                        source,
                        seconds,
                        until,
                    ),
                )
            if trial is not None:
                stop_until = max(until, (now // DAY_MS + 1) * DAY_MS)
                db.execute(
                    """INSERT INTO pool_trial_stop_v1 VALUES (?,?,?,?)
                       ON CONFLICT(account) DO UPDATE SET trial_id=excluded.trial_id,
                         generation=excluded.generation, until_ms=MAX(until_ms,excluded.until_ms)""",
                    (self.account, *trial, stop_until),
                )
            db.commit()
        return retry("hold", until)


@dataclass(frozen=True)
class DialogInventory:
    groups: int
    pool_rooms: tuple[types.Channel, ...]


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
        self.latest_budget: tuple[int, int] | None = None
        self.mutation_lock = asyncio.Lock()
        self.inventory_observation: tuple[float, DialogInventory] | None = None
        self.inventory_scan: asyncio.Task[DialogInventory] | None = None
        self.inventory_scan_failed = False

    async def identity(self) -> bool:
        with raise_flood_waits():
            user = await self.client.get_me()
        validate_identity(user, int(self.account))
        return bool(getattr(user, "premium", False))

    def record_read_wait(self, error: Exception) -> dict[str, Any] | None:
        request = getattr(error, "request", None)
        methods = {
            functions.channels.GetFullChannelRequest: "channels.getFullChannel",
            functions.channels.GetParticipantRequest: "channels.getParticipant",
            functions.messages.GetDialogsRequest: "messages.getDialogs",
            functions.users.GetUsersRequest: "users.getUsers",
        }
        method = methods.get(type(request), "unknown")
        return self.journal.wait(method, f"method:{method}", error)

    async def dialogs(self, limit: int, *, wait_for_capacity: bool = False):
        method = "messages.getDialogs"
        group = f"method:{method}"
        if self.journal.held(group) is not None:
            raise BrokerError("account_held")
        dialogs = self.client.iter_dialogs(limit=limit + 1)
        if isinstance(dialogs, RequestIter):
            dialogs.wait_time = DIALOG_PAGE_SPACING_SECONDS
        iterator = aiter(dialogs)
        deadline = monotonic() + CAPACITY_READ_SECONDS
        wait_budget = CAPACITY_MAX_WAIT_SECONDS if wait_for_capacity else 0
        while True:
            try:
                dialog = await anext(iterator)
            except StopAsyncIteration:
                return
            except Exception as error:
                if self.journal.wait(method, group, error) is None:
                    raise
                if isinstance(error, errors.FloodWaitError):
                    held_until = self.journal.held(group)
                    delay = (held_until - now_ms()) / 1000 + 1 if held_until is not None else 0
                    if 0 < delay <= wait_budget and monotonic() + delay < deadline:
                        # Keep the same pagination cursor after the recorded wait.
                        await asyncio.sleep(delay)
                        wait_budget -= delay
                        if self.journal.held(group) is None:
                            continue
                raise BrokerError("account_held") from None
            if limit == 0:
                raise BrokerError("group_count_unavailable")
            limit -= 1
            yield dialog

    async def scan_inventory(self) -> DialogInventory:
        groups = 0
        pool_rooms = []
        with raise_flood_waits():
            async for dialog in self.dialogs(DIALOG_SCAN_LIMIT, wait_for_capacity=True):
                entity = dialog.entity
                if isinstance(entity, types.Channel) and not entity.left:
                    groups += 1
                    if entity.creator and entity.title == TITLE:
                        pool_rooms.append(entity)
        return DialogInventory(groups, tuple(pool_rooms))

    def cached_inventory(self) -> DialogInventory | None:
        if self.inventory_observation is not None:
            observed, inventory = self.inventory_observation
            if monotonic() - observed <= CAPACITY_OBSERVATION_SECONDS:
                return inventory
        return None

    def start_inventory_scan(self) -> asyncio.Task[DialogInventory]:
        if self.inventory_scan is None:
            self.inventory_scan = asyncio.create_task(self.record_inventory())
            self.inventory_scan.add_done_callback(self.finish_inventory_scan)
        return self.inventory_scan

    def finish_inventory_scan(self, scan: asyncio.Task[DialogInventory]) -> None:
        if self.inventory_scan is scan:
            self.inventory_scan = None
        if not scan.cancelled() and (error := scan.exception()) is not None:
            self.inventory_scan_failed = True
            detail = error.code if isinstance(error, BrokerError) else type(error).__name__
            print(f"Chat pool inventory scan failed ({detail})", file=sys.stderr)

    def invalidate_inventory(self) -> None:
        self.inventory_observation = None
        self.inventory_scan_failed = False
        if self.inventory_scan is not None:
            self.inventory_scan.cancel()
            self.inventory_scan = None

    async def read_inventory(self) -> DialogInventory:
        if (inventory := self.cached_inventory()) is not None:
            return inventory
        # A timed-out health check does not discard the in-progress scan.
        return await asyncio.shield(self.start_inventory_scan())

    def inventory_for_action(
        self, *, consume_failure: bool = True
    ) -> DialogInventory | dict[str, Any]:
        if (hold := self.journal.held("method:messages.getDialogs")) is not None:
            return retry("hold", hold)
        if (inventory := self.cached_inventory()) is not None:
            self.inventory_scan_failed = False
            return inventory
        if consume_failure and self.inventory_scan_failed:
            self.inventory_scan_failed = False
            self.start_inventory_scan()
            return retry("room_observation_failed")
        self.start_inventory_scan()
        return retry("inventory_refresh_pending", now_ms() + INVENTORY_RETRY_MS)

    async def record_inventory(self) -> DialogInventory:
        inventory = await asyncio.wait_for(self.scan_inventory(), CAPACITY_READ_SECONDS)
        self.inventory_observation = (monotonic(), inventory)
        return inventory

    async def dispatch(
        self, body: dict[str, Any], peer_connected: PeerConnected | None = None
    ) -> dict[str, Any]:
        if body["op"] == "trial.configure":
            if set(body) != {"v", "id", "op", "accountId", "trial"}:
                raise BrokerError("invalid_request")
            async with self.mutation_lock:
                self.journal.configure_trial(body["trial"])
                if body["trial"] is not None:
                    self.latest_budget = None
            return {"configured": True}
        if body["op"] == "trial.events":
            if set(body) != {"v", "id", "op", "accountId", "trialId", "after"}:
                raise BrokerError("invalid_request")
            after = body["after"]
            try:
                trial_id = str(UUID(body["trialId"]))
                if type(after) is not int or not 0 <= after < 2**63:
                    raise ValueError
            except (TypeError, AttributeError, ValueError):
                raise BrokerError("invalid_request") from None
            return {"events": self.journal.trial_events(trial_id, after)}
        action = Action.parse(body)
        if body["op"] == "fill.observe":
            return await self.observe(action)
        if action.expires > now_ms():
            self.latest_budget = (action.cap, action.expires)
        return await self.execute(action, peer_connected)

    async def broker_snapshot(self) -> dict[str, Any]:
        return await self.snapshot(wait_for_inventory=False)

    async def snapshot(self, *, wait_for_inventory: bool = True) -> dict[str, Any]:
        try:
            if wait_for_inventory:
                inventory = await self.read_inventory()
            else:
                inventory = self.inventory_for_action(consume_failure=False)
                if isinstance(inventory, dict):
                    raise BrokerError(inventory["errorCode"])
            premium = await asyncio.wait_for(self.identity(), MAX_RPC_SECONDS)
            return {
                "accountId": self.account,
                "groupCount": inventory.groups,
                "telegramPremium": premium,
                **self.journal.snapshot(now_ms()),
            }
        except BrokerError:
            raise
        except Exception as error:
            if self.record_read_wait(error) is not None:
                raise BrokerError("hold") from None
            frame = traceback.extract_tb(error.__traceback__)[-1]
            print(
                f"Chat pool capacity read failed ({type(error).__name__} at "
                f"{os.path.basename(frame.filename)}:{frame.lineno})",
                file=sys.stderr,
            )
            raise BrokerError("group_count_unavailable") from None

    async def read_room(self, selector: hints.EntityLike, ref: str, *, strict: bool = False):
        peer = await self.client.get_input_entity(selector)
        if not isinstance(peer, types.InputPeerChannel):
            if strict:
                raise BrokerError("room_observation_unavailable")
            return None
        if strict and isinstance(selector, types.Channel) and peer.channel_id != selector.id:
            raise BrokerError("room_observation_unavailable")
        channel = types.InputChannel(peer.channel_id, peer.access_hash)
        full = await self.client(functions.channels.GetFullChannelRequest(channel))
        rooms = [
            chat
            for chat in full.chats
            if isinstance(chat, types.Channel) and chat.id == peer.channel_id
        ]
        if len(rooms) != 1:
            if strict:
                raise BrokerError("room_observation_unavailable")
            return None
        room = rooms[0]
        exact_ref = (
            full.full_chat.about.endswith(f"Ref: {ref}")
            and re.search(r"(?<![\w])Ref: [a-f0-9]{8}$", full.full_chat.about) is not None
        )
        if (
            not room.creator
            or not room.megagroup
            or room.forum
            or room.broadcast
            or room.title != TITLE
        ):
            if strict and exact_ref:
                raise BrokerError("pool_candidate_mismatch")
            return None
        if not exact_ref:
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
            for method in ("users.getUsers", "channels.getFullChannel"):
                if (hold := self.journal.held(f"method:{method}")) is not None:
                    return retry("hold", hold)
            await asyncio.wait_for(self.identity(), MAX_RPC_SECONDS)
            with raise_flood_waits():
                return await self._observe(action)
        except Exception as error:
            if (hold := self.record_read_wait(error)) is not None:
                return hold
            if isinstance(error, BrokerError) and error.code == "pool_candidate_mismatch":
                return retry(error.code)
            detail = error.code if isinstance(error, BrokerError) else type(error).__name__
            print(f"Chat pool observation failed ({action.step}, {detail})", file=sys.stderr)
            return retry("room_observation_failed")

    async def _observe(self, action: Action) -> dict[str, Any]:
        if action.step == "create":
            inventory = self.inventory_for_action()
            if isinstance(inventory, dict):
                return inventory
            matches = await asyncio.wait_for(
                self.matching_rooms(inventory, action.ref), MAX_RPC_SECONDS
            )
            if len(matches) > 1:
                return retry("duplicate_pool_reference")
            if matches:
                return {"outcome": "succeeded", "chatId": matches.pop()}
            # A later absence result must use a distinct complete scan.
            self.invalidate_inventory()
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

    async def matching_rooms(self, inventory: DialogInventory, ref: str) -> set[str]:
        matches = set()
        for entity in inventory.pool_rooms:
            if await self.read_room(entity, ref, strict=True):
                matches.add(str(utils.get_peer_id(entity)))
        return matches

    async def execute(
        self,
        action: Action,
        peer_connected: PeerConnected | None = None,
    ) -> dict[str, Any]:
        async with self.mutation_lock:
            return await self.execute_locked(action, peer_connected)

    async def execute_locked(
        self,
        action: Action,
        peer_connected: PeerConnected | None = None,
    ) -> dict[str, Any]:
        if action.reconcile or action.step == "verify":
            return retry("invalid_action_binding")
        dispatched = False
        try:
            await asyncio.wait_for(self.identity(), MAX_RPC_SECONDS)
            with raise_flood_waits():
                refusal = self.journal.guard(action, now_ms(), reserve=False)
                if refusal is not None:
                    return refusal
                if action.step == "create":
                    inventory = self.inventory_for_action()
                    if isinstance(inventory, dict):
                        return inventory
                    if inventory.groups >= action.groups:
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
                method, group = STEPS[action.step]
                event_id = None

                async def send():
                    nonlocal dispatched, event_id
                    if not await still_connected(peer_connected):
                        return None, retry("broker_peer_gone")
                    refusal = self.journal.guard(action, now_ms(), reserve=True)
                    if refusal is not None:
                        return None, refusal
                    if action.step in ("create", "add_bot"):
                        event_id = self.journal.trial_attempt(action, method)
                    if now_ms() >= action.expires:
                        self.journal.finish_trial_attempt(event_id, "reserved")
                        return None, retry("lease_unavailable")
                    if action.step == "create":
                        self.invalidate_inventory()
                    dispatched = True
                    return await self.client(request), None

                remaining = (action.expires - now_ms()) / 1000
                if remaining <= 0:
                    return retry("lease_unavailable")
                try:
                    with single_request():
                        answer, refusal = await asyncio.wait_for(
                            send(), min(MAX_RPC_SECONDS, remaining)
                        )
                except Exception as error:
                    if not dispatched:
                        return retry("room_preflight_unavailable")
                    return self.journal.wait(
                        method,
                        group,
                        error,
                        origin="managed" if event_id else "other",
                        event_id=event_id,
                    ) or {"outcome": "uncertain"}
                if refusal is not None:
                    return refusal
                if action.step == "create":
                    rooms = [
                        room
                        for room in getattr(answer, "chats", ())
                        if isinstance(room, types.Channel) and room.title == TITLE and room.creator
                    ]
                    if len(rooms) == 1 and await self.read_room(rooms[0], action.ref):
                        self.journal.finish_trial_attempt(event_id, "succeeded")
                        return {"outcome": "succeeded", "chatId": str(utils.get_peer_id(rooms[0]))}
                else:
                    evidence = await self._observe(action)
                    if evidence["outcome"] == "succeeded":
                        self.journal.finish_trial_attempt(event_id, "succeeded")
                        return evidence
        except Exception as error:
            self.record_read_wait(error)
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
