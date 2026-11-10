"""Replay sanitized handler results and synthetic RPC errors without networking.

Observed envelopes retain their provenance and SDK schema. Handler tests use
synthetic objects and compare only the text those handlers produce. They do
not establish the source revision or transport behavior of the live process.
"""

from datetime import datetime, timezone
from pathlib import Path
from types import SimpleNamespace
from typing import Literal

import pytest
from mcp.types import CallToolResult, TextContent
from pydantic import BaseModel
from telethon import errors
from telethon.tl.types import RpcError, User, PeerUser
from telethon.tl.custom import Message

from telegram_mcp import runtime
from telegram_mcp.request_policy import (
    CooldownTracker,
    RequestLimited,
    format_request_error,
    run_limited,
)
from telegram_mcp.tools import messages, profile

FIXTURES = Path(__file__).parent / "fixtures"
NOW = 1_800_000_000.0


class ObservedCase(BaseModel):
    name: str
    tool: Literal["get_me", "get_messages"]
    arguments: dict[str, int]
    response: CallToolResult


class ObservedFixture(BaseModel):
    provenance: dict[str, object]
    cases: list[ObservedCase]


class RpcCase(BaseModel):
    wire_code: int
    wire_message: str
    class_name: str
    kind: Literal["flood_wait", "premium_flood_wait", "slow_mode"] | None = None
    seconds: int | None = None
    retry_at: str | None = None
    request: str | None = None
    scope: str | None = None
    unrecorded_scope: str | None = None


class RpcFixture(BaseModel):
    provenance: dict[str, object]
    cases: list[RpcCase]
    generic_message_template: str


OBSERVED = ObservedFixture.model_validate_json(
    (FIXTURES / "anonymized_live_success.json").read_text()
)
RPC = RpcFixture.model_validate_json((FIXTURES / "synthetic_rpc_errors.json").read_text())


def observed_text(case: ObservedCase) -> str:
    assert case.response.isError is False
    assert len(case.response.content) == 1
    content = case.response.content[0]
    assert isinstance(content, TextContent)
    assert case.response.structuredContent == {"result": content.text}
    assert content.annotations is not None
    assert content.annotations.audience == ["user"]
    return content.text


@pytest.mark.asyncio
async def test_identity_handler_matches_anonymized_observed_text(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    user = User(
        id=900000001,
        first_name="Example User",
        username="example_user",
        phone="15550000001",
    )

    async def read_profile(client: object) -> User:
        return user

    monkeypatch.setattr(profile, "get_client", lambda account: object())
    monkeypatch.setattr(profile, "read_account_profile", read_profile)
    assert await profile.get_me() == observed_text(OBSERVED.cases[0])


@pytest.mark.asyncio
async def test_messages_handler_matches_anonymized_observed_text(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    message = Message(
        id=101,
        peer_id=PeerUser(user_id=900000002),
        date=datetime(2000, 1, 1, tzinfo=timezone.utc),
        message="Example message",
    )
    message._sender = User(id=900000003, first_name="Example Bot")
    calls: list[tuple[int, int]] = []

    class Client:
        async def get_messages(
            self, entity: object, *, limit: int, add_offset: int
        ) -> list[Message]:
            calls.append((limit, add_offset))
            return [message]

    async def resolve(chat_id: int, client: object) -> object:
        assert chat_id == 900000002
        return object()

    monkeypatch.setattr(messages, "get_client", lambda account: Client())
    monkeypatch.setattr(messages, "resolve_entity", resolve)
    assert await messages.get_messages(chat_id=900000002, page=1, page_size=1) == observed_text(
        OBSERVED.cases[1]
    )
    assert calls == [(1, 0)]


@pytest.mark.parametrize("case", RPC.cases, ids=lambda case: case.wire_message)
@pytest.mark.asyncio
async def test_synthetic_wire_errors_match_mapping_and_current_formatter(
    case: RpcCase, monkeypatch: pytest.MonkeyPatch
) -> None:
    error = errors.rpc_message_to_error(
        RpcError(error_code=case.wire_code, error_message=case.wire_message),
        request=None,
    )
    assert type(error).__name__ == case.class_name
    monkeypatch.setattr(runtime, "time", SimpleNamespace(time=lambda: NOW))
    rendered = runtime.log_and_format_error("get_chat", error, chat_id=900000002)
    if case.kind is None:
        assert rendered == RPC.generic_message_template.format(
            process_hash=f"{abs(hash('get_chat')) % 1000:03d}"
        )
        return

    assert case.seconds is not None and case.request is not None
    assert case.scope is not None and case.unrecorded_scope is not None
    import json

    chat_id = 900000002 if case.kind == "slow_mode" else None
    expected = {
        "type": case.kind,
        "request": "get_chat",
        "method": None,
        "chat_id": chat_id,
        "wait_seconds": case.seconds,
        "retry_at": case.retry_at,
        "source": "telegram",
        "recorded": False,
        "message": f"{case.unrecorded_scope}. Wait {case.seconds} s. No cooldown was recorded. Earlier steps of get_chat may have taken effect; check state before retrying a write.",
    }
    assert json.loads(rendered) == {"error": expected}

    tracker = CooldownTracker(lambda: NOW)
    client = object()
    calls = 0

    async def operation() -> None:
        nonlocal calls
        calls += 1
        raise error

    with pytest.raises(RequestLimited) as caught:
        await run_limited(
            client,
            case.request,
            operation,
            timeout=30.0,
            chat_id=chat_id,
            tracker=tracker,
        )
    expected.update(
        request=case.request,
        recorded=True,
        message=f"{case.scope}. Wait {case.seconds} s. The server refuses matching requests until retry_at.",
    )
    assert json.loads(format_request_error(caught.value)) == {"error": expected}
    with pytest.raises(RequestLimited) as cooldown:
        await run_limited(
            client,
            case.request,
            operation,
            timeout=30.0,
            chat_id=chat_id,
            tracker=tracker,
        )
    assert cooldown.value.limit.source == "cooldown"
    assert calls == 1
