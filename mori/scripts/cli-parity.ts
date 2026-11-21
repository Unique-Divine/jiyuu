#!/usr/bin/env bun
// CLI parity check: runs the same commands through `mori` and the C binary
// `codebase-memory-mcp` built from the same commit, and fails when their
// outputs differ after normalization.
//
// Usage (from mori/):
//   bun scripts/cli-parity.ts --mori PATH --c PATH [--fixture DIR] [--keep]
//
// Each binary gets its own scratch HOME, CBM_CACHE_DIR and CBM_RUNTIME_DIR
// under one short /tmp/mori-bin-* directory (the runtime dir holds a Unix
// socket, and macOS caps socket paths at 104 bytes). Both index the same copy
// of the fixture repository, so project names match. The environment passed
// to the binaries is built from scratch: only PATH comes from the caller.
//
// Normalization replaces the binary path and scratch directories with
// placeholders, the program name before a version with `<PROGRAM>` (the
// version itself must match), and ISO-8601 timestamps and elapsed-time
// fields with placeholders. Everything else, including exit codes and
// stderr, must match.
//
// Matching output alone would pass when both binaries fail the same way, so
// each invocation also has fixed expectations on its exit code and stdout,
// checked on both sides.

import { cp, mkdir, mkdtemp, realpath, rm } from "node:fs/promises"
import { join, resolve } from "node:path"

export const PROGRAM_NAMES = ["mori", "codebase-memory-mcp"] as const

export interface Invocation {
  name: string
  args: string[]
  stdin?: string
  // Extra environment for this invocation only.
  env?: Record<string, string>
  // MCP stdio session: `args` start the server; `mcp` holds the requests.
  mcp?: McpRequest[]
  expect: Expectation
}

export interface Expectation {
  exit: number
  // Each string must appear in stdout.
  stdout?: string[]
}

export interface McpRequest {
  method: string
  id?: number
  params?: Record<string, unknown>
}

export interface RunResult {
  exitCode: number | null
  stdout: string
  stderr: string
}

export interface Side {
  label: string
  bin: string
  // Scratch root for this side: home/, cache/ and rt/ live below it.
  root: string
}

export interface NormalizeContext {
  bin: string
  scratch: string
  sideRoot: string
}

export interface Comparison {
  name: string
  same: boolean
  diff: string
  // Unmet expectations, as `<side>: <reason>`.
  failures: string[]
}

const ISO_TIME =
  /\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})?/g
// JSON fields that carry wall-clock durations. Configuration values such as
// `timeout_ms` stay compared.
const ELAPSED_FIELD = /("(?:elapsed|duration)[a-z_]*":)\d+(\.\d+)?/g

function escapeRegExp(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")
}

export function normalize(text: string, ctx: NormalizeContext): string {
  let out = text
  // Longest first: the side root sits inside the scratch dir.
  for (const [path, label] of [
    [ctx.sideRoot, "<SIDE>"],
    [ctx.scratch, "<SCRATCH>"],
    [ctx.bin, "<BIN>"],
  ] as const) {
    out = out.split(path).join(label)
  }
  // Project names are paths with `/` turned into `-`.
  const dashed = (path: string) => path.replace(/^\//, "").replaceAll("/", "-")
  out = out.split(dashed(ctx.sideRoot)).join("<SIDE>")
  out = out.split(dashed(ctx.scratch)).join("<SCRATCH>")
  const names = PROGRAM_NAMES.map(escapeRegExp).join("|")
  out = out.replace(
    new RegExp(
      `\\b(?:${names})(?= (?:v?\\d+\\.\\d+\\.\\d+[0-9A-Za-z.+-]*|dev)\\b)`,
      "g",
    ),
    "<PROGRAM>",
  )
  out = out.replace(ISO_TIME, "<TIME>")
  out = out.replace(ELAPSED_FIELD, "$1<MS>")
  return out
}

// A line diff that marks each differing line pair; enough to read a failure
// in CI logs without a diff dependency.
export function diffLines(a: string, b: string, labels = ["c", "mori"]): string {
  const left = a.split("\n")
  const right = b.split("\n")
  const out: string[] = []
  const n = Math.max(left.length, right.length)
  for (let i = 0; i < n; i++) {
    if (left[i] === right[i]) continue
    if (left[i] !== undefined) out.push(`${i + 1} -${labels[0]}: ${left[i]}`)
    if (right[i] !== undefined) out.push(`${i + 1} +${labels[1]}: ${right[i]}`)
  }
  return out.join("\n")
}

export function formatResult(r: RunResult): string {
  return [
    `exit: ${r.exitCode}`,
    "--- stdout",
    r.stdout.trimEnd(),
    "--- stderr",
    r.stderr.trimEnd(),
  ].join("\n")
}

export function compare(
  name: string,
  c: string,
  mori: string,
  failures: string[] = [],
): Comparison {
  return {
    name,
    same: c === mori,
    diff: c === mori ? "" : diffLines(c, mori),
    failures,
  }
}

export function passed(cmp: Comparison): boolean {
  return cmp.same && cmp.failures.length === 0
}

// Reasons `result` misses `expect`; empty when it meets it.
export function checkExpectation(
  result: RunResult,
  expect: Expectation,
): string[] {
  const reasons: string[] = []
  if (result.exitCode !== expect.exit) {
    reasons.push(`exit ${result.exitCode}, expected ${expect.exit}`)
  }
  for (const needle of expect.stdout ?? []) {
    if (!result.stdout.includes(needle)) {
      reasons.push(`stdout lacks ${JSON.stringify(needle)}`)
    }
  }
  return reasons
}

export const MCP_REQUESTS: McpRequest[] = [
  {
    method: "initialize",
    id: 1,
    params: {
      protocolVersion: "2025-06-18",
      capabilities: {},
      clientInfo: { name: "cli-parity", version: "1" },
    },
  },
  { method: "notifications/initialized" },
  { method: "tools/list", id: 2, params: {} },
]

export function hookPayload(repo: string): string {
  return JSON.stringify({
    session_id: "cli-parity",
    hook_event_name: "PreToolUse",
    tool_name: "Grep",
    tool_input: { pattern: "helper", path: repo },
    cwd: repo,
  })
}

// The fixed command list, with what each must print on the fixture.
// `project` is the fixture's project name.
export function invocations(repo: string, project: string): Invocation[] {
  const q = (
    name: string,
    tool: string,
    rest: string[],
    stdout: string[],
  ): Invocation => ({
    name,
    args: ["cli", tool, "--project", project, ...rest],
    expect: { exit: 0, stdout },
  })
  return [
    {
      name: "--version",
      args: ["--version"],
      expect: { exit: 0, stdout: [" "] },
    },
    { name: "--help", args: ["--help"], expect: { exit: 0, stdout: ["Usage:"] } },
    {
      name: "cli list_projects (empty)",
      args: ["cli", "list_projects"],
      expect: { exit: 0, stdout: ["projects: 0"] },
    },
    {
      name: "cli index_repository",
      args: ["cli", "--quiet", "index_repository", "--repo-path", repo],
      expect: { exit: 0, stdout: ['"status":"indexed"'] },
    },
    {
      name: "cli list_projects",
      args: ["cli", "list_projects"],
      expect: { exit: 0, stdout: ["projects: 1", project] },
    },
    q(
      "cli search_graph",
      "search_graph",
      [
        "--name-pattern",
        "^(Add|Twice|helper|run|greet|formatName)$",
        "--format",
        "json",
      ],
      ['["Twice","Function"', '["helper","Function"', '["greet","Function"'],
    ),
    q(
      "cli query_graph",
      "query_graph",
      [
        "--query",
        "MATCH (f:Function)-[:CALLS]->(g:Function) " +
          "RETURN f.name, g.name ORDER BY f.name, g.name",
      ],
      ["Twice Add", "run helper", "greet formatName"],
    ),
    q(
      "cli trace_path helper inbound",
      "trace_path",
      ["--function-name", "helper", "--direction", "inbound"],
      ["callers: 2", `${project}.py.app.run 1`],
    ),
    q(
      "cli trace_path Twice outbound",
      "trace_path",
      ["--function-name", "Twice", "--direction", "outbound"],
      ["callees: 1", `${project}.go.Add 1`],
    ),
    q(
      "cli get_architecture",
      "get_architecture",
      [],
      [`project: ${project}`, "Function", "Python"],
    ),
    // The hook stops at a wall-clock deadline (2 s by default) and prints
    // only a timeout notice; a slow cold start on one side then reads as a
    // difference. The maximum deadline keeps the output deterministic. With
    // no daemon running it adds no graph context, so only the exit code is
    // fixed.
    {
      name: "hook-augment",
      args: ["hook-augment"],
      stdin: hookPayload(repo),
      env: { CBM_HOOK_DEADLINE_MS: "10000" },
      expect: { exit: 0 },
    },
    {
      name: "mcp stdio initialize + tools/list",
      args: [],
      mcp: MCP_REQUESTS,
      expect: {
        exit: 0,
        stdout: [
          '"serverInfo"',
          '"name": "index_repository"',
          '"name": "search_graph"',
          '"name": "trace_path"',
        ],
      },
    },
  ]
}

export function projectName(repo: string): string {
  return repo.replace(/^\//, "").replaceAll("/", "-")
}

export function sideEnv(side: Side, path: string): Record<string, string> {
  return {
    PATH: path,
    HOME: join(side.root, "home"),
    CBM_CACHE_DIR: join(side.root, "cache"),
    CBM_RUNTIME_DIR: join(side.root, "rt"),
  }
}

const TIMEOUT_MS = 180_000

async function runOne(
  side: Side,
  inv: Invocation,
  cwd: string,
  env: Record<string, string>,
): Promise<RunResult> {
  if (inv.mcp) return runMcp(side, inv, cwd, env)
  const proc = Bun.spawn([side.bin, ...inv.args], {
    cwd,
    env,
    stdin: inv.stdin === undefined ? "ignore" : new Blob([inv.stdin]),
    stdout: "pipe",
    stderr: "pipe",
    timeout: TIMEOUT_MS,
  })
  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ])
  return { exitCode, stdout, stderr }
}

// Sends each request and waits for its response before the next, so the
// server never sees EOF with work in flight. Responses are kept in order;
// server notifications in between are dropped.
async function runMcp(
  side: Side,
  inv: Invocation,
  cwd: string,
  env: Record<string, string>,
): Promise<RunResult> {
  const proc = Bun.spawn([side.bin, ...inv.args], {
    cwd,
    env,
    stdin: "pipe",
    stdout: "pipe",
    stderr: "pipe",
    timeout: TIMEOUT_MS,
  })
  const stderrText = new Response(proc.stderr).text()
  const reader = proc.stdout.getReader()
  const decoder = new TextDecoder()
  let buffer = ""
  const responses: string[] = []
  const nextLine = async (): Promise<string | undefined> => {
    for (;;) {
      const nl = buffer.indexOf("\n")
      if (nl !== -1) {
        const line = buffer.slice(0, nl)
        buffer = buffer.slice(nl + 1)
        return line
      }
      const { value, done } = await reader.read()
      if (done) return undefined
      buffer += decoder.decode(value, { stream: true })
    }
  }
  try {
    for (const req of inv.mcp ?? []) {
      const msg = { jsonrpc: "2.0", ...req }
      proc.stdin.write(JSON.stringify(msg) + "\n")
      await proc.stdin.flush()
      if (req.id === undefined) continue
      for (;;) {
        const line = await nextLine()
        if (line === undefined) {
          responses.push(`<EOF before response ${req.id}>`)
          break
        }
        if (!line.trim()) continue
        const parsed = JSON.parse(line) as { id?: number }
        if (parsed.id === req.id) {
          responses.push(JSON.stringify(parsed, null, 2))
          break
        }
      }
    }
  } finally {
    await proc.stdin.end()
  }
  const exitCode = await proc.exited
  return { exitCode, stdout: responses.join("\n"), stderr: await stderrText }
}

async function makeSide(label: string, bin: string, scratch: string) {
  const root = join(scratch, label)
  for (const sub of ["home", "cache", "rt"]) {
    await mkdir(join(root, sub), { recursive: true, mode: 0o700 })
  }
  return { label, bin: resolve(bin), root }
}

export interface ParityOptions {
  mori: string
  c: string
  fixture: string
  keep?: boolean
  log?: (line: string) => void
}

export async function runParity(opts: ParityOptions): Promise<Comparison[]> {
  const log = opts.log ?? (() => {})
  // Canonical, because the binaries name projects after the resolved repo
  // path: on macOS /tmp is a symlink to /private/tmp.
  const scratch = await realpath(await mkdtemp("/tmp/mori-bin-"))
  try {
    const repo = join(scratch, "repo")
    await cp(opts.fixture, repo, { recursive: true })
    const sides = [
      await makeSide("c", opts.c, scratch),
      await makeSide("rs", opts.mori, scratch),
    ]
    const path = process.env.PATH ?? "/usr/bin:/bin"
    const results: Comparison[] = []
    for (const inv of invocations(repo, projectName(repo))) {
      const outs: string[] = []
      const failures: string[] = []
      for (const side of sides) {
        const env = { ...sideEnv(side, path), ...inv.env }
        const raw = await runOne(side, inv, repo, env)
        for (const reason of checkExpectation(raw, inv.expect)) {
          failures.push(`${side.label}: ${reason}`)
        }
        outs.push(
          normalize(formatResult(raw), {
            bin: side.bin,
            scratch,
            sideRoot: side.root,
          }),
        )
      }
      const cmp = compare(inv.name, outs[0]!, outs[1]!, failures)
      results.push(cmp)
      const status = !cmp.same ? "DIFF" : failures.length ? "FAIL" : "same"
      log(`${status}  ${inv.name}`)
      if (!cmp.same) log(cmp.diff)
      for (const failure of failures) log(`  ${failure}`)
      if (failures.length) log(outs[0]!)
    }
    return results
  } finally {
    if (opts.keep) log(`kept ${scratch}`)
    else await rm(scratch, { recursive: true, force: true })
  }
}

export function parseArgs(argv: string[], root: string): ParityOptions {
  const opts: Partial<ParityOptions> = {
    fixture: join(root, "tests", "fixtures", "parity-repo"),
  }
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i]
    const value = () => {
      const v = argv[++i]
      if (v === undefined) throw new Error(`${arg} needs a value`)
      return v
    }
    if (arg === "--mori") opts.mori = value()
    else if (arg === "--c") opts.c = value()
    else if (arg === "--fixture") opts.fixture = value()
    else if (arg === "--keep") opts.keep = true
    else throw new Error(`unknown argument '${arg}'`)
  }
  if (!opts.mori || !opts.c) {
    throw new Error("usage: cli-parity.ts --mori PATH --c PATH [--fixture DIR]")
  }
  return opts as ParityOptions
}

if (import.meta.main) {
  const root = resolve(import.meta.dir, "..")
  try {
    const opts = parseArgs(process.argv.slice(2), root)
    const results = await runParity({ ...opts, log: console.log })
    const failed = results.filter((r) => !passed(r))
    console.log(
      `${results.length - failed.length}/${results.length} invocations ` +
        "match and meet their expectations",
    )
    if (failed.length > 0) process.exitCode = 1
  } catch (error: unknown) {
    console.error(
      `cli-parity: ${error instanceof Error ? error.message : String(error)}`,
    )
    process.exitCode = 2
  }
}
