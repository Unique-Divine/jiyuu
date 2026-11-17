"""Tell the user when a newer published release of this server exists.

The server never updates itself: the MCP client config pins an exact version,
so what runs is what was reviewed. This module reads the registry's latest
version in the background, caches it for a day, and stays silent on any
failure. Set ``JIYUU_MCP_NO_UPDATE_CHECK=1`` to turn the check off.

Only names that Jiyuu publishes are checked. Before the PyPI rename the
distribution was ``telegram-mcp``, a name an unrelated project owns, and its
latest version says nothing about this server.
"""

from __future__ import annotations

import json
import os
import re
import threading
import urllib.request
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Callable, Mapping, Optional

from packaging.version import InvalidVersion, Version

OPT_OUT_ENV = "JIYUU_MCP_NO_UPDATE_CHECK"
CACHE_TTL = timedelta(hours=24)
FETCH_TIMEOUT_SECONDS = 2.0
PUBLISHED_PREFIX = "uniquedivine-"
USER_AGENT = "jiyuu-mcp-update-check"

Fetcher = Callable[[str, float], bytes]
"""Return the response body for a URL, or raise on any failure."""


@dataclass(frozen=True)
class UpdateStatus:
    """The result of one update check."""

    name: str
    running_version: str
    latest_version: Optional[str]
    checked_at: Optional[str]

    @property
    def update_available(self) -> bool:
        if self.latest_version is None:
            return False
        return is_newer(self.latest_version, self.running_version)

    @property
    def pin_command(self) -> Optional[str]:
        if self.latest_version is None:
            return None
        return pin_command(self.name, self.latest_version)


def pin_command(name: str, version: str) -> str:
    """Return the exact-version launch command for a published release."""
    return f"uvx {name}@{version}"


def is_published_name(name: str) -> bool:
    """Report whether Jiyuu publishes this distribution name on PyPI."""
    return name.startswith(PUBLISHED_PREFIX)


def check_disabled(env: Mapping[str, str]) -> bool:
    value = env.get(OPT_OUT_ENV, "").strip().lower()
    return value not in ("", "0", "false", "no")


def parse_version(version: str) -> Optional[Version]:
    """Parse a PEP 440 version, or return ``None`` for anything else."""
    try:
        return Version(version)
    except InvalidVersion:
        return None


def is_newer(candidate: str, running: str) -> bool:
    """Report whether ``candidate`` sorts after ``running`` under PEP 440.

    PEP 440 orders pre-, post- and dev releases, so a running ``2.0.1.post1``
    is ahead of ``2.0.1``. Invalid versions never count as newer, so a bad
    registry response produces no notice.
    """
    parsed_candidate = parse_version(candidate)
    parsed_running = parse_version(running)
    if parsed_candidate is None or parsed_running is None:
        return False
    return parsed_candidate > parsed_running


def latest_version_url(name: str) -> str:
    return f"https://pypi.org/pypi/{name}/json"


def parse_latest_version(body: bytes) -> Optional[str]:
    try:
        data = json.loads(body)
    except (ValueError, UnicodeDecodeError):
        return None
    info = data.get("info") if isinstance(data, dict) else None
    version = info.get("version") if isinstance(info, dict) else None
    if not isinstance(version, str) or parse_version(version) is None:
        return None
    return version


def default_cache_dir(env: Mapping[str, str]) -> Path:
    base = env.get("XDG_CACHE_HOME") or str(Path.home() / ".cache")
    return Path(base) / "jiyuu-mcp"


def cache_file(cache_dir: Path, name: str) -> Path:
    safe_name = re.sub(r"[^A-Za-z0-9._-]", "_", name)
    return cache_dir / f"{safe_name}.json"


def read_cache(path: Path, name: str, now: datetime) -> Optional[tuple[str, str]]:
    """Return ``(latest_version, checked_at)`` from a fresh cache entry."""
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
        if data.get("name") != name:
            return None
        latest = data["latestVersion"]
        checked_at = data["checkedAt"]
        checked = datetime.fromisoformat(checked_at)
    except (OSError, ValueError, KeyError, TypeError, AttributeError):
        return None
    if not isinstance(latest, str) or checked.tzinfo is None:
        return None
    age = now - checked
    if age < timedelta(0) or age >= CACHE_TTL:
        return None
    return latest, checked_at


def write_cache(path: Path, name: str, latest: str, checked_at: str) -> None:
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        payload = {"name": name, "latestVersion": latest, "checkedAt": checked_at}
        tmp = path.with_suffix(f".{os.getpid()}.tmp")
        tmp.write_text(json.dumps(payload) + "\n", encoding="utf-8")
        tmp.replace(path)
    except OSError:
        pass


def urllib_fetch(url: str, timeout: float) -> bytes:
    request = urllib.request.Request(
        url, headers={"Accept": "application/json", "User-Agent": USER_AGENT}
    )
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return response.read()


def check_for_update(
    name: str,
    running_version: str,
    *,
    fetch: Fetcher = urllib_fetch,
    env: Optional[Mapping[str, str]] = None,
    cache_dir: Optional[Path] = None,
    now: Optional[datetime] = None,
    timeout: float = FETCH_TIMEOUT_SECONDS,
) -> Optional[UpdateStatus]:
    """Return the latest published version, from cache or the registry.

    Returns ``None`` when the check is off, the name is not Jiyuu's, or the
    registry can't be read. Never raises.
    """
    env = os.environ if env is None else env
    if check_disabled(env) or not is_published_name(name):
        return None
    try:
        now = now or datetime.now(timezone.utc)
        path = cache_file(cache_dir or default_cache_dir(env), name)
        cached = read_cache(path, name, now)
        if cached is not None:
            return UpdateStatus(name, running_version, cached[0], cached[1])
        latest = parse_latest_version(fetch(latest_version_url(name), timeout))
        if latest is None:
            return None
        checked_at = now.isoformat(timespec="seconds")
        write_cache(path, name, latest, checked_at)
        return UpdateStatus(name, running_version, latest, checked_at)
    except Exception:
        return None


def format_notice(status: UpdateStatus) -> str:
    return (
        f"Update available: {status.name} {status.latest_version} "
        f"(running {status.running_version}). Change the pin to "
        f"`{status.pin_command}` and reload the MCP server."
    )


class UpdateNotifier:
    """Run one background check per process and hand out its notice once."""

    def __init__(self, check: Callable[[], Optional[UpdateStatus]]):
        self._check = check
        self._lock = threading.Lock()
        self._done = threading.Event()
        self._thread: Optional[threading.Thread] = None
        self._status: Optional[UpdateStatus] = None
        self._notice_sent = False

    def start(self) -> None:
        with self._lock:
            if self._thread is not None:
                return
            self._thread = threading.Thread(
                target=self._run, name="jiyuu-mcp-update-check", daemon=True
            )
            self._thread.start()

    def _run(self) -> None:
        try:
            self._status = self._check()
        except Exception:
            self._status = None
        finally:
            self._done.set()

    def status(self, wait: float = 0.0) -> Optional[UpdateStatus]:
        """Return the result, waiting up to ``wait`` seconds for the check."""
        if wait > 0 and self._thread is not None:
            self._done.wait(wait)
        return self._status if self._done.is_set() else None

    def take_notice(self) -> Optional[str]:
        """Return the notice once, after the check finds a newer release."""
        status = self.status()
        if status is None or not status.update_available:
            return None
        with self._lock:
            if self._notice_sent:
                return None
            self._notice_sent = True
        return format_notice(status)
