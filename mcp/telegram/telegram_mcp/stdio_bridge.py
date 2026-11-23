"""Connect an MCP stdio client to the private Telegram MCP service."""

from collections.abc import AsyncIterable
from datetime import timedelta
import logging
import os
from pathlib import Path
import stat

import anyio
from anyio.abc import ObjectSendStream
import httpx
from mcp.client.streamable_http import streamablehttp_client
from mcp.server.stdio import stdio_server
from mcp.shared.message import SessionMessage

from .session_store import State, StoreError


def private_socket(state: State) -> Path:
    path = state.tool / "run" / "mcp.sock"
    metadata = path.lstat()
    if (
        not stat.S_ISSOCK(metadata.st_mode)
        or metadata.st_uid != os.getuid()
        or stat.S_IMODE(metadata.st_mode) != 0o600
    ):
        raise StoreError("Telegram MCP service socket must be private")
    return path


async def relay(
    source: AsyncIterable[SessionMessage | Exception],
    destination: ObjectSendStream[SessionMessage],
) -> None:
    async for message in source:
        if isinstance(message, Exception):
            raise StoreError("Telegram MCP transport returned an invalid message")
        await destination.send(message)


async def bridge(path: Path) -> None:
    def http_client(
        headers: dict[str, str] | None = None,
        timeout: httpx.Timeout | None = None,
        auth: httpx.Auth | None = None,
    ) -> httpx.AsyncClient:
        return httpx.AsyncClient(
            transport=httpx.AsyncHTTPTransport(uds=os.fspath(path)),
            headers=headers,
            timeout=timeout,
            auth=auth,
            trust_env=False,
        )

    async with (
        stdio_server() as (client_input, client_output),
        streamablehttp_client(
            "http://localhost/mcp",
            httpx_client_factory=http_client,
            sse_read_timeout=timedelta(minutes=5),
        ) as (service_input, service_output, _),
        anyio.create_task_group() as tasks,
    ):

        async def forward(source, destination) -> None:
            try:
                await relay(source, destination)
            finally:
                tasks.cancel_scope.cancel()

        tasks.start_soon(forward, client_input, service_output)
        tasks.start_soon(forward, service_input, client_output)


def main() -> None:
    # Transport errors can contain request or response bodies.
    logging.disable(logging.CRITICAL)
    try:
        anyio.run(bridge, private_socket(State.discover()))
    except Exception:
        raise SystemExit("Telegram MCP bridge could not reach the local service") from None


if __name__ == "__main__":
    main()
