"""mcp_info: identity, provenance, update fields, and no secret values."""

import json
import subprocess
from datetime import datetime, timezone
from pathlib import Path
from types import SimpleNamespace

import pytest

import main  # noqa: F401 - registers every tool, including mcp_info
from telegram_mcp import runtime, server_info
from telegram_mcp.server_info import PackageIdentity, build_mcp_info
from telegram_mcp.tools.info import mcp_info
from telegram_mcp.update_check import UpdateNotifier, UpdateStatus

NAME = "uniquedivine-telegram-mcp"
CHECKED_AT = datetime(2026, 11, 17, 12, tzinfo=timezone.utc).isoformat()
TELEGRAM_ROOT = Path(__file__).resolve().parents[1]

SECRETS = {
    "TELEGRAM_SESSION_STRING": "1BVtsOK8Bu-fake-default-session-string-value",
    "TELEGRAM_SESSION_STRING_WORK": "1BVtsOK8Bu-fake-work-session-string-value",
    "TELEGRAM_API_HASH": "fake0123456789abcdef0123456789ab",
    "TELEGRAM_API_ID": "987654",
}


def fake_client(device_model: str, app_version: str):
    init = SimpleNamespace(
        api_id=987654, device_model=device_model, system_version="6.18", app_version=app_version
    )
    return SimpleNamespace(_init_request=init)


def write_pyproject(root: Path, name: str, extra: str = "") -> None:
    root.mkdir(parents=True, exist_ok=True)
    (root / "pyproject.toml").write_text(
        f'[project]\nname = "{name}"\nversion = "2.0.1"\n\n'
        '[tool.setuptools]\npackages = ["telegram_mcp", "telegram_mcp.tools"]\n' + extra
    )


def test_source_identity_reads_name_version_and_upstream(tmp_path):
    write_pyproject(
        tmp_path,
        NAME,
        '\n[tool.jiyuu.upstream]\nrepository = "chigwell/telegram-mcp"\nversion = "2.0.1"\n',
    )

    identity = server_info.read_source_identity(tmp_path)

    assert identity == PackageIdentity(
        name=NAME,
        version="2.0.1",
        provenance="source-checkout",
        source_root=tmp_path,
        upstream={"repository": "chigwell/telegram-mcp", "version": "2.0.1"},
    )


def test_source_identity_ignores_unrelated_pyproject(tmp_path):
    (tmp_path / "pyproject.toml").write_text('[project]\nname = "other"\nversion = "1.0.0"\n')

    assert server_info.read_source_identity(tmp_path) is None


def test_this_checkout_reads_its_own_pyproject():
    identity = server_info.resolve_identity(TELEGRAM_ROOT)

    assert identity.provenance == "source-checkout"
    assert identity.source_root == TELEGRAM_ROOT
    assert identity.version


def test_pick_distribution_prefers_the_published_name():
    assert server_info.pick_distribution(["telegram-mcp", NAME, "telegram-mcp"]) == NAME
    assert server_info.pick_distribution(["telegram-mcp"]) == "telegram-mcp"
    assert server_info.pick_distribution([]) is None


def test_installed_identity_reads_real_distribution_metadata():
    # uv sync installs this checkout, so its metadata is really present.
    identity = server_info.read_installed_identity()
    source = server_info.read_source_identity(TELEGRAM_ROOT)

    assert identity is not None and source is not None
    assert identity.provenance == "installed"
    assert (identity.name, identity.version) == (source.name, source.version)
    assert server_info.read_installed_identity({}) is None


def test_git_state_reports_commit_and_dirty_flag(tmp_path):
    run = lambda *args: subprocess.run(  # noqa: E731
        ["git", "-C", str(tmp_path), *args], check=True, capture_output=True
    )
    run("init", "-q")
    (tmp_path / "a.txt").write_text("a")
    run("add", "a.txt")
    run("-c", "user.name=t", "-c", "user.email=t@example.com", "commit", "-qm", "init")
    commit = subprocess.run(
        ["git", "-C", str(tmp_path), "rev-parse", "HEAD"], capture_output=True, text=True
    ).stdout.strip()

    assert server_info.git_state(tmp_path) == {"commit": commit, "dirty": False}
    (tmp_path / "a.txt").write_text("b")
    assert server_info.git_state(tmp_path) == {"commit": commit, "dirty": True}


def test_git_state_outside_a_checkout_is_none(tmp_path):
    assert server_info.git_state(tmp_path) is None


def test_build_mcp_info_reports_update_and_accounts():
    identity = PackageIdentity(NAME, "2.0.1", "installed", upstream={"url": "x"})
    status = UpdateStatus(NAME, "2.0.1", "2.1.0", CHECKED_AT)
    clients = {
        "default": fake_client("Jiyuu MCP", "1.0"),
        "work": fake_client("Jiyuu MCP Work", "1.0"),
    }

    report = build_mcp_info(identity, status=status, git=None, clients=clients, env=SECRETS)

    assert report["name"] == NAME
    assert report["version"] == "2.0.1"
    assert report["provenance"] == {"kind": "installed", "sourceRoot": None, "git": None}
    assert report["upstream"] == {"url": "x"}
    assert report["update"] == {
        "latestVersion": "2.1.0",
        "updateAvailable": True,
        "checkedAt": CHECKED_AT,
        "pinCommand": f"uvx {NAME}@2.1.0",
        "source": "live",
        "notChecked": None,
    }
    work = report["accounts"][1]
    assert work["label"] == "work"
    assert work["deviceModel"] == "Jiyuu MCP Work"
    assert work["sessionEnvVar"] == "TELEGRAM_SESSION_STRING_WORK"
    assert work["sessionKind"] == "string"
    assert work["sessionFingerprint"] == server_info.fingerprint(
        SECRETS["TELEGRAM_SESSION_STRING_WORK"]
    )
    assert report["accounts"][0]["sessionEnvVar"] == "TELEGRAM_SESSION_STRING"
    assert report["envVarsSet"] == sorted(SECRETS)


@pytest.mark.parametrize(
    ("name", "env", "expected"),
    [
        ("telegram-mcp", {}, "'telegram-mcp' is not a name Jiyuu publishes"),
        (NAME, {"JIYUU_MCP_NO_UPDATE_CHECK": "1"}, "disabled by JIYUU_MCP_NO_UPDATE_CHECK"),
        (NAME, {}, "registry unreachable and no cached result"),
    ],
)
def test_update_fields_explain_a_missing_check(name, env, expected):
    identity = PackageIdentity(name, "2.0.1", "source-checkout")

    report = build_mcp_info(identity, status=None, git=None, clients={}, env=env)

    assert report["update"]["notChecked"] == expected
    assert report["update"]["updateAvailable"] is False
    assert report["update"]["latestVersion"] is None


def test_build_mcp_info_never_contains_secret_values():
    identity = PackageIdentity(NAME, "2.0.1", "installed")
    clients = {"default": fake_client("Jiyuu", "1.0"), "work": fake_client("Jiyuu", "1.0")}

    text = json.dumps(
        build_mcp_info(identity, status=None, git=None, clients=clients, env=SECRETS)
    )

    for value in SECRETS.values():
        assert value not in text


@pytest.mark.asyncio
async def test_mcp_info_tool_is_registered_and_leaks_no_secrets(monkeypatch):
    for name, value in SECRETS.items():
        monkeypatch.setenv(name, value)
    monkeypatch.setattr(server_info, "_notifier", UpdateNotifier(lambda: None))
    monkeypatch.setattr(server_info, "live_update_check", lambda identity, env: None)

    tool_names = {tool.name for tool in runtime.mcp._tool_manager.list_tools()}
    text = await mcp_info()

    assert "mcp_info" in tool_names
    for value in SECRETS.values():
        assert value not in text
    report = json.loads(text)
    assert report["server"] == "telegram"
    assert report["provenance"]["kind"] == "source-checkout"
    assert "TELEGRAM_SESSION_STRING_WORK" in report["envVarsSet"]


@pytest.mark.asyncio
async def test_mcp_info_tool_runs_the_live_check_on_every_call(monkeypatch):
    calls = []

    def live(identity, env):
        calls.append(identity.version)
        return UpdateStatus(identity.name, identity.version, identity.version, CHECKED_AT, "live")

    monkeypatch.setattr(server_info, "live_update_check", live)

    first = json.loads(await mcp_info())
    await mcp_info()

    assert len(calls) == 2
    assert first["update"]["source"] == "live"
    assert first["update"]["updateAvailable"] is False
    assert first["update"]["pinCommand"] is None


def test_instructions_point_agents_at_mcp_info():
    assert "mcp_info" in (runtime.mcp.instructions or "")
