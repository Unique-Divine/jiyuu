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
    is_newer,
)

NAME = "uniquedivine-telegram-mcp"
NOW = datetime(2026, 11, 17, 12, 0, tzinfo=timezone.utc)


def pypi_body(version: str) -> bytes:
    return json.dumps({"info": {"name": NAME, "version": version}}).encode()


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
    ],
)
def test_is_newer_orders_versions(candidate, running, expected):
    assert is_newer(candidate, running) is expected


def test_running_version_newer_than_registry_means_no_update():
    status = UpdateStatus(NAME, "2.2.0", "2.1.0", NOW.isoformat())

    assert status.update_available is False


def test_cache_miss_fetches_pypi_and_writes_cache(tmp_path):
    fetch = RecordingFetcher(pypi_body("2.1.0"))

    status = check_for_update(NAME, "2.0.1", fetch=fetch, env={}, cache_dir=tmp_path, now=NOW)

    assert fetch.urls == [f"https://pypi.org/pypi/{NAME}/json"]
    assert status == UpdateStatus(NAME, "2.0.1", "2.1.0", "2026-11-17T12:00:00+00:00")
    assert status.update_available
    assert status.pin_command == f"uvx {NAME}@2.1.0"
    cached = json.loads((tmp_path / f"{NAME}.json").read_text())
    assert cached == {
        "name": NAME,
        "latestVersion": "2.1.0",
        "checkedAt": "2026-11-17T12:00:00+00:00",
    }


def test_cache_hit_within_a_day_skips_the_registry(tmp_path):
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
        NAME, "2.0.1", fetch=fetch, env={}, cache_dir=tmp_path, now=NOW + timedelta(hours=23)
    )

    assert fetch.urls == []
    assert status is not None and status.latest_version == "2.1.0"
    assert status.checked_at == "2026-11-17T12:00:00+00:00"


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
        NAME, "2.0.1", fetch=fetch, env={}, cache_dir=tmp_path, now=NOW + timedelta(hours=24)
    )

    assert len(fetch.urls) == 1
    assert status is not None and status.latest_version == "2.2.0"


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
