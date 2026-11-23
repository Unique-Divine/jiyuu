"""Private versioned IPC for the Telegram MCP process."""

import argparse
import asyncio
from collections.abc import Awaitable, Callable, Mapping
from contextlib import asynccontextmanager
import json
import os
import re
import socket
import stat
import struct
import sys
from typing import Any

from telegram_mcp.session_store import State, StoreError, lock_file

MAX_FRAME = 16_384
READ_TIMEOUT = 5
REQUEST_TIMEOUT = 95
ID = re.compile(r"[a-zA-Z0-9._:-]{1,96}")
OPERATIONS = frozenset({"account.snapshot", "fill.observe", "fill.execute"})
Snapshot = Callable[[], Awaitable[dict[str, Any]]]
Fill = Callable[[dict[str, Any]], Awaitable[dict[str, Any]]]


class BrokerError(Exception):
    """An allowlisted wire refusal, independent of exception details."""

    def __init__(self, code: str):
        self.code = code
        super().__init__(code)


def frame(value: dict[str, Any]) -> bytes:
    return json.dumps(value, ensure_ascii=True, separators=(",", ":")).encode() + b"\n"


def request(data: bytes) -> dict[str, Any]:
    if len(data) > MAX_FRAME or not data.endswith(b"\n"):
        raise BrokerError("invalid_request")
    try:
        body = json.loads(data)
    except (UnicodeError, ValueError):
        raise BrokerError("invalid_request") from None
    if not isinstance(body, dict):
        raise BrokerError("invalid_request")
    if type(body.get("v")) is not int or body["v"] != 1:
        raise BrokerError("unsupported_version")
    request_id = body.get("id")
    operation = body.get("op")
    account = body.get("accountId")
    if (
        not isinstance(request_id, str)
        or ID.fullmatch(request_id) is None
        or not isinstance(operation, str)
        or operation not in OPERATIONS
        or not isinstance(account, str)
        or re.fullmatch(r"[1-9][0-9]{0,18}", account) is None
        or int(account) >= 2**63
    ):
        raise BrokerError("invalid_request")
    if operation == "account.snapshot" and set(body) != {"v", "id", "op", "accountId"}:
        raise BrokerError("invalid_request")
    return body


class Broker:
    def __init__(self, snapshots: Mapping[str, Snapshot], fills: Mapping[str, Fill] | None = None):
        self.snapshots = snapshots
        self.fills = fills or {}
        self.locks = {account: asyncio.Lock() for account in snapshots}
        self.handlers: set[asyncio.Task] = set()

    async def drain(self) -> None:
        if self.handlers:
            await asyncio.gather(*self.handlers, return_exceptions=True)

    async def handle(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        active = asyncio.current_task()
        if active is not None:
            self.handlers.add(active)
        request_id = None
        reply = None
        try:
            if not same_user(writer):
                return
            raw = await asyncio.wait_for(reader.readline(), READ_TIMEOUT)
            body = request(raw)
            request_id = body["id"]
            account = body["accountId"]
            if account not in self.snapshots:
                raise BrokerError("account_unavailable")
            lock = self.locks[account]
            if lock.locked():
                raise BrokerError("account_busy")
            async with lock:
                if body["op"] == "account.snapshot":
                    requested = self.snapshots[account]()
                else:
                    fill = self.fills.get(account)
                    if fill is None:
                        raise BrokerError("operation_unavailable")
                    requested = fill(body)
                result = await asyncio.wait_for(requested, REQUEST_TIMEOUT)
            reply = {"v": 1, "id": request_id, "result": result}
        except BrokerError as error:
            reply = {"v": 1, "id": request_id, "error": {"code": error.code}}
        except Exception:
            reply = {"v": 1, "id": request_id, "error": {"code": "broker_unavailable"}}
        finally:
            try:
                if reply is not None:
                    writer.write(frame(reply))
                    await asyncio.wait_for(writer.drain(), READ_TIMEOUT)
            except (OSError, asyncio.TimeoutError):
                pass
            finally:
                writer.close()
                try:
                    await asyncio.wait_for(writer.wait_closed(), READ_TIMEOUT)
                except (OSError, asyncio.TimeoutError):
                    pass
                finally:
                    if active is not None:
                        self.handlers.discard(active)


def same_user(writer: asyncio.StreamWriter) -> bool:
    peer = writer.get_extra_info("socket")
    if peer is None or not hasattr(socket, "SO_PEERCRED"):
        return False
    size = struct.calcsize("3i")
    _pid, uid, _gid = struct.unpack(
        "3i", peer.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, size)
    )
    return uid == os.getuid()


@asynccontextmanager
async def listen(state: State, broker: Broker):
    if not hasattr(socket, "SO_PEERCRED"):
        raise StoreError("Telegram broker requires a platform with Unix peer credentials")
    with lock_file(state, "broker-v1.lock", "Telegram broker is already running here"):
        try:
            metadata = state.socket.lstat()
        except FileNotFoundError:
            pass
        else:
            if not stat.S_ISSOCK(metadata.st_mode) or metadata.st_uid != os.getuid():
                raise StoreError("Telegram broker socket path is not safe")
            state.socket.unlink()
        server = await asyncio.start_unix_server(broker.handle, path=state.socket, limit=MAX_FRAME)
        state.socket.chmod(0o600)
        try:
            async with server:
                yield
        finally:
            try:
                await broker.drain()
            finally:
                state.socket.unlink(missing_ok=True)


async def bridge(state: State, raw: bytes) -> bytes:
    request(raw)
    metadata = state.socket.lstat()
    if (
        not stat.S_ISSOCK(metadata.st_mode)
        or metadata.st_uid != os.getuid()
        or stat.S_IMODE(metadata.st_mode) & 0o077
    ):
        raise StoreError("Telegram broker socket is not private")
    reader, writer = await asyncio.wait_for(
        asyncio.open_unix_connection(state.socket, limit=MAX_FRAME), READ_TIMEOUT
    )
    try:
        writer.write(raw)
        await asyncio.wait_for(writer.drain(), READ_TIMEOUT)
        reply = await asyncio.wait_for(reader.readline(), REQUEST_TIMEOUT + READ_TIMEOUT)
        if not reply or len(reply) > MAX_FRAME or not reply.endswith(b"\n"):
            raise StoreError("Telegram broker returned an invalid frame")
        return reply
    finally:
        writer.close()
        await writer.wait_closed()


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Forward one framed request to the local Telegram broker"
    )
    parser.add_argument("command", choices=["bridge"])
    parser.parse_args()
    try:
        raw = sys.stdin.buffer.readline(MAX_FRAME + 1)
        reply = asyncio.run(bridge(State.discover(), raw))
    except (OSError, StoreError, BrokerError, asyncio.TimeoutError, ValueError):
        raise SystemExit("Telegram broker request could not be completed") from None
    sys.stdout.buffer.write(reply)


if __name__ == "__main__":
    main()
