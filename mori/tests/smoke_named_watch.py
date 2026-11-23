#!/usr/bin/env python3
"""Exercise opt-in named watches against an isolated Mori daemon."""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import sqlite3
import subprocess
import tempfile
import time
from collections.abc import Callable
from contextlib import closing
from pathlib import Path


def run(command: list[str], env: dict[str, str], cwd: Path) -> str:
    result = subprocess.run(
        command,
        cwd=cwd,
        env=env,
        capture_output=True,
        text=True,
        check=False,
        timeout=45,
    )
    if result.returncode:
        raise RuntimeError(
            f"{command[:3]} exited {result.returncode}\n{result.stderr[-2400:]}"
        )
    return result.stdout


def wait_for(
    description: str, condition: Callable[[], bool], timeout: float = 30
) -> float:
    start = time.monotonic()
    while time.monotonic() - start < timeout:
        if condition():
            return time.monotonic() - start
        time.sleep(0.15)
    raise AssertionError(f"Timed out waiting for {description}")


def write_source(root: Path, symbol: str) -> None:
    (root / "test.py").write_text(f"def {symbol}():\n    return 42\n", encoding="utf-8")


def checkout(root: Path, symbol: str, env: dict[str, str]) -> None:
    root.mkdir()
    run(["git", "init", "-q"], env, root)
    write_source(root, symbol)
    run(["git", "add", "test.py"], env, root)
    commit(root, env)


def commit(root: Path, env: dict[str, str]) -> None:
    run(
        [
            "git",
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "commit",
            "-qam",
            "fixture",
        ],
        env,
        root,
    )


def replace_snapshot(path: Path, content: str, mtime_ns: int | None = None) -> None:
    temporary = path.with_suffix(".new")
    temporary.write_text(content, encoding="utf-8")
    if mtime_ns is not None:
        os.utime(temporary, ns=(time.time_ns(), mtime_ns))
    temporary.replace(path)


def indexed(cache: Path, project: str, symbol: str) -> bool:
    database = cache / f"{project}.db"
    try:
        with closing(
            sqlite3.connect(database.as_uri() + "?mode=ro", uri=True, timeout=0.1)
        ) as db:
            return (
                db.execute(
                    "SELECT 1 FROM nodes WHERE name = ? LIMIT 1", (symbol,)
                ).fetchone()
                is not None
            )
    except sqlite3.OperationalError:
        # The first generation may be unpublished or the writer may be replacing it.
        return False


def cpu_seconds(pid: int) -> float | None:
    stat = Path(f"/proc/{pid}/stat")
    if not stat.exists():
        return None
    # Include reaped git children so the measurement covers poll commands.
    fields = stat.read_text().rsplit(")", 1)[-1].split()
    return sum(map(int, fields[11:15])) / os.sysconf("SC_CLK_TCK")


def fixture_environment(root: Path) -> dict[str, str]:
    env = {
        "PATH": os.environ.get("PATH", os.defpath),
        "HOME": str(root),
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": "/dev/null",
        "CBM_ALLOWED_ROOT": str(root),
        "CBM_CACHE_DIR": str(root / "cache"),
        "CBM_RUNTIME_DIR": str(root / "run"),
        "MORI_NAMED_WATCH_FILE": str(root / "watches.json"),
    }
    Path(env["CBM_CACHE_DIR"]).mkdir(mode=0o700)
    Path(env["CBM_RUNTIME_DIR"]).mkdir(mode=0o700)
    return env


def smoke(binary: Path, root: Path) -> None:
    env = fixture_environment(root)
    named = root / "named"
    missing = root / "missing"
    denied = root / "denied"
    unlisted = root / "unlisted"
    cache = Path(env["CBM_CACHE_DIR"])
    checkout(named, "initial_name", env)
    run(["git", "worktree", "add", "--detach", str(unlisted), "HEAD"], env, named)
    snapshot = Path(env["MORI_NAMED_WATCH_FILE"])
    log = cache / "logs/cbm-daemon.log"

    def daemon(action: str) -> str:
        return run([str(binary), "daemon", action], env, root)

    def log_count(event: str, project: str | None = None) -> int:
        try:
            return sum(
                event in line and (not project or f"project={project}" in line)
                for line in log.read_text(encoding="utf-8").splitlines()
            )
        except FileNotFoundError:
            return 0

    def snapshot_text(paths: dict[str, Path]) -> str:
        return json.dumps(
            [{"project": name, "root": str(path)} for name, path in paths.items()]
        )

    def wait_index(project: str, symbol: str) -> float:
        return wait_for(f"{project}.{symbol}", lambda: indexed(cache, project, symbol))

    complete = snapshot_text({"named": named, "missing": missing, "denied": denied})
    replace_snapshot(snapshot, complete)
    try:
        started = daemon("start")
        match = re.search(r"pid (\d+)", started)
        assert match, "Daemon start did not report a PID"
        pid = int(match.group(1))
        wait_index("named", "initial_name")
        wait_for(
            "missing checkout report",
            lambda: log_count("daemon.named_watch.root_missing", "missing") > 0,
        )
        assert not (cache / "missing.db").exists()

        # Observe enough time for at least two unchanged scans.
        starts = log_count("daemon.index.worker_budget", "named")
        assert starts == 1, "Startup did not reserve the configured project name"
        status_command = [
            str(binary),
            "cli",
            "index_repository",
            "--repo-path",
            str(named),
            "--name",
            "named",
            "--status",
            "true",
        ]
        wait_for(
            "named job status",
            lambda: (
                json.loads(run(status_command, env, root)).get("state") == "succeeded"
            ),
        )
        initial_cpu = cpu_seconds(pid)
        idle_start = time.monotonic()
        time.sleep(11)
        idle_elapsed = time.monotonic() - idle_start
        final_cpu = cpu_seconds(pid)
        assert log_count("daemon.index.worker_budget", "named") == starts
        if initial_cpu is not None and final_cpu is not None:
            print(
                f"Idle CPU {final_cpu - initial_cpu:.3f}s over {idle_elapsed:.3f}s",
                flush=True,
            )
        print("Unchanged polls started no index worker", flush=True)

        write_source(named, "intermediate_save")
        write_source(named, "closed_clients_edit")
        latency = wait_index("named", "closed_clients_edit")
        print(f"Closed-client edit published in {latency:.3f}s", flush=True)
        write_source(unlisted, "unlisted_edit")
        denied.symlink_to(unlisted, target_is_directory=True)
        checkout(missing, "restored_checkout", env)
        wait_index("missing", "restored_checkout")
        assert not (cache / "unlisted.db").exists()
        assert not (cache / "denied.db").exists()
        denied.unlink()
        print(
            "Missing checkout restored and unlisted worktree stayed inactive",
            flush=True,
        )

        # An older persisted graph must catch up even at a clean Git HEAD.
        daemon("stop")
        write_source(named, "clean_offline_commit")
        commit(named, env)
        assert not run(["git", "status", "--porcelain"], env, named).strip()
        assert not indexed(cache, "named", "clean_offline_commit")
        daemon("start")
        latency = wait_index("named", "clean_offline_commit")
        print(
            f"Restart published the clean offline commit in {latency:.3f}s", flush=True
        )

        rejected = log_count("daemon.named_watch.invalid_snapshot")
        replace_snapshot(snapshot, "[")
        wait_for(
            "invalid reload rejection",
            lambda: log_count("daemon.named_watch.invalid_snapshot") > rejected,
        )
        write_source(named, "rejected_reload_edit")
        wait_index("named", "rejected_reload_edit")
        print("Invalid snapshot preserved the previous membership", flush=True)

        denied.symlink_to(unlisted, target_is_directory=True)
        suspended = log_count("daemon.named_watch.suspended")
        run([str(binary), "config", "set", "auto_watch", "false"], env, root)
        wait_for(
            "watch suspension",
            lambda: log_count("daemon.named_watch.suspended") > suspended,
        )
        write_source(named, "suspended_edit")
        assert not indexed(cache, "named", "suspended_edit")
        run([str(binary), "config", "set", "auto_watch", "true"], env, root)
        wait_index("named", "suspended_edit")
        assert not (cache / "denied.db").exists()
        denied.unlink()
        print("Re-enabling auto_watch restored the accepted membership", flush=True)

        applied = log_count("daemon.named_watch.applied")
        replace_snapshot(snapshot, complete)
        wait_for(
            "valid snapshot after invalid reload",
            lambda: log_count("daemon.named_watch.applied") > applied,
        )
        applied = log_count("daemon.named_watch.applied")
        initial = snapshot.stat()
        updated = snapshot_text({"missing": missing}).ljust(len(complete))
        replace_snapshot(snapshot, updated, initial.st_mtime_ns)
        replacement = snapshot.stat()
        assert replacement.st_size == initial.st_size
        assert replacement.st_mtime_ns == initial.st_mtime_ns
        wait_for(
            "replacement snapshot",
            lambda: log_count("daemon.named_watch.applied") > applied,
        )
        write_source(named, "removed_watch_edit")
        write_source(missing, "retained_watch_edit")
        wait_index("missing", "retained_watch_edit")
        assert not indexed(cache, "named", "removed_watch_edit")
        assert indexed(cache, "named", "suspended_edit")
        print(
            "Equal-metadata replacement removed the watch and preserved its graph",
            flush=True,
        )
    finally:
        daemon("stop")


def project_databases(cache: Path) -> list[str]:
    # The daemon opens internal SQLite files before any project publishes.
    return sorted(
        path.name
        for path in cache.glob("*.db")
        if path.name not in {"_config.db", "_cross_repo.db", "activity.db"}
        and not path.name.endswith(".adr.db")
    )


def smoke_cold_join(binary: Path, root: Path, *, use_path_name: bool = False) -> None:
    env = fixture_environment(root)
    checkout_path = root / "checkout"
    checkout(checkout_path, "cold_checkout", env)
    (checkout_path / "test.py").rename(checkout_path / "hang_me.py")
    run(["git", "add", "-A"], env, checkout_path)
    commit(checkout_path, env)

    # The fixed checkout suffix and short temporary path use only ASCII.
    project = (
        str(checkout_path).strip("/").replace("/", "-")
        if use_path_name
        else "configured-name"
    )
    cache = Path(env["CBM_CACHE_DIR"])
    snapshot = Path(env["MORI_NAMED_WATCH_FILE"])
    replace_snapshot(
        snapshot, json.dumps([{"project": project, "root": str(checkout_path)}])
    )
    marker = root / "worker-lock.pid"
    env["CBM_TEST_HANG_ON"] = "hang_me.py"
    env["CBM_TEST_WORKER_PROJECT_LOCK_PID_FILE"] = str(marker)
    index_command = [
        str(binary),
        "cli",
        "--quiet",
        "index_repository",
        "--repo-path",
        str(checkout_path),
    ]

    try:
        run([str(binary), "daemon", "start"], env, root)

        def worker_has_lease() -> bool:
            return marker.is_file() and marker.read_text(encoding="utf-8").endswith(
                "\n"
            )

        wait_for(
            "the first worker's project lease (requires TEST_SEAMS=1)", worker_has_lease
        )
        databases = project_databases(cache)
        assert not databases, (
            f"Unexpected graph databases before the request: {databases}"
        )

        status = json.loads(run([*index_command, "--status", "true"], env, root))
        assert status.get("project") == project, status
        assert status.get("state") == "running", status

        result = json.loads(run([*index_command, "--async", "true"], env, root))
        assert result.get("project") == project, result
        assert result.get("joined") is True, result

        log = (cache / "logs/cbm-daemon.log").read_text(encoding="utf-8")
        starts = [
            line for line in log.splitlines() if "daemon.index.worker_budget" in line
        ]
        assert len(starts) == 1 and f"project={project}" in starts[0], starts
        assert project_databases(cache) == [], (
            "A worker published during the join check"
        )
    finally:
        run([str(binary), "daemon", "stop"], env, root)

    del env["CBM_TEST_HANG_ON"]
    del env["CBM_TEST_WORKER_PROJECT_LOCK_PID_FILE"]
    try:
        run([str(binary), "daemon", "start"], env, root)
        wait_for(
            "the named graph after restart",
            lambda: indexed(cache, project, "cold_checkout"),
        )
        assert project_databases(cache) == [f"{project}.db"]
    finally:
        run([str(binary), "daemon", "stop"], env, root)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mori", type=Path, required=True)
    parser.add_argument("--daemon-log", type=Path)
    parser.add_argument(
        "--cold-join",
        action="store_true",
        help="check a cold named watch with a TEST_SEAMS=1 build",
    )
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="mnw-", dir="/tmp") as temporary:
        root = Path(temporary).resolve()
        try:
            binary = args.mori.resolve(strict=True)
            if args.cold_join:
                smoke_cold_join(binary, root)
                derived_root = root / "path-derived"
                derived_root.mkdir()
                smoke_cold_join(binary, derived_root, use_path_name=True)
            else:
                smoke(binary, root)
        finally:
            log = root / "cache/logs/cbm-daemon.log"
            if args.daemon_log and log.is_file():
                shutil.copyfile(log, args.daemon_log)


if __name__ == "__main__":
    main()
