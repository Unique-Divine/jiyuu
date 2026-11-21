import { describe, expect, test } from "bun:test"
import { chmod, mkdtemp, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"

import {
  compare,
  diffLines,
  hookPayload,
  invocations,
  normalize,
  parseArgs,
  projectName,
  runParity,
  sideEnv,
} from "./cli-parity"

const CTX = {
  bin: "/build/mori",
  scratch: "/tmp/mori-bin-abc123",
  sideRoot: "/tmp/mori-bin-abc123/rs",
}

async function withDir<T>(fn: (dir: string) => Promise<T>): Promise<T> {
  const dir = await mkdtemp(join(tmpdir(), "cli-parity-"))
  try {
    return await fn(dir)
  } finally {
    await rm(dir, { recursive: true, force: true })
  }
}

// A stand-in binary: a shell script that prints its program name and
// version for --version, echoes its arguments otherwise, and answers MCP
// requests line by line.
function fakeBinary(name: string, extra = ""): string {
  return `#!/bin/sh
if [ "$1" = --version ]; then echo "${name} 0.13.0"; exit 0; fi
if [ "$#" -eq 0 ]; then
  while IFS= read -r line; do
    case "$line" in
      *'"id":1'*) echo '{"jsonrpc":"2.0","id":1,"result":{"ok":true}}' ;;
      *'"id":2'*) echo '{"jsonrpc":"2.0","id":2,"result":{"tools":[]}}' ;;
    esac
  done
  exit 0
fi
echo "args: $*"
echo "at 2026-11-21T10:00:00Z cache=$CBM_CACHE_DIR"
${extra}
exit 0
`
}

describe("normalize", () => {
  test("replaces program names and versions", () => {
    expect(normalize("mori 0.13.0\n", CTX)).toBe("<PROGRAM> <VERSION>\n")
    expect(normalize("codebase-memory-mcp 0.14.0-dev\n", CTX)).toBe(
      "<PROGRAM> <VERSION>\n",
    )
    expect(normalize("mori dev", CTX)).toBe("<PROGRAM> <VERSION>")
    // A bare program name in other text is left alone.
    expect(normalize("run codebase-memory-mcp cli", CTX)).toBe(
      "run codebase-memory-mcp cli",
    )
  })

  test("replaces paths, project names, times and durations", () => {
    const text =
      "/build/mori /tmp/mori-bin-abc123/rs/cache " +
      "tmp-mori-bin-abc123-repo /tmp/mori-bin-abc123/repo " +
      '2026-11-21T10:00:00.123+09:00 {"elapsed_ms":42,"nodes":7}'
    expect(normalize(text, CTX)).toBe(
      "<BIN> <SIDE>/cache <SCRATCH>-repo <SCRATCH>/repo " +
        '<TIME> {"elapsed_ms":<MS>,"nodes":7}',
    )
  })
})

describe("diff and compare", () => {
  test("lists only differing lines", () => {
    expect(diffLines("a\nb\nc", "a\nB\nc")).toBe("2 -c: b\n2 +mori: B")
    expect(diffLines("a", "a\nextra")).toBe("2 +mori: extra")
  })

  test("compare reports equality", () => {
    expect(compare("x", "same", "same")).toEqual({
      name: "x",
      same: true,
      diff: "",
    })
    expect(compare("x", "a", "b").same).toBe(false)
  })
})

describe("invocations", () => {
  test("cover the fixed command list", () => {
    const repo = "/tmp/mori-bin-x/repo"
    const names = invocations(repo, projectName(repo)).map((i) => i.name)
    expect(names).toEqual([
      "--version",
      "--help",
      "cli list_projects (empty)",
      "cli index_repository",
      "cli list_projects",
      "cli search_graph",
      "cli query_graph",
      "cli trace_path",
      "cli trace_path",
      "cli get_architecture",
      "hook-augment",
      "mcp stdio initialize + tools/list",
    ])
    expect(projectName(repo)).toBe("tmp-mori-bin-x-repo")
    expect(JSON.parse(hookPayload(repo)).cwd).toBe(repo)
    const hook = invocations(repo, "p").find((i) => i.name === "hook-augment")
    expect(hook?.env).toEqual({ CBM_HOOK_DEADLINE_MS: "10000" })
  })

  test("side environment holds only PATH and scratch dirs", () => {
    const env = sideEnv({ label: "c", bin: "/b", root: "/s/c" }, "/usr/bin")
    expect(env).toEqual({
      PATH: "/usr/bin",
      HOME: "/s/c/home",
      CBM_CACHE_DIR: "/s/c/cache",
      CBM_RUNTIME_DIR: "/s/c/rt",
    })
  })

  test("parseArgs needs both binaries", () => {
    expect(() => parseArgs(["--mori", "/m"], "/r")).toThrow("usage")
    expect(parseArgs(["--mori", "/m", "--c", "/c"], "/r")).toEqual({
      mori: "/m",
      c: "/c",
      fixture: "/r/tests/fixtures/parity-repo",
    })
    expect(() => parseArgs(["--bogus"], "/r")).toThrow("unknown")
  })
})

describe("runParity with stand-in binaries", () => {
  async function setup(dir: string, moriExtra = "") {
    const fixture = join(dir, "fixture")
    await Bun.write(join(fixture, "a.py"), "def a():\n    return 1\n")
    const c = join(dir, "codebase-memory-mcp")
    const mori = join(dir, "mori")
    await writeFile(c, fakeBinary("codebase-memory-mcp"))
    await writeFile(mori, fakeBinary("mori", moriExtra))
    await chmod(c, 0o755)
    await chmod(mori, 0o755)
    return { fixture, c, mori }
  }

  test("binaries that differ only in name and paths match", async () => {
    await withDir(async (dir) => {
      const results = await runParity(await setup(dir))
      expect(results.filter((r) => !r.same)).toEqual([])
      expect(results.length).toBe(12)
    })
  })

  test("a real difference is reported", async () => {
    await withDir(async (dir) => {
      const opts = await setup(dir, 'echo "only in mori" >&2')
      const failed = (await runParity(opts)).filter((r) => !r.same)
      expect(failed.length).toBe(10)
      expect(failed[0]!.diff).toContain("+mori: only in mori")
    })
  })
})
