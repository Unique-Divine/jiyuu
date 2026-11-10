"""Typed Telegram request limits: flood waits, slow mode, and timeouts.

Telethon sleeps through flood waits shorter than ``flood_sleep_threshold``.
An outer timeout can then cancel that sleep, and the caller sees a generic
timeout instead of Telegram's wait. Clients built by this server set the
threshold to ``FLOOD_SLEEP_THRESHOLD`` (0), so every wait raises. This module
converts the raised error into a ``RequestLimit``, records it as a cooldown,
and refuses matching requests until the cooldown passes. It never reconnects:
a wait is an API limit, not a transport failure.

Telegram scopes the two kinds of wait differently:

- ``FLOOD_WAIT_X`` applies to one request method on one account. The cooldown
  key is ``(client, request)``.
- ``SLOWMODE_WAIT_X`` applies to posting in one chat, whatever method posts.
  The cooldown key is ``(client, chat_id)``. Pass ``chat_id`` only for
  requests that post into that chat.
"""

from __future__ import annotations

import asyncio
import json
import math
import time
from dataclasses import dataclass, replace
from datetime import datetime, timezone
from typing import Awaitable, Callable, Dict, Literal, Optional, Tuple, TypeVar, Union

from telethon import errors

FLOOD_SLEEP_THRESHOLD = 0

LimitKind = Literal["flood_wait", "slow_mode", "premium_flood_wait"]
LimitSource = Literal["telegram", "cooldown"]
ChatKey = Union[int, str]
T = TypeVar("T")


@dataclass(frozen=True)
class RequestLimit:
    """A wait Telegram imposed, or the remaining part of a recorded one."""

    kind: LimitKind
    request: str
    wait_seconds: int
    retry_at: float
    chat_id: Optional[ChatKey] = None
    source: LimitSource = "telegram"

    def to_payload(self) -> dict:
        retry_at = datetime.fromtimestamp(self.retry_at, tz=timezone.utc)
        return {
            "error": {
                "type": self.kind,
                "request": self.request,
                "chat_id": self.chat_id,
                "wait_seconds": self.wait_seconds,
                "retry_at": retry_at.isoformat().replace("+00:00", "Z"),
                "source": self.source,
                "message": self.message(),
            }
        }

    def message(self) -> str:
        if self.kind == "slow_mode":
            scope = f"Slow mode in chat {self.chat_id}"
        elif self.kind == "premium_flood_wait":
            scope = f"Telegram limited transfer speed for {self.request}"
        else:
            scope = f"Telegram rate-limited {self.request}"
        return (
            f"{scope}. Wait {self.wait_seconds} s before retrying. "
            "Do not retry sooner; the server refuses matching requests until then."
        )


@dataclass(frozen=True)
class RequestTimeout:
    """A request that did not finish within its time bound."""

    request: str
    timeout_seconds: float
    hint: str

    def to_payload(self) -> dict:
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


def limit_from_error(
    error: Exception,
    request: str,
    now: float,
    chat_id: Optional[ChatKey] = None,
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
    )


CooldownKey = Tuple[int, str, Union[str, ChatKey]]


class CooldownTracker:
    """Remember Telegram waits per client and refuse requests they cover."""

    def __init__(self, clock: Callable[[], float] = time.time):
        self._clock = clock
        self._limits: Dict[CooldownKey, RequestLimit] = {}

    @staticmethod
    def _request_key(client_key: int, request: str) -> CooldownKey:
        return (client_key, "request", request)

    @staticmethod
    def _chat_key(client_key: int, chat_id: ChatKey) -> CooldownKey:
        return (client_key, "chat", chat_id)

    def active(
        self, client_key: int, request: str, chat_id: Optional[ChatKey] = None
    ) -> Optional[RequestLimit]:
        """Return the longest unexpired cooldown covering this request."""
        now = self._clock()
        keys = [self._request_key(client_key, request)]
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
) -> T:
    """Run one Telegram request under the cooldown and timeout policy.

    Raises ``RequestLimited`` for an active cooldown or a new Telegram wait,
    and ``RequestTimedOut`` when ``timeout`` elapses. Pass ``timeout=None``
    for writes, where a timeout cannot tell whether Telegram applied the write.
    """
    tracker = tracker or request_cooldowns
    raise_if_cooling_down(client, request, chat_id, tracker)
    try:
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
