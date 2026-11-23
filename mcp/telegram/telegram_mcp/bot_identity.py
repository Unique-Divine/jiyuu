"""Pin the managed fill bot and keep staging profiles apart from production."""

from dataclasses import dataclass
import os
from pathlib import Path
import re
from typing import Mapping

from telegram_mcp.session_store import CUSTODY_ID, State, StoreError

PRODUCTION_BOT_ID = 8983431101
PRODUCTION_BOT_USERNAME = "SaiTeam_bot"
PRODUCTION_IDENTITIES = frozenset({PRODUCTION_BOT_ID, CUSTODY_ID, 1912700300, 7930936303})
BOT_ID, CUSTODY, OWNER, USERNAME = (
    "SAI_STAGING_BOT_ID",
    "SAI_STAGING_CUSTODY_ID",
    "SAI_STAGING_OWNER_ID",
    "SAI_STAGING_BOT_USERNAME",
)
STAGE_VARIABLES = (BOT_ID, CUSTODY, OWNER, USERNAME)
TOKEN_VARIABLES = ("SDD_TELEGRAM_BOT_TOKEN", "SAI_BOT_TOKEN", "TELEGRAM_BOT_TOKEN")


@dataclass(frozen=True)
class BotIdentity:
    id: int = PRODUCTION_BOT_ID
    username: str = PRODUCTION_BOT_USERNAME


def for_broker(
    state: State,
    accounts: set[int],
    env: Mapping[str, str] = os.environ,
) -> BotIdentity:
    if not any(name in env for name in STAGE_VARIABLES):
        pin_environment(state, "production")
        return BotIdentity()
    root = env.get("BOKU_HOME")
    if (
        not root
        or not Path(root).is_absolute()
        or Path(root).resolve() == (Path.home() / ".boku").resolve()
    ):
        raise StoreError("Staging requires a separate explicit BOKU_HOME")
    pins: list[int] = []
    for key in (BOT_ID, CUSTODY, OWNER):
        raw = env.get(key, "")
        if re.fullmatch(r"[1-9][0-9]{0,18}", raw) is None:
            raise StoreError("Staging requires distinct disposable Telegram identities")
        number = int(raw)
        if number >= 2**63 or number in PRODUCTION_IDENTITIES:
            raise StoreError("Staging refuses production Telegram identities")
        pins.append(number)
    bot, custody, owner = pins
    if len({bot, custody, owner}) != 3 or accounts != {owner}:
        raise StoreError("Staging broker requires exactly its pinned disposable owner profile")
    username = env.get(USERNAME, "")
    if (
        re.fullmatch(r"[a-zA-Z][a-zA-Z0-9_]{4,31}", username) is None
        or username.casefold() == PRODUCTION_BOT_USERNAME.casefold()
    ):
        raise StoreError("Staging requires a disposable bot username")
    for key in TOKEN_VARIABLES:
        token = env.get(key)
        if token:
            account, separator, secret = token.partition(":")
            if account != str(bot) or not separator or not secret:
                raise StoreError("Staging refuses a token outside its pinned bot identity")
    pin_environment(state, "staging")
    return BotIdentity(bot, username)


def pin_environment(state: State, mode: str) -> None:
    marker = state.tool / "run" / "broker-environment"
    try:
        descriptor = os.open(marker, os.O_NOFOLLOW | os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    except FileExistsError:
        with os.fdopen(
            os.open(marker, os.O_NOFOLLOW | os.O_RDONLY | os.O_NONBLOCK), "rb"
        ) as existing:
            if existing.read(32) != f"{mode}\n".encode():
                raise StoreError("Telegram broker state belongs to a different environment")
    else:
        with os.fdopen(descriptor, "wb") as destination:
            destination.write(f"{mode}\n".encode())
