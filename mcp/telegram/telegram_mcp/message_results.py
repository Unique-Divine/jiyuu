"""Check Telethon's scalar/collection message variants at the SDK boundary."""

from typing import Awaitable

from telethon.tl.custom import Message


class MessagePage(list[Message]):
    """Available messages, retaining Telegram's total rather than the page length."""

    total: int | None

    def __init__(self, messages: list[Message], total: int | None):
        super().__init__(messages)
        self.total = total


async def message_page(response: Awaitable[object]) -> MessagePage:
    result = await response
    if not isinstance(result, list):
        raise TypeError("Telegram returned a scalar for a collection message request.")
    messages: list[Message] = []
    for item in result:
        # Explicit ID lists contain None for IDs Telegram could not retrieve.
        if item is None:
            continue
        if not isinstance(item, Message):
            raise TypeError("Telegram returned an unexpected message element.")
        messages.append(item)
    total = getattr(result, "total", None)
    if total is not None and (isinstance(total, bool) or not isinstance(total, int)):
        raise TypeError("Telegram returned an invalid message total.")
    return MessagePage(messages, total)


async def single_message(response: Awaitable[object]) -> Message | None:
    result = await response
    if result is None or isinstance(result, Message):
        return result
    # Telethon treats ids=0 as an ordinary collection request. Never pick an
    # unrelated recent message to satisfy that scalar lookup.
    raise TypeError("Telegram returned a collection for a scalar message request.")
