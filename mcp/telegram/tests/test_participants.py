# Modified by Unique Divine in 2026: connection recovery and participant pagination fixes.
"""Participant pagination must work with Telethon's actual iterator signature."""

import json
from types import SimpleNamespace

import pytest

from telegram_mcp.tools import groups


@pytest.mark.asyncio
@pytest.mark.parametrize("page, expected", [(1, [1, 2]), (2, [3, 4]), (3, [5]), (4, [])])
async def test_participant_pages_do_not_pass_unsupported_offset(monkeypatch, page, expected):
    class Client:
        async def iter_participants(self, chat_id, *, limit):
            for number in range(1, min(limit, 5) + 1):
                yield SimpleNamespace(id=number, first_name=f"User {number}", last_name=None)

    async def connected(client):
        pass

    monkeypatch.setattr(groups, "get_client", lambda account=None: Client())
    monkeypatch.setattr(groups, "ensure_connected", connected)

    output = await groups.get_participants(chat_id=-123, page=page, page_size=2)
    result, _ = json.JSONDecoder().raw_decode(output)

    assert [row["id"] for row in result["results"]] == expected
