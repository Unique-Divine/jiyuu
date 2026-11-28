"""Regression checks for frozen C reference cache identity and restore."""

import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import c_reference_cache as cache


class CacheTest(unittest.TestCase):
    def test_reuses_verified_binary_and_rebuilds_a_bad_restore(self) -> None:
        with tempfile.TemporaryDirectory() as scratch:
            directory = Path(scratch) / "reference"
            inputs = {"target": "linux-x64"}

            def build(argv: list[str], *, check: bool, env: dict[str, str]) -> None:
                assert check
                assert "ARCHFLAGS" not in env
                assert env["CCACHE_COMPILERCHECK"] == "content"
                binary = Path(argv[-1])
                binary.write_bytes(b"reference")
                binary.chmod(0o755)

            with (
                patch.dict(
                    os.environ, ARCHFLAGS="-m32", CCACHE_COMPILERCHECK="content"
                ),
                patch.object(cache.subprocess, "run", side_effect=build) as run,
            ):
                cache.build_or_verify(directory, inputs)
                cache.build_or_verify(directory, inputs)
                self.assertEqual(run.call_count, 1)
                (directory / cache.EXECUTABLE).write_bytes(b"invalid")
                cache.build_or_verify(directory, inputs)
                self.assertEqual(run.call_count, 2)

            cache.verify(directory, inputs)

    def test_linux_linker_changes_key_even_with_same_compiler_banner(self) -> None:
        with tempfile.TemporaryDirectory() as scratch:
            linker = Path(scratch) / "ld"
            with (
                patch.object(cache.platform, "system", return_value="Linux"),
                patch.object(cache, "output", return_value="fixed tool version"),
                patch.object(cache.shutil, "which", return_value=str(linker)),
            ):
                linker.write_bytes(b"linker one")
                first = cache.key(cache.identity("linux-x64"))
                linker.write_bytes(b"linker two")
                self.assertNotEqual(first, cache.key(cache.identity("linux-x64")))
