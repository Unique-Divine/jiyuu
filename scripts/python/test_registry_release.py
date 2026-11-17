"""Offline checks for registry_release against a local HTTP registry."""

from __future__ import annotations

import os
import sys
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from registry_release import (
    RegistryLookupError,
    Retry,
    Status,
    classify,
    lookup,
    main,
    version_url,
)

NO_WAIT = Retry(attempts=3, delay_s=0, sleep=lambda _s: None)


class FakeRegistry:
    """Serves a scripted list of statuses per path, repeating the last one."""

    def __init__(self) -> None:
        self.scripts: dict[str, list[int]] = {}
        self.hits: dict[str, int] = {}
        registry = self

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self) -> None:  # noqa: N802
                hits = registry.hits.get(self.path, 0)
                registry.hits[self.path] = hits + 1
                script = registry.scripts.get(self.path, [404])
                status = script[min(hits, len(script) - 1)]
                self.send_response(status)
                self.end_headers()
                self.wfile.write(b"{}")

            def log_message(self, format: str, *args: object) -> None:
                pass

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.base = f"http://127.0.0.1:{self.server.server_address[1]}"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def close(self) -> None:
        self.server.shutdown()
        self.server.server_close()


class RegistryTests(unittest.TestCase):
    def setUp(self) -> None:
        self.registry = FakeRegistry()
        self.addCleanup(self.registry.close)

    def url(self, path: str) -> str:
        return self.registry.base + path

    def test_version_urls(self) -> None:
        self.assertEqual(
            version_url("npm", "@uniquedivine/tg-sai", "0.3.1"),
            "https://registry.npmjs.org/@uniquedivine%2Ftg-sai/0.3.1",
        )
        self.assertEqual(
            version_url("pypi", "uniquedivine-telegram-mcp", "2.0.1"),
            "https://pypi.org/pypi/uniquedivine-telegram-mcp/2.0.1/json",
        )

    def test_classify(self) -> None:
        cases = [
            (200, Status.PUBLISHED),
            (404, Status.MISSING),
            (None, Status.RETRY),
            (429, Status.RETRY),
            (503, Status.RETRY),
            (401, Status.ERROR),
            (403, Status.ERROR),
        ]
        for status, expected in cases:
            with self.subTest(status=status):
                self.assertEqual(classify(status), expected)

    def test_found_and_missing(self) -> None:
        self.registry.scripts["/a"] = [200]
        self.assertTrue(lookup(self.url("/a"), NO_WAIT))
        self.assertFalse(lookup(self.url("/b"), NO_WAIT))
        self.assertEqual(self.registry.hits["/b"], 1)

    def test_retries_transient_errors(self) -> None:
        self.registry.scripts["/a"] = [503, 429, 200]
        self.assertTrue(lookup(self.url("/a"), NO_WAIT))
        self.assertEqual(self.registry.hits["/a"], 3)

    def test_gives_up_on_persistent_server_errors(self) -> None:
        self.registry.scripts["/a"] = [502]
        with self.assertRaises(RegistryLookupError):
            lookup(self.url("/a"), NO_WAIT)
        self.assertEqual(self.registry.hits["/a"], 3)

    def test_fails_fast_on_unexpected_status(self) -> None:
        self.registry.scripts["/a"] = [401]
        with self.assertRaises(RegistryLookupError):
            lookup(self.url("/a"), NO_WAIT)
        self.assertEqual(self.registry.hits["/a"], 1)

    def test_network_error_is_retried_then_raised(self) -> None:
        self.registry.close()
        with self.assertRaises(RegistryLookupError):
            lookup(self.url("/a"), NO_WAIT)

    def test_retry_missing_waits_for_lagging_version(self) -> None:
        self.registry.scripts["/a"] = [404, 404, 200]
        self.assertTrue(lookup(self.url("/a"), NO_WAIT, retry_missing=True))
        self.registry.scripts["/b"] = [404]
        self.assertFalse(lookup(self.url("/b"), NO_WAIT, retry_missing=True))
        self.assertEqual(self.registry.hits["/b"], 3)


class CliTests(unittest.TestCase):
    def setUp(self) -> None:
        self.registry = FakeRegistry()
        self.addCleanup(self.registry.close)
        out = tempfile.NamedTemporaryFile(delete=False)
        out.close()
        self.output = Path(out.name)
        self.addCleanup(self.output.unlink)
        old = os.environ.get("GITHUB_OUTPUT")
        os.environ["GITHUB_OUTPUT"] = str(self.output)
        self.addCleanup(self.restore_env, old)

    @staticmethod
    def restore_env(old: str | None) -> None:
        if old is None:
            os.environ.pop("GITHUB_OUTPUT", None)
        else:
            os.environ["GITHUB_OUTPUT"] = old

    def run_cli(self, *args: str) -> int:
        return main([args[0], "--base-url", self.registry.base, *args[1:]])

    def test_check_writes_publish_flag(self) -> None:
        self.registry.scripts["/@s%2Fpkg/1.0.0"] = [200]
        self.assertEqual(self.run_cli("check", "npm", "@s/pkg", "1.0.0"), 0)
        self.assertEqual(self.run_cli("check", "npm", "@s/pkg", "2.0.0"), 0)
        self.assertEqual(
            self.output.read_text().splitlines(),
            ["publish=false", "publish=true"],
        )

    def test_check_fails_without_publishing_on_lookup_error(self) -> None:
        self.registry.scripts["/pypi/pkg/1.0.0/json"] = [403]
        self.assertEqual(self.run_cli("check", "pypi", "pkg", "1.0.0"), 1)
        self.assertEqual(self.output.read_text(), "")

    def test_publish_success_skips_registry(self) -> None:
        ok = [sys.executable, "-c", "pass"]
        code = self.run_cli("publish", "npm", "@s/pkg", "1.0.0", "--", *ok)
        self.assertEqual(code, 0)
        self.assertEqual(self.registry.hits, {})

    def test_failed_publish_of_existing_version_passes(self) -> None:
        self.registry.scripts["/@s%2Fpkg/1.0.0"] = [200]
        fail = [sys.executable, "-c", "raise SystemExit(1)"]
        code = self.run_cli("publish", "npm", "@s/pkg", "1.0.0", "--", *fail)
        self.assertEqual(code, 0)

    def test_failed_publish_of_missing_version_fails(self) -> None:
        fail = [sys.executable, "-c", "raise SystemExit(7)"]
        code = self.run_cli(
            "publish", "--delay", "0", "npm", "@s/pkg", "1.0.0", "--", *fail
        )
        self.assertEqual(code, 7)
        self.assertEqual(self.registry.hits["/@s%2Fpkg/1.0.0"], 6)

if __name__ == "__main__":
    unittest.main()
