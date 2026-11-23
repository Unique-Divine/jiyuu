"""Encrypted Telegram MCP profile storage and a host-local authorization lock."""

import argparse
import asyncio
from contextlib import contextmanager
from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
from typing import Protocol

from telethon import TelegramClient
from telethon.sessions import StringSession

MAX_PROFILE_BYTES = 4096
CUSTODY_ID = 8274304175
NAME = re.compile(r"[a-z0-9][a-z0-9_-]{0,47}")


class StoreError(Exception):
    """A safe diagnostic that can be shown without credential values."""


def valid_name(name: str) -> str:
    if NAME.fullmatch(name) is None:
        raise StoreError("Invalid stored profile name")
    return name


def _private_dir(path: Path) -> Path:
    path.mkdir(mode=0o700, exist_ok=True)
    metadata = path.lstat()
    if (
        not stat.S_ISDIR(metadata.st_mode)
        or metadata.st_uid != os.getuid()
        or stat.S_IMODE(metadata.st_mode) & 0o077
    ):
        raise StoreError("Telegram MCP state directory must be private and owned by this user")
    return path


@dataclass(frozen=True)
class State:
    tool: Path

    @classmethod
    def discover(cls) -> "State":
        override = os.environ.get("BOKU_HOME")
        base = Path(override) if override else Path.home() / ".boku"
        if not base.is_absolute():
            raise StoreError("BOKU_HOME must be an absolute path")
        _private_dir(base)
        tool = _private_dir(base / "telegram-mcp")
        for name in ("secrets", "logs", "run"):
            _private_dir(tool / name)
        return cls(tool)

    @property
    def socket(self) -> Path:
        return self.tool / "run" / "broker-v1.sock"


@dataclass(frozen=True, repr=False)
class Profile:
    api_id: int
    api_hash: str
    session_string: str
    user_id: int

    @classmethod
    def parse(cls, data: bytes) -> "Profile":
        if not data or len(data) > MAX_PROFILE_BYTES:
            raise StoreError("Invalid encrypted profile payload")
        try:
            body = json.loads(data)
            if not isinstance(body, dict):
                raise ValueError
            api_id = body.get("apiId")
            api_hash = body.get("apiHash")
            session_string = body.get("sessionString")
            user_id = body.get("userId")
            if (
                type(api_id) is not int
                or not 0 < api_id < 2**31
                or not isinstance(api_hash, str)
                or re.fullmatch(r"[a-fA-F0-9]{32}", api_hash) is None
                or not isinstance(session_string, str)
                or type(user_id) is not int
                or not 0 < user_id < 2**63
                or user_id == CUSTODY_ID
            ):
                raise ValueError
            if self_key := StringSession(session_string).auth_key:
                if len(self_key.key) == 256:
                    return cls(api_id, api_hash, session_string, user_id)
        except Exception:
            pass
        raise StoreError("Invalid encrypted Telegram MCP profile")

    def encoded(self) -> bytes:
        return json.dumps(
            {
                "apiId": self.api_id,
                "apiHash": self.api_hash,
                "sessionString": self.session_string,
                "userId": self.user_id,
            },
            separators=(",", ":"),
        ).encode()

    def session(self) -> StringSession:
        return StringSession(self.session_string)


class SessionIdentity(Protocol):
    id: int
    bot: bool | None


def validate_identity(user: SessionIdentity | None, expected: int) -> None:
    if user is None or user.id != expected or user.bot or expected == CUSTODY_ID:
        raise StoreError("Telegram authorization does not match the approved human profile")


def _helper(state: State, operation: str, name: str, data: bytes = b"") -> bytes:
    helper = shutil.which("local-secret-store")
    if helper is None:
        raise StoreError("Install the local-secret-store executable before using saved profiles")
    try:
        result = subprocess.run(
            [
                helper,
                "--store",
                os.fspath(state.tool / "secrets"),
                operation,
                f"telegram-{valid_name(name)}",
            ],
            input=data,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            timeout=15,
            check=True,
            env={},
        )
    except (OSError, subprocess.SubprocessError):
        raise StoreError("Encrypted profile operation failed") from None
    return result.stdout


def read_profile(state: State, name: str) -> Profile:
    return Profile.parse(_helper(state, "get", name))


@contextmanager
def lock_authorization(state: State, session: StringSession):
    """Prevent use of the same saved authorization by another local broker."""
    import fcntl

    key = session.auth_key
    if key is None:
        raise StoreError("Telegram profile has no authorization")
    digest = hashlib.sha256(key.key).hexdigest()
    path = state.tool / "run" / f"authorization-{digest}.lock"
    fd = os.open(path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        info = os.fstat(fd)
        if (
            not stat.S_ISREG(info.st_mode)
            or info.st_uid != os.getuid()
            or stat.S_IMODE(info.st_mode) & 0o077
        ):
            raise StoreError("Telegram authorization lock is not private")
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise StoreError("Telegram authorization is already in use by this broker") from None
        yield
    finally:
        os.close(fd)


async def import_profile(state: State, name: str, profile: Profile) -> None:
    valid_name(name)
    session = profile.session()
    with lock_authorization(state, session):
        client = TelegramClient(session, profile.api_id, profile.api_hash, request_retries=0)
        try:

            async def check() -> None:
                await client.connect()
                user = await client.get_me()
                if user is None or not hasattr(user, "bot"):
                    raise StoreError("Telegram user identity could not be verified")
                validate_identity(user, profile.user_id)

            await asyncio.wait_for(check(), timeout=20)
        except Exception:
            raise StoreError("Telegram authorization could not be verified") from None
        finally:
            await asyncio.wait_for(client.disconnect(), timeout=5)
        _helper(state, "put", name, profile.encoded())


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Verify an existing Telegram user profile and store it encrypted"
    )
    parser.add_argument("name")
    args = parser.parse_args()
    try:
        if not stat.S_ISFIFO(os.fstat(sys.stdin.fileno()).st_mode):
            raise StoreError("Supply the profile JSON through a private input pipe")
        profile = Profile.parse(sys.stdin.buffer.read(MAX_PROFILE_BYTES + 1))
        asyncio.run(import_profile(State.discover(), args.name, profile))
    except StoreError as error:
        raise SystemExit(str(error)) from None
    except (OSError, asyncio.TimeoutError):
        raise SystemExit("Telegram profile import could not finish") from None
    print("Telegram profile verified and stored")


if __name__ == "__main__":
    main()
