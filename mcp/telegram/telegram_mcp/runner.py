"""Application entrypoints for the Telegram MCP server."""

from telegram_mcp.install_guard import UnsafeInstallationError, assert_safe_distribution

try:
    assert_safe_distribution()
except UnsafeInstallationError as exc:
    raise SystemExit(str(exc)) from None

from contextlib import AsyncExitStack
from datetime import datetime, timezone
from functools import partial

from telegram_mcp.broker import Broker, BrokerError, listen
from telegram_mcp.session_store import lock_authorization, validate_identity

from telegram_mcp import runtime as _runtime
from telegram_mcp import server_info
from telegram_mcp.runtime import *
import telegram_mcp.tools  # noqa: F401 - registers MCP tools via decorators


async def _connect_authorized_client(label, client) -> None:
    await client.connect()
    if await client.is_user_authorized():
        return

    raise RuntimeError(
        f"Telegram client '{label}' is not authorized. Interactive phone login "
        "is disabled for the MCP server because it runs over stdio. Generate a "
        "session string with `uv run session_string_generator.py`, then set "
        "TELEGRAM_SESSION_STRING or TELEGRAM_SESSION_STRING_<LABEL> in .env. "
        "For existing file sessions, run the login outside the MCP server first."
    )


async def _snapshot(label: str) -> dict:
    client = clients[label]
    profile = _runtime.STORED_PROFILES[label]
    with raise_flood_waits():
        user = await client.get_me()
        if not isinstance(user, User):
            raise BrokerError("account_unavailable")
        validate_identity(user, profile.user_id)
        dialogs = client.iter_dialogs(limit=2001)
        count = 0
        async for index, dialog in _enumerate(dialogs):
            if index == 2000:
                raise BrokerError("group_count_unavailable")
            if isinstance(
                dialog.entity, (Chat, Channel, types.ChatForbidden, types.ChannelForbidden)
            ):
                count += 1
    return {
        "accountId": str(profile.user_id),
        "groupCount": count,
        "observedAt": datetime.now(timezone.utc).isoformat(),
        "budgetAvailable": False,
    }


async def _enumerate(values):
    index = 0
    async for value in values:
        yield index, value
        index += 1


async def _main() -> None:
    state = _runtime.STORED_STATE
    warm_task = None
    broker_enabled = os.environ.get("TELEGRAM_BROKER") == "1"
    try:
        if broker_enabled and state is None:
            raise StoreError("Telegram broker requires explicitly selected encrypted profiles")
        async with AsyncExitStack() as stack:
            if state is not None:
                for profile in _runtime.STORED_PROFILES.values():
                    stack.enter_context(lock_authorization(state, profile.session()))
            stack.push_async_callback(_disconnect_clients)
            labels = ", ".join(clients.keys())
            print(f"Starting {len(clients)} Telegram client(s) ({labels})...", file=sys.stderr)
            await asyncio.gather(
                *(_connect_authorized_client(label, cl) for label, cl in clients.items())
            )
            if state is not None:
                for label, profile in _runtime.STORED_PROFILES.items():
                    validate_identity(await clients[label].get_me(), profile.user_id)
            if broker_enabled and state is not None:
                snapshots = {
                    str(profile.user_id): partial(_snapshot, label)
                    for label, profile in _runtime.STORED_PROFILES.items()
                }
                if len(snapshots) != len(_runtime.STORED_PROFILES):
                    raise StoreError("Telegram broker profiles must identify distinct accounts")
                await stack.enter_async_context(listen(state, Broker(snapshots)))

            async def _warm_caches() -> None:
                try:
                    await asyncio.gather(*(warm_dialogs(cl) for cl in clients.values()))
                except Exception:
                    print("Telegram entity cache warm failed", file=sys.stderr)

            warm_task = asyncio.create_task(_warm_caches())
            stack.push_async_callback(_cancel_cache_warm, warm_task)
            print(f"Telegram client(s) started ({labels}). Running MCP server...", file=sys.stderr)
            await mcp.run_stdio_async()
    except Exception as error:
        if isinstance(error, StoreError):
            print(str(error), file=sys.stderr)
        elif state is not None:
            print("Stored Telegram session could not start", file=sys.stderr)
        else:
            print(f"Error starting client: {error}", file=sys.stderr)
            if isinstance(error, sqlite3.OperationalError) and "database is locked" in str(error):
                print(
                    "Database lock detected. Please ensure no other instances are running.",
                    file=sys.stderr,
                )
        sys.exit(1)


async def _disconnect_clients() -> None:
    await asyncio.gather(*(cl.disconnect() for cl in clients.values()), return_exceptions=True)


async def _cancel_cache_warm(task: asyncio.Task) -> None:
    task.cancel()
    await asyncio.gather(task, return_exceptions=True)


def main() -> None:
    _configure_allowed_roots_from_cli(sys.argv[1:])
    _runtime._apply_exposed_tools_mode()
    # Read the latest release in the background; the first tool result after
    # it lands carries a one-line notice if this version is behind.
    server_info.update_notifier().start()
    nest_asyncio.apply()
    asyncio.run(_main())


if __name__ == "__main__":
    main()
