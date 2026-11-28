import json
from copy import deepcopy

import pytest
from report_daily_packages import (
    BOT,
    LABELS,
    REPO,
    RUNBOOK,
    WORKFLOW,
    GitHub,
    ReportError,
    Run,
    report,
)

SHA = "1" * 40
LATER_SHA = "2" * 40
SECRET = "untrusted-content-never-published"
START = "2026-11-26T04:23:00Z"
STEP_START = "2026-11-26T04:24:00Z"
END = "2026-11-26T04:25:00Z"
SYMBOL_ERROR = (
    "2026-11-26T04:24:32.1982106Z ##[error]release executable is missing mori_example"
)
DNS_ERROR = (
    "2026-11-26T04:24:09.1234Z fatal: unable to access "
    f"'https://{SECRET}/': Could not resolve host: github.com"
)


def job(id, target="darwin-x64", *, conclusion="failure", step=None, stage=None):
    return {
        "id": id,
        "name": stage or f"{target} / Build candidate",
        "status": "completed",
        "conclusion": conclusion,
        "steps": [
            {
                "name": step
                or "Verify retired native implementations before stripping",
                "conclusion": conclusion,
                "number": 6,
                "started_at": STEP_START,
                "completed_at": END,
            }
        ],
    }


class FakeGitHub(GitHub):
    def __init__(self):
        self.issues = []
        self.comments = {}
        self.writes = []
        self.logs = {}
        self.attempt = Run(SHA, 300, 1)
        self.metadata = {}
        self.jobs = []

    def set_attempt(self, run, *jobs):
        self.attempt = run
        self.jobs = [
            {**item, "run_id": run.id, "run_attempt": run.attempt, "head_sha": run.sha}
            for item in jobs
        ]
        self.metadata = {
            "id": run.id,
            "run_attempt": run.attempt,
            "head_sha": run.sha,
            "head_branch": "main",
            "event": "schedule",
            "path": WORKFLOW,
            "repository": {"full_name": REPO},
            "head_repository": {"full_name": REPO},
            "created_at": START,
        }

    def request(self, endpoint, *, paginate=False, payload=None):
        if endpoint == self.attempt.endpoint:
            return deepcopy(self.metadata)
        if endpoint == f"{self.attempt.endpoint}/jobs?per_page=100" and paginate:
            return [
                {"total_count": len(self.jobs), "jobs": self.jobs[:1]},
                {"total_count": len(self.jobs), "jobs": self.jobs[1:]},
            ]
        if (
            endpoint == "issues?state=all&creator=github-actions%5Bbot%5D&per_page=100"
            and paginate
        ):
            return [deepcopy(self.issues[:1]), deepcopy(self.issues[1:])]
        if (
            endpoint.startswith("issues/")
            and endpoint.endswith("/comments?per_page=100")
            and paginate
        ):
            return [deepcopy(self.comments.get(int(endpoint.split("/")[1]), []))]
        if payload is not None:
            self.writes.append((endpoint, payload))
            if endpoint == "issues":
                item = {
                    **payload,
                    "number": len(self.issues) + 1,
                    "state": "open",
                    "updated_at": END,
                    "user": {"login": BOT},
                }
                self.issues.append(item)
                return deepcopy(item)
            number = int(endpoint.split("/")[1])
            self.comments.setdefault(number, []).append(
                {**payload, "user": {"login": BOT}}
            )
            self.issues[number - 1]["updated_at"] = END
            return {}
        raise AssertionError(f"Unexpected request {endpoint}")

    def log_tail(self, job_id):
        return self.logs.get(job_id, "")


def test_known_cross_target_failures_share_one_issue_and_occurrence(tmp_path):
    api = FakeGitHub()
    run = Run(SHA, 300, 1)
    api.set_attempt(
        run,
        job(10),
        job(11, "darwin-arm64"),
        job(12, "linux-arm64", conclusion="cancelled"),
    )
    api.logs = {
        10: SYMBOL_ERROR + f"\n2026-11-26T04:24:33.123Z detail {SECRET}",
        11: SYMBOL_ERROR,
    }
    path = tmp_path / "evidence.json"
    entries = report(run, api, path)
    assert [row["action"] for row in entries] == ["opened", "opened"]
    assert len(api.writes) == 1
    endpoint, payload = api.writes[0]
    assert endpoint == "issues"
    assert payload["labels"] == LABELS
    assert (
        payload["title"] == "Mori daily package check fails in native symbol validation"
    )
    assert RUNBOOK in payload["body"]
    assert "/attempts/1" in payload["body"]
    assert "darwin-x64" in payload["body"] and "darwin-arm64" in payload["body"]
    assert "mori_example" not in payload["body"] and SECRET not in payload["body"]
    evidence = path.read_text()
    assert SECRET not in evidence and "mori_example" not in evidence
    assert len(json.loads(evidence)["failures"]) == 2

    entries = report(run, api, path)
    assert all(row["action"] == "already recorded" for row in entries)
    assert len(api.writes) == 1

    rerun = Run(SHA, 300, 2)
    api.set_attempt(rerun, job(20), job(21, "darwin-arm64"))
    api.logs = {20: SYMBOL_ERROR, 21: SYMBOL_ERROR}
    entries = report(rerun, api, path)
    assert [row["action"] for row in entries] == ["commented", "commented"]
    assert [endpoint for endpoint, _ in api.writes] == ["issues", "issues/1/comments"]
    assert "/attempts/2" in api.writes[1][1]["body"]


def test_different_diagnostics_same_sha_and_closed_recurrence(tmp_path):
    api = FakeGitHub()
    path = tmp_path / "evidence.json"
    run = Run(SHA, 300, 1)
    api.set_attempt(run, job(10), job(11))
    api.logs = {
        10: SYMBOL_ERROR,
        11: SYMBOL_ERROR.replace("mori_example", "mori_different"),
    }
    result = report(run, api, path)
    assert [row["issue"].split("/")[-1] for row in result] == ["1", "2"]
    api.issues[0]["state"] = "closed"

    same_sha = Run(SHA, 301, 1)
    api.set_attempt(same_sha, job(12))
    api.logs = {12: SYMBOL_ERROR}
    assert report(same_sha, api, path)[0]["action"] == "commented"
    assert api.writes[-1][0] == "issues/1/comments"
    assert api.issues[0]["state"] == "closed"

    later_sha = Run(LATER_SHA, 302, 1)
    api.set_attempt(later_sha, job(20), job(21))
    api.logs = {
        20: SYMBOL_ERROR,
        21: SYMBOL_ERROR.replace("mori_example", "mori_different"),
    }
    result = report(later_sha, api, path)
    assert [row["action"] for row in result] == ["opened", "commented"]
    assert [row["issue"].split("/")[-1] for row in result] == ["3", "2"]


def test_missing_log_classification_and_late_log_retry(tmp_path):
    api = FakeGitHub()
    first = Run(SHA, 300, 1)
    api.set_attempt(first, job(10))
    path = tmp_path / "evidence.json"
    first_entry = report(first, api, path)[0]
    assert first_entry["kind"] == "unclassified"

    api.logs = {10: SYMBOL_ERROR}
    late_log_entry = report(first, api, path)[0]
    assert late_log_entry["kind"] == "verification"
    assert late_log_entry["issue"] == first_entry["issue"]
    assert late_log_entry["action"] == "already recorded"
    assert len(api.writes) == 1

    same_sha = Run(SHA, 301, 1)
    api.set_attempt(same_sha, job(11), job(12, "darwin-arm64"))
    assert [row["action"] for row in report(same_sha, api, path)] == [
        "commented",
        "opened",
    ]

    later_sha = Run(LATER_SHA, 302, 1)
    api.set_attempt(later_sha, job(13))
    assert report(later_sha, api, path)[0]["action"] == "opened"


@pytest.mark.parametrize(
    ("log", "kind"),
    [
        (DNS_ERROR, "infrastructure"),
        (
            DNS_ERROR + "\n2026-11-26T04:24:40Z C reference source some-sha",
            "unclassified",
        ),
        (DNS_ERROR.replace("04:24:09", "04:23:09"), "unclassified"),
        (DNS_ERROR + "\n" + SYMBOL_ERROR, "unclassified"),
        (f"2026-11-26T04:24:09Z echo '{SECRET}'", "unclassified"),
    ],
)
def test_diagnostics_are_restricted_to_failed_step_and_terminal_errors(
    tmp_path, log, kind
):
    api = FakeGitHub()
    run = Run(SHA, 300, 1)
    api.set_attempt(
        run,
        job(
            10,
            step="Verify or build the frozen C release reference",
            stage="darwin-x64 / Build C reference",
        ),
    )
    api.logs = {10: log}
    path = tmp_path / "evidence.json"
    assert report(run, api, path)[0]["kind"] == kind
    assert SECRET not in api.writes[0][1]["body"]
    assert SECRET not in path.read_text()


def test_no_failed_jobs_records_empty_evidence_and_refuses_issue(tmp_path):
    api = FakeGitHub()
    run = Run(SHA, 301, 1)
    api.set_attempt(run, job(10, conclusion="cancelled"), job(11, conclusion="skipped"))
    evidence = tmp_path / "evidence.json"
    with pytest.raises(ReportError, match="No failed"):
        report(run, api, evidence)
    assert not api.writes
    assert json.loads(evidence.read_text())["failures"] == []


def test_unknown_timed_out_job_is_visible_without_leaking_job_name(tmp_path):
    api = FakeGitHub()
    run = Run(SHA, 301, 1)
    api.set_attempt(run, job(10, stage=SECRET, conclusion="timed_out", step=SECRET))
    path = tmp_path / "evidence.json"
    entry = report(run, api, path)[0]
    assert entry["stage"] == "unrecognized package job"
    assert entry["kind"] == "timeout"
    assert SECRET not in path.read_text()
    assert SECRET not in api.writes[0][1]["body"]


def test_refuses_wrong_api_attempt_before_any_writes(tmp_path):
    api = FakeGitHub()
    run = Run(SHA, 300, 2)
    api.set_attempt(run, job(10))
    api.metadata["head_sha"] = LATER_SHA
    with pytest.raises(ReportError, match="unexpected workflow attempt"):
        report(run, api, tmp_path / "evidence.json")
    assert not api.writes


def test_canonical_environment():
    env = {
        "GITHUB_REPOSITORY": REPO,
        "GITHUB_REF": "refs/heads/main",
        "GITHUB_EVENT_NAME": "schedule",
        "GITHUB_WORKFLOW_REF": f"{REPO}/{WORKFLOW}@refs/heads/main",
        "GITHUB_RUN_ID": "300",
        "GITHUB_RUN_ATTEMPT": "2",
        "GITHUB_SHA": SHA,
    }
    assert Run.from_env(env) == Run(SHA, 300, 2)
    with pytest.raises(ReportError, match="canonical scheduled"):
        Run.from_env({**env, "GITHUB_EVENT_NAME": "workflow_dispatch"})


def test_paginated_api_and_issue_payload_transmitted_without_shell(monkeypatch):
    from subprocess import CompletedProcess

    calls = []

    def run(argv, **kwargs):
        calls.append((argv, kwargs))
        return CompletedProcess(argv, 0, "{}")

    monkeypatch.setattr("report_daily_packages.subprocess.run", run)
    api = GitHub()
    api.request("issues?state=all", paginate=True)
    api.request("issues", payload={"body": "Keep ** formatting"})
    assert calls[0][0] == [
        "gh",
        "api",
        f"repos/{REPO}/issues?state=all",
        "--paginate",
        "--slurp",
    ]
    assert calls[1][0] == [
        "gh",
        "api",
        f"repos/{REPO}/issues",
        "--method",
        "POST",
        "--input",
        "-",
    ]
    assert json.loads(calls[1][1]["input"]) == {"body": "Keep ** formatting"}
