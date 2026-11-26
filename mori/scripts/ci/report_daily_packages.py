"""File public triage issues for the canonical Mori daily package workflow."""

import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
from collections.abc import Mapping
from dataclasses import asdict, dataclass
from datetime import datetime
from pathlib import Path
from typing import Any

REPO = "Unique-Divine/jiyuu"
WORKFLOW = ".github/workflows/mori-release.yml"
REPO_URL = f"https://github.com/{REPO}"
RUNBOOK = f"{REPO_URL}/blob/main/mori/docs/ci-operations.md"
BOT = "github-actions[bot]"
LABELS = ["Z-mori", "A-tech-investment", "S-triage"]
TARGETS = {"linux-x64", "linux-arm64", "darwin-arm64", "darwin-x64"}
JOB_STAGES = {
    "Build candidate": "candidate build",
    "Build C reference": "frozen C reference",
    "CLI parity": "CLI parity",
}
SHARED_STAGES = {
    "Validate release request": "release validation",
    "Prepare package bundle": "package verification",
}
STEP_STAGES = {
    "Build mori": "candidate build",
    "Select package targets and validate the version": "release version validation",
    "CLI parity with codebase-memory-mcp": "CLI parity",
    "Fetch grammars": "grammar download",
    "Install Rust": "Rust toolchain installation",
    "Install build dependencies (macOS)": "build dependency setup",
    "Install build dependencies (Linux)": "build dependency setup",
    "Package archive": "archive packaging",
    "Replay graph fixtures against the published C 0.13.1": "graph replay",
    "Smoke-test version, a tiny index and the graph UI": "candidate smoke test",
    "Strip and check static linking (Linux)": "Linux linking validation",
    "Strip, sign ad hoc and check linking (macOS)": "macOS signing and linking",
    "Verify or build the frozen C release reference": "frozen C reference",
    "Verify retired native implementations before stripping": "native symbol validation",
    "Write checksums and metadata": "package verification",
}
SHA = re.compile(r"[a-f0-9]{40}")
KEY = re.compile(r"^<!-- mori-daily-package:v1 ([a-f0-9]{64}) -->$", re.MULTILINE)
OCCURRENCE = re.compile(
    r"^<!-- mori-daily-occurrence:v1 ([a-f0-9]{40}) (\d+) (\d+) (\d+) -->$",
    re.MULTILINE,
)
ANSI = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]")
TIMESTAMP = re.compile(r"^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?Z)\s+")
LOG_TAIL_BYTES = 1 << 20
FAILURES = {"failure", "timed_out"}
# A job log also repeats script source. Match actual error lines, not the
# shell command that would print one. Never copy the matched text into a post.
DNS = re.compile(
    r"(?:fatal: unable to access .*?: |curl: \(6\) )Could not resolve host: "
    r"(github\.com|api\.github\.com|objects\.githubusercontent\.com)"
)
MISSING_SYMBOL = re.compile(
    r"##\[error\]release executable is missing (mori_[a-z0-9_]+|cbm_[a-z0-9_]+)"
)
RETIRED_OBJECT = re.compile(
    r"##\[error\]release archive includes retired object ([a-z0-9_]+\.o)"
)
NATIVE_FAILURES = {
    "##[error]release archive includes the retired C index drivers": "Retired C index drivers are present.",
    "##[error]release executable is missing the Rust managed-text editor": "The Rust managed-text editor is missing.",
    "##[error]release executable includes the retired C FTS writer": "The retired C FTS writer is present.",
    "##[error]release executable includes managed-text test hooks": "Managed-text test hooks are present.",
    "##[error]links a library outside the OS": "The executable links a library outside the OS.",
}


class ReportError(Exception):
    """An invalid workflow context or a failed read or write."""


def require_id(value: Any) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
        raise ReportError("Expected a positive GitHub identifier")
    return value


@dataclass(frozen=True)
class Run:
    sha: str
    id: int
    attempt: int

    @classmethod
    def from_env(cls, env: Mapping[str, str]) -> "Run":
        required = {
            "GITHUB_REPOSITORY": REPO,
            "GITHUB_REF": "refs/heads/main",
            "GITHUB_EVENT_NAME": "schedule",
            "GITHUB_WORKFLOW_REF": f"{REPO}/{WORKFLOW}@refs/heads/main",
        }
        if any(env.get(key) != value for key, value in required.items()):
            raise ReportError(
                "Reporting requires the canonical scheduled Mori package workflow"
            )
        sha = env.get("GITHUB_SHA", "")
        if not SHA.fullmatch(sha):
            raise ReportError("Expected an exact GITHUB_SHA")
        try:
            return cls(
                sha=sha,
                id=require_id(int(env["GITHUB_RUN_ID"])),
                attempt=require_id(int(env["GITHUB_RUN_ATTEMPT"])),
            )
        except (KeyError, ValueError):
            raise ReportError("Expected GITHUB_RUN_ID and GITHUB_RUN_ATTEMPT") from None

    @property
    def endpoint(self) -> str:
        return f"actions/runs/{self.id}/attempts/{self.attempt}"

    @property
    def url(self) -> str:
        return f"{REPO_URL}/actions/runs/{self.id}/attempts/{self.attempt}"

    def verify(self, payload: Mapping[str, Any]) -> datetime:
        expected = {
            "id": self.id,
            "run_attempt": self.attempt,
            "head_sha": self.sha,
            "head_branch": "main",
            "event": "schedule",
            "path": WORKFLOW,
        }
        if any(payload.get(key) != value for key, value in expected.items()) or any(
            (payload.get(field) or {}).get("full_name") != REPO
            for field in ("repository", "head_repository")
        ):
            raise ReportError("GitHub returned an unexpected workflow attempt")
        return datetime.fromisoformat(payload["created_at"])


class GitHub:
    def request(
        self,
        endpoint: str,
        *,
        paginate: bool = False,
        payload: dict[str, Any] | None = None,
    ) -> Any:
        command = ["gh", "api", f"repos/{REPO}/{endpoint}"]
        if paginate:
            command.extend(["--paginate", "--slurp"])
        if payload is not None:
            command.extend(["--method", "POST", "--input", "-"])
        try:
            result = subprocess.run(
                command,
                input=json.dumps(payload) if payload is not None else None,
                capture_output=True,
                text=True,
                check=True,
                timeout=90,
            )
            return json.loads(result.stdout)
        except (OSError, subprocess.SubprocessError, ValueError):
            # An API response can include an issue body. Avoid echoing it.
            raise ReportError(f"GitHub API request failed for {endpoint}") from None

    def log_tail(self, job_id: int) -> str:
        with tempfile.TemporaryFile() as log:
            try:
                subprocess.run(
                    [
                        "gh",
                        "api",
                        "--allow-escape-sequences",
                        f"repos/{REPO}/actions/jobs/{job_id}/logs",
                    ],
                    stdout=log,
                    stderr=subprocess.DEVNULL,
                    check=True,
                    timeout=45,
                )
            except (OSError, subprocess.SubprocessError):
                return ""
            log.seek(max(0, log.tell() - LOG_TAIL_BYTES))
            return log.read().decode(errors="replace")


@dataclass(frozen=True)
class Failure:
    target: str
    stage: str
    step: str
    kind: str
    diagnostic: str
    fingerprint: str
    job_id: int


def diagnosis(
    log: str, conclusion: str, failed_step: Mapping[str, Any] | None
) -> tuple[str, str, str]:
    if conclusion == "timed_out":
        return "timeout", "The job exceeded its time limit.", ""
    matches = set()
    if (
        not failed_step
        or not failed_step.get("started_at")
        or not failed_step.get("completed_at")
    ):
        return "unclassified", "Open the job log to identify the failure.", ""
    started = datetime.fromisoformat(failed_step["started_at"])
    completed = datetime.fromisoformat(failed_step["completed_at"])
    for raw in log.splitlines():
        stamped = ANSI.sub("", raw)
        timestamp = TIMESTAMP.match(stamped)
        if not timestamp:
            continue
        line = stamped[timestamp.end() :].strip()
        # Actions metadata records whole seconds, while logs record fractions.
        moment = datetime.fromisoformat(timestamp[1]).replace(microsecond=0)
        if not started <= moment <= completed:
            continue
        if line.startswith("C reference source "):
            # The pinned fetch retries before checkout. Later build errors must
            # not inherit a DNS fingerprint from a recovered attempt.
            matches = {match for match in matches if match[0] != "infrastructure"}
        elif match := DNS.fullmatch(line):
            matches.add(
                ("infrastructure", f"DNS lookup failed for {match[1]}.", match[1])
            )
        elif match := MISSING_SYMBOL.fullmatch(line):
            matches.add(
                ("verification", "A required native symbol is missing.", match[1])
            )
        elif match := RETIRED_OBJECT.fullmatch(line):
            matches.add(
                (
                    "verification",
                    "The archive contains a retired native object.",
                    match[1],
                )
            )
        elif line in NATIVE_FAILURES:
            matches.add(("verification", NATIVE_FAILURES[line], line))
    if len(matches) == 1:
        return matches.pop()
    return "unclassified", "Open the job log to identify the failure.", ""


def collect(run: Run, api: GitHub) -> list[Failure]:
    failures = []
    pages = api.request(f"{run.endpoint}/jobs?per_page=100", paginate=True)
    for page in pages:
        for job in page["jobs"]:
            if job["conclusion"] not in FAILURES:
                continue
            if (
                job["run_id"] != run.id
                or job["run_attempt"] != run.attempt
                or job["head_sha"] != run.sha
                or job["status"] != "completed"
            ):
                raise ReportError("GitHub returned a job outside the requested attempt")
            job_id = require_id(job["id"])
            name = job["name"]
            target, _, child = name.partition(" / ")
            if target in TARGETS and child in JOB_STAGES:
                stage = JOB_STAGES[child]
            elif name in SHARED_STAGES:
                target, stage = "all platforms", SHARED_STAGES[name]
            else:
                # Keep new jobs visible without copying an unrecognized name.
                target = target if target in TARGETS else "unresolved platform"
                stage = "unrecognized package job"
            failed_step = next(
                (step for step in job["steps"] if step["conclusion"] in FAILURES), None
            )
            if failed_step is not None:
                step_number = require_id(failed_step["number"])
                step_stage = STEP_STAGES.get(failed_step["name"])
                step = f"Step {step_number}"
                if step_stage:
                    step = f"{step} ({step_stage})"
                    stage = step_stage
            else:
                step = "Job setup or teardown"
            kind, diagnostic, detail = diagnosis(
                api.log_tail(job_id), job["conclusion"], failed_step
            )
            signature = (
                (stage, kind, diagnostic, detail)
                if detail
                else (run.sha, target, stage, step)
            )
            fingerprint = hashlib.sha256(json.dumps(signature).encode()).hexdigest()
            failures.append(
                Failure(target, stage, step, kind, diagnostic, fingerprint, job_id)
            )
    return failures


@dataclass
class Issue:
    number: int
    state: str
    occurrences: set[tuple[str, str, str, str]]


def bot_body(item: Mapping[str, Any]) -> str:
    if (item.get("user") or {}).get("login") != BOT:
        return ""
    return item.get("body") or ""


class Issues:
    def __init__(self, api: GitHub, run_started: datetime):
        self.api = api
        self.run_started = run_started
        self.issues: dict[str, list[Issue]] = {}
        self.listed: dict[str, list[tuple[dict[str, Any], str]]] = {}
        for page in api.request(
            "issues?state=all&creator=github-actions%5Bbot%5D&per_page=100",
            paginate=True,
        ):
            for item in page:
                body = bot_body(item)
                if "pull_request" not in item and (marker := KEY.search(body)):
                    self.listed.setdefault(marker[1], []).append((item, body))

    def matching(self, fingerprint: str) -> list[Issue]:
        if fingerprint not in self.issues:
            matches = []
            for item, body in self.listed.get(fingerprint, []):
                number = require_id(item["number"])
                comments = self.api.request(
                    f"issues/{number}/comments?per_page=100", paginate=True
                )
                seen = set(OCCURRENCE.findall(body))
                for page in comments:
                    for comment in page:
                        seen.update(OCCURRENCE.findall(bot_body(comment)))
                matches.append(Issue(number, item["state"], seen))
            self.issues[fingerprint] = matches
        return self.issues[fingerprint]

    def duplicate(self, key: tuple[str, str, str, str]) -> int | None:
        for fingerprint, items in self.listed.items():
            # A job's report can only have updated an issue after its run began.
            # Check recent issues even when a formerly missing log becomes readable.
            if not any(
                datetime.fromisoformat(item["updated_at"]) >= self.run_started
                for item, _ in items
            ):
                continue
            for issue in self.matching(fingerprint):
                if key in issue.occurrences:
                    return issue.number
        return None

    def record(self, run: Run, failures: list[Failure]) -> dict[int, tuple[int, str]]:
        scope = (run.sha, str(run.id), str(run.attempt))
        fingerprint = failures[0].fingerprint
        matches = self.matching(fingerprint)
        for issue in matches:
            if any(row[:3] == scope for row in issue.occurrences):
                return {
                    failure.job_id: (issue.number, "already recorded")
                    for failure in failures
                }

        results = {}
        pending = {}
        for failure in failures:
            key = (*scope, str(failure.job_id))
            if number := self.duplicate(key):
                results[failure.job_id] = (number, "already recorded")
            else:
                pending[key] = failure
        if not pending:
            return results

        exact = [
            issue
            for issue in matches
            if any(row[0] == run.sha for row in issue.occurrences)
        ]
        candidates = exact or [issue for issue in matches if issue.state == "open"]
        failure = failures[0]
        markers = [
            f"<!-- mori-daily-occurrence:v1 {' '.join(key)} -->" for key in pending
        ]
        body = "\n".join(
            [
                *markers,
                "",
                f"The **{failure.stage}** stage failed. {failure.diagnostic}",
                "",
                f"- Main commit `{run.sha}`",
                f"- [Run attempt {run.attempt}]({run.url})",
                *(
                    f"- [{job.target}, {job.step}]({REPO_URL}/actions/runs/{run.id}/job/{job.job_id})"
                    for job in pending.values()
                ),
                f"- [CI operations runbook]({RUNBOOK})",
                "",
            ]
        )
        if candidates:
            issue = min(
                candidates, key=lambda item: (item.state != "open", item.number)
            )
            self.api.request(f"issues/{issue.number}/comments", payload={"body": body})
            outcome = "commented"
        else:
            response = self.api.request(
                "issues",
                payload={
                    "labels": LABELS,
                    "title": f"Mori daily package check fails in {failure.stage}",
                    "body": f"<!-- mori-daily-package:v1 {fingerprint} -->\n\n{body}",
                },
            )
            issue = Issue(require_id(response["number"]), "open", set())
            matches.append(issue)
            outcome = "opened"
        issue.occurrences.update(pending)
        results.update(
            (job.job_id, (issue.number, outcome)) for job in pending.values()
        )
        return results


def report(run: Run, api: GitHub, evidence_path: Path) -> list[dict[str, Any]]:
    run_started = run.verify(api.request(run.endpoint))
    failures = collect(run, api)
    entries = [
        {
            **asdict(failure),
            "job_url": f"{REPO_URL}/actions/runs/{run.id}/job/{failure.job_id}",
        }
        for failure in failures
    ]

    def save() -> None:
        temporary = evidence_path.with_suffix(".tmp")
        temporary.write_text(
            json.dumps(
                {
                    "schema": 1,
                    "run": asdict(run),
                    "run_url": run.url,
                    "failures": entries,
                },
                indent=2,
            )
            + "\n",
            encoding="utf-8",
        )
        temporary.replace(evidence_path)

    save()
    if not failures:
        raise ReportError("No failed or timed-out jobs appeared in this attempt")
    groups: dict[str, list[Failure]] = {}
    for failure in failures:
        groups.setdefault(failure.fingerprint, []).append(failure)
    issues = Issues(api, run_started)
    for group in groups.values():
        results = issues.record(run, group)
        for entry in entries:
            if entry["job_id"] in results:
                number, action = results[entry["job_id"]]
                entry.update(issue=f"{REPO_URL}/issues/{number}", action=action)
        save()
    return entries


def main() -> int:
    try:
        run = Run.from_env(os.environ)
        entries = report(run, GitHub(), Path("daily-package-failures.json"))
        if summary := os.environ.get("GITHUB_STEP_SUMMARY"):
            with Path(summary).open("a", encoding="utf-8") as output:
                output.write("## Mori Daily Package Failures\n\n")
                output.writelines(
                    f"- {entry['target']}, {entry['stage']}. "
                    f"[Job log]({entry['job_url']}) · [Triage issue]({entry['issue']})\n"
                    for entry in entries
                )
        return 0
    except (ReportError, OSError, KeyError, TypeError, ValueError) as error:
        # Only ReportError messages are authored by the reporter.
        message = (
            str(error)
            if isinstance(error, ReportError)
            else "Invalid report data or output path"
        )
        print(f"Mori daily package reporting failed. {message}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
