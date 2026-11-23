"""Migrate selected Telegram MCP launch settings into encrypted profiles."""

from __future__ import annotations

import argparse
import asyncio
from contextlib import ExitStack
from dataclasses import dataclass
import os
from pathlib import Path
import re
from typing import NoReturn

from telethon.sessions import StringSession

from .migration_sources import (
    API_KEYS,
    SESSION_KEY,
    STORED,
    ConfigSource,
    FileSnapshot,
    SQLiteSource,
    credential_key,
    read_dotenv,
    read_mcp_config,
    replace_configs,
    session_absent,
)
from .session_store import (
    Profile,
    ProfileMissing,
    State,
    StoreError,
    lock_authorization,
    lock_profile_writes,
    read_profile,
    valid_name,
    verify_authorization,
    write_profile,
)


class MigrationParser(argparse.ArgumentParser):
    def error(self, message: str) -> NoReturn:
        self.exit(2, "Invalid migration arguments. Run telegram-mcp-migrate --help.\n")


@dataclass(frozen=True, repr=False)
class Migration:
    labels: tuple[str, ...]
    strings: dict[str, str]
    sessions: dict[str, SQLiteSource]
    api: dict[str, str]
    inputs: tuple[FileSnapshot, ...]
    updates: list[tuple[FileSnapshot, bytes]]


def _record(values: dict[str, str], key: str, value: str) -> None:
    if key in values and values[key] != value:
        raise StoreError(
            "Selected legacy settings disagree. Resolve the conflict before migrating"
        )
    values[key] = value


def _legacy_session_path(value: str, cwd: Path) -> Path:
    path = Path(value)
    if not value.endswith(".session"):
        path = Path(value + ".session")
    return Path(os.path.abspath(cwd / path))


def _selected_sessions(pairs: list[str]) -> dict[str, Path]:
    selected: dict[str, Path] = {}
    for pair in pairs:
        label, separator, filename = pair.partition("=")
        if not separator or not filename or label in selected:
            raise StoreError("Use --session LABEL=PATH once for each selected SQLite session")
        selected[valid_name(label)] = Path(os.path.abspath(Path(filename).expanduser()))
    if len(set(selected.values())) != len(selected):
        raise StoreError("Select each SQLite session file only once")
    return selected


def plan(sources: list[ConfigSource], selected: dict[str, Path], cwd: Path) -> Migration:
    stored: set[str] = set()
    api: dict[str, str] = {}
    strings: dict[str, str] = {}
    names: dict[str, str] = {}
    for source in sources:
        stored.update(
            valid_name(label.strip())
            for label in source.values.get(STORED, "").split(",")
            if label.strip()
        )
        for key, value in source.values.items():
            if key in API_KEYS:
                _record(api, key, value)
            elif match := SESSION_KEY.fullmatch(key):
                label = valid_name((match[2] or "default").lower())
                destination = strings if match[1] == "STRING" else names
                _record(destination, label, value)
            elif value and key.startswith(("TELEGRAM_SESSION_STRING_", "TELEGRAM_SESSION_NAME_")):
                raise StoreError("Rename the unsupported legacy account label before migrating")

    for label, value in names.items():
        if label not in selected or selected[label] != _legacy_session_path(value, cwd):
            raise StoreError(
                "Select the file named by each legacy session setting with --session LABEL=PATH. Set --cwd to the legacy launch directory if needed"
            )

    if set(strings) & set(selected):
        raise StoreError(
            "A label has both a session string and a SQLite session. Select one source per account"
        )

    sessions: dict[str, SQLiteSource] = {}
    reserved = {source.file.path for source in sources}
    for label, path in selected.items():
        if path in reserved:
            raise StoreError("A settings file cannot also be selected as a SQLite session")
        if label in stored and label not in names and session_absent(path):
            continue
        sessions[label] = SQLiteSource.capture(path)
        if any(sidecar.path in reserved for sidecar in sessions[label].sidecars):
            raise StoreError("A settings file cannot also be a SQLite sidecar")

    labels = tuple(sorted(stored | strings.keys() | sessions.keys()))
    if not labels:
        raise StoreError("No Telegram accounts found in the selected inputs")

    api_id = api.get("TELEGRAM_API_ID")
    api_hash = api.get("TELEGRAM_API_HASH")
    if api_id is not None and (
        not re.fullmatch(r"[0-9]{1,10}", api_id) or not 0 < int(api_id) < 2**31
    ):
        raise StoreError("Selected Telegram API ID is invalid")
    if api_hash is not None and not re.fullmatch(r"[a-fA-F0-9]{32}", api_hash):
        raise StoreError("Selected Telegram API hash is invalid")
    if (set(strings) | set(sessions)) - stored and (api_id is None or api_hash is None):
        raise StoreError(
            "Supply literal API credentials for new profiles in a selected settings file"
        )

    desired = ",".join(labels)
    updates = []
    for source in sources:
        remove = {key for key in source.values if credential_key(key)}
        if remove or source.values.get(STORED) != desired:
            updates.append((source.file, source.render(remove, desired)))
    return Migration(
        labels, strings, sessions, api, tuple(source.file for source in sources), updates
    )


def _load_if_present(state: State, label: str) -> Profile | None:
    try:
        return read_profile(state, label)
    except ProfileMissing:
        return None


def _same_authorization(left: StringSession, right: StringSession) -> bool:
    return (
        left.auth_key is not None
        and right.auth_key is not None
        and left.auth_key.key == right.auth_key.key
    )


async def _verified_profiles(state: State, migration: Migration, locks: ExitStack) -> None:
    expected: dict[str, Profile] = {}
    pending: dict[str, Profile] = {}
    seen: set[bytes] = set()
    for label in migration.labels:
        prior = _load_if_present(state, label)
        source = migration.sessions.get(label)
        raw = source.authorize() if source is not None else migration.strings.get(label)
        if raw is None:
            if prior is None:
                raise StoreError("A configured encrypted profile is missing")
            profile = prior
        else:
            api_id = int(migration.api.get("TELEGRAM_API_ID") or (prior.api_id if prior else 0))
            api_hash = migration.api.get("TELEGRAM_API_HASH") or (prior.api_hash if prior else "")
            if not api_id or not api_hash:
                raise StoreError(
                    "Supply literal Telegram API credentials in a selected settings file"
                )
            try:
                session = StringSession(raw)
            except Exception:
                raise StoreError("Selected Telegram session string is invalid") from None
            if prior is not None and (
                not _same_authorization(session, prior.session())
                or prior.api_id != api_id
                or prior.api_hash.lower() != api_hash.lower()
            ):
                raise StoreError(
                    "A label already has a different encrypted profile. Resolve the collision before migrating"
                )
            locks.enter_context(lock_authorization(state, session))
            user_id = await verify_authorization(session, api_id, api_hash)
            if session.save() != raw:
                raise StoreError("Telegram changed the selected authorization during verification")
            if prior is not None and prior.user_id != user_id:
                raise StoreError(
                    "Selected Telegram account differs from the existing encrypted profile"
                )
            profile = prior or Profile.parse(Profile(api_id, api_hash, raw, user_id).encoded())
            if prior is None:
                pending[label] = profile
        auth_key = profile.session().auth_key
        if auth_key is None or auth_key.key in seen:
            raise StoreError("Multiple selected labels share one Telegram authorization")
        seen.add(auth_key.key)
        expected[label] = profile

    for label, profile in pending.items():
        write_profile(state, label, profile)
    for label, profile in expected.items():
        if read_profile(state, label) != profile:
            raise StoreError("An encrypted profile could not be verified after reopening")


async def apply(migration: Migration, remove_session_files: bool) -> None:
    state = State.discover()
    with lock_profile_writes(state), ExitStack() as locks:
        await _verified_profiles(state, migration, locks)
        for snapshot in migration.inputs:
            snapshot.check()
        for source in migration.sessions.values():
            source.check()
        replace_configs(migration.updates)
        if remove_session_files:
            for source in migration.sessions.values():
                source.remove()


def _report_environment(sources: list[ConfigSource]) -> None:
    inherited = sorted(key for key in os.environ if credential_key(key) or key == STORED)
    if inherited:
        print("Parent environment names to review before restarting:")
        for key in inherited:
            print(f"  {key}")
    if any(source.passthrough for source in sources):
        print("The selected MCP server has environment passthrough settings. Review them locally.")
    if any(
        key.startswith("TELEGRAM_PROXY_") or "PASSWORD" in key or "2FA" in key or "TOKEN" in key
        for source in sources
        for key in source.values
    ):
        print(
            "Selected inputs contain proxy, password, 2FA, or token settings outside this migration."
        )


def run(args: argparse.Namespace) -> None:
    if not (args.env_file or args.mcp_config):
        raise StoreError("Select at least one settings source")
    if bool(args.server) != bool(args.mcp_config):
        raise StoreError("Use --mcp-config and --server together")
    if args.remove_session_files and (not args.apply or not args.session):
        raise StoreError(
            "Use --remove-session-files with --apply and explicitly selected --session files"
        )

    sources = []
    if args.env_file:
        sources.append(read_dotenv(Path(os.path.abspath(args.env_file.expanduser()))))
    if args.mcp_config:
        sources.append(
            read_mcp_config(Path(os.path.abspath(args.mcp_config.expanduser())), args.server)
        )
    _report_environment(sources)
    migration = plan(sources, _selected_sessions(args.session), args.cwd.expanduser())
    if args.apply:
        asyncio.run(apply(migration, args.remove_session_files))
        print(f"Migration complete for {len(migration.labels)} profile(s).")
        if args.remove_session_files:
            print(
                f"Removed {len(migration.sessions)} selected SQLite session file(s) and their sidecars."
            )
        elif migration.sessions:
            print("Selected SQLite sessions are still on disk.")
        print("Restart the MCP host and remove inherited legacy assignments at their source.")
    else:
        print(
            f"Preview only. {len(migration.labels)} profile(s), "
            f"{len(migration.updates)} settings file update(s), "
            f"{len(migration.sessions)} selected SQLite session(s)."
        )


def main() -> None:
    parser = MigrationParser(
        description="Move selected Telegram MCP credentials into the encrypted profile store"
    )
    parser.add_argument("--env-file", type=Path, help="dotenv file to update")
    parser.add_argument(
        "--mcp-config", type=Path, help="Codex TOML or Cursor or Claude Desktop JSON"
    )
    parser.add_argument("--server", help="name of the Telegram MCP server entry")
    parser.add_argument(
        "--session",
        action="append",
        default=[],
        metavar="LABEL=PATH",
        help="explicitly select a legacy Telethon SQLite session",
    )
    parser.add_argument(
        "--cwd",
        type=Path,
        default=Path.cwd(),
        help="legacy launch directory for relative session names",
    )
    parser.add_argument(
        "--apply", action="store_true", help="verify, encrypt, and update selected settings"
    )
    parser.add_argument(
        "--remove-session-files",
        action="store_true",
        help="delete explicitly selected SQLite sessions after verification",
    )
    try:
        run(parser.parse_args())
    except StoreError as error:
        raise SystemExit(str(error)) from None
    except KeyboardInterrupt:
        raise SystemExit("Telegram migration interrupted. Run the preview again") from None
    except Exception:
        raise SystemExit(
            "Telegram migration could not finish. Check the selected files and run the preview again"
        ) from None


if __name__ == "__main__":
    main()
