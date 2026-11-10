"""Typed Telegram request limits: flood waits, slow mode, and timeouts.

Telethon sleeps through flood waits up to the client's
``flood_sleep_threshold``. Clients built by this server use
``PolicyTelegramClient`` with ``CLIENT_FLOOD_SLEEP_THRESHOLD`` (10 s), so
transfers, paginated reads, and cache warming still absorb short waits.

A request with an outer timeout must not sleep inside it: the timeout would
cancel the sleep and the caller would see a generic timeout instead of the
wait. ``run_limited`` therefore lowers the threshold to 0 for the requests it
runs, through a context variable that only the calling task and tasks it
spawns see. ``PolicyTelegramClient.connect`` clears the variable, so
Telethon's background tasks keep the client default. Every wait inside raises, becomes a ``RequestLimit``, and is recorded as
a cooldown that refuses matching requests until it passes. Nothing here
reconnects: a wait is an API limit, not a transport failure.

Telegram scopes the two kinds of wait differently:

- ``FLOOD_WAIT_X`` applies to one request method on one account. The cooldown
  key is ``(client, request)``.
- ``SLOWMODE_WAIT_X`` applies to posting in one chat, whatever method posts.
  The cooldown key is ``(client, chat_id)``. Pass ``chat_id`` only for
  requests that post into that chat.
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import math
import time
from contextvars import ContextVar
from dataclasses import dataclass, replace
from datetime import datetime, timezone
from typing import (
    Awaitable,
    Callable,
    Dict,
    Generator,
    Literal,
    Optional,
    Tuple,
    TypeVar,
    TypedDict,
    Union,
)

from telethon import TelegramClient, errors

CLIENT_FLOOD_SLEEP_THRESHOLD = 10

LimitKind = Literal["flood_wait", "slow_mode", "premium_flood_wait"]
LimitSource = Literal["telegram", "cooldown"]
ChatKey = Union[int, str]
T = TypeVar("T")

_flood_sleep_override: ContextVar[Optional[int]] = ContextVar(
    "telegram_mcp_flood_sleep_override", default=None
)


class PolicyTelegramClient(TelegramClient):
    """TelegramClient whose flood-sleep threshold a task can lower to 0.

    Telethon reads ``self.flood_sleep_threshold`` in the task that awaits the
    request, so a context variable scopes the override to that task without
    affecting concurrent tool calls on the same client.
    """

    @property
    def flood_sleep_threshold(self):
        override = _flood_sleep_override.get()
        return self._flood_sleep_threshold if override is None else override

    @flood_sleep_threshold.setter
    def flood_sleep_threshold(self, value):
        self._flood_sleep_threshold = min(value or 0, 24 * 60 * 60)

    async def connect(self) -> None:
        """Connect with the override cleared.

        ``connect`` starts long-lived update and keepalive tasks, which copy
        the current context. A reconnect inside a bounded request (for
        example ``_switch_dc`` after a migrate error) must not hand them
        threshold 0 for the life of the client.
        """
        token = _flood_sleep_override.set(None)
        try:
            await super().connect()
        finally:
            _flood_sleep_override.reset(token)


@contextlib.contextmanager
def raise_flood_waits() -> Generator[None, None, None]:
    """Make Telethon raise every wait in this task instead of sleeping."""
    token = _flood_sleep_override.set(0)
    try:
        yield
    finally:
        _flood_sleep_override.reset(token)


class LimitErrorPayload(TypedDict):
    type: LimitKind
    request: str
    method: Optional[str]
    chat_id: Optional[ChatKey]
    wait_seconds: int
    retry_at: str
    source: LimitSource
    recorded: bool
    message: str


class LimitPayload(TypedDict):
    error: LimitErrorPayload


class TimeoutErrorPayload(TypedDict):
    type: Literal["timeout"]
    request: str
    timeout_seconds: float
    message: str


class TimeoutPayload(TypedDict):
    error: TimeoutErrorPayload


@dataclass(frozen=True)
class RequestLimit:
    """A wait Telegram imposed, or the remaining part of a recorded one.

    ``request`` is the policy's request name (``get_dialogs``,
    ``send_message``) for recorded limits, and the MCP tool name for waits
    reported by ``log_and_format_error``. ``method`` is the Telethon request
    class that waited, when known. ``recorded`` is False when no cooldown was
    stored, so nothing refuses a retry locally.
    """

    kind: LimitKind
    request: str
    wait_seconds: int
    retry_at: float
    chat_id: Optional[ChatKey] = None
    source: LimitSource = "telegram"
    method: Optional[str] = None
    recorded: bool = True

    def to_payload(self) -> LimitPayload:
        retry_at = datetime.fromtimestamp(self.retry_at, tz=timezone.utc)
        return {
            "error": {
                "type": self.kind,
                "request": self.request,
                "method": self.method,
                "chat_id": self.chat_id,
                "wait_seconds": self.wait_seconds,
                "retry_at": retry_at.isoformat().replace("+00:00", "Z"),
                "source": self.source,
                "recorded": self.recorded,
                "message": self.message(),
            }
        }

    def message(self) -> str:
        if self.kind == "slow_mode":
            where = "this chat" if self.chat_id is None else f"chat {self.chat_id}"
            scope = f"Slow mode in {where}"
        elif self.kind == "premium_flood_wait":
            scope = f"Telegram limited transfer speed for {self.request}"
        else:
            scope = f"Telegram rate-limited {self.request}"
        if self.method:
            scope += f" ({self.method})"
        wait = f"{scope}. Wait {self.wait_seconds} s."
        if self.recorded:
            return f"{wait} The server refuses matching requests until retry_at."
        return (
            f"{wait} No cooldown was recorded. Earlier steps of {self.request} "
            "may have taken effect; check state before retrying a write."
        )


@dataclass(frozen=True)
class RequestTimeout:
    """A request that did not finish within its time bound."""

    request: str
    timeout_seconds: float
    hint: str

    def to_payload(self) -> TimeoutPayload:
        return {
            "error": {
                "type": "timeout",
                "request": self.request,
                "timeout_seconds": self.timeout_seconds,
                "message": f"Telegram {self.request} timed out. {self.hint}",
            }
        }


class RequestLimited(Exception):
    """Raised when Telegram imposed a wait or a recorded cooldown is active."""

    def __init__(self, limit: RequestLimit):
        super().__init__(limit.message())
        self.limit = limit


class RequestTimedOut(Exception):
    """Raised when a bounded request exceeds its timeout."""

    def __init__(self, timeout: RequestTimeout):
        super().__init__(timeout.request)
        self.timeout = timeout


def format_request_error(error: Union[RequestLimited, RequestTimedOut]) -> str:
    """Return the JSON tool result for a typed request failure."""
    detail = error.limit if isinstance(error, RequestLimited) else error.timeout
    return json.dumps(detail.to_payload(), ensure_ascii=False)


def _wait_seconds(error: Exception) -> int:
    """Validate the wait Telethon parsed from Telegram's error."""
    seconds = getattr(error, "seconds", None)
    if isinstance(seconds, bool) or not isinstance(seconds, int) or seconds < 0:
        raise ValueError(f"Telegram wait has invalid seconds: {seconds!r}")
    # Telethon treats FLOOD_WAIT_0 as one second; match it.
    return max(seconds, 1)


def is_wait_error(error: BaseException) -> bool:
    """True for Telegram errors that carry a wait in seconds."""
    return isinstance(error, errors.FloodError) and hasattr(error, "seconds")


def _method_name(error: Exception) -> Optional[str]:
    request = getattr(error, "request", None)
    return None if request is None else type(request).__name__


def limit_from_error(
    error: Exception,
    request: str,
    now: float,
    chat_id: Optional[ChatKey] = None,
    recorded: bool = True,
) -> RequestLimit:
    """Convert a Telethon wait error into a ``RequestLimit``."""
    seconds = _wait_seconds(error)
    if isinstance(error, errors.SlowModeWaitError):
        kind: LimitKind = "slow_mode"
    elif isinstance(error, errors.FloodPremiumWaitError):
        kind = "premium_flood_wait"
    else:
        kind = "flood_wait"
    return RequestLimit(
        kind=kind,
        request=request,
        wait_seconds=seconds,
        retry_at=now + seconds,
        chat_id=chat_id if kind == "slow_mode" else None,
        method=_method_name(error),
        recorded=recorded,
    )


RequestCooldownKey = Tuple[int, Literal["request"], str]
ChatCooldownKey = Tuple[int, Literal["chat"], ChatKey]
CooldownKey = Union[RequestCooldownKey, ChatCooldownKey]


class CooldownTracker:
    """Remember Telegram waits per client and refuse requests they cover."""

    def __init__(self, clock: Callable[[], float] = time.time):
        self._clock = clock
        self._limits: Dict[CooldownKey, RequestLimit] = {}

    @staticmethod
    def _request_key(client_key: int, request: str) -> RequestCooldownKey:
        return (client_key, "request", request)

    @staticmethod
    def _chat_key(client_key: int, chat_id: ChatKey) -> ChatCooldownKey:
        return (client_key, "chat", chat_id)

    def active(
        self, client_key: int, request: str, chat_id: Optional[ChatKey] = None
    ) -> Optional[RequestLimit]:
        """Return the longest unexpired cooldown covering this request."""
        now = self._clock()
        keys: list[CooldownKey] = [self._request_key(client_key, request)]
        if chat_id is not None:
            keys.append(self._chat_key(client_key, chat_id))
        found: Optional[RequestLimit] = None
        for key in keys:
            limit = self._limits.get(key)
            if limit is None:
                continue
            if limit.retry_at <= now:
                del self._limits[key]
                continue
            if found is None or limit.retry_at > found.retry_at:
                found = limit
        if found is None:
            return None
        remaining = max(1, math.ceil(found.retry_at - now))
        return replace(found, request=request, wait_seconds=remaining, source="cooldown")

    def record(
        self,
        client_key: int,
        request: str,
        error: Exception,
        chat_id: Optional[ChatKey] = None,
    ) -> RequestLimit:
        """Store the wait carried by ``error`` and return it."""
        limit = limit_from_error(error, request, self._clock(), chat_id)
        if limit.kind == "slow_mode":
            if chat_id is not None:
                self._limits[self._chat_key(client_key, chat_id)] = limit
        else:
            self._limits[self._request_key(client_key, request)] = limit
        return limit

    def clear(self) -> None:
        self._limits.clear()


request_cooldowns = CooldownTracker()


def raise_if_cooling_down(
    client: object,
    request: str,
    chat_id: Optional[ChatKey] = None,
    tracker: Optional[CooldownTracker] = None,
) -> None:
    """Refuse a request covered by a recorded cooldown, before any network I/O."""
    tracker = tracker or request_cooldowns
    limit = tracker.active(id(client), request, chat_id)
    if limit is not None:
        raise RequestLimited(limit)


async def run_limited(
    client: object,
    request: str,
    operation: Callable[[], Awaitable[T]],
    *,
    timeout: Optional[float],
    timeout_hint: str = "Retry later.",
    chat_id: Optional[ChatKey] = None,
    tracker: Optional[CooldownTracker] = None,
    raise_waits: bool = True,
) -> T:
    """Run one Telegram request under the cooldown and timeout policy.

    Raises ``RequestLimited`` for an active cooldown or a Telegram wait, and
    ``RequestTimedOut`` when ``timeout`` elapses. With ``raise_waits`` (the
    default) Telethon raises every wait instead of sleeping, so a timeout
    never cancels a wait. Pass ``raise_waits=False`` for unbounded background
    reads that should absorb short waits; only longer waits are recorded.
    Pass ``timeout=None`` for writes, where a timeout cannot tell whether
    Telegram applied the write.
    """
    tracker = tracker or request_cooldowns
    raise_if_cooling_down(client, request, chat_id, tracker)
    scope = raise_flood_waits() if raise_waits else contextlib.nullcontext()
    try:
        with scope:
            if timeout is None:
                return await operation()
            return await asyncio.wait_for(operation(), timeout=timeout)
    except asyncio.TimeoutError as exc:
        raise RequestTimedOut(RequestTimeout(request, timeout, timeout_hint)) from exc
    except errors.FloodError as exc:
        if not is_wait_error(exc):
            raise
        limit = tracker.record(id(client), request, exc, chat_id)
        raise RequestLimited(limit) from exc
