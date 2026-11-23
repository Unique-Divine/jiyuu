"""Expose the existing FastMCP server on a private Unix HTTP socket."""

import asyncio
from contextlib import contextmanager
import os
import signal
import socket
import stat

from mcp.server.fastmcp import FastMCP
import uvicorn

from telegram_mcp.session_store import State, StoreError, lock_file


class SocketServer(uvicorn.Server):
    @contextmanager
    def capture_signals(self):
        # The runner owns shutdown so it can drain fill calls before
        # disconnecting the shared Telegram clients.
        yield


async def serve_mcp(state: State, app: FastMCP, *, stop: asyncio.Event | None = None) -> None:
    path = state.tool / "run" / "mcp.sock"
    loop = asyncio.get_running_loop()
    own_signals = stop is None
    stopping = stop if stop is not None else asyncio.Event()
    with lock_file(state, "mcp-http.lock", "Telegram MCP HTTP server is already running"):
        try:
            metadata = path.lstat()
        except FileNotFoundError:
            pass
        else:
            if not stat.S_ISSOCK(metadata.st_mode) or metadata.st_uid != os.getuid():
                raise StoreError("Telegram MCP socket path is not safe")
            path.unlink()
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
            listener.bind(os.fspath(path))
            path.chmod(0o600)
            server = SocketServer(
                uvicorn.Config(
                    app.streamable_http_app(),
                    access_log=False,
                    log_level="warning",
                    proxy_headers=False,
                    server_header=False,
                    timeout_graceful_shutdown=10,
                )
            )

            async def shutdown() -> None:
                await stopping.wait()
                server.should_exit = True

            if own_signals:
                for signum in (signal.SIGINT, signal.SIGTERM):
                    loop.add_signal_handler(signum, stopping.set)
            shutdown_task = asyncio.create_task(shutdown())
            try:
                await server.serve(sockets=[listener])
            finally:
                shutdown_task.cancel()
                await asyncio.gather(shutdown_task, return_exceptions=True)
                if own_signals:
                    for signum in (signal.SIGINT, signal.SIGTERM):
                        loop.remove_signal_handler(signum)
                path.unlink(missing_ok=True)
