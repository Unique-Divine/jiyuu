"""Application entrypoints for the Telegram MCP server."""

from telegram_mcp.install_guard import UnsafeInstallationError, assert_safe_distribution

try:
    assert_safe_distribution()
except UnsafeInstallationError as exc:
    raise SystemExit(str(exc)) from None

from contextlib import AsyncExitStack
from importlib.resources import files as resource_files

from telegram_mcp.broker import Broker, listen
from telegram_mcp.bot_identity import BotIdentity, for_broker
from telegram_mcp.fill import PoolTransport
from telegram_mcp.http_service import serve_mcp
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


def _lock_saved_authorizations(stack: AsyncExitStack, state: State | None) -> None:
    if state is not None:
        for profile in _runtime.STORED_PROFILES.values():
            stack.enter_context(lock_authorization(state, profile.session()))
    elif os.name == "posix":
        sessions = []
        for client in clients.values():
            session = client.session
            if session is not None and session.auth_key is not None and session.auth_key.key:
                sessions.append(session)
        if sessions:
            legacy_state = State.discover()
            for session in sessions:
                stack.enter_context(lock_authorization(legacy_state, session))


async def _main() -> None:
    state = _runtime.STORED_STATE
    broker_enabled = os.environ.get("TELEGRAM_BROKER") == "1"
    try:
        if broker_enabled and state is None:
            raise StoreError("Telegram broker requires explicitly selected encrypted profiles")
        bot = BotIdentity()
        if broker_enabled and state is not None:
            bot = for_broker(
                state, {profile.user_id for profile in _runtime.STORED_PROFILES.values()}
            )
        async with AsyncExitStack() as stack:
            _lock_saved_authorizations(stack, state)
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
                photo = resource_files("telegram_mcp").joinpath("sai-chat-photo.jpg").read_bytes()
                accounts = {
                    str(profile.user_id): PoolTransport(
                        clients[label], state, str(profile.user_id), photo, bot
                    )
                    for label, profile in _runtime.STORED_PROFILES.items()
                }
                if len(accounts) != len(_runtime.STORED_PROFILES):
                    raise StoreError("Telegram broker profiles must identify distinct accounts")
                await stack.enter_async_context(
                    listen(
                        state,
                        Broker(
                            {key: account.snapshot for key, account in accounts.items()},
                            {key: account.dispatch for key, account in accounts.items()},
                        ),
                    )
                )

            async def _warm_caches() -> None:
                try:
                    await asyncio.gather(*(warm_dialogs(cl) for cl in clients.values()))
                except Exception:
                    print("Telegram entity cache warm failed", file=sys.stderr)

            warm_task = asyncio.create_task(_warm_caches())
            stack.push_async_callback(_cancel_cache_warm, warm_task)
            print(f"Telegram client(s) started ({labels}). Running MCP server...", file=sys.stderr)
            if broker_enabled and state is not None:
                await serve_mcp(state, mcp)
            else:
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


def _configure_broker_tools(server: FastMCP) -> None:
    # These tools create groups, add members, or join chats outside the
    # managed journal. Existing messaging, photo, invite-link, and admin
    # tools remain available under the user's configured exposure mode.
    for name in (
        "create_group",
        "create_channel",
        "subscribe_public_channel",
        "invite_to_group",
        "join_chat_by_link",
        "import_chat_invite",
    ):
        if server._tool_manager.get_tool(name) is not None:
            server._tool_manager.remove_tool(name)


def main() -> None:
    _configure_allowed_roots_from_cli(sys.argv[1:])
    _runtime._apply_exposed_tools_mode()
    if os.environ.get("TELEGRAM_BROKER") == "1":
        _configure_broker_tools(mcp)
    # Read the latest release in the background; the first tool result after
    # it lands carries a one-line notice if this version is behind.
    server_info.update_notifier().start()
    nest_asyncio.apply()
    asyncio.run(_main())


if __name__ == "__main__":
    main()
