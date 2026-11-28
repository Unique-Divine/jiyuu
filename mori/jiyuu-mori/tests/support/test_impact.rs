use super::{MORI, Scratch, assert_same, c_binary, command_in, run_in};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

const CONFIG: &str =
    r#"{"test_impact":{"version":1,"tests":{"presets":{"c-cbm":true}}}}"#;
const RUNNER: &str = r#"#include "test_framework.h"
int lib_value(void);
static int worker(int argc, char **argv) {
    return argc > 9 ? lib_value() : -1;
}
int main(int argc, char **argv) {
    if (worker(argc, argv) >= 0) {
        return 0;
    }
    RUN_SELECTED_SUITE(alpha);
    return 0;
}
"#;
const SUITE: &str = r#"int lib_value(void);
TEST(alpha_uses_lib) {
    return lib_value() == 1 ? 0 : 1;
}
SUITE(alpha) {
    RUN_TEST(alpha_uses_lib);
}
"#;

fn git(scratch: &Scratch, repo: &Path, args: &[&str]) -> String {
    let out = command_in(Path::new("git"), scratch)
        .current_dir(repo)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args([
            "-c",
            "user.name=test-impact",
            "-c",
            "user.email=test-impact@example.invalid",
            "-c",
            "commit.gpgSign=false",
            "-c",
            "core.autocrlf=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8(out.stdout).unwrap()
}

fn repo(scratch: &Scratch) -> PathBuf {
    let repo = scratch.0.join("repo");
    fs::create_dir_all(repo.join("src")).unwrap();
    fs::create_dir_all(repo.join("tests")).unwrap();
    git(
        scratch,
        &repo,
        &["init", "--initial-branch=main", "--template="],
    );
    fs::write(repo.join(".codebase-memory.json"), CONFIG).unwrap();
    fs::write(repo.join("tests/test_main.c"), RUNNER).unwrap();
    fs::write(repo.join("tests/test_alpha.c"), SUITE).unwrap();
    fs::write(
        repo.join("src/lib.c"),
        "int lib_value(void) {\n    return 1;\n}\n",
    )
    .unwrap();
    git(scratch, &repo, &["add", "-A"]);
    git(scratch, &repo, &["commit", "-m", "base"]);
    repo
}

fn json_line(bytes: &[u8]) -> Value {
    assert!(bytes.ends_with(b"\n"), "{bytes:?}");
    assert_eq!(bytes.iter().filter(|&&b| b == b'\n').count(), 1);
    serde_json::from_slice(bytes).unwrap()
}

// A fresh snapshot can have a different physical SQLite digest on each run.
// Keep the rest of the JSON and its byte-level framing in the comparison.
fn selection_bytes(bytes: &[u8]) -> String {
    let value = json_line(bytes);
    let text = std::str::from_utf8(bytes).unwrap();
    if let Some(digest) = value["receipt"]["graph"]["sha256"].as_str() {
        assert_eq!(digest.len(), 64);
        assert!(digest.bytes().all(|b| b.is_ascii_hexdigit()));
        let field = format!(r#""sha256":"{digest}""#);
        assert!(text.contains(&field));
        text.replacen(&field, r#""sha256":null"#, 1)
    } else {
        text.to_owned()
    }
}

fn assert_output_matches(candidate: &Output, reference: &Output) {
    assert_eq!(
        (
            candidate.status.code(),
            &candidate.stdout,
            &candidate.stderr
        ),
        (
            reference.status.code(),
            &reference.stdout,
            &reference.stderr
        ),
        "candidate {candidate:?}\nreference {reference:?}",
    );
}

fn assert_selection_matches(candidate: &Output, reference: &Output) {
    assert_eq!(candidate.status.code(), reference.status.code());
    assert_eq!(candidate.stderr, reference.stderr);
    assert_eq!(
        selection_bytes(&candidate.stdout),
        selection_bytes(&reference.stdout)
    );
}

#[test]
fn test_impact_usage_matches_reference() {
    for args in [
        &["test-impact"][..],
        &["test-impact", "--help"],
        &["test-impact", "select", "--unrecognized"],
        &["test-impact", "select", "--repo"],
        &["test-impact", "select", "--repo", "", "--base", "HEAD"],
        &["test-impact", "select", "--repo", "", "--base", "--unknown"],
        &[
            "test-impact",
            "select",
            "--repo",
            ".",
            "--base",
            "HEAD",
            "--deadline-s",
            "1 ",
        ],
        &[
            "test-impact",
            "publish",
            "--repo",
            ".",
            "--out",
            ".",
            "--observed-at",
            "0",
        ],
        &["test-impact", "publish", "--artifact-verified"],
    ] {
        assert_same(args);
    }
    // Capture the missing required flag's exit status independently of parity.
    let out = run_in(
        Path::new(MORI),
        &Scratch::new("ti-usage"),
        &["test-impact", "publish", "--repo", "."],
    );
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(out.stdout.is_empty());
    assert!(out.stderr.starts_with(b"usage: mori test-impact publish"));
}

#[test]
fn test_impact_publish_matches_reference() {
    let fixture = Scratch::new("ti-pub");
    let repo = repo(&fixture);
    let head = git(&fixture, &repo, &["rev-parse", "HEAD"]);
    let reference = c_binary();
    let mut outputs = Vec::new();
    let mut receipts = Vec::new();
    for (bin, side) in [(reference.as_path(), "c"), (Path::new(MORI), "rs")] {
        let scratch = Scratch::new(side);
        let bundle = scratch.0.join("bundle");
        let config = repo.join(".codebase-memory.json");
        let args = [
            "test-impact",
            "publish",
            "--repo",
            "/unused",
            "--repo",
            repo.to_str().unwrap(),
            "--out",
            bundle.to_str().unwrap(),
            "--observed-at",
            "",
            "--deadline-s",
            " +120",
            "--platform",
            "fixture",
            "--config",
            config.to_str().unwrap(),
        ];
        let out = run_in(bin, &scratch, &args);
        assert!(out.status.success(), "{out:?}");
        assert!(out.stdout.is_empty());
        assert!(
            out.stderr
                .starts_with(b"test-impact publish: status 0: published ")
        );
        assert!(scratch.0.join("cache/test-impact").is_dir());
        let receipt: Value = serde_json::from_slice(
            &fs::read(bundle.join("receipt.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(receipt["commit"], head.trim());
        assert_eq!(receipt["platform"], "fixture");
        assert!(receipt["coverage"].is_null());
        receipts.push(receipt);
        outputs.push(out);

        let refused = run_in(bin, &scratch, &args);
        assert_eq!(refused.status.code(), Some(1), "{refused:?}");
        assert!(refused.stdout.is_empty());
        assert_eq!(
            refused.stderr,
            b"test-impact publish: status 1: publish: the output directory already exists\n"
        );
    }
    // The physical SQLite digest may depend on the database build. The
    // topology and content digests describe the exported graph itself.
    for field in ["schema", "graph_content_sha256", "graph_topology_sha256"] {
        assert_eq!(receipts[0][field], receipts[1][field]);
    }
    assert_output_matches(&outputs[1], &outputs[0]);
}

#[test]
fn test_impact_select_matches_reference() {
    let fixture = Scratch::new("ti-sel");
    let repo = repo(&fixture);
    let reference = c_binary();
    let run = |bin: &Path, base: &str, work: &Path, vetoed: &Path| {
        run_in(
            bin,
            &fixture,
            &[
                "test-impact",
                "select",
                "--repo",
                repo.to_str().unwrap(),
                "--base",
                base,
                "--work",
                work.to_str().unwrap(),
                "--deadline-s",
                "",
                "--vetoed-out",
                vetoed.to_str().unwrap(),
            ],
        )
    };
    let work = fixture.0.join("cache");
    let vetoed = fixture.0.join("vetoed.json");
    let candidate = run(Path::new(MORI), "main", &work, &vetoed);
    let c = run(&reference, "main", &work, &vetoed);
    assert_output_matches(&candidate, &c);
    assert!(candidate.status.success(), "{candidate:?}");
    assert_eq!(json_line(&candidate.stdout)["decision"], "nothing");
    assert!(!vetoed.exists());

    git(&fixture, &repo, &["checkout", "-b", "topic"]);
    fs::write(
        repo.join("src/lib.c"),
        "int lib_value(void) {\n    return 1 + 0;\n}\n",
    )
    .unwrap();
    git(&fixture, &repo, &["commit", "-am", "changed code"]);

    let candidate = run(Path::new(MORI), "main", &work, &vetoed);
    assert!(candidate.status.success(), "{candidate:?}");
    let answer = json_line(&candidate.stdout);
    assert_eq!(answer["decision"], "run_all");
    assert!(answer.to_string().contains("RUNNER_REACHED"), "{answer}");
    let masked = fs::read(&vetoed).unwrap();
    let masked_answer = json_line(&masked);
    assert_ne!(masked_answer["decision"], "run_all");
    assert!(!masked_answer.to_string().contains("RUNNER_REACHED"));
    fs::remove_file(&vetoed).unwrap();
    let c = run(&reference, "main", &work, &vetoed);
    assert_selection_matches(&candidate, &c);
    assert_eq!(
        selection_bytes(&masked),
        selection_bytes(&fs::read(&vetoed).unwrap())
    );

    // The output directory is an invalid sidecar target. The answer has
    // already been printed when this I/O failure determines the exit code.
    let failed = run(Path::new(MORI), "main", &work, &work);
    let c_failed = run(&reference, "main", &work, &work);
    assert_selection_matches(&failed, &c_failed);
    assert_eq!(failed.status.code(), Some(1), "{failed:?}");
    assert_eq!(
        selection_bytes(&failed.stdout),
        selection_bytes(&candidate.stdout)
    );

    for bin in [reference.as_path(), Path::new(MORI)] {
        // The existing unsigned seconds-to-ms calculation wraps to now minus
        // 1000 ms. This hits engine cancellation without waiting on a timer.
        let expired = run_in(
            bin,
            &fixture,
            &[
                "test-impact",
                "select",
                "--repo",
                repo.to_str().unwrap(),
                "--base",
                "main",
                "--deadline-s",
                "2305843009213693951",
            ],
        );
        assert_eq!(expired.status.code(), Some(1), "{expired:?}");
        assert!(expired.stdout.is_empty(), "{expired:?}");
        assert!(
            expired.stderr.ends_with(b"test-impact select: status 3\n"),
            "{expired:?}"
        );
    }
}
