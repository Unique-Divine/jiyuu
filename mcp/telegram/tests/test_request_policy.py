"""Flood waits, slow mode, and timeouts become typed, recorded limits."""

import asyncio
import json

import pytest
from telethon import errors

from telegram_mcp.request_policy import (
    CooldownTracker,
    RequestLimited,
    RequestTimedOut,
    format_request_error,
    limit_from_error,
    run_limited,
)


class Clock:
    def __init__(self, now=1_800_000_000.0):
        self.now = now

    def __call__(self):
        return self.now


def flood(seconds):
    return errors.FloodWaitError(request=None, capture=seconds)


def slow_mode(seconds):
    return errors.SlowModeWaitError(request=None, capture=seconds)


def raising(error, calls):
    async def operation():
        calls.append("request")
        raise error

    return operation


def test_flood_wait_names_seconds_and_retry_time():
    limit = limit_from_error(flood(420), "get_dialogs", now=1_800_000_000.0)
    payload = limit.to_payload()["error"]

    assert payload["type"] == "flood_wait"
    assert payload["request"] == "get_dialogs"
    assert payload["wait_seconds"] == 420
    assert payload["retry_at"] == "2027-01-15T08:07:00Z"
    assert payload["chat_id"] is None
    assert payload["source"] == "telegram"


def test_wait_error_kinds_are_distinguished():
    premium = errors.FloodPremiumWaitError(request=None, capture=5)

    assert limit_from_error(slow_mode(30), "send_message", 0.0, chat_id=-7).kind == "slow_mode"
    assert limit_from_error(premium, "download_media", 0.0).kind == "premium_flood_wait"
    # Flood waits are per account and method, so they never carry a chat.
    assert limit_from_error(flood(30), "send_message", 0.0, chat_id=-7).chat_id is None


def test_wait_seconds_from_telegram_are_validated():
    error = flood(10)
    error.seconds = "10"

    with pytest.raises(ValueError, match="invalid seconds"):
        limit_from_error(error, "get_dialogs", 0.0)

    error.seconds = 0
    assert limit_from_error(error, "get_dialogs", 0.0).wait_seconds == 1


@pytest.mark.asyncio
async def test_flood_wait_is_recorded_and_refuses_matching_requests():
    clock = Clock()
    tracker = CooldownTracker(clock)
    client = object()
    calls = []

    with pytest.raises(RequestLimited) as first:
        await run_limited(
            client, "get_dialogs", raising(flood(60), calls), timeout=30, tracker=tracker
        )
    assert first.value.limit.source == "telegram"

    clock.now += 45
    with pytest.raises(RequestLimited) as second:
        await run_limited(
            client, "get_dialogs", raising(flood(60), calls), timeout=30, tracker=tracker
        )

    assert calls == ["request"]
    assert second.value.limit.source == "cooldown"
    assert second.value.limit.wait_seconds == 15
    assert second.value.limit.retry_at == first.value.limit.retry_at


@pytest.mark.asyncio
async def test_cooldown_is_scoped_to_client_and_request_and_expires():
    clock = Clock()
    tracker = CooldownTracker(clock)
    client, other_client = object(), object()

    async def ok():
        return "ok"

    with pytest.raises(RequestLimited):
        await run_limited(
            client, "get_dialogs", raising(flood(60), []), timeout=1, tracker=tracker
        )

    assert await run_limited(client, "get_me", ok, timeout=1, tracker=tracker) == "ok"
    assert await run_limited(other_client, "get_dialogs", ok, timeout=1, tracker=tracker) == "ok"

    clock.now += 60
    assert await run_limited(client, "get_dialogs", ok, timeout=1, tracker=tracker) == "ok"


@pytest.mark.asyncio
async def test_slow_mode_blocks_only_posts_to_that_chat():
    tracker = CooldownTracker(Clock())
    client = object()

    async def ok():
        return "sent"

    with pytest.raises(RequestLimited) as caught:
        await run_limited(
            client,
            "send_message",
            raising(slow_mode(30), []),
            timeout=None,
            chat_id=-100,
            tracker=tracker,
        )
    assert caught.value.limit.kind == "slow_mode"
    assert caught.value.limit.chat_id == -100

    # Another chat, and other methods posting elsewhere, are unaffected.
    sent = await run_limited(
        client, "send_message", ok, timeout=None, chat_id=-200, tracker=tracker
    )
    assert sent == "sent"

    # Any method posting into the slow-mode chat is refused.
    with pytest.raises(RequestLimited) as refused:
        await run_limited(client, "send_file", ok, timeout=None, chat_id=-100, tracker=tracker)
    assert refused.value.limit.kind == "slow_mode"
    assert refused.value.limit.request == "send_file"


@pytest.mark.asyncio
async def test_timeout_is_typed_and_cancels_the_request():
    cancelled = asyncio.Event()

    async def stalled():
        try:
            await asyncio.Event().wait()
        finally:
            cancelled.set()

    with pytest.raises(RequestTimedOut) as caught:
        await run_limited(
            object(),
            "get_dialogs",
            stalled,
            timeout=0.01,
            timeout_hint="Retry with a smaller page_size.",
            tracker=CooldownTracker(Clock()),
        )

    assert cancelled.is_set()
    payload = json.loads(format_request_error(caught.value))["error"]
    assert payload["type"] == "timeout"
    assert payload["timeout_seconds"] == 0.01
    assert payload["message"].endswith("Retry with a smaller page_size.")


@pytest.mark.asyncio
async def test_other_rpc_errors_pass_through_unrecorded():
    tracker = CooldownTracker(Clock())
    client = object()
    error = errors.ChatWriteForbiddenError(request=None)

    with pytest.raises(errors.ChatWriteForbiddenError):
        await run_limited(client, "send_message", raising(error, []), timeout=1, tracker=tracker)

    assert tracker.active(id(client), "send_message") is None


@pytest.mark.parametrize("timeout", [None, 1.0])
@pytest.mark.asyncio
async def test_operation_timeout_is_not_a_policy_timeout(timeout):
    tracker = CooldownTracker(Clock())
    client = object()
    error = asyncio.TimeoutError("operation's own deadline")
    calls = []
    with pytest.raises(asyncio.TimeoutError) as caught:
        await run_limited(
            client, "send_message", raising(error, calls), timeout=timeout, tracker=tracker
        )
    assert caught.value is error
    assert len(calls) == 1
    assert tracker.active(id(client), "send_message") is None


@pytest.mark.asyncio
async def test_caller_cancellation_awaits_child_cleanup_and_restores_context():
    from telegram_mcp.request_policy import _flood_sleep_override

    started = asyncio.Event()
    cleaned = asyncio.Event()
    observed = []

    async def operation():
        observed.append(_flood_sleep_override.get())
        started.set()
        try:
            await asyncio.Event().wait()
        finally:
            await asyncio.sleep(0)
            cleaned.set()

    async def call():
        try:
            await run_limited(object(), "get_dialogs", operation, timeout=30)
        finally:
            observed.append(_flood_sleep_override.get())

    task = asyncio.create_task(call())
    await started.wait()
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert cleaned.is_set()
    assert observed == [0, None]
    assert _flood_sleep_override.get() is None


@pytest.mark.asyncio
async def test_policy_expiration_awaits_cleanup_and_does_not_retry():
    from telegram_mcp.request_policy import _flood_sleep_override

    calls = 0
    cleaned = asyncio.Event()

    async def operation():
        nonlocal calls
        calls += 1
        assert _flood_sleep_override.get() == 0
        try:
            await asyncio.Event().wait()
        finally:
            await asyncio.sleep(0)
            cleaned.set()

    with pytest.raises(RequestTimedOut):
        await run_limited(object(), "get_dialogs", operation, timeout=0.01)
    assert calls == 1
    assert cleaned.is_set()
    assert _flood_sleep_override.get() is None
