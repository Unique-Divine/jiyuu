import argparse
import subprocess

import pytest
from mori_ci_shadow import exact_diff, report, select

BASELINE = frozenset(
    {"edge_types_probe", "pipeline", "pipeline_frozen", "doclinks", "httpd"}
)
OPERATOR = b"mori/src/pipeline/pass_operator.c"
SEMANTIC = b"mori/src/pipeline/pass_semantic_edges.c"


def test_known_leaf_paths_combine_their_suites():
    selection = select(b"M\0" + OPERATOR + b"\0M\0" + SEMANTIC + b"\0", BASELINE)
    assert selection.suites == BASELINE - {"httpd"}


@pytest.mark.parametrize(
    "diff",
    [
        b"",
        b"M\0" + OPERATOR,
        b"A\0" + OPERATOR + b"\0",
        b"D\0" + OPERATOR + b"\0",
        b"M\0" + OPERATOR + b"\0M\0mori/Makefile.cbm\0",
        b"M\0mori/src/pipeline/pipeline_internal.h\0",
        b"M\0Cargo.lock\0",
        b"M\0mori/internal/cbm/extract_defs.c\0",
        b"M\0mori/tests/test_main.c\0",
    ],
)
def test_unqualified_diff_runs_baseline(diff):
    assert select(diff, BASELINE).suites == BASELINE


def test_rule_cannot_name_a_suite_outside_the_baseline():
    baseline = BASELINE - {"pipeline_frozen"}
    assert select(b"M\0" + OPERATOR + b"\0", baseline).suites == baseline


@pytest.fixture
def merge_checkout(tmp_path):
    def git(*args):
        return subprocess.run(
            ["git", "-C", str(tmp_path), *args],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()

    git("init", "-b", "main")
    git("config", "user.name", "Shadow fixture")
    git("config", "user.email", "shadow@example.invalid")
    source = tmp_path / OPERATOR.decode()
    source.parent.mkdir(parents=True)
    source.write_text("original\n")
    git("add", ".")
    git("commit", "-m", "base")
    base = git("rev-parse", "HEAD")
    git("switch", "-c", "candidate")
    source.write_text("candidate\n")
    git("commit", "-am", "candidate")
    head = git("rev-parse", "HEAD")
    git("switch", "main")
    git("merge", "--no-ff", "candidate", "-m", "merge fixture")
    return tmp_path, base, head, git("rev-parse", "HEAD")


def test_exact_ci_merge_parents(merge_checkout):
    repo, base, head, tested = merge_checkout
    assert exact_diff(repo, base, head, tested) == b"M\0" + OPERATOR + b"\0"
    assert exact_diff(repo, head, base, tested) is None
    assert exact_diff(repo, base, head, head) is None


def test_hosted_checkout_depth_provides_verified_parents(merge_checkout, tmp_path):
    repo, base, head, tested = merge_checkout
    checkout = tmp_path / "checkout"
    subprocess.run(
        ["git", "clone", "--depth", "2", repo.as_uri(), str(checkout)],
        check=True,
        capture_output=True,
    )
    assert exact_diff(checkout, base, head, tested) == b"M\0" + OPERATOR + b"\0"


def test_detects_failure_outside_focused_subset(merge_checkout, tmp_path):
    repo, base, head, tested = merge_checkout
    log = tmp_path / "baseline.log"
    log.write_text(
        "=== edge_types_probe ===\n  test_a PASS\n"
        "=== pipeline ===\n  test_b PASS\n"
        "=== pipeline_frozen ===\n  test_c PASS\n"
        "=== doclinks ===\n  test_d PASS\n"
        "=== httpd ===\n  test_e FAIL tests/test_httpd.c:12: failed\n"
        "  4 passed, 1 failed\n"
    )
    args = argparse.Namespace(
        repo=repo,
        base=base,
        head=head,
        tested=tested,
        arch="x64",
        log=log,
        outcome="failure",
        baseline_suites=" ".join(BASELINE),
    )
    result = report(args)
    assert result["selected_suites"] == sorted(BASELINE - {"doclinks", "httpd"})
    assert result["omitted_failure_suites"] == ["httpd"]
    assert result["comparison_complete"] is True
    log.write_text("could not link candidate\n")
    assert report(args)["comparison_complete"] is False
