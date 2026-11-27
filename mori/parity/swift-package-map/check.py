"""Compare package-map construction with the pinned C pass on Linux."""

import difflib
import subprocess
from pathlib import Path
from tempfile import TemporaryDirectory

PROBE = Path(__file__).resolve().with_name("probe.c")
MORI = PROBE.parents[2]
BASE = "e5ff9708830a5fbfe879dfe925996b9b193c3e74"
PKGMAP = "src/pipeline/pass_pkgmap.c"
BRIDGE = "src/pipeline/pkgmap_bridge.c"

MANIFESTS = {
    "App/Package.swift": """
        let package = Package(dependencies: [.package(path: "../Core")], targets: [
            .target(name: "App", dependencies: [.target(name: "Phantom")])
        ])
    """,
    "Core/Package.swift": r"""
        // .target(name: "Comment")
        /* /* .target(name: "NestedComment") */ .target(name: "OuterComment") */
        let text = ".target(name: \"String\")"
        let package = Package(products: [.library(name: "Alias", targets: ["Core"])],
            targets: [
                .target(name: "Core", path: "./Old/../Library"),
                .testTarget(name: "CoreTests"),
                .executableTarget(name: "Tool"),
                .macro(name: "Macros"),
                .plugin(name: "Plugin"),
                .target(name: "AtRoot", path: "."),
                .target(name: "Dynamic", path: "Sources/" + suffix),
                .target(name: "Escapes", path: "../Outside")
            ])
    """,
    "Core/Duplicate/Package.swift": '.target(name: "Core", path: "Wrong")',
    "Boundary/Package.swift": "// Only a package boundary.\n",
    "Empty/Package.swift": "",
    "Excluded/Package.swift": '.target(name: "Excluded")',
    ".git/Package.swift": '.target(name: "Skipped")',
    "Web/package.json": (
        '{"name":"js-fixture","main":"index.js",'
        '"exports":{".":{"default":"./fallback.js","import":"./src/main.ts"}}}'
    ),
    "go.mod": "module example.com/demo\n\ngo 1.21\n",
    "Workspace/Cargo.toml": "[workspace]\n[package]\nname = 'rust-pkg'\n",
    "App/pyproject.toml": '[project]\nname = "flask-example-celery"\n',
    "Php/composer.json": (
        r'{"autoload":{"psr-4":{"Z\\":"./src/","A\\":".",'
        r'"Z\\":"duplicate/"}},"autoload-dev":{"psr-4":{"Z\\":"dev/"}}}'
    ),
    "Flutter/pubspec.yaml": "name: dart_pkg\n",
    "Java/pom.xml": (
        "<parent><groupId>ignored</groupId><artifactId>x</artifactId></parent>"
        "<groupId>com.acme</groupId><artifactId>example</artifactId>"
    ),
    "Kotlin/build.gradle.kts": 'group = "com.acme.kotlin"\n',
    "Elixir/mix.exs": "  app: :elixir_pkg,\n",
    "Ruby/example.gemspec": "spec.name = 'ruby-pkg'\n",
}

C_SOURCES = [
    "src/pipeline/fqn.c",
    "src/pipeline/path_alias.c",
    "src/pipeline/pipeline.c",
    "src/pipeline/pass_parallel.c",
    "src/pipeline/pass_lsp_cross.c",
    "src/pipeline/pass_terraform.c",
    "src/pipeline/registry.c",
    "src/discover/discover.c",
    "src/discover/language.c",
    "src/discover/userconfig.c",
    "src/graph_buffer/graph_buffer.c",
    "src/foundation/compat.c",
    "src/foundation/compat_fs.c",
    "src/foundation/compat_thread.c",
    "src/foundation/hash_table.c",
    "src/foundation/log.c",
    "src/foundation/mem_core.c",
    "src/foundation/mem.c",
    "src/foundation/mem_events.c",
    "src/foundation/path_syntax_internal.c",
    "src/foundation/platform.c",
    "src/foundation/profile.c",
    "src/foundation/str_util.c",
    "src/foundation/yaml.c",
    "internal/cbm/helpers.c",
    "vendored/yyjson/yyjson.c",
]

CC = [
    "cc",
    "-std=c11",
    "-O0",
    "-D_GNU_SOURCE",
    "-DCBM_SANITIZED_BUILD=1",
    "-Wno-format-truncation",
    "-ffunction-sections",
    "-fdata-sections",
    "-Wl,--gc-sections",
    "-Isrc",
    "-Ivendored",
    "-Ivendored/sqlite3",
    "-Ivendored/mimalloc/include",
    "-Iinternal/cbm",
    "-Iinternal/cbm/vendored/ts_runtime/include",
]


def run(*args: str | Path) -> str:
    return subprocess.run(
        args, cwd=MORI, check=True, stdout=subprocess.PIPE, text=True
    ).stdout


def probe(scratch: Path) -> str:
    repo = scratch / "swift-package-map-fixture"
    for relative, source in MANIFESTS.items():
        manifest = repo / relative
        manifest.parent.mkdir(parents=True, exist_ok=True)
        manifest.write_text(source, encoding="utf-8")
    (repo / "Symlink").symlink_to("Core", target_is_directory=True)

    reference_c = scratch / "reference.c"
    reference_c.write_text(
        run("git", "show", f"{BASE}:mori/{PKGMAP}"), encoding="utf-8"
    )
    rust_archive = scratch / "pkgmap.a"
    rust_probe = scratch / "pkgmap.rs"
    rust_probe.write_text(
        "".join(
            f'#[path = "../src/{path}"]\nmod {module};\n'
            for module, path in (
                ("native_hash_table", "foundation/native_hash_table.rs"),
                ("swift_package_map", "pipeline/swift_package_map.rs"),
                ("pkgmap", "pipeline/pkgmap/mod.rs"),
            )
        ),
        encoding="utf-8",
    )
    run(
        "rustc",
        "--edition",
        "2024",
        "--crate-type",
        "staticlib",
        rust_probe,
        "-o",
        rust_archive,
    )

    reference = scratch / "reference"
    candidate = scratch / "candidate"
    # The pinned source used to get this declaration transitively.
    run(
        *CC,
        "-include",
        "foundation/path_syntax_internal.h",
        PROBE,
        *C_SOURCES,
        reference_c,
        "-o",
        reference,
    )
    run(*CC, PROBE, *C_SOURCES, PKGMAP, BRIDGE, rust_archive, "-o", candidate)

    before = run(reference, repo)
    after = run(candidate, repo)
    if before != after:
        diff = difflib.unified_diff(
            before.splitlines(),
            after.splitlines(),
            fromfile="C",
            tofile="Rust",
            lineterm="",
        )
        raise SystemExit("\n".join(diff))
    return after


if __name__ == "__main__":
    with TemporaryDirectory(prefix=".swift-package-map-", dir=MORI) as directory:
        print(probe(Path(directory)), end="")
