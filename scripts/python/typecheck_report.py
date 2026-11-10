"""Write ty diagnostics with explicit report-only or blocking enforcement mode.

Run through a project's locked uv environment. Report mode softens diagnostic
exit 1 only. Enforce mode fails on errors and warnings. Setup, configuration,
and checker failures remain failures in both modes.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from collections import Counter
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Callable, Literal, Sequence

TY_VERSION = "0.0.100"
Runner = Callable[[Sequence[str]], subprocess.CompletedProcess[str]]
_DIAGNOSTIC = re.compile(r"^(.+?):(\d+):(\d+): (error|warning)\[([^\]]+)\] (.*)$")
_TOTAL = re.compile(r"^Found (\d+) diagnostics?\s*$", re.MULTILINE)


@dataclass(frozen=True)
class Diagnostic:
    path: str
    line: int
    column: int
    severity: Literal["error", "warning"]
    rule: str
    message: str


@dataclass(frozen=True)
class Report:
    project: str
    ty_version: str
    checker_exit_code: int
    status: Literal["clean", "findings", "failure"]
    errors: int
    warnings: int
    rules: dict[str, int]
    diagnostics: list[Diagnostic]
    enforced: bool = False


def run_command(argv: Sequence[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(argv, capture_output=True, text=True, check=False)


def report_exit_code(checker_exit_code: int, *, enforce: bool = False) -> int:
    """Apply diagnostic enforcement, preserve failures, and reject signal exits."""
    if checker_exit_code in (0, 1):
        return checker_exit_code if enforce else 0
    return checker_exit_code if checker_exit_code > 0 else 2


def parse_diagnostics(output: str) -> list[Diagnostic]:
    diagnostics: list[Diagnostic] = []
    for line in output.splitlines():
        match = _DIAGNOSTIC.match(line)
        if match is None:
            continue
        severity: Literal["error", "warning"] = "error" if match[4] == "error" else "warning"
        diagnostics.append(
            Diagnostic(match[1], int(match[2]), int(match[3]), severity, match[5], match[6])
        )
    return diagnostics


def build_report(
    project: str, result: subprocess.CompletedProcess[str], *, enforce: bool = False
) -> Report:
    output = result.stdout + result.stderr
    diagnostics = parse_diagnostics(output)
    if result.returncode in (0, 1):
        total = _TOTAL.search(output)
        if result.returncode == 1 and total is None:
            raise ValueError(
                "ty exited 1 without its diagnostic total; refusing to hide a failure"
            )
        if total is not None and int(total[1]) != len(diagnostics):
            raise ValueError("ty diagnostic count does not match the parsed report")
    status: Literal["clean", "findings", "failure"]
    if result.returncode not in (0, 1):
        status = "failure"
    elif result.returncode == 1 or diagnostics:
        status = "findings"
    else:
        status = "clean"
    return Report(
        project=project,
        ty_version=TY_VERSION,
        checker_exit_code=result.returncode,
        status=status,
        errors=sum(item.severity == "error" for item in diagnostics),
        warnings=sum(item.severity == "warning" for item in diagnostics),
        rules=dict(sorted(Counter(item.rule for item in diagnostics).items())),
        diagnostics=diagnostics,
        enforced=enforce,
    )


def markdown_summary(report: Report) -> str:
    result = (
        f"{report.errors} errors and {report.warnings} warnings; "
        + ("CI fails" if report.enforced else "reported without blocking CI")
        if report.status == "findings"
        else "No findings" if report.status == "clean" else "Checker failed; CI fails"
    )
    return (
        f"### Python Types: {report.project}\n\n"
        f"ty {report.ty_version}, raw exit {report.checker_exit_code}, {report.status}. "
        f"{result}. Full diagnostics are in the uploaded artifact.\n"
    )


def run_report(
    project: str,
    output_dir: Path,
    paths: Sequence[str],
    *,
    runner: Runner = run_command,
    summary_path: Path | None = None,
    enforce: bool = False,
) -> int:
    version = runner(["ty", "version"])
    if version.returncode != 0 or version.stdout.strip() != f"ty {TY_VERSION}":
        raise ValueError(f"Expected installed ty {TY_VERSION}; run the project's frozen uv sync")
    options = ["--error-on-warning"] if enforce else []
    result = runner(
        [
            "ty",
            "check",
            "--output-format",
            "concise",
            "--color",
            "never",
            *options,
            *paths,
        ]
    )
    output_dir.mkdir(parents=True, exist_ok=True)
    (output_dir / "diagnostics.txt").write_text(result.stdout + result.stderr, encoding="utf-8")
    report = build_report(project, result, enforce=enforce)
    (output_dir / "report.json").write_text(
        json.dumps(asdict(report), indent=2) + "\n", encoding="utf-8"
    )
    summary = markdown_summary(report)
    (output_dir / "summary.md").write_text(summary, encoding="utf-8")
    if summary_path is not None:
        with summary_path.open("a", encoding="utf-8") as stream:
            stream.write(summary + "\n")
    print(summary, end="")
    if enforce and report.status == "findings":
        return 1
    return report_exit_code(result.returncode, enforce=enforce)


class Arguments(argparse.Namespace):
    project: str = ""
    output_dir: Path = Path(".")
    paths: list[str]
    mode: str = "report"

    def __init__(self) -> None:
        super().__init__()
        self.paths = []


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project", required=True, help="Project name shown in the report")
    parser.add_argument("--output-dir", required=True, type=Path)
    parser.add_argument("--mode", choices=("report", "enforce"), default="report")
    parser.add_argument("paths", nargs="+", help="Project and shared helper paths to check")
    args = Arguments()
    parser.parse_args(argv, namespace=args)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    try:
        return run_report(
            args.project,
            args.output_dir,
            args.paths,
            summary_path=Path(summary) if summary else None,
            enforce=args.mode == "enforce",
        )
    except (OSError, ValueError) as error:
        print(f"Python typecheck reporting failed: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
