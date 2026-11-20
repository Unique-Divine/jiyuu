"""Update check: version order, cache, failures, opt-out and one-time notice."""

import json
import threading
import time
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest

from telegram_mcp import update_check
from telegram_mcp.update_check import (
    UpdateNotifier,
    UpdateStatus,
    check_for_update,
    check_latest,
    format_notice,
    is_newer,
)

NAME = "uniquedivine-telegram-mcp"
NOW = datetime(2026, 11, 17, 12, 0, tzinfo=timezone.utc)


def pypi_body(version: str) -> bytes:
    return json.dumps({"info": {"name": NAME, "version": version}}).encode()


def seed_cache(cache_dir, latest: str, checked_at: datetime) -> None:
    payload = {"name": NAME, "latestVersion": latest, "checkedAt": checked_at.isoformat()}
    (cache_dir / f"{NAME}.json").write_text(json.dumps(payload))


def cached_latest(cache_dir) -> str:
    return json.loads((cache_dir / f"{NAME}.json").read_text())["latestVersion"]


class RecordingFetcher:
    """Serve one fixed body and record the requested URLs."""

    def __init__(self, body: bytes = b"", error: Exception | None = None):
        self.body = body
        self.error = error
        self.urls: list[str] = []

    def __call__(self, url: str, timeout: float) -> bytes:
        self.urls.append(url)
        if self.error is not None:
            raise self.error
        return self.body


@pytest.mark.parametrize(
    ("candidate", "running", "expected"),
    [
        ("2.1.0", "2.0.1", True),
        ("2.0.10", "2.0.9", True),
        ("2.0.1", "2.0.1", False),
        ("2.0.0", "2.0.1", False),
        ("3", "2.9.9", True),
        ("2.0.1", "2.0.1-rc.1", True),
        ("2.0.1rc2", "2.0.1rc1", True),
        ("2.0.1-rc.1", "2.0.1", False),
        ("2.0.1-rc.10", "2.0.1-rc.9", True),
        ("2.0.1-beta", "2.0.1-alpha", True),
        ("v2.1.0", "2.0.1", True),
        ("not-a-version", "2.0.1", False),
        ("2.1.0", "local", False),
        # PEP 440 post- and dev releases, which a hand-rolled semver parse
        # misread as prereleases.
        ("2.0.1", "2.0.1.post1", False),
        ("2.0.1.post1", "2.0.1", True),
        ("2.0.1.post2", "2.0.1.post1", True),
        ("2.0.1", "2.0.1.dev3", True),
        ("2.0.1.dev3", "2.0.1", False),
        ("2.0.1rc1", "2.0.1.dev3", True),
    ],
)
def test_is_newer_orders_versions(candidate, running, expected):
    assert is_newer(candidate, running) is expected


def test_post_release_running_ahead_of_registry_gets_no_notice():
    status = UpdateStatus(NAME, "2.0.1.post1", "2.0.1", NOW.isoformat())

    assert status.update_available is False


def test_running_version_newer_than_registry_means_no_update():
    status = UpdateStatus(NAME, "2.2.0", "2.1.0", NOW.isoformat())

    assert status.update_available is False


@pytest.mark.parametrize("latest", ["2.0.1", "2.0.0"])
def test_pin_command_is_none_unless_latest_is_newer(latest):
    status = UpdateStatus(NAME, "2.0.1", latest, NOW.isoformat(), "cache")

    assert status.pin_command is None


def test_cache_lasts_one_hour():
    assert update_check.CACHE_TTL == timedelta(hours=1)


def test_cache_miss_fetches_pypi_and_writes_cache(tmp_path):
    fetch = RecordingFetcher(pypi_body("2.1.0"))

    status = check_for_update(NAME, "2.0.1", fetch=fetch, env={}, cache_dir=tmp_path, now=NOW)

    assert fetch.urls == [f"https://pypi.org/pypi/{NAME}/json"]
    assert status == UpdateStatus(NAME, "2.0.1", "2.1.0", "2026-11-17T12:00:00+00:00", "live")
    assert status.update_available
    assert status.pin_command == f"uvx {NAME}@2.1.0"
    cached = json.loads((tmp_path / f"{NAME}.json").read_text())
    assert cached == {
        "name": NAME,
        "latestVersion": "2.1.0",
        "checkedAt": "2026-11-17T12:00:00+00:00",
    }


def test_cache_hit_within_the_hour_skips_the_registry(tmp_path):
    check_for_update(
        NAME,
        "2.0.1",
        fetch=RecordingFetcher(pypi_body("2.1.0")),
        env={},
        cache_dir=tmp_path,
        now=NOW,
    )
    fetch = RecordingFetcher(pypi_body("9.9.9"))

    status = check_for_update(
        NAME, "2.0.1", fetch=fetch, env={}, cache_dir=tmp_path, now=NOW + timedelta(minutes=59)
    )

    assert fetch.urls == []
    assert status is not None and status.latest_version == "2.1.0"
    assert status.checked_at == "2026-11-17T12:00:00+00:00"
    assert status.source == "cache"


def test_expired_cache_fetches_again(tmp_path):
    check_for_update(
        NAME,
        "2.0.1",
        fetch=RecordingFetcher(pypi_body("2.1.0")),
        env={},
        cache_dir=tmp_path,
        now=NOW,
    )
    fetch = RecordingFetcher(pypi_body("2.2.0"))

    status = check_for_update(
        NAME, "2.0.1", fetch=fetch, env={}, cache_dir=tmp_path, now=NOW + timedelta(hours=1)
    )

    assert len(fetch.urls) == 1
    assert status is not None and status.latest_version == "2.2.0"
    assert status.source == "live"


def test_fresh_cache_older_than_running_version_fetches_again(tmp_path):
    seed_cache(tmp_path, "2.0.1", NOW - timedelta(minutes=5))
    fetch = RecordingFetcher(pypi_body("2.0.3"))

    status = check_for_update(NAME, "2.0.2", fetch=fetch, env={}, cache_dir=tmp_path, now=NOW)

    assert len(fetch.urls) == 1
    assert status == UpdateStatus(NAME, "2.0.2", "2.0.3", NOW.isoformat(), "live")
    assert cached_latest(tmp_path) == "2.0.3"


def test_check_latest_reads_pypi_past_a_fresh_cache_and_rewrites_it(tmp_path):
    seed_cache(tmp_path, "2.1.0", NOW - timedelta(minutes=1))
    fetch = RecordingFetcher(pypi_body("2.2.0"))

    status = check_latest(NAME, "2.0.1", fetch=fetch, env={}, cache_dir=tmp_path, now=NOW)

    assert fetch.urls == [f"https://pypi.org/pypi/{NAME}/json"]
    assert status == UpdateStatus(NAME, "2.0.1", "2.2.0", NOW.isoformat(), "live")
    assert status.pin_command == f"uvx {NAME}@2.2.0"
    assert cached_latest(tmp_path) == "2.2.0"


def test_check_latest_falls_back_to_a_cache_of_any_age(tmp_path):
    week_old = NOW - timedelta(days=7)
    seed_cache(tmp_path, "2.1.0", week_old)
    fetch = RecordingFetcher(error=OSError("network unreachable"))

    status = check_latest(NAME, "2.0.1", fetch=fetch, env={}, cache_dir=tmp_path, now=NOW)

    assert status == UpdateStatus(NAME, "2.0.1", "2.1.0", week_old.isoformat(), "cache")


def test_check_latest_fallback_ignores_a_cache_older_than_running(tmp_path):
    seed_cache(tmp_path, "2.0.1", NOW)
    fetch = RecordingFetcher(b"<html>rate limited</html>")

    assert check_latest(NAME, "2.0.2", fetch=fetch, env={}, cache_dir=tmp_path, now=NOW) is None


def test_check_latest_without_pypi_or_cache_is_none(tmp_path):
    fetch = RecordingFetcher(error=TimeoutError("timed out"))

    assert check_latest(NAME, "2.0.1", fetch=fetch, env={}, cache_dir=tmp_path, now=NOW) is None


def test_check_latest_honors_opt_out_and_unpublished_names(tmp_path):
    fetch = RecordingFetcher(pypi_body("2.1.0"))
    opted_out = {"JIYUU_MCP_NO_UPDATE_CHECK": "1"}

    assert check_latest(NAME, "2.0.1", fetch=fetch, env=opted_out, cache_dir=tmp_path) is None
    assert check_latest("telegram-mcp", "2.0.1", fetch=fetch, env={}, cache_dir=tmp_path) is None
    assert fetch.urls == []


def test_corrupt_cache_counts_as_a_miss(tmp_path):
    (tmp_path / f"{NAME}.json").write_text("{not json")
    fetch = RecordingFetcher(pypi_body("2.1.0"))

    status = check_for_update(NAME, "2.0.1", fetch=fetch, env={}, cache_dir=tmp_path, now=NOW)

    assert len(fetch.urls) == 1
    assert status is not None and status.latest_version == "2.1.0"


@pytest.mark.parametrize(
    "fetch",
    [
        RecordingFetcher(error=TimeoutError("timed out")),
        RecordingFetcher(error=OSError("network unreachable")),
        RecordingFetcher(b"<html>rate limited</html>"),
        RecordingFetcher(json.dumps({"info": {"version": "garbage"}}).encode()),
    ],
)
def test_registry_failures_are_silent_and_not_cached(tmp_path, fetch):
    status = check_for_update(NAME, "2.0.1", fetch=fetch, env={}, cache_dir=tmp_path, now=NOW)

    assert status is None
    assert not (tmp_path / f"{NAME}.json").exists()


@pytest.mark.parametrize("value", ["1", "true", "yes"])
def test_env_opt_out_skips_the_check(tmp_path, value):
    fetch = RecordingFetcher(pypi_body("2.1.0"))

    status = check_for_update(
        NAME,
        "2.0.1",
        fetch=fetch,
        env={"JIYUU_MCP_NO_UPDATE_CHECK": value},
        cache_dir=tmp_path,
        now=NOW,
    )

    assert status is None
    assert fetch.urls == []


def test_names_jiyuu_does_not_publish_are_not_checked(tmp_path):
    fetch = RecordingFetcher(pypi_body("99.0.0"))

    status = check_for_update(
        "telegram-mcp", "2.0.1", fetch=fetch, env={}, cache_dir=tmp_path, now=NOW
    )

    assert status is None
    assert fetch.urls == []


def test_concurrent_cache_writes_never_expose_a_partial_file(tmp_path):
    # The startup thread and mcp_info worker threads share one process ID, so
    # each write needs its own temporary file.
    path = tmp_path / f"{NAME}.json"
    versions = [f"2.{i}.0" for i in range(16)]
    stop = threading.Event()
    bad_reads: list[str] = []

    def writer(version: str) -> None:
        for _ in range(200):
            update_check.write_cache(path, NAME, version, NOW.isoformat())

    def reader() -> None:
        while not stop.is_set():
            try:
                text = path.read_text()
            except FileNotFoundError:
                continue
            try:
                assert json.loads(text)["latestVersion"] in versions
            except (ValueError, KeyError, AssertionError):
                bad_reads.append(text)

    watcher = threading.Thread(target=reader)
    watcher.start()
    writers = [threading.Thread(target=writer, args=(v,)) for v in versions]
    for thread in writers:
        thread.start()
    for thread in writers:
        thread.join()
    stop.set()
    watcher.join()

    assert bad_reads == []
    assert json.loads(path.read_text())["latestVersion"] in versions
    assert [entry.name for entry in tmp_path.iterdir()] == [path.name]


def test_failed_cache_write_leaves_no_temporary_file(tmp_path):
    path = tmp_path / f"{NAME}.json"
    path.mkdir()  # Replacing a directory with a file fails.

    update_check.write_cache(path, NAME, "2.1.0", NOW.isoformat())

    assert [entry.name for entry in tmp_path.iterdir()] == [path.name]


def test_default_cache_dir_follows_xdg_cache_home(tmp_path):
    env = {"XDG_CACHE_HOME": str(tmp_path)}

    assert update_check.default_cache_dir(env) == tmp_path / "jiyuu-mcp"


class _SlowHandler(BaseHTTPRequestHandler):
    delay = 0.0
    body = b""

    def do_GET(self):
        time.sleep(self.delay)
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        try:
            self.wfile.write(self.body)
        except (BrokenPipeError, ConnectionResetError):
            pass  # The client already timed out.

    def log_message(self, format: str, *args: object) -> None:
        pass


@pytest.fixture
def local_registry():
    servers = []

    def start(body: bytes, delay: float = 0.0) -> str:
        handler = type("Handler", (_SlowHandler,), {"body": body, "delay": delay})
        server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        servers.append(server)
        return f"http://127.0.0.1:{server.server_address[1]}/"

    yield start
    for server in servers:
        server.shutdown()
        server.server_close()


def test_urllib_fetch_reads_a_real_http_response(tmp_path, local_registry):
    url = local_registry(pypi_body("2.1.0"))

    def fetch(_url: str, timeout: float) -> bytes:
        return update_check.urllib_fetch(url, timeout)

    status = check_for_update(NAME, "2.0.1", fetch=fetch, env={}, cache_dir=tmp_path, now=NOW)

    assert status is not None and status.latest_version == "2.1.0"


def test_slow_registry_times_out_silently(tmp_path, local_registry):
    url = local_registry(pypi_body("2.1.0"), delay=1.0)

    def fetch(_url: str, timeout: float) -> bytes:
        return update_check.urllib_fetch(url, timeout)

    started = time.monotonic()
    status = check_for_update(
        NAME, "2.0.1", fetch=fetch, env={}, cache_dir=tmp_path, now=NOW, timeout=0.2
    )

    assert status is None
    assert time.monotonic() - started < 0.9


def test_notifier_emits_the_notice_once():
    status = UpdateStatus(NAME, "2.0.1", "2.1.0", NOW.isoformat())
    notifier = UpdateNotifier(lambda: status)
    notifier.start()

    assert notifier.status(wait=2.0) == status
    first = notifier.take_notice()
    assert first == (
        f"Update available: {NAME} 2.1.0 (running 2.0.1). "
        f"Change the pin to `uvx {NAME}@2.1.0` and reload the MCP server."
    )
    assert notifier.take_notice() is None


def test_notifier_is_quiet_before_the_check_and_when_current():
    release = threading.Event()
    status = UpdateStatus(NAME, "2.1.0", "2.1.0", NOW.isoformat())

    def check():
        release.wait(2.0)
        return status

    notifier = UpdateNotifier(check)
    notifier.start()
    assert notifier.take_notice() is None
    release.set()
    assert notifier.status(wait=2.0) == status
    assert notifier.take_notice() is None


def test_notifier_survives_a_raising_check():
    def check():
        raise RuntimeError("boom")

    notifier = UpdateNotifier(check)
    notifier.start()

    assert notifier.status(wait=2.0) is None
    assert notifier.take_notice() is None


async def _call_tool(server, name: str):
    from mcp.types import CallToolRequest, CallToolRequestParams

    handler = server._mcp_server.request_handlers[CallToolRequest]
    request = CallToolRequest(method="tools/call", params=CallToolRequestParams(name=name))
    return (await handler(request)).root


@pytest.mark.asyncio
async def test_first_tool_result_carries_the_notice_once():
    from mcp.server.fastmcp import FastMCP

    from telegram_mcp import runtime

    server = FastMCP("test")

    @server.tool()
    def ping() -> str:
        return "pong"

    @server.tool()
    def fail() -> str:
        raise ValueError("nope")

    status = UpdateStatus(NAME, "2.0.1", "2.1.0", NOW.isoformat())
    notifier = UpdateNotifier(lambda: status)
    notifier.start()
    notifier.status(wait=2.0)
    runtime._install_annotation_hook(server, notifier.take_notice)

    failed = await _call_tool(server, "fail")
    first = await _call_tool(server, "ping")
    second = await _call_tool(server, "ping")

    assert failed.isError and len(failed.content) == 1
    assert [block.text for block in first.content] == ["pong", format_notice(status)]
    assert first.content[0].annotations.audience == ["user"]
    assert first.content[1].annotations.audience == ["user", "assistant"]
    assert [block.text for block in second.content] == ["pong"]
