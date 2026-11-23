import { describe, expect, test } from "bun:test"
import { chmod, mkdtemp, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import {
  checkExpectation,
  compare,
  diffLines,
  hookPayload,
  invocations,
  normalize,
  parseArgs,
  passed,
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

// A stand-in binary: a shell script that prints its program name and a
// version for --version, prints every string the invocations expect for
// any other command (with the project name taken from its working
// directory, the fixture copy), and answers MCP requests line by line.
// `version` and `extra` let a test make one side differ; `exitCode` makes
// every command fail.
function fakeBinary(
  name: string,
  opts: {
    version?: string
    extra?: string
    exitCode?: number
    endBeforeResponse?: 1 | 6
  } = {},
): string {
  const code = opts.exitCode ?? 0
  return `#!/bin/sh
if [ "$1" = --version ]; then echo "${name} ${opts.version ?? "0.13.0"}"; exit ${code}; fi
project=$(pwd | sed 's#^/##; s#/#-#g')
if [ "$#" -eq 0 ]; then
  while IFS= read -r line; do
    ${opts.endBeforeResponse === undefined ? "" : `case "$line" in *'"id":${opts.endBeforeResponse}'*) exit ${code} ;; esac`}
    case "$line" in
      *'"id":1'*) echo '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","serverInfo":{"name":"${name}","version":"0.13.0"}}}' ;;
      *'"id":2'*) echo '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"index_repository"},{"name":"search_graph"},{"name":"trace_path"}]}}' ;;
      *'"id":3'*) printf '{"jsonrpc":"2.0","id":3,"result":{"projects":["%s"]}}\n' "$project" ;;
      *'"id":4'*) printf '{"jsonrpc":"2.0","id":4,"result":{"name":"helper","qn_prefix":"%s.py.util"}}\n' "$project" ;;
      *'"id":5'*) echo '{"jsonrpc":"2.0","id":5,"result":{"rows":["Twice Add","run helper","greet formatName"]}}' ;;
      *'"id":6'*) echo '{"jsonrpc":"2.0","id":6,"result":{"nodes":["Function"],"edges":["CALLS"]}}' ;;
    esac
  done
  exit ${code}
fi
echo "args: $*"
echo "at 2026-11-21T10:00:00Z cache=$CBM_CACHE_DIR"
echo 'Usage: projects: 0 projects: 1 "status":"indexed"'
echo '["Twice","Function" ["helper","Function" ["greet","Function"'
echo "Twice Add | run helper | greet formatName | Function | Python"
echo "callers: 2 | $project.py.app.run 1 | callees: 1 | $project.go.Add 1"
echo "project: $project"
${opts.extra ?? ""}
exit ${code}
`
}

describe("normalize", () => {
  test("replaces the program name before a version, keeping the version", () => {
    expect(normalize("mori 0.13.0\n", CTX)).toBe("<PROGRAM> 0.13.0\n")
    expect(normalize("codebase-memory-mcp 0.14.0-dev\n", CTX)).toBe(
      "<PROGRAM> 0.14.0-dev\n",
    )
    expect(normalize("mori dev", CTX)).toBe("<PROGRAM> dev")
    // A bare program name in other text is left alone.
    expect(normalize("run codebase-memory-mcp cli", CTX)).toBe(
      "run codebase-memory-mcp cli",
    )
  })

  test("replaces paths, project names, times and durations", () => {
    const text =
      "/build/mori /tmp/mori-bin-abc123/rs/cache " +
      "tmp-mori-bin-abc123-repo /tmp/mori-bin-abc123/repo " +
      '2026-11-21T10:00:00.123+09:00 {"elapsed_ms":42,"nodes":7,' +
      '"duration_s":1.5,"timeout_ms":300}'
    expect(normalize(text, CTX)).toBe(
      "<BIN> <SIDE>/cache <SCRATCH>-repo <SCRATCH>/repo " +
        '<TIME> {"elapsed_ms":<MS>,"nodes":7,"duration_s":<MS>,' +
        '"timeout_ms":300}',
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
      failures: [],
    })
    expect(compare("x", "a", "b").same).toBe(false)
    expect(passed(compare("x", "a", "a"))).toBe(true)
    expect(passed(compare("x", "a", "a", ["c: exit 1, expected 0"]))).toBe(
      false,
    )
  })

  test("checkExpectation reports exit codes and missing output", () => {
    const ok = { exitCode: 0, stdout: "callers: 2\n", stderr: "" }
    expect(checkExpectation(ok, { exit: 0, stdout: ["callers: 2"] })).toEqual(
      [],
    )
    expect(
      checkExpectation(
        { ...ok, exitCode: 1 },
        { exit: 0, stdout: ["callers: 2", "Twice Add"] },
      ),
    ).toEqual(["exit 1, expected 0", 'stdout lacks "Twice Add"'])
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
      "cli trace_path helper inbound",
      "cli trace_path Twice outbound",
      "cli trace_path with repeated and literal edge types",
      "cli get_architecture",
      "hook-augment",
      "mcp stdio initialization and graph reads",
    ])
    expect(projectName(repo)).toBe("tmp-mori-bin-x-repo")
    expect(JSON.parse(hookPayload(repo)).cwd).toBe(repo)
    const hook = invocations(repo, "p").find((i) => i.name === "hook-augment")
    expect(hook?.env).toEqual({ CBM_HOOK_DEADLINE_MS: "10000" })
    for (const inv of invocations(repo, "p")) expect(inv.expect.exit).toBe(0)
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
  type FakeOpts = Parameters<typeof fakeBinary>[1]
  async function setup(dir: string, c: FakeOpts = {}, mori: FakeOpts = c) {
    const fixture = join(dir, "fixture")
    await Bun.write(join(fixture, "a.py"), "def a():\n    return 1\n")
    const cBin = join(dir, "codebase-memory-mcp")
    const moriBin = join(dir, "mori")
    await writeFile(cBin, fakeBinary("codebase-memory-mcp", c))
    await writeFile(moriBin, fakeBinary("mori", mori))
    await chmod(cBin, 0o755)
    await chmod(moriBin, 0o755)
    return { fixture, c: cBin, mori: moriBin }
  }

  test("binaries that differ only in name and paths pass", async () => {
    await withDir(async (dir) => {
      const results = await runParity(await setup(dir))
      expect(results.filter((r) => !passed(r))).toEqual([])
      expect(results.length).toBe(13)
    })
  })

  test("a real difference is reported", async () => {
    await withDir(async (dir) => {
      const opts = await setup(dir, {}, { extra: 'echo "only in mori" >&2' })
      const failed = (await runParity(opts)).filter((r) => !r.same)
      expect(failed.length).toBe(11)
      expect(failed[0]!.diff).toContain("+mori: only in mori")
    })
  })

  test("a version mismatch is a difference", async () => {
    await withDir(async (dir) => {
      const opts = await setup(dir, { version: "dev" }, { version: "0.13.0" })
      const failed = (await runParity(opts)).filter((r) => !passed(r))
      expect(failed.map((r) => r.name)).toEqual(["--version"])
    })
  })

  test("identical failures on both sides fail the run", async () => {
    await withDir(async (dir) => {
      const results = await runParity(await setup(dir, { exitCode: 1 }))
      expect(results.every((r) => r.same)).toBe(true)
      expect(results.every((r) => !passed(r))).toBe(true)
      const index = results.find((r) => r.name === "cli index_repository")!
      expect(index.failures).toEqual([
        "c: exit 1, expected 0",
        "rs: exit 1, expected 0",
      ])
    })
  })

  test.each([[1, "initialize"], [6, "tools/call"]] as const)(
    "matching streams missing response %i fail with request diagnostics",
    async (id, method) => {
      await withDir(async (dir) => {
        const results = await runParity(
          await setup(dir, { endBeforeResponse: id }),
        )
        const failed = results.filter((r) => !passed(r))
        expect(failed).toHaveLength(1)
        expect(failed[0]!.failures).toEqual([
          `c: ${method} id=${id}: EOF before response`,
          `rs: ${method} id=${id}: EOF before response`,
        ])
      })
    },
  )
})
