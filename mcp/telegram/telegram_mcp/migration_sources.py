"""Read explicitly selected legacy Telegram MCP sources without exposing values."""

from collections.abc import Callable, Mapping
from contextlib import ExitStack, closing
from dataclasses import dataclass, field
import io
import json
import os
from pathlib import Path
import re
import sqlite3
import stat
import tempfile

from dotenv.parser import parse_stream
from telethon.sessions import StringSession

from .session_store import StoreError

STORED = "TELEGRAM_STORED_ACCOUNTS"
API_KEYS = {"TELEGRAM_API_ID", "TELEGRAM_API_HASH"}
SESSION_KEY = re.compile(r"TELEGRAM_SESSION_(STRING|NAME)(?:_([a-zA-Z0-9_-]{1,48}))?")
MAX_CONFIG_BYTES = 8 * 1024 * 1024
SIDECARS = ("-wal", "-shm", "-journal")


def credential_key(key: str) -> bool:
    return key in API_KEYS or SESSION_KEY.fullmatch(key) is not None


@dataclass(frozen=True, repr=False)
class FileSnapshot:
    path: Path
    info: os.stat_result

    @classmethod
    def capture(cls, path: Path) -> "FileSnapshot":
        info = path.lstat()
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1:
            raise StoreError("Select a regular file owned by you with no additional hard links")
        return cls(path, info)

    def check(self, current: os.stat_result | None = None) -> None:
        current = self.path.lstat() if current is None else current
        fields = (
            "st_dev",
            "st_ino",
            "st_mode",
            "st_uid",
            "st_nlink",
            "st_size",
            "st_mtime_ns",
            "st_ctime_ns",
        )
        if any(getattr(self.info, attr) != getattr(current, attr) for attr in fields):
            raise StoreError(
                "A migration input changed. Stop its writer and run the preview again"
            )

    def read_config(self) -> str:
        fd = os.open(self.path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(fd, "rb") as stream:
            self.check(os.fstat(stream.fileno()))
            data = stream.read(MAX_CONFIG_BYTES + 1)
            self.check(os.fstat(stream.fileno()))
        if len(data) > MAX_CONFIG_BYTES:
            raise StoreError("Selected settings file exceeds the migration size limit")
        return data.decode("utf-8")


@dataclass(frozen=True, repr=False)
class ConfigSource:
    file: FileSnapshot
    values: dict[str, str]
    render: Callable[[set[str], str], bytes] = field(repr=False)
    passthrough: bool = False


def read_dotenv(path: Path) -> ConfigSource:
    snapshot = FileSnapshot.capture(path)
    bindings = list(parse_stream(io.StringIO(snapshot.read_config())))
    values: dict[str, str] = {}
    for binding in bindings:
        if binding.error:
            raise StoreError("Selected dotenv file contains an unsupported assignment")
        key = binding.key
        value = binding.value
        if key is None or not (key.startswith("TELEGRAM_") or key == "TG_2FA_PASSWORD"):
            continue
        if credential_key(key) or key == STORED:
            if key in values or value is None or "${" in value:
                raise StoreError(
                    "Use unique, literal Telegram assignments in the selected dotenv file"
                )
        if value is not None:
            values[key] = value

    def render(remove: set[str], stored: str) -> bytes:
        # A retained line could expose a credential through a dotenv reference
        # or change meaning when that credential is removed.
        for binding in bindings:
            if binding.key not in remove | {STORED} and binding.value is not None:
                if any(
                    re.search(r"\$\{" + re.escape(key) + r"(?=[:}])", binding.value)
                    for key in remove
                ):
                    raise StoreError(
                        "A retained dotenv setting references a credential being migrated"
                    )
        original = "".join(binding.original.string for binding in bindings)
        newline = "\r\n" if "\r\n" in original else "\n"
        kept = "".join(
            (
                binding.original.string
                if binding.key not in remove | {STORED}
                else "".join(re.findall(r"^(?:[ \t]*(?:\r\n|\n))+", binding.original.string))
            )
            for binding in bindings
        )
        if kept and not kept.endswith("\n"):
            kept += newline
        return (kept + STORED + "=" + stored + newline).encode()

    return ConfigSource(snapshot, values, render)


def _unique_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            raise StoreError("Selected JSON configuration contains a duplicate key")
        result[key] = value
    return result


def _invalid_json_constant(_value: str) -> None:
    raise StoreError("Selected configuration contains an invalid JSON value")


def read_mcp_config(path: Path, server: str) -> ConfigSource:
    snapshot = FileSnapshot.capture(path)
    raw = snapshot.read_config()
    if path.suffix.lower() == ".toml":
        import tomlkit
        from tomlkit.exceptions import ParseError

        try:
            document = tomlkit.parse(raw)
        except ParseError:
            raise StoreError("Selected Codex TOML could not be parsed") from None
        root_key = "mcp_servers"
        empty_env = tomlkit.table

        def serialize() -> bytes:
            return document.as_string().encode()

    elif path.suffix.lower() == ".json":
        document = json.loads(
            raw, object_pairs_hook=_unique_object, parse_constant=_invalid_json_constant
        )
        root_key = "mcpServers"
        empty_env = dict

        def serialize() -> bytes:
            return (json.dumps(document, indent=2, ensure_ascii=False) + "\n").encode()

    else:
        raise StoreError("Select a Codex TOML or Cursor or Claude Desktop JSON configuration")

    servers = document.get(root_key) if isinstance(document, Mapping) else None
    selected = servers.get(server) if isinstance(servers, Mapping) else None
    if not isinstance(selected, Mapping) or not isinstance(selected.get("command"), str):
        raise StoreError("The named MCP server must be a local command entry")
    env = selected["env"] if "env" in selected else empty_env()
    if not isinstance(env, Mapping):
        raise StoreError("The named MCP server environment must be a map")
    values: dict[str, str] = {}
    for key, value in env.items():
        if not (key.startswith("TELEGRAM_") or key == "TG_2FA_PASSWORD"):
            continue
        if credential_key(key) or key == STORED:
            if not isinstance(value, str) or any(token in value for token in ("${", "$env:")):
                raise StoreError(
                    "Use literal Telegram settings in the selected MCP server environment"
                )
        if isinstance(value, str):
            values[key] = value

    def render(remove: set[str], stored: str) -> bytes:
        for key in remove:
            del env[key]
        env[STORED] = stored
        if "env" not in selected:
            selected["env"] = env
        return serialize()

    return ConfigSource(snapshot, values, render, bool(selected.get("env_vars")))


@dataclass(frozen=True, repr=False)
class SQLiteSource:
    file: FileSnapshot
    sidecars: tuple[FileSnapshot, ...]

    @classmethod
    def capture(cls, path: Path) -> "SQLiteSource":
        session = cls(FileSnapshot.capture(path), _sidecars(path))
        if any(
            item.info.st_size and item.path.name.endswith(("-wal", "-journal"))
            for item in session.sidecars
        ):
            raise StoreError(
                "A SQLite session has an active journal. Close the legacy tool before migrating"
            )
        return session

    def check(self) -> None:
        self.file.check()
        current = _sidecars(self.file.path)
        if [item.path for item in current] != [item.path for item in self.sidecars]:
            raise StoreError(
                "SQLite session sidecars changed. Close the legacy tool before migrating"
            )
        for saved, actual in zip(self.sidecars, current):
            saved.check(actual.info)

    def authorize(self) -> str:
        # The immutable connection neither creates sidecars nor recovers a journal.
        # A stopped legacy client and unchanged snapshots are required.
        self.check()
        uri = self.file.path.as_uri() + "?mode=ro&immutable=1"
        with closing(sqlite3.connect(uri, uri=True)) as db:
            db.execute("PRAGMA trusted_schema=OFF")
            table = db.execute("SELECT type FROM sqlite_master WHERE name = 'sessions'").fetchone()
            if table != ("table",):
                raise StoreError("Selected file does not contain a Telethon sessions table")
            rows = db.execute(
                "SELECT dc_id, server_address, port, auth_key FROM sessions LIMIT 2"
            ).fetchall()
        self.check()
        if len(rows) != 1:
            raise StoreError("Selected Telethon session must contain one authorization")
        dc, address, port, key = rows[0]
        if not isinstance(key, bytes) or len(key) != 256:
            raise StoreError("Selected Telethon session has no authorization")
        from telethon.crypto import AuthKey

        session = StringSession()
        session.set_dc(dc, address, port)
        session.auth_key = AuthKey(key)
        return session.save()

    def remove(self) -> None:
        self.check()
        for item in (*self.sidecars, self.file):
            item.check()
            item.path.unlink()
        _sync_parent(self.file.path)


def _sidecars(path: Path) -> tuple[FileSnapshot, ...]:
    snapshots = []
    for suffix in SIDECARS:
        try:
            snapshots.append(FileSnapshot.capture(path.with_name(path.name + suffix)))
        except FileNotFoundError:
            continue
    return tuple(snapshots)


def session_absent(path: Path) -> bool:
    try:
        FileSnapshot.capture(path)
    except FileNotFoundError:
        return not _sidecars(path)
    return False


def _sync_parent(path: Path) -> None:
    fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def replace_configs(updates: list[tuple[FileSnapshot, bytes]]) -> None:
    """Stage all cleaned documents before publishing the first replacement."""
    with ExitStack() as cleanup:
        staged: list[tuple[FileSnapshot, Path]] = []
        for snapshot, content in updates:
            snapshot.check()
            fd, name = tempfile.mkstemp(prefix=".telegram-migrate-", dir=snapshot.path.parent)
            temporary = Path(name)
            cleanup.callback(temporary.unlink, missing_ok=True)
            with os.fdopen(fd, "wb") as stream:
                stream.write(content)
                stream.flush()
                os.fsync(stream.fileno())
            staged.append((snapshot, temporary))
        for snapshot, _ in staged:
            snapshot.check()
        for snapshot, temporary in staged:
            snapshot.check()
            os.replace(temporary, snapshot.path)
            _sync_parent(snapshot.path)
