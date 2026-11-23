import json
import os
import shutil
import sqlite3
import sys

import pytest
from telethon.crypto import AuthKey
from telethon.sessions import StringSession
import tomlkit

from telegram_mcp import migrate, session_store
from telegram_mcp import migration_sources
from telegram_mcp.migration_sources import read_dotenv, read_mcp_config

API_HASH = "ac" * 16


def session_string(marker=b"d"):
    session = StringSession()
    session.set_dc(2, "192.0.2.25", 443)
    session.auth_key = AuthKey(marker * 256)
    return session.save()


def dotenv(path, sessions=None):
    entries = ["# kept", "OTHER=setting", "TELEGRAM_API_ID=123", f"TELEGRAM_API_HASH={API_HASH}"]
    for key, value in (sessions or {"TELEGRAM_SESSION_STRING": session_string()}).items():
        entries.append(f"{key}={value}")
    path.write_bytes(("\r\n".join(entries) + "\r\n").encode())
    return path


def invoke(monkeypatch, *args):
    monkeypatch.setattr(sys, "argv", ["telegram-mcp-migrate", *map(os.fspath, args)])
    migrate.main()


@pytest.fixture
def isolated(monkeypatch, tmp_path):
    state_path = tmp_path / "boku"
    monkeypatch.setenv("BOKU_HOME", str(state_path))
    saved = {}

    def read(_state, label):
        if label not in saved:
            raise session_store.ProfileMissing("missing")
        return saved[label]

    async def verify(session, api_id, api_hash):
        assert api_id == 123
        assert api_hash == API_HASH
        return session.auth_key.key[0]

    monkeypatch.setattr(migrate, "read_profile", read)
    monkeypatch.setattr(
        migrate, "write_profile", lambda _, label, profile: saved.update({label: profile})
    )
    monkeypatch.setattr(migrate, "verify_authorization", verify)
    return state_path, saved


def test_preview_checks_selected_sources_without_writes(monkeypatch, tmp_path, capsys, isolated):
    state_path, saved = isolated
    env = dotenv(tmp_path / ".env")
    original = env.read_bytes()
    monkeypatch.setenv("TELEGRAM_SESSION_STRING_OTHER", "never-print-this")
    invoke(monkeypatch, "--env-file", env)
    output = capsys.readouterr().out
    assert "Preview only" in output
    assert "TELEGRAM_SESSION_STRING_OTHER" in output
    assert not any(value in output for value in ("never-print-this", session_string(), API_HASH))
    assert env.read_bytes() == original
    assert not state_path.exists()
    assert not saved


@pytest.mark.parametrize("kind", ["toml", "json"])
def test_apply_preserves_unrelated_settings_and_can_rerun(
    monkeypatch, tmp_path, capsys, isolated, kind
):
    _, saved = isolated
    env = dotenv(tmp_path / ".env")
    launch = tmp_path / f"config.{kind}"
    unrelated = {
        "command": "unrelated",
        "env": {"TELEGRAM_API_HASH": "unrelated", "OTHER": "same"},
    }
    settings = {
        "TELEGRAM_SESSION_STRING_TEAM": session_string(b"t"),
        "TELEGRAM_PROXY_HOST": "internal",
    }
    servers = {
        "peer": unrelated,
        "chosen": {"command": "uvx", "args": ["telegram-mcp"], "env": settings},
    }
    if kind == "toml":
        launch.write_text("# Keep this comment\n" + tomlkit.dumps({"mcp_servers": servers}))
    else:
        launch.write_text(json.dumps({"mcpServers": servers}))
    args = ("--env-file", env, "--mcp-config", launch, "--server", "chosen", "--apply")
    invoke(monkeypatch, *args)
    assert {label: profile.user_id for label, profile in saved.items()} == {
        "default": 100,
        "team": 116,
    }
    assert (
        env.read_bytes() == b"# kept\r\nOTHER=setting\r\nTELEGRAM_STORED_ACCOUNTS=default,team\r\n"
    )
    if kind == "toml":
        assert launch.read_text().startswith("# Keep this comment\n")
        assert tomlkit.parse(launch.read_text()).unwrap()["mcp_servers"]["peer"] == unrelated
    else:
        assert json.loads(launch.read_text())["mcpServers"]["peer"] == unrelated
    assert read_mcp_config(launch, "chosen").values == {
        "TELEGRAM_PROXY_HOST": "internal",
        "TELEGRAM_STORED_ACCOUNTS": "default,team",
    }
    before = (env.stat().st_ino, launch.stat().st_ino)
    invoke(monkeypatch, *args)
    assert (env.stat().st_ino, launch.stat().st_ino) == before
    output = capsys.readouterr().out
    assert "proxy" in output
    assert all(value not in output for value in ("internal", API_HASH, session_string(b"t")))


def make_database(path):
    with sqlite3.connect(path) as db:
        db.execute("CREATE TABLE sessions (dc_id, server_address, port, auth_key)")
        db.execute("INSERT INTO sessions VALUES (2, '192.0.2.25', 443, ?)", (b"d" * 256,))


def test_file_cleanup_after_verified_migration(monkeypatch, tmp_path, isolated):
    _, saved = isolated
    env = dotenv(tmp_path / ".env", {"TELEGRAM_SESSION_NAME": "legacy"})
    legacy = tmp_path / "legacy.session"
    make_database(legacy)
    sidecar = tmp_path / "legacy.session-wal"
    sidecar.touch()
    args = ("--env-file", env, "--cwd", tmp_path, "--session", f"default={legacy}", "--apply")
    invoke(monkeypatch, *args)
    assert saved["default"].session_string == session_string()
    assert legacy.exists()
    invoke(monkeypatch, *args, "--remove-session-files")
    assert not legacy.exists()
    assert not sidecar.exists()
    invoke(monkeypatch, *args, "--remove-session-files")


def test_active_journal_is_left_alone(monkeypatch, tmp_path, isolated):
    env = dotenv(tmp_path / ".env", {"TELEGRAM_SESSION_NAME": "legacy"})
    original = env.read_bytes()
    legacy = tmp_path / "legacy.session"
    make_database(legacy)
    sidecar = tmp_path / "legacy.session-wal"
    sidecar.write_bytes(b"synthetic pending transaction")
    with pytest.raises(SystemExit, match="active journal"):
        invoke(
            monkeypatch,
            "--env-file",
            env,
            "--cwd",
            tmp_path,
            "--session",
            f"default={legacy}",
            "--apply",
            "--remove-session-files",
        )
    assert env.read_bytes() == original
    assert legacy.exists() and sidecar.exists()


def test_collision_keeps_original_settings(monkeypatch, tmp_path, isolated):
    _, saved = isolated
    saved["default"] = session_store.Profile(123, API_HASH, session_string(b"a"), 97)
    original_profile = saved["default"]
    env = dotenv(tmp_path / ".env")
    original = env.read_bytes()
    with pytest.raises(SystemExit, match="different encrypted profile"):
        invoke(monkeypatch, "--env-file", env, "--apply")
    assert saved["default"] == original_profile
    assert env.read_bytes() == original


def test_resume_after_first_config_was_replaced(monkeypatch, tmp_path, isolated):
    _, saved = isolated
    env = dotenv(tmp_path / ".env", {"TELEGRAM_SESSION_STRING": session_string()})
    launch = tmp_path / "settings.json"
    launch.write_text(
        json.dumps(
            {
                "mcpServers": {
                    "telegram": {
                        "command": "telegram-mcp",
                        "env": {"TELEGRAM_SESSION_STRING_TEAM": session_string(b"t")},
                    }
                }
            }
        )
    )
    original_launch = launch.read_bytes()
    replace = os.replace

    def interrupt(source, destination):
        if destination == launch:
            raise OSError("simulated interruption")
        replace(source, destination)

    args = ("--env-file", env, "--mcp-config", launch, "--server", "telegram", "--apply")
    with monkeypatch.context() as patch:
        patch.setattr(migration_sources.os, "replace", interrupt)
        with pytest.raises(SystemExit, match="could not finish"):
            invoke(patch, *args)
    assert read_dotenv(env).values == {"TELEGRAM_STORED_ACCOUNTS": "default,team"}
    assert launch.read_bytes() == original_launch
    original_profiles = saved.copy()
    invoke(monkeypatch, *args)
    assert read_mcp_config(launch, "telegram").values == {
        "TELEGRAM_STORED_ACCOUNTS": "default,team"
    }
    assert saved == original_profiles
    assert not list(tmp_path.glob(".telegram-migrate-*"))


@pytest.mark.parametrize("failure", ["identity", "reopen", "concurrent_edit"])
def test_failed_migration_keeps_source(monkeypatch, tmp_path, isolated, failure):
    _, saved = isolated
    env = dotenv(tmp_path / ".env")
    original = env.read_bytes()

    if failure == "reopen":
        monkeypatch.setattr(migrate, "write_profile", lambda *_: None)
    else:

        async def verify(*_args):
            if failure == "identity":
                raise RuntimeError("sensitive verifier diagnostics")
            env.write_bytes(original + b"USER_EDIT=1\n")
            return 100

        monkeypatch.setattr(migrate, "verify_authorization", verify)

    with pytest.raises(SystemExit) as caught:
        invoke(monkeypatch, "--env-file", env, "--apply")
    assert "sensitive verifier diagnostics" not in str(caught.value)
    suffix = b"USER_EDIT=1\n" if failure == "concurrent_edit" else b""
    assert env.read_bytes() == original + suffix
    if failure != "concurrent_edit":
        assert not saved


@pytest.mark.parametrize("assignment", ["TELEGRAM_API_ID=456", "RETAIN=${TELEGRAM_API_HASH}"])
def test_ambiguous_dotenv_is_left_alone(monkeypatch, tmp_path, isolated, assignment):
    env = dotenv(tmp_path / ".env")
    with env.open("a") as stream:
        stream.write(assignment + "\n")
    original = env.read_bytes()
    with pytest.raises(SystemExit):
        invoke(monkeypatch, "--env-file", env, "--apply")
    assert env.read_bytes() == original


def test_real_store_round_trip_when_helper_installed(monkeypatch, tmp_path):
    if not shutil.which("local-secret-store"):
        pytest.skip("local-secret-store executable is not on PATH")
    monkeypatch.setenv("BOKU_HOME", str(tmp_path / "boku"))
    env = dotenv(tmp_path / ".env")

    async def verify(*_args):
        return 42

    monkeypatch.setattr(migrate, "verify_authorization", verify)
    invoke(monkeypatch, "--env-file", env, "--apply")
    state = session_store.State.discover()
    assert session_store.read_profile(state, "default").user_id == 42
    assert read_dotenv(env).values == {"TELEGRAM_STORED_ACCOUNTS": "default"}
    assert all(
        API_HASH.encode() not in path.read_bytes()
        and session_string().encode() not in path.read_bytes()
        for path in (state.tool / "secrets").iterdir()
        if path.is_file()
    )
