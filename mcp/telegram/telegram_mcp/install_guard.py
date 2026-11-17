"""Installation provenance checks for the Telegram MCP server.

This server publishes to PyPI as ``uniquedivine-telegram-mcp``. The older
distribution name ``telegram-mcp`` is occupied on PyPI by an unrelated project,
so metadata under that name is trusted only when an installer recorded an
explicit source checkout. This guard can only protect executions that reach
this repository's code; it cannot run when a user launches the third-party
package.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from importlib import metadata
from pathlib import Path
from urllib.parse import unquote, urlparse

DISTRIBUTION_NAME = "uniquedivine-telegram-mcp"
"""The PyPI distribution this repository publishes."""

LEGACY_DISTRIBUTION_NAME = "telegram-mcp"
"""The colliding PyPI name that source checkouts used before the rename."""

SOURCE_DISTRIBUTION_NAMES = (DISTRIBUTION_NAME, LEGACY_DISTRIBUTION_NAME)


class UnsafeInstallationError(RuntimeError):
    """Raised when installed package metadata points at the wrong project."""


@dataclass(frozen=True)
class DistributionIdentity:
    """Small, testable view of Python package metadata."""

    name: str
    version: str
    authors: tuple[str, ...] = ()
    maintainers: tuple[str, ...] = ()
    urls: tuple[str, ...] = ()
    summary: str = ""
    direct_url: str = ""
    source_root: Path | None = None

    @classmethod
    def from_distribution(cls, dist: metadata.Distribution) -> "DistributionIdentity":
        package_metadata = dist.metadata

        def value(name: str) -> str:
            values = package_metadata.get_all(name)
            return values[0] if values else ""

        urls = tuple(package_metadata.get_all("Project-URL") or ())
        homepage = value("Home-page")
        if homepage:
            urls += (homepage,)

        authors = tuple(
            value
            for value in (
                value("Author"),
                value("Author-email"),
            )
            if value
        )
        maintainers = tuple(
            value
            for value in (
                value("Maintainer"),
                value("Maintainer-email"),
            )
            if value
        )

        read_text = getattr(dist, "read_text", None)
        direct_url = read_text("direct_url.json") if callable(read_text) else ""

        return cls(
            name=value("Name") or getattr(dist, "name", LEGACY_DISTRIBUTION_NAME),
            version=value("Version") or dist.version,
            authors=authors,
            maintainers=maintainers,
            urls=urls,
            summary=value("Summary") or "",
            direct_url=direct_url or "",
            source_root=_distribution_source_root(dist),
        )


def _project_root_declares_distribution_name(path: Path) -> bool:
    pyproject_path = path / "pyproject.toml"
    if not pyproject_path.is_file():
        return False

    try:
        pyproject_text = pyproject_path.read_text(encoding="utf-8")
    except OSError:
        return False

    return any(f'name = "{name}"' in pyproject_text for name in SOURCE_DISTRIBUTION_NAMES)


def _resolve_path(path: Path) -> Path:
    try:
        return path.resolve()
    except OSError:
        return path


def _candidate_is_project_root(path: Path) -> bool:
    return _project_root_declares_distribution_name(_resolve_path(path))


def _distribution_source_root(dist: metadata.Distribution) -> Path | None:
    """Return an editable/source-checkout root for installer metadata.

    ``uv sync`` installs the project editably. In that mode ``importlib.metadata``
    can resolve the active distribution to ``telegram_mcp.egg-info`` in the
    checkout, while the PEP 610 ``direct_url.json`` file lives in the
    environment's ``.dist-info`` directory. Treating the adjacent project root as
    source provenance keeps the PyPI-collision guard strict for normal installs
    without blocking cloned checkouts.
    """

    dist_path = getattr(dist, "_path", None)
    if dist_path is not None:
        metadata_path = Path(dist_path)
        if metadata_path.name.endswith(".egg-info"):
            candidate = metadata_path.parent
            if _candidate_is_project_root(candidate):
                return _resolve_path(candidate)

    files = getattr(dist, "files", None)
    locate_file = getattr(dist, "locate_file", None)
    if not files or not callable(locate_file):
        return None

    for package_file in files:
        if Path(str(package_file)) != Path("pyproject.toml"):
            continue

        candidate = Path(locate_file(package_file)).parent
        if _candidate_is_project_root(candidate):
            return _resolve_path(candidate)

    return None


def _direct_url_json(direct_url: str) -> dict:
    if not direct_url:
        return {}

    try:
        direct_url_data = json.loads(direct_url)
    except json.JSONDecodeError:
        return {}

    return direct_url_data if isinstance(direct_url_data, dict) else {}


def _direct_url_is_explicit_source_install(direct_url: str) -> bool:
    direct_url_data = _direct_url_json(direct_url)
    if not direct_url_data:
        return False

    raw_url = str(direct_url_data.get("url", "")).strip()
    if not raw_url:
        return False

    parsed_url = urlparse(raw_url)

    if parsed_url.scheme == "file":
        source_path = Path(unquote(parsed_url.path)).resolve()
        return _project_root_declares_distribution_name(source_path)

    vcs_info = direct_url_data.get("vcs_info")
    return isinstance(vcs_info, dict) and bool(vcs_info.get("vcs"))


def _looks_like_explicit_source_install(identity: DistributionIdentity) -> bool:
    return (
        _direct_url_is_explicit_source_install(identity.direct_url)
        or identity.source_root is not None
    )


def _format_unsafe_installation_message(identity: DistributionIdentity) -> str:
    authors = ", ".join(identity.authors) or "unknown"
    maintainers = ", ".join(identity.maintainers) or "unknown"
    urls = "; ".join(identity.urls) or "unknown"

    return (
        f"Refusing to start: the installed {LEGACY_DISTRIBUTION_NAME!r} distribution "
        "was not installed from an explicit source checkout.\n"
        f"Detected distribution: name={identity.name!r}, version={identity.version!r}, "
        f"authors={authors!r}, maintainers={maintainers!r}, urls={urls!r}.\n"
        f"The {LEGACY_DISTRIBUTION_NAME!r} name on PyPI is owned by a different "
        "project. Remove it from this environment, then run this server from "
        f"PyPI as {DISTRIBUTION_NAME!r} (for example "
        f'"uvx {DISTRIBUTION_NAME}@<version>") or from a cloned checkout of '
        "https://github.com/Unique-Divine/jiyuu."
    )


def assert_safe_distribution(distribution_name: str = LEGACY_DISTRIBUTION_NAME) -> None:
    """Abort when ``telegram-mcp`` metadata in this environment is not this project.

    Installs of ``uniquedivine-telegram-mcp``, from PyPI or from source, are
    allowed: this repository owns that name. Source checkouts that have not
    been installed as a distribution are allowed too:
    ``uv --directory /path/to/telegram run main.py`` runs without package
    metadata. If metadata for ``telegram-mcp`` is present, it must come from an
    explicit git or file install recorded by the installer.
    """

    try:
        dist = metadata.distribution(distribution_name)
    except metadata.PackageNotFoundError:
        return

    identity = DistributionIdentity.from_distribution(dist)
    if _looks_like_explicit_source_install(identity):
        return

    raise UnsafeInstallationError(_format_unsafe_installation_message(identity))
