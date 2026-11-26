"""Build or verify one frozen C reference for an exact runner configuration."""

import argparse
import hashlib
import json
import os
import platform
import shlex
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

REFERENCE = "8bdd8fc20327fcfea4d920138fef77a3b00ca1ef"
GRAMMARS = "965435e26af6b1a3f13433100d9d60b91176f35f03b6b2fe248695b096378270"
BUILDER = Path(__file__).with_name("build-c-reference.sh")
EXECUTABLE = "codebase-memory-mcp"
MANIFEST = "manifest.json"
BUILD_ENV = (
    "CC",
    "CXX",
    "CFLAGS_EXTRA",
    "CPPFLAGS",
    "LDFLAGS",
    "SDKROOT",
    "MACOSX_DEPLOYMENT_TARGET",
    "MORI_REFERENCE_VERSION",
    "MORI_WITH_UI",
    "TEST_SEAMS",
)
PROCESS_ENV = (
    "PATH",
    "HOME",
    "TMPDIR",
    "MORI_MAKE_JOBS",
    "CCACHE_COMPILERCHECK",
    "CCACHE_MAXSIZE",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
)


def digest(path: Path) -> str:
    checksum = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            checksum.update(chunk)
    return checksum.hexdigest()


def output(*args: str) -> str:
    return subprocess.check_output(args, text=True).strip()


def identity(target: str) -> dict[str, str]:
    system = platform.system()
    cc = os.environ.get("CC") or ("cc" if system == "Darwin" else "gcc")
    cxx = os.environ.get("CXX") or ("c++" if system == "Darwin" else "g++")
    sdk = (
        output("xcrun", "--show-sdk-version")
        if system == "Darwin"
        else output("getconf", "GNU_LIBC_VERSION")
    )
    dependency_versions = (
        output("sw_vers")
        if system == "Darwin"
        else output("dpkg-query", "-W", "libc6-dev", "zlib1g-dev")
    )
    ui = os.environ.get("MORI_WITH_UI") == "1"
    linker = shutil.which("ld") if system == "Linux" else None
    if system == "Linux" and linker is None:
        raise ValueError("Linux reference build requires ld on PATH")
    return {
        "reference": REFERENCE,
        "grammars": GRAMMARS,
        "target": target,
        "system": system,
        "architecture": platform.machine(),
        "sdk": sdk,
        "linker": digest(Path(linker))
        if linker is not None
        else output("xcodebuild", "-version"),
        "dependencies": dependency_versions,
        "cc": output(*shlex.split(cc), "--version"),
        "cxx": output(*shlex.split(cxx), "--version"),
        "node": output("node", "--version") if ui else "",
        "npm": output("npm", "--version") if ui else "",
        "bun": output("bun", "--version"),
        "builder": digest(BUILDER),
        "cache_tool": digest(Path(__file__)),
        "runner_image": os.environ.get("ImageVersion", ""),
        "make": output("make", "--version"),
        **{name: os.environ.get(name, "") for name in BUILD_ENV},
    }


def key(inputs: dict[str, str]) -> str:
    return hashlib.sha256(json.dumps(inputs, sort_keys=True).encode()).hexdigest()


def verify(directory: Path, inputs: dict[str, str]) -> None:
    binary = directory / EXECUTABLE
    manifest = directory / MANIFEST
    if binary.is_symlink() or manifest.is_symlink():
        raise ValueError(f"reference cache contains a symbolic link in {directory}")
    expected = json.loads(manifest.read_text())
    if expected != {"inputs": inputs, "sha256": digest(binary)}:
        raise ValueError(
            f"reference cache identity or checksum mismatch in {directory}"
        )
    if not os.access(binary, os.X_OK):
        raise ValueError(f"reference is not executable: {binary}")


def build_or_verify(directory: Path, inputs: dict[str, str]) -> None:
    if directory.is_symlink():
        raise ValueError(f"cache destination must not be a symbolic link: {directory}")
    if directory.exists():
        if not directory.is_dir() or any(
            child.name not in {EXECUTABLE, MANIFEST} for child in directory.iterdir()
        ):
            raise ValueError(f"invalid reference cache directory {directory}")
        try:
            verify(directory, inputs)
        except (OSError, ValueError) as error:
            print(
                f"Discarding restored C reference {key(inputs)}: {error}",
                file=sys.stderr,
            )
        else:
            print(f"Verified cached C reference {key(inputs)}")
            return

    directory.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=directory.parent) as scratch:
        staged = Path(scratch) / "reference"
        staged.mkdir()
        environment = {
            name: os.environ[name]
            for name in (*BUILD_ENV, *PROCESS_ENV)
            if name in os.environ
        }
        environment.update(LC_ALL="C", GIT_TERMINAL_PROMPT="0")
        subprocess.run(
            ["bash", str(BUILDER), str(staged / EXECUTABLE)],
            check=True,
            env=environment,
        )
        (staged / MANIFEST).write_text(
            json.dumps(
                {"inputs": inputs, "sha256": digest(staged / EXECUTABLE)},
                sort_keys=True,
            )
            + "\n"
        )
        if directory.exists():
            directory.rename(Path(scratch) / "rejected")
        staged.rename(directory)
    print(f"Built C reference {key(inputs)}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("pin", help="print the frozen reference source commit")
    grammar = commands.add_parser(
        "verify-grammar", help="verify the pinned grammar lock"
    )
    grammar.add_argument("path", type=Path)
    for command in ("key", "ensure"):
        cache = commands.add_parser(command)
        cache.add_argument("target")
        if command == "ensure":
            cache.add_argument("directory", type=Path)
    args = parser.parse_args()

    try:
        if args.command == "pin":
            print(REFERENCE)
        elif args.command == "verify-grammar":
            if digest(args.path) != GRAMMARS:
                raise ValueError(f"reference grammar lock mismatch in {args.path}")
        elif args.command == "key":
            print(key(identity(args.target)))
        else:
            build_or_verify(args.directory.absolute(), identity(args.target))
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"C reference: {error}\n")


if __name__ == "__main__":
    main()
