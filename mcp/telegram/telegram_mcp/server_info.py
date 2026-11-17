"""Build the ``mcp_info`` report: what runs, from where, and whether it's current.

Agents call ``mcp_info`` instead of reading MCP client configs, which hold
secrets. The report therefore carries names, paths and short fingerprints,
never secret values.
"""

from __future__ import annotations

import hashlib
import os
import platform
import subprocess
import sys
from dataclasses import dataclass
from importlib import metadata
from pathlib import Path
from typing import Any, Mapping, Optional

from telegram_mcp.update_check import (
    UpdateNotifier,
    UpdateStatus,
    check_disabled,
    check_for_update,
    is_published_name,
)

PACKAGE_MODULE = "telegram_mcp"
SOURCE_ROOT = Path(__file__).resolve().parents[1]
FINGERPRINT_LENGTH = 12


@dataclass(frozen=True)
class PackageIdentity:
    """Name, version and install provenance of the running server."""

    name: str
    version: str
    provenance: str
    """``source-checkout``, ``installed`` or ``unknown``."""
    source_root: Optional[Path] = None
    upstream: Optional[dict[str, Any]] = None


def _load_toml(path: Path) -> Optional[dict[str, Any]]:
    if sys.version_info < (3, 11):
        return None
    import tomllib

    try:
        with path.open("rb") as handle:
            return tomllib.load(handle)
    except (OSError, tomllib.TOMLDecodeError):
        return None


def read_source_identity(root: Path) -> Optional[PackageIdentity]:
    """Read name, version and upstream from a checkout's ``pyproject.toml``.

    The file counts only when it declares this package, so a stray
    ``pyproject.toml`` in ``site-packages`` can't pose as a checkout.
    """
    data = _load_toml(root / "pyproject.toml")
    if data is None:
        return None
    project = data.get("project", {})
    packages = data.get("tool", {}).get("setuptools", {}).get("packages", [])
    name, version = project.get("name"), project.get("version")
    if PACKAGE_MODULE not in packages or not isinstance(name, str):
        return None
    if not isinstance(version, str):
        return None
    upstream = data.get("tool", {}).get("jiyuu", {}).get("upstream")
    return PackageIdentity(
        name=name,
        version=version,
        provenance="source-checkout",
        source_root=root,
        upstream=dict(upstream) if isinstance(upstream, dict) else None,
    )


def _upstream_from_urls(urls: list[str]) -> Optional[dict[str, Any]]:
    for entry in urls:
        label, _, url = entry.partition(",")
        if label.strip().lower() == "upstream" and url.strip():
            return {"url": url.strip()}
    return None


def pick_distribution(names: list[str]) -> Optional[str]:
    """Prefer the name Jiyuu publishes if an old install left a second one."""
    ordered = sorted(dict.fromkeys(names), key=lambda name: not is_published_name(name))
    return ordered[0] if ordered else None


def read_installed_identity(
    distributions: Optional[Mapping[str, list[str]]] = None,
) -> Optional[PackageIdentity]:
    """Find the installed distribution that provides ``telegram_mcp``."""
    try:
        mapping = metadata.packages_distributions() if distributions is None else distributions
        name = pick_distribution(mapping.get(PACKAGE_MODULE, []))
        if name is None:
            return None
        dist = metadata.distribution(name)
        urls = list(dist.metadata.get_all("Project-URL") or [])
        return PackageIdentity(
            name=dist.metadata["Name"] or name,
            version=dist.version,
            provenance="installed",
            upstream=_upstream_from_urls(urls),
        )
    except Exception:
        return None


def resolve_identity(root: Path = SOURCE_ROOT) -> PackageIdentity:
    return (
        read_source_identity(root)
        or read_installed_identity()
        or PackageIdentity(name="unknown", version="0.0.0", provenance="unknown")
    )


def git_state(root: Path) -> Optional[dict[str, Any]]:
    """Return the commit and dirty flag for a checkout, or ``None``."""

    def git(*args: str) -> str:
        return subprocess.run(
            ["git", "-C", str(root), *args],
            capture_output=True,
            text=True,
            timeout=2,
            check=True,
        ).stdout.strip()

    try:
        commit = git("rev-parse", "HEAD")
        dirty = bool(git("status", "--porcelain", "--", "."))
    except (OSError, subprocess.SubprocessError):
        return None
    return {"commit": commit, "dirty": dirty}


def fingerprint(secret: str) -> str:
    """Short, one-way tag that tells two sessions apart without revealing them."""
    return hashlib.sha256(secret.encode()).hexdigest()[:FINGERPRINT_LENGTH]


def _session_env_name(label: str, env: Mapping[str, str]) -> Optional[str]:
    """Mirror ``runtime._discover_accounts``: suffixed names win."""
    names = [f"TELEGRAM_SESSION_STRING_{label.upper()}", f"TELEGRAM_SESSION_NAME_{label.upper()}"]
    if label == "default":
        names += ["TELEGRAM_SESSION_STRING", "TELEGRAM_SESSION_NAME"]
    return next((name for name in names if env.get(name)), None)


def account_info(label: str, client: Any, env: Mapping[str, str]) -> dict[str, Any]:
    """Describe one account by device identity and session fingerprint."""
    init = getattr(client, "_init_request", None)
    env_name = _session_env_name(label, env)
    session_value = env.get(env_name, "") if env_name else ""
    return {
        "label": label,
        "deviceModel": getattr(init, "device_model", None),
        "systemVersion": getattr(init, "system_version", None),
        "appVersion": getattr(init, "app_version", None),
        "sessionEnvVar": env_name,
        "sessionKind": (
            "string" if env_name and "SESSION_STRING" in env_name else "file" if env_name else None
        ),
        "sessionFingerprint": fingerprint(session_value) if session_value else None,
    }


def update_info(
    identity: PackageIdentity,
    status: Optional[UpdateStatus],
    env: Mapping[str, str],
) -> dict[str, Any]:
    if check_disabled(env):
        reason = "disabled by JIYUU_MCP_NO_UPDATE_CHECK"
    elif not is_published_name(identity.name):
        reason = f"{identity.name!r} is not a name Jiyuu publishes"
    elif status is None:
        reason = "no registry result yet (offline, pending, or failed)"
    else:
        reason = None
    return {
        "latestVersion": status.latest_version if status else None,
        "updateAvailable": status.update_available if status else False,
        "checkedAt": status.checked_at if status else None,
        "pinCommand": status.pin_command if status else None,
        "notChecked": reason,
    }


def build_mcp_info(
    identity: PackageIdentity,
    *,
    status: Optional[UpdateStatus],
    git: Optional[dict[str, Any]],
    clients: Mapping[str, Any],
    env: Mapping[str, str],
) -> dict[str, Any]:
    """Assemble the ``mcp_info`` report from already-gathered facts."""
    return {
        "server": "telegram",
        "name": identity.name,
        "version": identity.version,
        "provenance": {
            "kind": identity.provenance,
            "sourceRoot": str(identity.source_root) if identity.source_root else None,
            "git": git,
        },
        "upstream": identity.upstream,
        "update": update_info(identity, status, env),
        "runtime": {
            "python": platform.python_version(),
            "executable": sys.executable,
            "pid": os.getpid(),
        },
        "accounts": [account_info(label, client, env) for label, client in clients.items()],
        "envVarsSet": sorted(
            name for name, value in env.items() if name.startswith("TELEGRAM_") and value
        ),
    }


_identity: Optional[PackageIdentity] = None
_notifier: Optional[UpdateNotifier] = None


def current_identity() -> PackageIdentity:
    global _identity
    if _identity is None:
        _identity = resolve_identity()
    return _identity


def update_notifier() -> UpdateNotifier:
    """The process-wide notifier; ``start()`` it once at server startup."""
    global _notifier
    if _notifier is None:
        identity = current_identity()
        _notifier = UpdateNotifier(lambda: check_for_update(identity.name, identity.version))
    return _notifier
