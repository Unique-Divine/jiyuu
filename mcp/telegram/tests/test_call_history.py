"""Offline tests for Telegram call history and its FastMCP hook."""

import asyncio
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
from threading import Event

import pytest
from mcp.server.fastmcp import FastMCP
from mcp.types import CallToolRequest, CallToolRequestParams, CallToolResult

from telegram_mcp import call_history
from telegram_mcp.call_history import History, read_rows
from telegram_mcp.request_policy import RequestLimit, RequestLimited, format_request_error


@pytest.mark.asyncio
async def test_call_handler_records_only_safe_fields(tmp_path, monkeypatch):
    root = tmp_path / "logs"
    history = History(root, version="2.0.5", transport="stdio")
    monkeypatch.setattr(call_history, "_history", history)
    server = FastMCP("fixture")

    @server.tool()
    async def fixture_tool(chat_content: str) -> str:
        call_history.record_account(101)
        call_history.record_account(102)
        return chat_content

    @server.tool()
    async def limited_tool() -> str:
        error = RequestLimited(
            RequestLimit("flood_wait", "RPC_SECRET", 42, 12345, "@PRIVATE_CHAT")
        )
        return format_request_error(error)

    @server.tool()
    async def failing_tool() -> str:
        raise ValueError("PRIVATE_EXCEPTION")

    @server.tool()
    async def cancelled_tool() -> str:
        raise asyncio.CancelledError

    call_history.install_mcp_hook(server)
    handler = server._mcp_server.request_handlers[CallToolRequest]

    async def invoke(name, arguments=None):
        return await handler(
            CallToolRequest(
                method="tools/call",
                params=CallToolRequestParams(name=name, arguments=arguments),
            )
        )

    result = await invoke("fixture_tool", {"chat_content": "PRIVATE_MESSAGE"})
    assert isinstance(result.root, CallToolResult) and not result.root.isError
    await invoke("limited_tool")
    await invoke("failing_tool")
    await invoke("SECRET_UNREGISTERED_NAME")
    await invoke("fixture_tool", {"wrong_field": "INVALID_ARGUMENT"})
    with pytest.raises(asyncio.CancelledError):
        await invoke("cancelled_tool")
    assert history.flush()

    files = list(root.glob("mcp-*.jsonl"))
    assert len(files) == 1
    assert stat.S_IMODE(root.stat().st_mode) == 0o700
    assert stat.S_IMODE(files[0].stat().st_mode) == 0o600
    text = files[0].read_text()
    for secret in (
        "PRIVATE_MESSAGE",
        "PRIVATE_EXCEPTION",
        "RPC_SECRET",
        "PRIVATE_CHAT",
        "SECRET_UNREGISTERED_NAME",
        "INVALID_ARGUMENT",
    ):
        assert secret not in text
    rows = list(read_rows(root))
    assert len(rows) == 12
    assert [row["seq"] for row in rows] == list(range(1, 13))
    for start, end in zip(rows[::2], rows[1::2]):
        assert start["requestId"] == end["requestId"]
        assert start["event"] == "call.start"
        assert end["event"] == "call.end"
        assert end["durationMs"] >= 0
        assert end["transport"] == "stdio"
    assert rows[1]["accountIds"] == ["101", "102"]
    assert rows[3]["outcome"] == "flood"
    assert rows[3]["protocolOutcome"] == "ok"
    assert rows[3]["waitSeconds"] == 42
    assert rows[5]["protocolOutcome"] == "error"
    assert rows[7]["method"] == "unknown"
    assert rows[7]["code"] == "unknown_method"
    assert rows[9]["outcome"] == "error"
    assert rows[11]["outcome"] == "cancelled"


def test_abrupt_exit_and_concurrent_processes(tmp_path):
    root = tmp_path / "logs"
    script = """
import os
from pathlib import Path
import sys

from telegram_mcp.call_history import History

history = History(Path(sys.argv[1]))
for _ in range(40):
    call = history.call("mcp", "fixture_tool")
    if sys.argv[2] == "complete":
        call.finish("ok")
assert history.flush()
os._exit(0)
"""
    processes = [
        subprocess.Popen([sys.executable, "-c", script, str(root), mode])
        for mode in ("complete", "complete", "incomplete")
    ]
    try:
        for process in processes:
            assert process.wait(timeout=15) == 0
    finally:
        for process in processes:
            if process.poll() is None:
                process.kill()
            process.wait()
    paths = list(root.glob("*.jsonl"))
    records = [json.loads(line) for path in paths for line in path.read_text().splitlines()]
    starts = {row["requestId"] for row in records if row["event"] == "call.start"}
    ends = {row["requestId"] for row in records if row["event"] == "call.end"}
    assert len(records) == 200
    assert len(starts - ends) == 40
    assert len({row["runId"] for row in records}) == 3


def test_short_write_then_next_record_repairs_tail(tmp_path, monkeypatch):
    root = tmp_path / "logs"
    history = History(root)
    history.call("rpc", "fixture")
    assert history.flush()

    original_write = os.write
    write_count = 0

    def fail_after_short_write(fd, data):
        nonlocal write_count
        write_count += 1
        if write_count == 1:
            return original_write(fd, data[:11])
        if write_count == 2:
            raise OSError("DISK PRIVATE DIAGNOSTIC")
        return original_write(fd, data)

    monkeypatch.setattr(call_history.os, "write", fail_after_short_write)
    lost = history.call("rpc", "fixture")
    assert history.flush()
    assert lost.id not in {row.get("requestId") for row in read_rows(root)}
    kept = history.call("rpc", "fixture", parent_id=lost.id)
    kept.finish("ok", code="SECRET_ARBITRARY_CODE")
    assert history.flush()
    rows = list(read_rows(root))
    assert rows[1]["requestId"] == kept.id
    assert rows[1]["parentId"] == lost.id
    assert rows[2]["event"] == "log.gap"
    assert rows[2]["repairedLines"] == 1
    assert rows[2]["writeFailures"] == 1
    assert rows[3]["code"] == "other"
    assert "SECRET_ARBITRARY_CODE" not in str(rows)
    for path in root.glob("*.jsonl"):
        for line in path.read_text().splitlines():
            json.loads(line)


def test_utc_rotation_and_root_byte_limit(tmp_path, monkeypatch):
    root = tmp_path / "logs"
    clock = datetime(2026, 10, 1, 23, 59, tzinfo=timezone.utc)
    monkeypatch.setattr(call_history, "_date_now", lambda: clock)
    history = History(root, keep_days=2, max_bytes=6400, segment_bytes=4096)
    initial = history.call("mcp", "fixture")
    assert history.flush()
    clock = datetime(2026, 10, 2, 0, 1, tzinfo=timezone.utc)
    initial.finish("ok")
    assert history.flush()
    assert (root / "mcp-2026-10-01.jsonl").exists()
    assert (root / "mcp-2026-10-02.jsonl").exists()

    clock = datetime(2026, 10, 3, 0, 1, tzinfo=timezone.utc)
    for _ in range(40):
        history.call("mcp", "fixture").finish("ok")
        history.call("broker", "fill.observe").finish("not_found")
    assert history.flush()
    files = list(root.glob("*.jsonl"))
    assert sum(path.stat().st_size for path in files) <= 6400
    assert all("2026-10-01" not in path.name for path in files)
    assert any("." in path.stem for path in files)
    assert all(row["ts"].endswith("Z") for row in read_rows(root))


def test_queue_is_bounded_and_reports_gap(tmp_path, monkeypatch):
    release = Event()
    enqueued = Event()
    original_write = History._write

    def slow_write(history, row):
        enqueued.set()
        assert release.wait(5)
        return original_write(history, row)

    monkeypatch.setattr(History, "_write", slow_write)
    history = History(tmp_path / "logs", queue_size=1)
    try:
        history.call("rpc", "fixture")
        assert enqueued.wait(5)
        for _ in range(40):
            history.call("rpc", "fixture")
        assert not history.flush(0)
    finally:
        release.set()
    assert history.flush()
    gaps = [row for row in read_rows(history.root) if row["event"] == "log.gap"]
    assert sum(row["droppedEvents"] for row in gaps) == 39


def test_private_writes_and_reads_refuse_redirected_files(tmp_path, monkeypatch):
    root = tmp_path / "logs"
    assert list(read_rows(root)) == []
    assert not root.exists()

    clock = datetime(2026, 10, 1, tzinfo=timezone.utc)
    monkeypatch.setattr(call_history, "_date_now", lambda: clock)
    target = tmp_path / "other"
    target.write_text("outside")
    root.mkdir(mode=0o700)
    history = History(root)
    (root / "mcp-2026-10-01.jsonl").symlink_to(target)
    history.call("mcp", "fixture")
    assert history.flush()
    assert target.read_text() == "outside"
    assert list(read_rows(root)) == []
