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
from typing import TypedDict, cast

ModelRequest = dict[str, list[dict[str, object]]]
UNTRUSTED_CONTEXT = "unexpected untrusted smoke hook context"


class HookEntry(TypedDict):
    key: str
    sourcePath: str
    eventName: str
    command: str | None
    currentHash: str
    trustStatus: str
    enabled: bool


class HookInventory(TypedDict):
    cwd: str
    hooks: list[HookEntry]


def hook_inventory(
    codex: str, env: dict[str, str], directories: list[Path]
) -> list[HookInventory]:
    with subprocess.Popen(
        [codex, "app-server", "--listen", "stdio://"],
        env=env,
        cwd=directories[0],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
    ) as process:
        assert process.stdin is not None and process.stdout is not None
        input_stream, output_stream = process.stdin, process.stdout

        def send(message: dict[str, object]) -> None:
            input_stream.write(json.dumps(message) + "\n")
            input_stream.flush()

        def receive(request_id: int) -> dict[str, object]:
            for line in output_stream:
                message = cast(dict[str, object], json.loads(line))
                if message.get("id") == request_id:
                    return cast(dict[str, object], message["result"])
            raise RuntimeError("Codex app server closed without a hook inventory")

        timer = threading.Timer(15, process.kill)
        timer.start()
        try:
            client = {"name": "mori-hook-smoke", "version": "1"}
            send({"id": 0, "method": "initialize", "params": {"clientInfo": client}})
            receive(0)
            send({"method": "initialized"})
            params = {"cwds": [str(directory) for directory in directories]}
            send({"id": 1, "method": "hooks/list", "params": params})
            inventory = cast(list[HookInventory], receive(1)["data"])
            if {entry["cwd"] for entry in inventory} != set(params["cwds"]):
                raise RuntimeError(
                    "Codex did not list hooks for each smoke test directory"
                )
            return inventory
        finally:
            process.kill()
            timer.cancel()


def trust_fixture_hook(
    inventory: list[HookInventory], config: Path, hook_source: Path, hook_command: str
) -> None:
    approvals: set[tuple[str, str]] = set()
    for directory in inventory:
        matched = False
        for hook in directory["hooks"]:
            if (
                hook["sourcePath"] == str(hook_source)
                and hook["eventName"] == "sessionStart"
                and hook["command"] == hook_command
            ):
                matched = True
                approvals.add((hook["key"], hook["currentHash"]))
            elif hook["enabled"] and hook["trustStatus"] in {"trusted", "managed"}:
                raise RuntimeError("An external Codex hook is active in the smoke test")
        if not matched:
            raise RuntimeError("Codex did not discover the Mori fixture hook")
    if len(approvals) != 1:
        raise RuntimeError("Expected one Mori fixture hook approval")
    key, content_hash = approvals.pop()
    with config.open("a", encoding="utf-8") as stream:
        stream.write(f"\n[hooks.state.{json.dumps(key, ensure_ascii=False)}]\n")
        stream.write(f"trusted_hash = {json.dumps(content_hash)}\n")


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


def run(
    command: list[str], env: dict[str, str], cwd: Path, *, report_stderr: bool = False
) -> None:
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
        problem = f"{shlex.join(command)} exited {result.returncode}"
        if report_stderr and result.stderr:
            problem += f"\n{result.stderr.strip()[:2000]}"
        raise RuntimeError(problem)


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
        # The daemon socket needs a short path even when TMPDIR is long.
        tempfile.TemporaryDirectory(prefix="mh-", dir="/tmp") as runtime_temporary,
        http.server.ThreadingHTTPServer(("127.0.0.1", 0), Responses) as server,
    ):
        root = Path(temporary).resolve()
        indexed, unindexed, codex_home, cache = (
            root / name for name in ("indexed", "unindexed", "codex", "cache")
        )
        for directory in (indexed, unindexed, codex_home, cache):
            directory.mkdir(mode=0o700)
        (indexed / "example.py").write_text(
            "def example():\n    return 'example'\n", encoding="utf-8"
        )
        env = {
            **{
                key: value
                for key, value in os.environ.items()
                if key.lower() not in {"http_proxy", "https_proxy", "all_proxy"}
            },
            "CODEX_HOME": str(codex_home),
            "CBM_CACHE_DIR": str(cache),
            "CBM_RUNTIME_DIR": str(Path(runtime_temporary).resolve()),
        }
        config = codex_home / "config.toml"
        config.write_text(
            f'\nmodel = "gpt-5"\nmodel_provider = "probe"\n'
            f'\n[model_providers.probe]\nname = "Mori probe"\n'
            f'base_url = "http://127.0.0.1:{server.server_port}/v1"\n'
            'wire_api = "responses"\nsupports_websockets = false\n',
            encoding="utf-8",
        )
        hook_command = shlex.join([str(mori), "hook-augment"])
        mori_hook = {
            "type": "command",
            "command": hook_command,
            "timeout": 5,
        }
        untrusted_hook = {
            "type": "command",
            "command": shlex.join(["printf", "%s\\n", UNTRUSTED_CONTEXT]),
            "timeout": 5,
        }
        matcher = "startup|resume|clear|compact|fork"
        hook_config = {
            "hooks": {
                "SessionStart": [
                    {"matcher": matcher, "hooks": [mori_hook, untrusted_hook]}
                ]
            }
        }
        hook_source = codex_home / "hooks.json"
        hook_source.write_text(json.dumps(hook_config), encoding="utf-8")
        trust_fixture_hook(
            hook_inventory(codex, env, [indexed, unindexed]),
            config,
            hook_source,
            hook_command,
        )
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            run([str(mori), "daemon", "start"], env, indexed, report_stderr=True)
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
                    report_stderr=True,
                )
                for cwd, expected in (
                    (indexed, 'indexed as graph project "codex-probe"'),
                    (
                        unindexed,
                        "no indexed graph project matched this working directory",
                    ),
                ):
                    run(
                        [
                            codex,
                            "exec",
                            "--skip-git-repo-check",
                            "-C",
                            str(cwd),
                            "Say ok.",
                        ],
                        env,
                        cwd,
                    )
                    contexts = (
                        []
                        if received.empty()
                        else developer_context(received.get_nowait())
                    )
                    if not any(expected in content for content in contexts):
                        raise AssertionError(
                            f"Codex did not send Mori developer context for {cwd.name}"
                        )
                    if any(UNTRUSTED_CONTEXT in content for content in contexts):
                        raise AssertionError("Codex ran an untrusted smoke test hook")
                    print(f"PASS Codex SessionStart in {cwd.name} directory")
            finally:
                run([str(mori), "daemon", "stop"], env, indexed, report_stderr=True)
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
