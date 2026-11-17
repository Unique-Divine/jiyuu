#!/usr/bin/env python3
"""Decide whether a package version needs publishing, and publish it safely.

Zero-dependency helper for the publish jobs in .github/workflows/ci-test.yaml.

  registry_release.py check npm NAME VERSION
  registry_release.py publish npm NAME VERSION -- npm publish PKG.tgz ...
  registry_release.py check --dist-dir dist pypi NAME VERSION

`check` asks the registry whether NAME@VERSION exists. It retries network
errors, HTTP 429, and HTTP 5xx, treats HTTP 404 as "not published", and fails
on anything else, so a broken lookup never turns into a blind publish. When
GITHUB_OUTPUT is set, it appends `publish=true|false` there.

`publish` runs the publish command. If the command fails, it polls the
registry for NAME@VERSION; if the version shows up, an earlier run already
published it and the step passes. npm answers a publish over an existing
version with E403 or, under trusted publishing, E404, so the registry is the
judge here rather than the error text.

For PyPI, pass --dist-dir with the built files. A version then counts as
published only when PyPI lists every one of those filenames, so an upload
that stopped after the wheel but before the sdist gets retried instead of
skipped.

npm lookups use the per-version endpoint (registry.npmjs.org/NAME/VERSION),
which the CDN doesn't cache. `npm view` reads the cached packument, which can
lag a fresh publish by minutes.
"""

from __future__ import annotations

import argparse
import io
import json
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


Fetch = Callable[[str], tuple[int | None, str, bytes]]


def http_status(
    url: str, timeout_s: float = 15.0
) -> tuple[int | None, str, bytes]:
    """GET url and return (status, detail, body).

    Status is None on network errors.
    """
    request = urllib.request.Request(
        url,
        headers={"User-Agent": USER_AGENT, "Cache-Control": "no-cache"},
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout_s) as response:
            return response.status, "ok", response.read()
    except urllib.error.HTTPError as err:
        return err.code, err.reason or "", b""
    except (urllib.error.URLError, TimeoutError, OSError) as err:
        return None, str(err), b""


def missing_files(body: bytes, expected: frozenset[str]) -> set[str]:
    """Expected filenames absent from a PyPI version JSON's `urls` list."""
    try:
        urls = json.loads(body).get("urls", [])
        present = {entry["filename"] for entry in urls}
    except (ValueError, AttributeError, TypeError, KeyError) as err:
        raise RegistryLookupError(f"unreadable PyPI JSON: {err}") from err
    return set(expected) - present


def dist_filenames(dist_dir: str) -> frozenset[str]:
    """Basenames of the built distributions in dist_dir."""
    names = frozenset(
        entry.name
        for entry in os.scandir(dist_dir)
        if entry.is_file()
        and (entry.name.endswith(".whl") or entry.name.endswith(".tar.gz"))
    )
    if not names:
        raise RegistryLookupError(f"no .whl or .tar.gz files in {dist_dir}")
    return names


def lookup(
    url: str,
    retry: Retry = Retry(),
    fetch: Fetch = http_status,
    retry_missing: bool = False,
    expected: frozenset[str] = frozenset(),
) -> bool:
    """Return True if the version exists, False if the registry says 404.

    With expected filenames (PyPI), the version counts as published only
    when the registry lists every one of them; a partial upload is missing.

    Retries transient failures; with retry_missing, also retries 404 (used
    after a failed publish, when the version is expected to appear).
    Raises RegistryLookupError when no attempt gives a usable answer.
    """
    last = ""
    outcome = Status.RETRY
    for attempt in range(1, retry.attempts + 1):
        status, detail, body = fetch(url)
        outcome = classify(status)
        last = f"HTTP {status}" if status is not None else f"network: {detail}"
        if outcome is Status.PUBLISHED and expected:
            missing = missing_files(body, expected)
            if missing:
                outcome = Status.MISSING
                last = "missing files: " + ", ".join(sorted(missing))
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
    if retry_missing and outcome is Status.MISSING:
        return False
    raise RegistryLookupError(
        f"{url}: gave up after {retry.attempts} tries ({last})"
    )


def prune_present(url: str, dist_dir: str, retry: Retry,
                  fetch: Fetch = http_status) -> list[str]:
    """Delete files in dist_dir that PyPI already lists for this version.

    A rebuilt wheel or sdist rarely matches the uploaded one byte for byte,
    and `uv publish --check-url` rejects a same-named file with a different
    hash. Removing those files lets a retry upload only what is missing.
    Returns the removed filenames.
    """
    last = ""
    for attempt in range(1, retry.attempts + 1):
        status, detail, body = fetch(url)
        outcome = classify(status)
        last = f"HTTP {status}" if status is not None else f"network: {detail}"
        if outcome is Status.MISSING:
            return []
        if outcome is Status.PUBLISHED:
            local = dist_filenames(dist_dir)
            present = sorted(local - missing_files(body, local))
            for name in present:
                os.remove(os.path.join(dist_dir, name))
            return present
        if outcome is Status.ERROR:
            raise RegistryLookupError(f"{url}: unexpected HTTP {status}")
        if attempt < retry.attempts:
            retry.sleep(retry.delay_s * attempt)
    raise RegistryLookupError(f"{url}: gave up ({last})")


def write_github_output(key: str, value: str) -> None:
    path = os.environ.get("GITHUB_OUTPUT")
    if path:
        with open(path, "a", encoding="utf-8") as out:
            out.write(f"{key}={value}\n")


def dist_filenames_or_empty(dist_dir: str) -> frozenset[str]:
    try:
        return dist_filenames(dist_dir)
    except RegistryLookupError:
        return frozenset()


def expected_files(args: argparse.Namespace) -> frozenset[str]:
    return dist_filenames(args.dist_dir) if args.dist_dir else frozenset()


def cmd_check(args: argparse.Namespace, retry: Retry) -> int:
    url = version_url(args.registry, args.name, args.version, args.base_url)
    try:
        published = lookup(url, retry, expected=expected_files(args))
    except RegistryLookupError as err:
        print(f"::error::Can't tell whether {args.name}@{args.version} is "
              f"on {args.registry}: {err}")
        return 1
    if published:
        print(f"{args.name}@{args.version} is already on {args.registry}; "
              "nothing to publish.")
    else:
        print(f"{args.name}@{args.version} is not fully on "
              f"{args.registry} yet.")
    write_github_output("publish", "false" if published else "true")
    return 0


def cmd_publish(args: argparse.Namespace, retry: Retry) -> int:
    command = list(args.command)
    if command and command[0] == "--":
        command = command[1:]
    if not command:
        print("::error::publish needs a command after --")
        return 2
    url = version_url(args.registry, args.name, args.version, args.base_url)
    if args.dist_dir:
        try:
            pruned = prune_present(url, args.dist_dir, retry)
        except RegistryLookupError as err:
            print(f"::error::Can't list files already on PyPI: {err}")
            return 1
        for name in pruned:
            print(f"{name} is already on PyPI; not uploading it again.")
        if pruned and not dist_filenames_or_empty(args.dist_dir):
            print(f"Every file of {args.name}@{args.version} is on PyPI.")
            return 0
    code = subprocess.call(command)
    if code == 0:
        return 0
    print(f"Publish exited {code}; checking whether {args.name}@"
          f"{args.version} is on {args.registry} anyway.")
    try:
        published = lookup(
            url, retry, retry_missing=True, expected=expected_files(args)
        )
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
        p.add_argument("--dist-dir", default="",
                       help="PyPI only: every .whl/.tar.gz here must be on "
                       "the registry for the version to count as published")
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
    if args.dist_dir and args.registry != "pypi":
        print("::error::--dist-dir applies to pypi only")
        return 2
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
