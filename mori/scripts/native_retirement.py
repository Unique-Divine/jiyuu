#!/usr/bin/env python3
"""Measure Mori's tracked C source and check native retirement on a linked ELF.

Source mode reads raw .c and .h blob sizes from Git. It checks changed
excluded paths for identical baseline blobs and Git's 50% rename/copy
matches. Review code splits and substantial rewrites separately.

Artifact mode uses GNU readelf on the unstripped candidate itself. If you
pass a GNU ld map, retain it from the same link and use required symbols
whose addresses the map records. Use object checks when a replacement
keeps the old symbol name or link-time optimization erases local names.
"""

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

BASELINE = "8bdd8fc20327fcfea4d920138fef77a3b00ca1ef"
BASELINE_BYTES = 20_016_799
EXCLUDED_PREFIXES = (
    "mori/tests/",
    "mori/vendored/",
    "mori/internal/cbm/vendored/",
    "mori/internal/cbm/lsp/generated/",
)


@dataclass(frozen=True)
class TreeEntry:
    mode: str
    oid: str
    size: int


@dataclass(frozen=True)
class ElfEvidence:
    defined: dict[str, set[int]]
    names: set[str]
    libraries: set[str]
    allocated_sections: set[str]


def run(*argv: str, cwd: Path | None = None) -> str:
    result = subprocess.run(
        argv,
        cwd=cwd,
        env={**os.environ, "LC_ALL": "C"},
        capture_output=True,
        check=False,
    )
    if result.returncode:
        detail = os.fsdecode(result.stderr).strip()
        raise ValueError(f"{argv[0]} {argv[1]} failed ({result.returncode}): {detail}")
    return os.fsdecode(result.stdout)


def git(repo: Path, *args: str) -> str:
    return run("git", "--no-replace-objects", *args, cwd=repo)


def mori_tree(repo: Path, rev: str) -> dict[str, TreeEntry]:
    entries = {}
    listing = git(repo, "ls-tree", "-rlz", "--full-tree", rev, "--", "mori/")
    for record in listing.split("\0")[:-1]:
        metadata, path = record.split("\t", 1)
        mode, _, oid, size = metadata.split()
        entries[path] = TreeEntry(mode, oid, int(size) if size != "-" else 0)
    if not entries:
        raise ValueError(f"{rev} has an empty or missing mori/ tree")
    return entries


def excluded(path: str) -> bool:
    return path.startswith(EXCLUDED_PREFIXES)


def counted(entries: dict[str, TreeEntry]) -> dict[str, TreeEntry]:
    files = {}
    for path, entry in entries.items():
        if excluded(path) or not path.endswith((".c", ".h")):
            continue
        if entry.mode not in ("100644", "100755"):
            raise ValueError(f"{path} must be a regular tracked file")
        files[path] = entry
    return files


def exclusion_moves(
    repo: Path,
    base_rev: str,
    rev: str,
    base: dict[str, TreeEntry],
    candidate: dict[str, TreeEntry],
) -> dict[str, set[str]]:
    destinations = {
        path
        for path, entry in candidate.items()
        if excluded(path) and entry != base.get(path)
    }
    if not destinations:
        return {}

    counted_base = counted(base)
    by_oid: dict[str, set[str]] = {}
    for path, entry in counted_base.items():
        by_oid.setdefault(entry.oid, set()).add(path)
    moves = {
        path: by_oid[candidate[path].oid].copy()
        for path in destinations
        if candidate[path].oid in by_oid
    }

    # Include edited moves and copies, including copies whose originals survive.
    # NUL framing preserves tabs and newlines in tracked paths.
    records = iter(
        git(
            repo,
            "diff-tree",
            "-r",
            "--no-commit-id",
            "--name-status",
            "-z",
            "--diff-filter=CR",
            "-M50%",
            "-C50%",
            "--find-copies-harder",
            "-l0",
            base_rev,
            rev,
            "--",
            "mori/",
        ).split("\0")[:-1]
    )
    for status, origin, destination in zip(records, records, records, strict=True):
        if status[0] in "CR" and origin in counted_base and destination in destinations:
            moves.setdefault(destination, set()).add(origin)
    return moves


def check_source(args: argparse.Namespace) -> tuple[dict[str, object], list[str]]:
    repo = Path(git(args.repo, "rev-parse", "--show-toplevel").rstrip("\n"))
    rev = git(
        repo, "rev-parse", "--verify", "--end-of-options", f"{args.rev}^{{commit}}"
    ).strip()
    base = mori_tree(repo, BASELINE)
    base_bytes = sum(entry.size for entry in counted(base).values())
    if base_bytes != BASELINE_BYTES:
        raise ValueError(
            f"pinned source must total {BASELINE_BYTES} bytes, got {base_bytes}"
        )

    candidate = base if rev == BASELINE else mori_tree(repo, rev)
    files = counted(candidate)
    size = sum(entry.size for entry in files.values())
    moves = exclusion_moves(repo, BASELINE, rev, base, candidate)
    failures = [
        f"counted source moved or copied to {path!r} from {', '.join(sorted(origins))}"
        for path, origins in sorted(moves.items())
    ]
    if args.max_bytes is not None and size > args.max_bytes:
        failures.append(f"counted {size} bytes, above the {args.max_bytes}-byte limit")

    report: dict[str, object] = {
        "baseline": BASELINE,
        "baseline_bytes": base_bytes,
        "revision": rev,
        "counted_bytes": size,
        "counted_files": len(files),
        "removed_bytes": base_bytes - size,
        "max_bytes": args.max_bytes,
        "excluded_prefixes": EXCLUDED_PREFIXES,
        "moves_into_exclusions": {path: sorted(moves[path]) for path in sorted(moves)},
    }
    if args.files:
        report["files"] = {path: entry.size for path, entry in files.items()}
    return report, failures


# GNU readelf --wide puts each complete symbol on one line.
SYMBOL_ROW = re.compile(
    r"^\s*\d+:\s+([0-9a-fA-F]+)\s+\d+\s+(\S+)\s+(\S+)\s+\S+\s+(\S+)(?:\s+(.*))?$"
)
SYMBOL_TABLE = re.compile(r"^Symbol table '([^']+)' contains \d+ entr(?:y|ies):$")
NEEDED = re.compile(r"\(NEEDED\)\s+Shared library: \[([^]]+)\]")
SECTION_HEADER = re.compile(
    r"^\s*\[\s*\d+\]\s+(\S+)\s+\S+(?:\s+[0-9a-fA-F]+){4}\s+([A-Za-z]+)\s+\d+\s+\d+\s+\d+$"
)


def elf_evidence(binary: Path) -> ElfEvidence:
    output = run(
        "readelf",
        "--file-header",
        "--program-headers",
        "--section-headers",
        "--dynamic",
        "--syms",
        "--wide",
        "--",
        str(binary.resolve()),
    )
    exec_type = re.search(r"^\s*Type:\s+EXEC\b", output, re.MULTILINE)
    pie_type = re.search(
        r"^\s*Type:\s+DYN\s+\(Position-Independent Executable file\)",
        output,
        re.MULTILINE,
    )
    if not (exec_type or pie_type) or not re.search(
        r"^\s+LOAD\s+0x", output, re.MULTILINE
    ):
        raise ValueError("candidate must be a linked ELF executable or PIE")
    if not re.search(r"Entry point address:\s+0x0*[1-9a-fA-F][0-9a-fA-F]*\b", output):
        raise ValueError("candidate has no ELF entry point")

    defined: dict[str, set[int]] = {}
    names = set()
    allocated_sections = set()
    has_local_definition = False
    table = None
    for line in output.splitlines():
        if (header := SECTION_HEADER.fullmatch(line)) and "A" in header[2]:
            allocated_sections.add(header[1])
        if header := SYMBOL_TABLE.fullmatch(line):
            table = header[1]
        if table is None or not (row := SYMBOL_ROW.fullmatch(line)):
            continue
        address, kind, binding, section, name = row.groups()
        if not name:
            continue
        if kind != "FILE":
            name = re.sub(r" \(\d+\)$", "", name).split("@", 1)[0]
        names.add(name)
        if table == ".symtab" and section.isdecimal() and kind != "SECTION":
            defined.setdefault(name, set()).add(int(address, 16))
            has_local_definition |= binding == "LOCAL"
    if not has_local_definition:
        raise ValueError(
            "candidate needs a non-stripped .symtab with local definitions"
        )
    return ElfEvidence(defined, names, set(NEEDED.findall(output)), allocated_sections)


# GNU ld writes an indented address, nonzero section size, and input file for
# contributing objects. Read only allocated output sections after the preamble.
GNU_MAP_SECTION = re.compile(
    r"^\s+(?:\.[^\s]+\s+)?0x[0-9a-fA-F]+\s+0x([0-9a-fA-F]+)\s+(.+)$"
)
GNU_MAP_SYMBOL = re.compile(r"^\s+0x([0-9a-fA-F]+)\s+(\S+)\s*$")
GNU_MAP_OUTPUT = re.compile(r"^(\.\S+)")
OBJECT_SUFFIX = re.compile(r"\.(?:o|obj)(?:\)|$)")


def map_evidence(
    link_map: Path, anchors: dict[str, set[int]], allocated_sections: set[str]
) -> set[str]:
    _, marker, sections = link_map.read_text(errors="surrogateescape").partition(
        "\nLinker script and memory map\n"
    )
    if not marker:
        raise ValueError("expected a GNU ld map from the final executable link")

    matched_anchors = set()
    objects = set()
    allocated = False
    for line in sections.splitlines():
        if match := GNU_MAP_OUTPUT.match(line):
            allocated = match[1] in allocated_sections
        if match := GNU_MAP_SYMBOL.fullmatch(line):
            address, name = match.groups()
            if int(address, 16) in anchors.get(name, ()):
                matched_anchors.add(name)
        if allocated and (match := GNU_MAP_SECTION.fullmatch(line)):
            size, path = match.groups()
            path = path.rstrip()
            if int(size, 16) and OBJECT_SUFFIX.search(path):
                objects.add(path)

    missing = anchors.keys() - matched_anchors
    if missing:
        raise ValueError(
            f"link map lacks executable symbol addresses for {', '.join(sorted(missing))}"
        )
    if not objects:
        raise ValueError("link map contains no contributing objects")
    return objects


def check_patterns(
    label: str, names: set[str], patterns: list[re.Pattern[str]], *, required: bool
) -> tuple[dict[str, list[str]], list[str]]:
    evidence = {}
    failures = []
    for pattern in patterns:
        matches = sorted(name for name in names if pattern.fullmatch(name))
        evidence[pattern.pattern] = matches
        if required and not matches:
            failures.append(f"missing required {label} matching {pattern.pattern!r}")
        if not required and matches:
            failures.append(
                f"forbidden {label} matching {pattern.pattern!r}: {', '.join(matches)}"
            )
    return evidence, failures


def check_artifact(args: argparse.Namespace) -> tuple[dict[str, object], list[str]]:
    artifact_hash = sha256(args.binary)
    elf = elf_evidence(args.binary)
    required, failures = check_patterns(
        "defined symbol", set(elf.defined), args.require_symbol, required=True
    )
    forbidden, violations = check_patterns(
        "symbol or C file entry", elf.names, args.forbid_symbol, required=False
    )
    failures.extend(violations)
    forbidden_libraries, violations = check_patterns(
        "dynamic library", elf.libraries, args.forbid_library, required=False
    )
    failures.extend(violations)
    report: dict[str, object] = {
        "binary": str(args.binary),
        "binary_sha256": artifact_hash,
        "required_symbols": required,
        "forbidden_symbols": forbidden,
        "needed_libraries": sorted(elf.libraries),
        "forbidden_libraries": forbidden_libraries,
    }
    if args.link_map and all(required.values()):
        map_hash = sha256(args.link_map)
        anchors = {
            name: elf.defined[name] for matches in required.values() for name in matches
        }
        objects = map_evidence(args.link_map, anchors, elf.allocated_sections)
        required_objects, violations = check_patterns(
            "link map object", objects, args.require_object, required=True
        )
        failures.extend(violations)
        forbidden_objects, violations = check_patterns(
            "link map object", objects, args.forbid_object, required=False
        )
        failures.extend(violations)
        if map_hash != sha256(args.link_map):
            raise ValueError("link map changed during inspection")
        report.update(
            link_map_sha256=map_hash,
            contributing_objects=len(objects),
            required_objects=required_objects,
            forbidden_objects=forbidden_objects,
        )

    if artifact_hash != sha256(args.binary):
        raise ValueError("candidate executable changed during inspection")
    return report, failures


def sha256(path: Path) -> str:
    with path.open("rb") as handle:
        digest = hashlib.sha256()
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def non_negative_int(value: str) -> int:
    number = int(value)
    if number < 0:
        raise argparse.ArgumentTypeError("expected a non-negative byte count")
    return number


def pattern(value: str) -> re.Pattern[str]:
    if not value:
        raise argparse.ArgumentTypeError("expected a nonempty regex")
    try:
        return re.compile(value)
    except re.error as exc:
        raise argparse.ArgumentTypeError(str(exc)) from exc


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    sub = parser.add_subparsers(dest="mode", required=True)
    source = sub.add_parser(
        "source", help="compare a Git commit with the fixed source baseline"
    )
    source.add_argument(
        "--repo", type=Path, default=Path(__file__).resolve().parents[2]
    )
    source.add_argument(
        "--rev", default="HEAD", help="candidate Git commit (default HEAD)"
    )
    source.add_argument(
        "--max-bytes", type=non_negative_int, help="cap the candidate's raw bytes"
    )
    source.add_argument(
        "--files", action="store_true", help="include each counted path and size"
    )

    artifact = sub.add_parser(
        "artifact", help="check a non-stripped ELF and optional GNU ld map"
    )
    artifact.add_argument("binary", type=Path)
    artifact.add_argument(
        "--require-symbol", type=pattern, action="append", required=True
    )
    artifact.add_argument("--forbid-symbol", type=pattern, action="append", default=[])
    artifact.add_argument("--forbid-library", type=pattern, action="append", default=[])
    artifact.add_argument(
        "--link-map", type=Path, help="GNU ld -Map output from the candidate link"
    )
    artifact.add_argument("--require-object", type=pattern, action="append", default=[])
    artifact.add_argument("--forbid-object", type=pattern, action="append", default=[])
    artifact.epilog = (
        "Repeat regex flags as needed. Each regex matches one entire symbol name, "
        "file entry, direct dynamic dependency, or contributing object path. "
        "Use the pre-strip executable and keep any map from the same final link."
    )

    args = parser.parse_args()
    if args.mode == "artifact":
        if (args.require_object or args.forbid_object) and not args.link_map:
            artifact.error("object checks require --link-map")
        if not (args.forbid_symbol or args.forbid_library or args.forbid_object):
            artifact.error(
                "supply at least one forbidden symbol, library, or object regex"
            )

    try:
        check = check_source if args.mode == "source" else check_artifact
        report, failures = check(args)
    except (OSError, ValueError) as exc:
        print(f"native retirement: {exc}", file=sys.stderr)
        return 1
    print(json.dumps({**report, "failures": failures}, indent=2))
    return int(bool(failures))


if __name__ == "__main__":
    sys.exit(main())
