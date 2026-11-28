"""Private, best-effort call history for Telegram MCP and the pool broker."""

from __future__ import annotations

import atexit
import json
import os
import re
import stat
import sys
from collections.abc import Generator, Iterator
from contextlib import contextmanager, suppress
from contextvars import ContextVar
from functools import wraps
from datetime import date, datetime, timedelta, timezone
from itertools import count
from pathlib import Path
from queue import Empty, Full, Queue
from threading import Lock, Thread
from time import monotonic
from typing import TYPE_CHECKING, Any
from uuid import UUID, uuid4

try:
    import fcntl
except ImportError:  # This writer needs descriptor-relative I/O and file locking.
    fcntl = None  # type: ignore[assignment]

if TYPE_CHECKING:
    from mcp.server.fastmcp import FastMCP

SCHEMA = 1
KEEP_DAYS = 30
MAX_BYTES = 100 * 1024 * 1024
SEGMENT_BYTES = 4 * 1024 * 1024
MAX_LINE = 4096
EXIT_FLUSH_SECONDS = 0.3

SURFACES = frozenset({"mcp", "worker", "broker", "rpc"})
TRANSPORTS = frozenset({"stdio", "http", "unix"})
OUTCOMES = frozenset(
    {
        "ok",
        "completed",
        "error",
        "denied",
        "timeout",
        "cancelled",
        "flood",
        "retry",
        "uncertain",
        "unavailable",
        "not_found",
        "succeeded",
        "transport_error",
    }
)
CODES = frozenset(
    {
        "account_busy",
        "account_held",
        "account_required",
        "account_unavailable",
        "add_spacing",
        "bot_not_visible",
        "broker_peer_gone",
        "broker_unavailable",
        "budget_unavailable",
        "cooldown",
        "daily_cap",
        "duplicate_pool_reference",
        "flood_wait",
        "group_count_unavailable",
        "group_limit",
        "hold",
        "invalid_action_binding",
        "invalid_bot",
        "invalid_request",
        "inventory_pending",
        "lease_unavailable",
        "operation_unavailable",
        "peer_disconnected",
        "photo_upload_unavailable",
        "premium_flood_wait",
        "room_observation_unavailable",
        "room_preflight_unavailable",
        "slow_mode",
        "timeout",
        "tool_error",
        "telegram_admin_required",
        "telegram_privacy_restricted",
        "transport_error",
        "unauthorized",
        "unknown_account",
        "unknown_method",
        "unsupported_version",
        "validation_error",
    }
)
_NAME = re.compile(r"(mcp|worker|broker|rpc)-(\d{4}-\d{2}-\d{2})(?:\.(\d{5}))?\.jsonl")
_METHOD = re.compile(r"[A-Za-z][A-Za-z0-9_.]{0,63}")
_DECIMAL = re.compile(r"[1-9][0-9]{0,18}")
_history: History | None = None
_active_call: ContextVar[Call | None] = ContextVar("telegram_call_history", default=None)


def default_root() -> Path:
    base = os.environ.get("BOKU_HOME")
    return (Path(base) if base else Path.home() / ".boku") / "telegram-mcp" / "logs"


def configure(
    root: Path | None = None, *, version: str = "unknown", transport: str | None = None
) -> History:
    """Set up the process history and prune expired records off the caller path."""
    global _history
    _history = History(
        default_root() if root is None else root, version=version, transport=transport
    )
    atexit.register(_history.flush, EXIT_FLUSH_SECONDS)
    _history.start()
    return _history


def current() -> History | None:
    return _history


def _id(value: str | None) -> str | None:
    if value is None or len(value) != 36:
        return None
    try:
        parsed = UUID(value)
    except ValueError:
        return None
    return value if str(parsed) == value else None


def _positive_id(value: str | int | None) -> str | None:
    if type(value) is int:
        return str(value) if 0 < value < 2**63 else None
    if isinstance(value, str) and _DECIMAL.fullmatch(value) and int(value) < 2**63:
        return value
    return None


def _date_now() -> datetime:
    return datetime.now(timezone.utc)


def _name(name: str) -> tuple[str, date, int] | None:
    matched = _NAME.fullmatch(name)
    if matched is not None:
        surface, day, index = matched.groups()
        try:
            return surface, date.fromisoformat(day), int(index or 0)
        except ValueError:
            pass
    return None


def _open_root(root: Path, *, create: bool = False) -> int:
    if not root.is_absolute() or fcntl is None:
        raise OSError("Call history needs a local, absolute path and file locking")
    if create:
        root.mkdir(mode=0o700, parents=True, exist_ok=True)
    fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        info = os.fstat(fd)
        if info.st_uid != os.geteuid() or info.st_mode & 0o077:
            raise OSError("Call history directory must be private and owned by the service user")
        return fd
    except OSError:
        os.close(fd)
        raise


def _open_file(root: int, name: str, flags: int) -> int:
    fd = os.open(name, flags | os.O_NOFOLLOW | os.O_NONBLOCK, 0o600, dir_fd=root)
    try:
        info = os.fstat(fd)
        if (
            not stat.S_ISREG(info.st_mode)
            or info.st_nlink != 1
            or info.st_uid != os.geteuid()
            or info.st_mode & 0o077
        ):
            raise OSError("Call history file must be private and owned by the service user")
        return fd
    except OSError:
        os.close(fd)
        raise


def _segments(root: int) -> Iterator[tuple[str, str, date, int, os.stat_result]]:
    for name in os.listdir(root):
        parsed = _name(name)
        if parsed is None:
            continue
        try:
            info = os.stat(name, dir_fd=root, follow_symlinks=False)
        except FileNotFoundError:
            continue
        if stat.S_ISREG(info.st_mode) and info.st_nlink == 1 and info.st_uid == os.geteuid():
            yield name, *parsed, info


def _encode(row: dict[str, Any]) -> bytes:
    return (json.dumps(row, ensure_ascii=True, separators=(",", ":")) + "\n").encode()


def _repair_tail(fd: int) -> int:
    end = os.lseek(fd, 0, os.SEEK_END)
    if not end:
        return 0
    start = max(0, end - MAX_LINE)
    os.lseek(fd, start, os.SEEK_SET)
    tail = os.read(fd, end - start)
    if tail.endswith(b"\n"):
        return 0
    newline = tail.rfind(b"\n")
    if newline < 0 and start:
        raise OSError("Call history segment has an oversized partial row")
    os.ftruncate(fd, start + newline + 1)
    return 1


def _append(fd: int, data: bytes) -> None:
    view = memoryview(data)
    while view:
        written = os.write(fd, view)
        if not written:
            raise OSError("Call history append stalled")
        view = view[written:]


class History:
    """A bounded writer. Calls never wait for the history filesystem."""

    def __init__(
        self,
        root: Path,
        *,
        version: str = "unknown",
        transport: str | None = None,
        max_bytes: int = MAX_BYTES,
        keep_days: int = KEEP_DAYS,
        queue_size: int = 2048,
        segment_bytes: int = SEGMENT_BYTES,
    ) -> None:
        if min(max_bytes, segment_bytes) < MAX_LINE or keep_days < 1 or queue_size < 1:
            raise ValueError("Invalid call history limits")
        self.root = root
        self.version = (
            version
            if _METHOD.fullmatch(version) or re.fullmatch(r"[0-9][A-Za-z0-9.+-]{0,63}", version)
            else "unknown"
        )
        self.transport = transport if transport in TRANSPORTS else None
        self.run_id = str(uuid4())
        self._max_bytes = max_bytes
        self._keep_days = keep_days
        self._segment_bytes = min(max_bytes, segment_bytes)
        # Rows have a fixed set of bounded fields, so bounding count also bounds memory.
        self._queue: Queue[dict[str, Any]] = Queue(maxsize=queue_size)
        self._seq = count(1)
        self._worker: Thread | None = None
        self._lock = Lock()
        self._dropped = 0
        self._failed = 0
        self._repaired = 0
        self._last_warning: float | None = None

    def call(
        self,
        surface: str,
        method: str,
        *,
        request_id: str | None = None,
        parent_id: str | None = None,
        account_id: str | int | None = None,
        fill_id: str | int | None = None,
        action_id: str | int | None = None,
        transport: str | None = None,
    ) -> Call:
        """Start a call with an already allowlisted method from the caller's registry."""
        fields = {
            "method": method if _METHOD.fullmatch(method) else "unknown",
            "requestId": _id(request_id) or str(uuid4()),
            "parentId": _id(parent_id),
            "accountId": _positive_id(account_id),
            "fillId": _positive_id(fill_id),
            "actionId": _positive_id(action_id),
            "transport": transport if transport in TRANSPORTS else self.transport,
        }
        label = surface if surface in SURFACES else "mcp"
        call = Call(self, label, fields)
        call._event("call.start", outcome="started")
        return call

    def _row(self, event: str, surface: str, **fields: Any) -> dict[str, Any]:
        return {
            "schema": SCHEMA,
            "ts": _date_now().isoformat(timespec="milliseconds").replace("+00:00", "Z"),
            "event": event,
            "surface": surface,
            "pid": os.getpid(),
            "version": self.version,
            "runId": self.run_id,
            "seq": next(self._seq),
            **{key: value for key, value in fields.items() if value is not None},
        }

    def start(self) -> None:
        with self._lock:
            self._start_locked()

    def _start_locked(self) -> None:
        if self._worker is None:
            worker = Thread(target=self._drain, name="telegram-call-history", daemon=True)
            worker.start()
            self._worker = worker

    def _enqueue(self, row: dict[str, Any]) -> None:
        with self._lock:
            try:
                self._start_locked()
                self._queue.put_nowait(row)
            except (Full, RuntimeError):
                self._dropped += 1

    def _drain(self) -> None:
        last_prune = 0.0
        while True:
            now = monotonic()
            if last_prune == 0.0 or now - last_prune >= 60:
                last_prune = now
                with suppress(OSError):
                    self._prune_expired()
            try:
                row = self._queue.get(timeout=60)
            except Empty:
                continue
            try:
                self._repaired += self._write(row)
                with self._lock:
                    dropped, failed, repaired = self._dropped, self._failed, self._repaired
                if dropped or failed or repaired:
                    self._write(
                        self._row(
                            "log.gap",
                            row["surface"],
                            droppedEvents=dropped,
                            writeFailures=failed,
                            repairedLines=repaired,
                        )
                    )
                    with self._lock:
                        self._dropped -= dropped
                        self._failed -= failed
                        self._repaired -= repaired
            except OSError:
                with self._lock:
                    self._failed += 1
                now = monotonic()
                if self._last_warning is None or now - self._last_warning >= 60:
                    self._last_warning = now
                    with suppress(OSError):
                        print("Telegram call history could not write a record", file=sys.stderr)
            finally:
                self._queue.task_done()

    def _write(self, row: dict[str, Any]) -> int:
        root = _open_root(self.root, create=True)
        try:
            guard = _open_file(root, ".history.lock", os.O_RDWR | os.O_CREAT)
            try:
                assert fcntl is not None
                fcntl.flock(guard, fcntl.LOCK_EX)
                return self._write_locked(root, row)
            finally:
                os.close(guard)
        finally:
            os.close(root)

    def _prune_expired(self) -> None:
        root = _open_root(self.root)
        try:
            guard = _open_file(root, ".history.lock", os.O_RDWR | os.O_CREAT)
            try:
                assert fcntl is not None
                fcntl.flock(guard, fcntl.LOCK_EX)
                cutoff = _date_now().date() - timedelta(days=self._keep_days - 1)
                for name, _, day, _, _ in _segments(root):
                    if day < cutoff:
                        os.unlink(name, dir_fd=root)
            finally:
                os.close(guard)
        finally:
            os.close(root)

    def _write_locked(self, root: int, row: dict[str, Any]) -> int:
        data = _encode(row)
        today = _date_now().date()
        day = date.fromisoformat(row["ts"][:10])
        cutoff = today - timedelta(days=self._keep_days - 1)
        if len(data) > MAX_LINE or day < cutoff:
            raise OSError("Call history record is outside its size or retention limit")

        surface = row["surface"]
        segments = sorted(_segments(root), key=lambda x: (x[2], x[4].st_mtime_ns, x[0]))
        latest = max(
            (segment for segment in segments if segment[1] == surface and segment[2] >= cutoff),
            key=lambda x: (x[2], x[3]),
            default=None,
        )
        repaired = 0
        if latest is not None:
            candidate = _open_file(root, latest[0], os.O_RDWR)
            try:
                repaired = _repair_tail(candidate)
            finally:
                os.close(candidate)
            if repaired:
                segments = sorted(_segments(root), key=lambda x: (x[2], x[4].st_mtime_ns, x[0]))
        active = max(
            (segment for segment in segments if segment[1:3] == (surface, day)),
            key=lambda x: x[3],
            default=None,
        )
        index = active[3] if active is not None else 0
        if active is not None and active[4].st_size + len(data) > self._segment_bytes:
            index += 1
        suffix = f".{index:05d}" if index else ""
        name = f"{surface}-{day.isoformat()}{suffix}.jsonl"
        if _name(name) is None:
            raise OSError("Call history segment index exhausted")

        total = sum(segment[4].st_size for segment in segments)
        for prior, _, prior_day, _, info in segments:
            if prior != name and (prior_day < cutoff or total + len(data) > self._max_bytes):
                os.unlink(prior, dir_fd=root)
                total -= info.st_size
        if total + len(data) > self._max_bytes:
            raise OSError("Call history byte limit reached")

        fd = _open_file(root, name, os.O_RDWR | os.O_CREAT | os.O_APPEND)
        try:
            repaired += _repair_tail(fd)
            _append(fd, data)
            return repaired
        finally:
            os.close(fd)

    def flush(self, timeout: float = 5.0) -> bool:
        """Wait at most timeout seconds for queued rows to be processed."""
        deadline = monotonic() + timeout
        with self._queue.all_tasks_done:
            while self._queue.unfinished_tasks:
                remaining = deadline - monotonic()
                if remaining <= 0:
                    return False
                self._queue.all_tasks_done.wait(remaining)
        return True


class Call:
    """The lifecycle of one requested operation."""

    def __init__(self, history: History, surface: str, fields: dict[str, Any]) -> None:
        self.history = history
        self.surface = surface
        self.id: str = fields["requestId"]
        self._fields = fields
        self._start = monotonic()
        self._accounts: set[str] = set()
        self._failure: tuple[str, str | None, int | None] | None = None
        self._finished = False

    def _event(self, event: str, **extra: Any) -> None:
        self.history._enqueue(self.history._row(event, self.surface, **self._fields, **extra))

    def dispatch(self) -> None:
        self._event("call.dispatch", outcome="dispatched")

    def finish(
        self,
        outcome: str,
        *,
        code: str | None = None,
        wait_s: int | None = None,
        protocol_outcome: str | None = None,
    ) -> None:
        if self._finished:
            return
        self._finished = True
        if outcome in {"ok", "completed", "error"} and self._failure is not None:
            outcome, code, wait_s = self._failure
        if len(self._accounts) == 1 and self._fields["accountId"] is None:
            self._fields["accountId"] = next(iter(self._accounts))
        self._event(
            "call.end",
            outcome=outcome if outcome in OUTCOMES else "error",
            protocolOutcome=protocol_outcome if protocol_outcome in OUTCOMES else None,
            code=(code if code in CODES else "other") if code is not None else None,
            waitSeconds=wait_s if type(wait_s) is int and 0 <= wait_s < 2**32 else None,
            durationMs=round((monotonic() - self._start) * 1000),
            accountIds=sorted(self._accounts) if len(self._accounts) > 1 else None,
        )


@contextmanager
def tool_call(call: Call) -> Generator[None, None, None]:
    token = _active_call.set(call)
    try:
        yield
    finally:
        _active_call.reset(token)


def record_account(account: int | None) -> None:
    call = _active_call.get()
    normalized = _positive_id(account)
    if call is not None and normalized is not None and len(call._accounts) < 64:
        call._accounts.add(normalized)


def record_failure(outcome: str, code: str | None = None, wait_s: int | None = None) -> None:
    call = _active_call.get()
    if call is not None and call._failure is None:
        call._failure = (outcome, code, wait_s)


def read_rows(root: Path) -> Iterator[dict[str, Any]]:
    """Read complete history records without creating a directory or lock."""
    try:
        fd = _open_root(root)
    except FileNotFoundError:
        return
    try:
        for name, _, _, _, _ in sorted(_segments(fd), key=lambda segment: segment[2:4]):
            try:
                entry = _open_file(fd, name, os.O_RDONLY)
            except FileNotFoundError:  # The writer can prune a segment between scan and open.
                continue
            with os.fdopen(entry, "rb") as file:
                for line in file:
                    if not line.endswith(b"\n") or len(line) > MAX_LINE:
                        continue
                    try:
                        row = json.loads(line)
                    except (ValueError, UnicodeError):
                        continue
                    if isinstance(row, dict) and row.get("schema") == SCHEMA:
                        yield row
    finally:
        os.close(fd)


def install_mcp_hook(server: FastMCP) -> None:
    """Wrap tools/call at the server so all its transports record the call once."""
    import asyncio

    from mcp.types import CallToolRequest, CallToolResult, ServerResult

    original_handler = server._mcp_server.request_handlers[CallToolRequest]

    def instrument_tool(tool: Any) -> None:
        original = tool.fn
        if getattr(original, "_telegram_history_dispatch", False):
            return

        @wraps(original)
        def dispatch(*args: Any, **kwargs: Any) -> Any:
            if (call := _active_call.get()) is not None:
                call.dispatch()
            return original(*args, **kwargs)

        setattr(dispatch, "_telegram_history_dispatch", True)
        tool.fn = dispatch

    async def handle(req: CallToolRequest) -> ServerResult:
        history = current()
        if history is None:
            return await original_handler(req)
        tool = server._tool_manager.get_tool(req.params.name)
        if tool is not None:
            instrument_tool(tool)
        call = history.call("mcp", tool.name if tool is not None else "unknown")
        with tool_call(call):
            try:
                response = await original_handler(req)
            except asyncio.CancelledError:
                call.finish("cancelled", protocol_outcome="cancelled")
                raise
            except Exception:
                call.finish("error", code="tool_error", protocol_outcome="error")
                raise

        protocol = "ok"
        if isinstance(response.root, CallToolResult) and response.root.isError:
            protocol = "error"
        if tool is None:
            call.finish("denied", code="unknown_method", protocol_outcome=protocol)
        elif protocol == "error":
            call.finish("error", code="tool_error", protocol_outcome=protocol)
        else:
            call.finish("completed", protocol_outcome=protocol)
        return response

    server._mcp_server.request_handlers[CallToolRequest] = handle
