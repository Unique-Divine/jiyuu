"""Compare a proposed focused suite list with the existing native CI run."""

import argparse
import json
import os
import re
import subprocess
from pathlib import Path
from typing import NamedTuple

PIPELINE_SUITES = frozenset({"edge_types_probe", "pipeline", "pipeline_frozen"})
LEAF_SUITES = {
    "mori/src/pipeline/pass_ansible.c": PIPELINE_SUITES,
    "mori/src/pipeline/pass_operator.c": PIPELINE_SUITES,
    "mori/src/pipeline/pass_semantic_edges.c": PIPELINE_SUITES | {"doclinks"},
    "mori/src/pipeline/pass_terraform.c": PIPELINE_SUITES,
}

SHA = re.compile(r"[0-9a-f]{40}")
SUITE = re.compile(r"^=== ([a-z_][a-z0-9_]*) ===$")
TOTALS = re.compile(r"\s*(\d+) passed(?:, (\d+) failed)?(?:, (\d+) skipped)?\s*")
FAILURE = re.compile(r"\bFAIL\s+\S+:\d+:")


class Selection(NamedTuple):
    suites: frozenset[str]
    reason: str


class Observation(NamedTuple):
    seen: set[str]
    failed: list[str]
    totals: tuple[int, int, int] | None


def select(diff: bytes, baseline: frozenset[str]) -> Selection:
    fields = diff.split(b"\0")
    if not diff or fields[-1] or len(fields) % 2 != 1:
        return Selection(baseline, "empty or malformed diff")
    selected: set[str] = set()
    for status, path in zip(fields[0:-1:2], fields[1:-1:2], strict=True):
        # Only an in-place edit to a named implementation qualifies. A rename
        # arrives as addition and deletion because diff uses --no-renames.
        suites = LEAF_SUITES.get(os.fsdecode(path))
        if status != b"M" or suites is None:
            return Selection(baseline, "change outside the leaf allowlist")
        selected.update(suites)
    if not selected < baseline:
        return Selection(baseline, "focused suites unavailable in the baseline")
    return Selection(frozenset(selected), "implementation-only leaf changes")


def observe(log: str, baseline: frozenset[str]) -> Observation:
    seen: set[str] = set()
    failed: list[str] = []
    totals = None
    suite = ""
    for line in log.splitlines():
        if header := SUITE.fullmatch(line):
            candidate = header[1]
            if candidate in baseline:
                suite = candidate
                seen.add(suite)
        if suite and FAILURE.search(line):
            failed.append(suite)
        if counts := TOTALS.fullmatch(line):
            totals = (int(counts[1]), int(counts[2] or 0), int(counts[3] or 0))
    return Observation(seen, failed, totals)


def git(repo: Path, *args: str) -> bytes:
    return subprocess.run(
        ["git", "--no-replace-objects", "-C", str(repo), *args],
        check=True,
        capture_output=True,
    ).stdout


def exact_diff(repo: Path, base: str, head: str, tested: str) -> bytes | None:
    if not all(SHA.fullmatch(sha) for sha in (base, head, tested)):
        return None
    try:
        if git(repo, "rev-list", "--parents", "-n", "1", "HEAD").split() != [
            sha.encode() for sha in (tested, base, head)
        ]:
            return None
        return git(repo, "diff", "--name-status", "-z", "--no-renames", base, tested)
    except (OSError, subprocess.CalledProcessError):
        return None


def report(args: argparse.Namespace) -> dict[str, object]:
    baseline = frozenset(args.baseline_suites.split())
    diff = exact_diff(args.repo, args.base, args.head, args.tested)
    selection = (
        select(diff, baseline)
        if diff is not None
        else Selection(baseline, "verified PR merge parents unavailable")
    )
    baseline_run = observe(args.log.read_text(errors="replace"), baseline)
    omitted = baseline - selection.suites
    complete = (
        baseline_run.seen == baseline
        and baseline_run.totals is not None
        and sum(baseline_run.totals) > 0
        and baseline_run.totals[1] == len(baseline_run.failed)
        and (baseline_run.totals[1] == 0) == (args.outcome == "success")
    )
    return {
        "schema": 1,
        "architecture": args.arch,
        "base": args.base or None,
        "head": args.head or None,
        "tested_commit": args.tested,
        "selection": "focused" if omitted else "full",
        "reason": selection.reason,
        "baseline_suites": sorted(baseline),
        "selected_suites": sorted(selection.suites),
        "omitted_suites": sorted(omitted),
        "baseline_outcome": args.outcome,
        "observed_suites": sorted(baseline_run.seen),
        "baseline_totals": (
            dict(zip(("passed", "failed", "skipped"), baseline_run.totals, strict=True))
            if baseline_run.totals is not None
            else None
        ),
        "comparison_complete": complete,
        "omitted_failure_suites": sorted(omitted & set(baseline_run.failed)),
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path("."))
    parser.add_argument("--base", default="")
    parser.add_argument("--head", default="")
    parser.add_argument("--tested", required=True)
    parser.add_argument("--arch", required=True)
    parser.add_argument("--log", type=Path, required=True)
    parser.add_argument("--baseline-suites", required=True)
    parser.add_argument("--outcome", choices=("success", "failure"), required=True)
    args = parser.parse_args()
    result = json.dumps(report(args), indent=2, sort_keys=True)
    print(result)
    if summary := os.environ.get("GITHUB_STEP_SUMMARY"):
        with Path(summary).open("a", encoding="utf-8") as output:
            output.write(
                "### Mori native suite selection (report only)\n\n"
                "The existing suite run supplies all results below.\n\n"
                f"```json\n{result}\n```\n"
            )


if __name__ == "__main__":
    main()
