"""Focused source and link evidence tests for native_retirement.py."""

import json
import shutil
import subprocess
import sys
from pathlib import Path

import native_retirement as inventory
import pytest

SCRIPT = Path(inventory.__file__).resolve()


def cli(*args: object) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), *(str(arg) for arg in args)],
        capture_output=True,
        text=True,
        check=False,
    )


def command(*args: str | Path, cwd: Path | None = None) -> str:
    return subprocess.run(
        args, cwd=cwd, capture_output=True, text=True, check=True
    ).stdout.strip()


def test_pinned_git_blob_sizes_and_limit() -> None:
    result = cli(
        "source", "--rev", inventory.BASELINE, "--max-bytes", 20_016_798, "--files"
    )
    assert result.returncode == 1, result.stderr
    report = json.loads(result.stdout)
    assert report["baseline"] == "8bdd8fc20327fcfea4d920138fef77a3b00ca1ef"
    assert report["counted_bytes"] == report["baseline_bytes"] == 20_016_799
    assert report["counted_files"] == len(report["files"]) == 641
    assert report["removed_bytes"] == 0
    assert "mori/tools/tree-sitter-sql/src/scanner.c" in report["files"]
    assert all(not inventory.excluded(path) for path in report["files"])
    assert report["failures"] == [
        "counted 20016799 bytes, above the 20016798-byte limit"
    ]


def commit(repo: Path) -> str:
    command("git", "add", "-A", cwd=repo)
    command(
        "git", "-c", "core.hooksPath=/dev/null", "commit", "-qm", "fixture", cwd=repo
    )
    return command("git", "rev-parse", "HEAD", cwd=repo)


def write(repo: Path, path: str, content: bytes) -> Path:
    target = repo / path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(content)
    return target


def test_exclusion_moves_and_ordinary_test_changes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    command("git", "init", "-q", tmp_path)
    command("git", "config", "user.name", "Inventory Test", cwd=tmp_path)
    command("git", "config", "user.email", "inventory@example.test", cwd=tmp_path)

    moved = b"int whole_move(void) { return 1; }\r\n"
    copied = b"int copied_implementation(void) { return 44; }\n"
    edited = b"".join(
        f"int helper_{i}(void) {{ return {i}; }}\n".encode() for i in range(30)
    )
    kept = b"int still_counted(void) { return 2; }\n"
    source = write(tmp_path, "mori/src/move.c", moved)
    write(tmp_path, "mori/src/copied.c", copied)
    edited_source = write(tmp_path, "mori/src/edited.c", edited)
    write(tmp_path, "mori/src/generated/kept.h", kept)
    write(tmp_path, "mori/tests/ordinary.c", b"/* original test */\n")
    write(tmp_path, "mori/tests/overwrite.c", b"/* original destination */\n")
    base = commit(tmp_path)
    monkeypatch.setattr(inventory, "BASELINE", base)
    monkeypatch.setattr(
        inventory, "BASELINE_BYTES", len(moved + copied + edited + kept)
    )

    write(tmp_path, "mori/tests/ordinary.c", b"/* revised test */\n")
    ordinary = commit(tmp_path)
    monkeypatch.setattr(sys, "argv", [str(SCRIPT), "source", "--repo", str(tmp_path)])
    assert inventory.main() == 0
    assert json.loads(capsys.readouterr().out)["revision"] == ordinary

    source.unlink()
    edited_source.unlink()
    write(tmp_path, "mori/tests/overwrite.c", moved)
    write(tmp_path, "mori/internal/cbm/vendored/copy\tspace.inc", copied)
    write(
        tmp_path, "mori/internal/cbm/lsp/generated/edited.c", edited + b"/* edit */\n"
    )
    candidate = commit(tmp_path)
    write(tmp_path, "mori/src/untracked.c", b"/* worktree only */\n")

    assert inventory.main() == 1
    report = json.loads(capsys.readouterr().out)
    assert report["revision"] == candidate
    assert report["counted_bytes"] == len(kept + copied)
    assert report["moves_into_exclusions"] == {
        "mori/tests/overwrite.c": ["mori/src/move.c"],
        "mori/internal/cbm/vendored/copy\tspace.inc": ["mori/src/copied.c"],
        "mori/internal/cbm/lsp/generated/edited.c": ["mori/src/edited.c"],
    }


def compile_object(directory: Path, name: str, source: str) -> Path:
    path = write(directory, f"{name}.c", source.encode())
    obj = path.with_suffix(".o")
    command("cc", "-O0", "-ffunction-sections", "-c", path, "-o", obj)
    return obj


def link(directory: Path, *objects: Path, name: str = "mori") -> tuple[Path, Path]:
    binary = directory / name
    link_map = directory / f"{name}.map"
    command(
        "cc",
        "-fuse-ld=bfd",
        *objects,
        "-Wl,--gc-sections",
        f"-Wl,-Map,{link_map}",
        "-o",
        binary,
    )
    return binary, link_map


def test_checks_retained_symbols_and_final_link_objects(tmp_path: Path) -> None:
    kept = compile_object(tmp_path, "kept engine", "int main(void) { return 0; }")
    unused = compile_object(tmp_path, "discarded", "int dead(void) { return 1; }")
    binary, link_map = link(tmp_path, kept, unused)
    result = cli(
        "artifact",
        binary,
        "--require-symbol",
        "main",
        "--forbid-symbol",
        "dead",
        "--link-map",
        link_map,
        "--require-object",
        r".*/kept engine\.o",
        "--forbid-object",
        r".*/discarded\.o",
    )
    assert result.returncode == 0, result.stdout + result.stderr
    report = json.loads(result.stdout)
    assert report["required_objects"] == {r".*/kept engine\.o": [str(kept)]}
    assert len(report["binary_sha256"]) == len(report["link_map_sha256"]) == 64


def test_rejects_static_native_symbol_and_archive_member(tmp_path: Path) -> None:
    old = compile_object(
        tmp_path,
        "retired",
        "static int old_driver(void) { return 0; }\n"
        "int wrapper(void) { return old_driver(); }\n",
    )
    kept = compile_object(
        tmp_path,
        "kept",
        "extern int wrapper(void);\nint main(void) { return wrapper(); }",
    )
    archive = tmp_path / "libnative.a"
    command("ar", "rcs", archive, old)
    binary, link_map = link(tmp_path, kept, archive)
    result = cli(
        "artifact",
        binary,
        "--require-symbol",
        "main",
        "--forbid-symbol",
        r"old_driver|retired\.c",
        "--link-map",
        link_map,
        "--forbid-object",
        r".*/libnative\.a\(retired\.o\)",
    )
    assert result.returncode == 1, result.stderr
    report = json.loads(result.stdout)
    assert report["forbidden_symbols"][r"old_driver|retired\.c"] == [
        "old_driver",
        "retired.c",
    ]
    assert report["forbidden_objects"][r".*/libnative\.a\(retired\.o\)"] == [
        f"{archive}(retired.o)"
    ]


def test_requires_positive_evidence_and_rejects_stripped_elf(tmp_path: Path) -> None:
    obj = compile_object(
        tmp_path,
        "kept",
        '#include <stdio.h>\nint main(void) { return puts("fixture"); }',
    )
    binary, _ = link(tmp_path, obj)
    result = cli(
        "artifact", binary, "--require-symbol", "puts", "--forbid-symbol", "puts"
    )
    assert result.returncode == 1, result.stderr
    report = json.loads(result.stdout)
    assert report["required_symbols"] == {"puts": []}
    assert report["forbidden_symbols"] == {"puts": ["puts"]}
    result = cli(
        "artifact",
        binary,
        "--require-symbol",
        "main",
        "--forbid-library",
        r"libc\.so\..*",
    )
    assert result.returncode == 1, result.stderr
    assert json.loads(result.stdout)["forbidden_libraries"][r"libc\.so\..*"]

    for strip_mode in ("--strip-all", "--discard-all"):
        stripped = tmp_path / "stripped"
        shutil.copyfile(binary, stripped)
        command("strip", strip_mode, stripped)
        result = cli(
            "artifact",
            stripped,
            "--require-symbol",
            "main",
            "--forbid-symbol",
            "legacy",
        )
        assert result.returncode == 1
        assert "non-stripped .symtab" in result.stderr

    result = cli(
        "artifact", obj, "--require-symbol", "main", "--forbid-symbol", "legacy"
    )
    assert result.returncode == 1
    assert "linked ELF executable" in result.stderr


def test_rejects_unrelated_link_map(tmp_path: Path) -> None:
    first = compile_object(tmp_path, "first", "int main(void) { return 0; }")
    second = compile_object(
        tmp_path,
        "second",
        "static int helper(void) { return 0; }\nint main(void) { return helper(); }",
    )
    binary, _ = link(tmp_path, first, name="first-bin")
    _, wrong_map = link(tmp_path, second, name="second-bin")
    result = cli(
        "artifact",
        binary,
        "--require-symbol",
        "main",
        "--forbid-object",
        r".*/retired\.o",
        "--link-map",
        wrong_map,
    )
    assert result.returncode == 1
    assert "link map lacks executable symbol addresses for main" in result.stderr
