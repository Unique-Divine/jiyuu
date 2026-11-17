#!/usr/bin/env python3
"""Decide whether a package version needs publishing, and publish it safely.

Zero-dependency helper for the publish jobs in .github/workflows/ci-test.yaml.

  registry_release.py check npm NAME VERSION
  registry_release.py publish npm NAME VERSION -- npm publish PKG.tgz ...

`check` asks the registry whether NAME@VERSION exists. It retries network
errors, HTTP 429, and HTTP 5xx, treats HTTP 404 as "not published", and fails
on anything else, so a broken lookup never turns into a blind publish. When
GITHUB_OUTPUT is set, it appends `publish=true|false` there.

`publish` runs the publish command. If the command fails, it polls the
registry for NAME@VERSION; if the version shows up, an earlier run already
published it and the step passes. npm answers a publish over an existing
version with E403 or, under trusted publishing, E404, so the registry is the
judge here rather than the error text.

npm lookups use the per-version endpoint (registry.npmjs.org/NAME/VERSION),
which the CDN doesn't cache. `npm view` reads the cached packument, which can
lag a fresh publish by minutes.
"""

from __future__ import annotations

import argparse
import io
import os
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from enum import Enum
from typing import Callable, Sequence

NPM_REGISTRY = "https://registry.npmjs.org"
PYPI_REGISTRY = "https://pypi.org"
USER_AGENT = "jiyuu-ci-registry-release"


class Status(Enum):
    PUBLISHED = "published"
    MISSING = "missing"
    RETRY = "retry"
    ERROR = "error"


class RegistryLookupError(Exception):
    """The registry gave no usable answer."""


@dataclass(frozen=True)
class Retry:
    attempts: int = 3
    delay_s: float = 2.0
    sleep: Callable[[float], None] = time.sleep


def version_url(registry: str, name: str, version: str, base: str = "") -> str:
    """URL whose HTTP status says whether NAME@VERSION exists."""
    quoted_name = urllib.parse.quote(name, safe="@")
    quoted_version = urllib.parse.quote(version, safe="")
    if registry == "npm":
        return f"{base or NPM_REGISTRY}/{quoted_name}/{quoted_version}"
    if registry == "pypi":
        root = base or PYPI_REGISTRY
        return f"{root}/pypi/{quoted_name}/{quoted_version}/json"
    raise ValueError(f"unknown registry: {registry}")


def classify(status: int | None) -> Status:
    """Map an HTTP status (None for a network error) to a lookup outcome."""
    if status is None or status == 429 or (500 <= status <= 599):
        return Status.RETRY
    if status == 200:
        return Status.PUBLISHED
    if status == 404:
        return Status.MISSING
    return Status.ERROR


def http_status(url: str, timeout_s: float = 15.0) -> tuple[int | None, str]:
    """GET url and return (status, detail). Status is None on network errors."""
    request = urllib.request.Request(
        url,
        headers={"User-Agent": USER_AGENT, "Cache-Control": "no-cache"},
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout_s) as response:
            return response.status, "ok"
    except urllib.error.HTTPError as err:
        return err.code, err.reason or ""
    except (urllib.error.URLError, TimeoutError, OSError) as err:
        return None, str(err)


def lookup(
    url: str,
    retry: Retry = Retry(),
    fetch: Callable[[str], tuple[int | None, str]] = http_status,
    retry_missing: bool = False,
) -> bool:
    """Return True if the version exists, False if the registry says 404.

    Retries transient failures; with retry_missing, also retries 404 (used
    after a failed publish, when the version is expected to appear).
    Raises RegistryLookupError when no attempt gives a usable answer.
    """
    last = ""
    for attempt in range(1, retry.attempts + 1):
        status, detail = fetch(url)
        outcome = classify(status)
        last = f"HTTP {status}" if status is not None else f"network: {detail}"
        if outcome is Status.PUBLISHED:
            return True
        if outcome is Status.MISSING and not retry_missing:
            return False
        if outcome is Status.ERROR:
            message = f"{url}: unexpected {last} {detail}"
            raise RegistryLookupError(message.strip())
        if attempt < retry.attempts:
            print(
                f"{url}: {last}; retry {attempt}/{retry.attempts - 1}",
                file=sys.stderr,
            )
            retry.sleep(retry.delay_s * attempt)
    if retry_missing and last == "HTTP 404":
        return False
    raise RegistryLookupError(
        f"{url}: gave up after {retry.attempts} tries ({last})"
    )


def write_github_output(key: str, value: str) -> None:
    path = os.environ.get("GITHUB_OUTPUT")
    if path:
        with open(path, "a", encoding="utf-8") as out:
            out.write(f"{key}={value}\n")


def cmd_check(args: argparse.Namespace, retry: Retry) -> int:
    url = version_url(args.registry, args.name, args.version, args.base_url)
    try:
        published = lookup(url, retry)
    except RegistryLookupError as err:
        print(f"::error::Can't tell whether {args.name}@{args.version} is "
              f"on {args.registry}: {err}")
        return 1
    if published:
        print(f"{args.name}@{args.version} is already on {args.registry}; "
              "nothing to publish.")
    else:
        print(f"{args.name}@{args.version} is not on {args.registry} yet.")
    write_github_output("publish", "false" if published else "true")
    return 0


def cmd_publish(args: argparse.Namespace, retry: Retry) -> int:
    command = list(args.command)
    if command and command[0] == "--":
        command = command[1:]
    if not command:
        print("::error::publish needs a command after --")
        return 2
    code = subprocess.call(command)
    if code == 0:
        return 0
    url = version_url(args.registry, args.name, args.version, args.base_url)
    print(f"Publish exited {code}; checking whether {args.name}@"
          f"{args.version} is on {args.registry} anyway.")
    try:
        published = lookup(url, retry, retry_missing=True)
    except RegistryLookupError as err:
        print(f"::error::Publish failed and the registry check failed: {err}")
        return code
    if published:
        print(f"::warning::{args.name}@{args.version} is already on "
              f"{args.registry}; an earlier run published it.")
        return 0
    print(f"::error::Publish failed and {args.name}@{args.version} is not "
          f"on {args.registry}.")
    return code


def parse_args(argv: Sequence[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="action", required=True)
    for action in ("check", "publish"):
        p = sub.add_parser(action)
        p.add_argument("registry", choices=["npm", "pypi"])
        p.add_argument("name")
        p.add_argument("version")
        p.add_argument("--base-url", default="",
                       help="registry base URL override (tests)")
        p.add_argument("--attempts", type=int, default=None,
                       help="lookup attempts (check: 3, publish: 6)")
        p.add_argument("--delay", type=float, default=None,
                       help="base backoff seconds (check: 2, publish: 5)")
        if action == "publish":
            p.add_argument("command", nargs=argparse.REMAINDER,
                           help="publish command, after --")
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    # Keep stdout, stderr, and the publish command's output in order in CI.
    if isinstance(sys.stdout, io.TextIOWrapper):
        sys.stdout.reconfigure(line_buffering=True)
    # After a failed publish the version may take a while to show up, so
    # publish polls longer than check.
    attempts, delay_s = (3, 2.0) if args.action == "check" else (6, 5.0)
    retry = Retry(
        attempts=attempts if args.attempts is None else args.attempts,
        delay_s=delay_s if args.delay is None else args.delay,
    )
    if args.action == "check":
        return cmd_check(args, retry)
    return cmd_publish(args, retry)


if __name__ == "__main__":
    sys.exit(main())
