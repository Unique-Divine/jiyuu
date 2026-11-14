"""Actual SDK messages exercise scalar and page result contracts."""

import pytest
from telethon.helpers import TotalList
from telethon.tl.custom import Message
from telethon.tl.types import PeerUser

from telegram_mcp.message_results import message_page, single_message


def message(number=1):
    return Message(id=number, peer_id=PeerUser(user_id=123), message="example")


async def response(value: object) -> object:
    return value


@pytest.mark.asyncio
async def test_page_keeps_remote_total_and_skips_missing_id_placeholders():
    item = message()
    result = TotalList([item, None])
    result.total = 40
    page = await message_page(response(result))
    assert page == [item]
    assert page.total == 40
    assert page[0] is item


@pytest.mark.asyncio
async def test_plain_collection_has_no_invented_remote_total():
    page = await message_page(response([message()]))
    assert page.total is None
    assert len(page) == 1
    assert await message_page(response([])) == []


@pytest.mark.parametrize("value", [None, message(), [object()]])
@pytest.mark.asyncio
async def test_page_rejects_wrong_response_variants(value):
    with pytest.raises(TypeError):
        await message_page(response(value))


@pytest.mark.asyncio
async def test_scalar_preserves_message_and_missing_result():
    item = message()
    assert await single_message(response(item)) is item
    assert await single_message(response(None)) is None


@pytest.mark.parametrize("value", [[], [message()], object()])
@pytest.mark.asyncio
async def test_scalar_rejects_collection_including_zero_id_response(value):
    with pytest.raises(TypeError):
        await single_message(response(value))
