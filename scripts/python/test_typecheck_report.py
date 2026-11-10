"""Offline checks for enforced/report-only exits and actual ty file selection."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Sequence

from typecheck_report import TY_VERSION, build_report, report_exit_code, run_report

ROOT = Path(__file__).resolve().parents[2]
FINDINGS = (
    "example.py:1:1: error[invalid-assignment] Expected int, found str\n"
    "example.py:2:1: warning[deprecated] Example warning\n"
    "Found 2 diagnostics\n"
)


class ReporterTests(unittest.TestCase):
    def test_only_diagnostic_status_is_softened(self) -> None:
        for raw, expected in [(0, 0), (1, 0), (2, 2), (101, 101), (9, 9), (-9, 2)]:
            with self.subTest(raw=raw):
                self.assertEqual(report_exit_code(raw), expected)

    def test_enforced_checker_failures_and_warning_only_results(self) -> None:
        warning = "test.py:1:1: warning[deprecated] Example\nFound 1 diagnostic\n"
        for raw, output, expected in [
            (0, "", 0),
            (0, warning, 1),
            (1, FINDINGS, 1),
            (2, "", 2),
            (101, "", 101),
            (-9, "", 2),
        ]:
            calls: list[list[str]] = []

            def run(argv: Sequence[str]) -> subprocess.CompletedProcess[str]:
                calls.append(list(argv))
                if list(argv) == ["ty", "version"]:
                    return subprocess.CompletedProcess(argv, 0, f"ty {TY_VERSION}\n", "")
                return subprocess.CompletedProcess(argv, raw, output, "")

            with self.subTest(raw=raw, output=output), tempfile.TemporaryDirectory() as directory:
                path = Path(directory)
                self.assertEqual(
                    run_report("example", path, ["."], runner=run, enforce=True),
                    expected,
                )
                self.assertIn("--error-on-warning", calls[1])
                self.assertTrue(json.loads((path / "report.json").read_text())["enforced"])
                if expected == 1:
                    self.assertIn("CI fails", (path / "summary.md").read_text())
                    self.assertNotIn("without blocking", (path / "summary.md").read_text())

    def test_typed_report_keeps_native_status_and_counts(self) -> None:
        result = subprocess.CompletedProcess(["ty"], 1, FINDINGS, "")
        report = build_report("example", result)
        self.assertEqual(report.status, "findings")
        self.assertEqual(report.checker_exit_code, 1)
        self.assertEqual((report.errors, report.warnings), (1, 1))
        self.assertEqual(report.rules, {"deprecated": 1, "invalid-assignment": 1})
        self.assertEqual(report.diagnostics[0].path, "example.py")

    def test_success_does_not_claim_a_warning_is_clean(self) -> None:
        warning = "test.py:1:1: warning[deprecated] Example\nFound 1 diagnostic\n"
        self.assertEqual(
            build_report("example", subprocess.CompletedProcess([], 0, warning, "")).status,
            "findings",
        )
        self.assertEqual(
            build_report(
                "example",
                subprocess.CompletedProcess([], 0, "All checks passed!\n", ""),
            ).status,
            "clean",
        )

    def test_unparseable_diagnostic_exit_is_fatal(self) -> None:
        for output in ["launch failed", "Found 2 diagnostics\n"]:
            with self.subTest(output=output), self.assertRaises(ValueError):
                build_report("example", subprocess.CompletedProcess([], 1, output, ""))

    def test_reporter_writes_artifacts_summary_and_preserves_failures(self) -> None:
        for raw in [0, 1, 2, 101, -9]:
            calls: list[list[str]] = []

            def run(argv: Sequence[str]) -> subprocess.CompletedProcess[str]:
                calls.append(list(argv))
                if list(argv) == ["ty", "version"]:
                    return subprocess.CompletedProcess(argv, 0, f"ty {TY_VERSION}\n", "")
                return subprocess.CompletedProcess(argv, raw, FINDINGS if raw == 1 else "", "")

            with self.subTest(raw=raw), tempfile.TemporaryDirectory() as directory:
                output = Path(directory) / "report"
                summary = Path(directory) / "step-summary.md"
                self.assertEqual(
                    run_report("example", output, ["."], runner=run, summary_path=summary),
                    report_exit_code(raw),
                )
                value: object = json.loads((output / "report.json").read_text())
                self.assertIsInstance(value, dict)
                self.assertTrue((output / "diagnostics.txt").exists())
                self.assertIn(f"raw exit {raw}", summary.read_text())
                self.assertEqual(
                    calls[1],
                    [
                        "ty",
                        "check",
                        "--output-format",
                        "concise",
                        "--color",
                        "never",
                        ".",
                    ],
                )

    def test_version_and_launch_failures_never_become_findings(self) -> None:
        calls: list[list[str]] = []

        def wrong_version(argv: Sequence[str]) -> subprocess.CompletedProcess[str]:
            calls.append(list(argv))
            return subprocess.CompletedProcess(argv, 0, "ty 9.9.9\n", "")

        def missing_command(argv: Sequence[str]) -> subprocess.CompletedProcess[str]:
            raise FileNotFoundError("ty is absent")

        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(ValueError):
                run_report("example", Path(directory), ["."], runner=wrong_version)
            with self.assertRaises(OSError):
                run_report("example", Path(directory), ["."], runner=missing_command)
        self.assertEqual(calls, [["ty", "version"]])

    def test_cli_missing_checker_exits_fatally(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            env = dict(os.environ, PATH=directory)
            result = subprocess.run(
                [
                    sys.executable,
                    str(ROOT / "scripts/python/typecheck_report.py"),
                    "--project",
                    "example",
                    "--output-dir",
                    directory,
                    ".",
                ],
                env=env,
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(result.returncode, 2)
            self.assertIn("reporting failed", result.stderr)

    def test_uv_launch_failure_is_not_softened(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            executable = path / "uv"
            executable.write_text("#!/bin/sh\nexit 17\n")
            executable.chmod(0o755)
            just = shutil.which("just")
            if just is None:
                self.skipTest("just is not installed in this environment")
            assert just is not None
            result = subprocess.run(
                [
                    just,
                    "--justfile",
                    str(ROOT / "mcp/telegram/justfile"),
                    "typecheck-report",
                    str(path / "output"),
                ],
                env=dict(os.environ, PATH=directory + os.pathsep + os.environ.get("PATH", "")),
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("exit code 17", result.stderr)
            self.assertFalse((path / "output").exists())

    def test_root_recipe_checks_both_projects_after_first_fails(self) -> None:
        just = shutil.which("just")
        if just is None:
            self.skipTest("just is not installed in this environment")
        assert just is not None
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            executable = path / "just"
            executable.write_text(
                '#!/bin/sh\nprintf "%s\\n" "$*" >> "$CALLS"\n'
                'case "$*" in *mcp/telegram*) exit 1 ;; *) exit 0 ;; esac\n'
            )
            executable.chmod(0o755)
            log = path / "calls.txt"
            env = dict(
                os.environ,
                PATH=directory + os.pathsep + os.environ.get("PATH", ""),
                CALLS=str(log),
            )
            result = subprocess.run(
                [just, "--justfile", str(ROOT / "justfile"), "python-typecheck"],
                env=env,
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertNotEqual(result.returncode, 0)
            calls = log.read_text().splitlines()
            self.assertEqual(len(calls), 2)
            self.assertIn("mcp/telegram/justfile typecheck", calls[0])
            self.assertIn("ts-pkg/tg-sai/justfile py-typecheck", calls[1])


class ActualTyTests(unittest.TestCase):
    def test_default_project_scan_includes_root_nested_and_test_files(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "pyproject.toml").write_text(
                '[project]\nname = "typecheck-fixture"\nversion = "0.0.0"\nrequires-python = ">=3.10"\n'
            )
            for name in ["module.py", "nested/module.py", "tests/test_module.py"]:
                file = root / name
                file.parent.mkdir(parents=True, exist_ok=True)
                file.write_text('value: int = "invalid"\n')
            result = subprocess.run(
                [
                    "ty",
                    "check",
                    "--project",
                    directory,
                    "--output-format",
                    "concise",
                    "--color",
                    "never",
                ],
                cwd=root,
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
            report = build_report("fixture", result)
            self.assertEqual(len(report.diagnostics), 3)
            for name in ["module.py", "nested/module.py", "tests/test_module.py"]:
                self.assertTrue(any(item.path.endswith(name) for item in report.diagnostics))

    def test_explicit_external_directory_is_checked(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            project = root / "project"
            shared = root / "shared"
            project.mkdir()
            shared.mkdir()
            (project / "pyproject.toml").write_text(
                '[project]\nname = "external-fixture"\nversion = "0.0.0"\n'
            )
            (shared / "helper.py").write_text('value: int = "invalid"\n')
            result = subprocess.run(
                ["ty", "check", "--output-format", "concise", ".", str(shared)],
                cwd=project,
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
            self.assertEqual(len(build_report("fixture", result).diagnostics), 1)
            self.assertIn("helper.py", result.stdout)

    def test_real_warning_only_fixture_blocks_enforce_but_not_report_mode(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            fixture = path / "warning.py"
            fixture.write_text("async def pending() -> None:\n    pass\npending()\n")
            report_only = path / "report"
            enforced = path / "enforced"
            self.assertEqual(run_report("warning", report_only, [str(fixture)]), 0)
            self.assertEqual(run_report("warning", enforced, [str(fixture)], enforce=True), 1)
            report = json.loads((enforced / "report.json").read_text())
            self.assertEqual(report["errors"], 0)
            self.assertEqual(report["warnings"], 1)
            self.assertEqual(report["checker_exit_code"], 1)
            self.assertTrue(report["enforced"])
            self.assertTrue((enforced / "diagnostics.txt").exists())

    def test_malformed_configuration_is_fatal_through_reporter(self) -> None:
        def bad_configuration(argv: Sequence[str]) -> subprocess.CompletedProcess[str]:
            if list(argv) == ["ty", "version"]:
                return subprocess.run(argv, capture_output=True, text=True, check=False)
            return subprocess.run(
                [*argv, "--config", "not-a-real-option = true"],
                capture_output=True,
                text=True,
                check=False,
            )

        with tempfile.TemporaryDirectory() as directory:
            self.assertEqual(
                run_report(
                    "bad-config",
                    Path(directory),
                    [str(ROOT / "scripts/python/typecheck_report.py")],
                    runner=bad_configuration,
                ),
                2,
            )


if __name__ == "__main__":
    unittest.main()
