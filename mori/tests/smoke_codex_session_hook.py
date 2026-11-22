#!/usr/bin/env python3
"""Check that Codex includes Mori's SessionStart context in model requests."""

from __future__ import annotations

import argparse
import http.server
import json
import os
import shlex
import subprocess
import tempfile
import threading
from pathlib import Path
from queue import Queue
from typing import cast

ModelRequest = dict[str, list[dict[str, object]]]


def response_body() -> bytes:
    response = {"id": "resp_mori_probe"}
    usage = {"input_tokens": 0, "output_tokens": 0, "total_tokens": 0}
    message = {
        "id": "msg_mori_probe",
        "type": "message",
        "role": "assistant",
        "content": [{"type": "output_text", "text": "ok", "annotations": []}],
        "status": "completed",
    }
    events = [
        {"type": "response.created", "response": response},
        {"type": "response.output_item.done", "item": message},
        {"type": "response.completed", "response": {**response, "usage": usage}},
    ]
    return "".join(
        f"event: {event['type']}\ndata: {json.dumps(event)}\n\n" for event in events
    ).encode()


def run(command: list[str], env: dict[str, str], cwd: Path) -> None:
    result = subprocess.run(
        command,
        env=env,
        cwd=cwd,
        check=False,
        text=True,
        capture_output=True,
        timeout=45,
    )
    if result.returncode:
        raise RuntimeError(f"{shlex.join(command)} exited {result.returncode}")


def developer_context(request: ModelRequest) -> list[str]:
    texts = []
    for message in request["input"]:
        if message.get("role") != "developer":
            continue
        for item in cast(list[dict[str, str]], message["content"]):
            if item.get("type") == "input_text":
                texts.append(item["text"])
    return texts


def smoke(mori: Path, codex: str) -> None:
    received: Queue[ModelRequest] = Queue()

    class Responses(http.server.BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            if self.path != "/v1/responses":
                self.send_error(404)
                return
            received.put(
                json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            )
            body = response_body()
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    with (
        tempfile.TemporaryDirectory(prefix="mori-hook-") as temporary,
        http.server.ThreadingHTTPServer(("127.0.0.1", 0), Responses) as server,
    ):
        root = Path(temporary)
        indexed, unindexed, codex_home, cache, runtime = (
            root / name
            for name in ("indexed", "unindexed", "codex", "cache", "runtime")
        )
        for directory in (indexed, unindexed, codex_home, cache, runtime):
            directory.mkdir(mode=0o700)
        (indexed / "example.py").write_text(
            "def example():\n    return 'example'\n", encoding="utf-8"
        )
        env = {
            **os.environ,
            "CODEX_HOME": str(codex_home),
            "CBM_CACHE_DIR": str(cache),
            "CBM_RUNTIME_DIR": str(runtime),
        }
        (codex_home / "config.toml").write_text(
            f'\nmodel = "gpt-5"\nmodel_provider = "probe"\n'
            f'\n[model_providers.probe]\nname = "Mori probe"\n'
            f'base_url = "http://127.0.0.1:{server.server_port}/v1"\n'
            'wire_api = "responses"\nsupports_websockets = false\n',
            encoding="utf-8",
        )
        hook = {
            "type": "command",
            "command": shlex.join([str(mori), "hook-augment"]),
            "timeout": 5,
        }
        matcher = "startup|resume|clear|compact|fork"
        hook_config = {
            "hooks": {"SessionStart": [{"matcher": matcher, "hooks": [hook]}]}
        }
        (codex_home / "hooks.json").write_text(
            json.dumps(hook_config), encoding="utf-8"
        )
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            run([str(mori), "daemon", "start"], env, indexed)
            try:
                run(
                    [
                        str(mori),
                        "cli",
                        "index_repository",
                        "--repo-path",
                        str(indexed),
                        "--name",
                        "codex-probe",
                        "--mode",
                        "fast",
                    ],
                    env,
                    indexed,
                )
                for cwd, expected in (
                    (indexed, 'indexed as graph project "codex-probe"'),
                    (
                        unindexed,
                        "no indexed graph project matched this working directory",
                    ),
                ):
                    # This isolated Codex home contains only the hook above.
                    run(
                        [
                            codex,
                            "exec",
                            "--dangerously-bypass-hook-trust",
                            "--skip-git-repo-check",
                            "-C",
                            str(cwd),
                            "Say ok.",
                        ],
                        env,
                        cwd,
                    )
                    if received.empty() or not any(
                        expected in content
                        for content in developer_context(received.get_nowait())
                    ):
                        raise AssertionError(
                            f"Codex did not send Mori developer context for {cwd.name}"
                        )
                    print(f"PASS Codex SessionStart in {cwd.name} directory")
            finally:
                run([str(mori), "daemon", "stop"], env, indexed)
        finally:
            server.shutdown()
            thread.join()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--mori", required=True, type=Path, help="Path to the Mori executable"
    )
    parser.add_argument(
        "--codex", default="codex", help="Codex executable (default: codex)"
    )
    args = parser.parse_args()
    smoke(args.mori.resolve(strict=True), args.codex)


if __name__ == "__main__":
    main()
