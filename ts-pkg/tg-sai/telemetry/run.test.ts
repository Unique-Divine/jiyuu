import { describe, expect, test } from "bun:test"
import { mkdtempSync, readFileSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

import { MtPeerNotFoundError, tl } from "@mtcute/bun"

import { NoopSink } from "./db"
import {
  activeTelemetryRun,
  BOOTSTRAP_FLOOD_MAX_WAIT_MS,
  floodMaxWaitMs,
  nextJsonlName,
  REDACTED,
  sanitizeArgv,
  startTelemetryRun,
  telemetryJsonlFields,
} from "./run"

function withDir<T>(fn: (dir: string) => T): T {
  const dir = mkdtempSync(join(tmpdir(), "tg-sai-run-"))
  try {
    return fn(dir)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
}

function readLines(path: string): Record<string, unknown>[] {
  return readFileSync(path, "utf8")
    .trim()
    .split("\n")
    .map((line) => JSON.parse(line))
}

describe("startTelemetryRun", () => {
  test("writes run.start/run.end to the run log and clears the active run",
    () => {
      withDir((dir) => {
        const run = startTelemetryRun({
          subsystem: "worker",
          command: "worker",
          argv: ["worker", "--custody-profile", "custody", "--run"],
          version: "0.0.0-test",
          env: { TG_SAI_DIR: dir },
          sink: new NoopSink(),
        })
        expect(activeTelemetryRun()).toBe(run)
        expect(run.floodMaxWaitMs).toBe(30000)
        run.appendJsonl({ type: "worker_tick", jobs: 0 })
        run.markAborted()
        run.end({ exitCode: 0 })
        run.end({ exitCode: 0 })
        expect(activeTelemetryRun()).toBeNull()

        expect(run.jsonlPath).toBe(join(dir, "logs", "worker-00.jsonl"))
        const lines = readLines(run.jsonlPath!)
        expect(lines.map((l) => l.event ?? l.type)).toEqual([
          "run.start",
          "worker_tick",
          "run.end",
        ])
        expect(lines.every((l) => l.run_id === run.context.runId)).toBe(true)
        expect(lines[2]).toMatchObject({ outcome: "aborted", error_code: 0 })
      })
    })

  test("failed runs record the exit code and error", () => {
    withDir((dir) => {
      const run = startTelemetryRun({
        subsystem: "cli",
        command: "ops sai-find",
        argv: [],
        version: "0.0.0-test",
        env: { TG_SAI_DIR: dir },
        logPrefix: "ops-sai-find",
        sink: new NoopSink(),
      })
      run.end({ exitCode: 1, error: new Error("FLOOD_WAIT_15382") })
      const end = readLines(run.jsonlPath!).at(-1)
      expect(end).toMatchObject({
        event: "run.end",
        outcome: "error",
        error_code: 1,
        err: "Error [FLOOD_WAIT_15382]",
      })
    })
  })

  test("run.end keeps free-form error messages out of the row", () => {
    const cases: [unknown, string][] = [
      [
        new Error(
          "Missing chatId in JSONL input line: " +
            '{"title":"x","link":"t.me/+abc"}',
        ),
        "Error",
      ],
      [new MtPeerNotFoundError("Peer @alice not found"), "MtPeerNotFoundError"],
      [new Error("Unknown profile: x"), "Error"],
      [
        tl.RpcError.fromTl({
          _: "mt_rpc_error",
          errorCode: 420,
          errorMessage: "FLOOD_WAIT_400",
        }),
        "RpcError [420 FLOOD_WAIT_400]",
      ],
    ]
    for (const [error, err] of cases) {
      withDir((dir) => {
        const run = startTelemetryRun({
          subsystem: "cli",
          command: "ops sai-bootstrap",
          argv: [],
          version: "0.0.0-test",
          env: { TG_SAI_DIR: dir },
          sink: new NoopSink(),
        })
        run.end({ exitCode: 1, error })
        expect(readLines(run.jsonlPath!).at(-1)?.err).toBe(err)
      })
    }
  })

  test("floodMaxWaitMs option sets the default; the env var still wins",
    () => {
      withDir((dir) => {
        const run = startTelemetryRun({
          subsystem: "cli",
          command: "ops sai-bootstrap",
          argv: [],
          version: "0.0.0-test",
          env: { TG_SAI_DIR: dir },
          floodMaxWaitMs: BOOTSTRAP_FLOOD_MAX_WAIT_MS,
          logPrefix: null,
          sink: new NoopSink(),
        })
        expect(run.floodMaxWaitMs).toBe(300_000)
        expect(run.client().floodWaiter).toEqual({ maxWait: 300_000 })
        run.end({ exitCode: 0 })
      })
      expect(
        floodMaxWaitMs("cli", { TG_SAI_FLOOD_MAX_WAIT_MS: "5000" }, 300_000),
      ).toBe(5000)
      expect(floodMaxWaitMs("cli", {}, 300_000)).toBe(300_000)
    })

  test("an unwritable logs dir disables the run log, not the run", () => {
    withDir((dir) => {
      const errors: string[] = []
      const run = startTelemetryRun({
        subsystem: "cli",
        command: "audit",
        argv: [],
        version: "0.0.0-test",
        logsDir: join(dir, "missing", "\0bad"),
        sink: new NoopSink(),
        stderr: (line) => errors.push(line),
      })
      expect(run.jsonlPath).toBeNull()
      expect(errors).toHaveLength(1)
      run.end({ exitCode: 0 })
    })
  })
})

describe("jsonl helpers", () => {
  test("nextJsonlName continues the highest index for its prefix", () => {
    expect(nextJsonlName([], "sai-bootstrap")).toBe("sai-bootstrap-00.jsonl")
    expect(
      nextJsonlName(
        ["sai-find-00.jsonl", "sai-find-07.jsonl", "ops-sai-find-02.jsonl"],
        "sai-find",
      ),
    ).toBe("sai-find-08.jsonl")
  })

  test("telemetryJsonlFields uses DB column names and drops nulls", () => {
    expect(
      telemetryJsonlFields({
        eventId: "e",
        requestId: "r",
        runId: "run",
        tsMs: 0,
        event: "tg.rpc.call",
        subsystem: "cli",
        outcome: "ok",
        errorCode: null,
      }),
    ).toEqual({
      ts: "1970-01-01T00:00:00.000Z",
      type: "telemetry",
      event_id: "e",
      request_id: "r",
      run_id: "run",
      ts_ms: 0,
      event: "tg.rpc.call",
      subsystem: "cli",
      outcome: "ok",
    })
  })
})

describe("sanitizeArgv", () => {
  const R = REDACTED
  const cases: [string, string[], string[]][] = [
    [
      "password value starting with a dash",
      ["profile", "add", "--password", "-hunter2", "--name", "custody"],
      ["profile", "add", "--password", R, "--name", R],
    ],
    [
      "target user handle",
      ["ops", "sai-bootstrap", "--target-user", "@alice", "--run"],
      ["ops", "sai-bootstrap", "--target-user", R, "--run"],
    ],
    [
      "free-text reason",
      ["provision", "remove", "--reason", "free text", "--json"],
      ["provision", "remove", "--reason", R, "--json"],
    ],
    [
      "positional profile selector",
      ["profile", "use", "alice"],
      ["profile", "use", R],
    ],
    [
      "file keeps only its basename",
      ["ops", "sai-bootstrap", "--file", "/home/x/chats-00.jsonl"],
      ["ops", "sai-bootstrap", "--file", "chats-00.jsonl"],
    ],
    [
      "numeric chat kept, username chat redacted",
      ["audit", "--chat", "-100123", "--chat", "@group"],
      ["audit", "--chat", "-100123", "--chat", R],
    ],
    [
      "numeric safe flags kept, odd shapes redacted",
      ["worker", "--limit", "10", "--interval-ms", "5000", "--limit", "x"],
      ["worker", "--limit", "10", "--interval-ms", "5000", "--limit", R],
    ],
    [
      "since durations kept",
      ["events", "tail", "--since", "15m", "--dry-run"],
      ["events", "tail", "--since", "15m", "--dry-run"],
    ],
    [
      "= forms follow the same rules",
      ["audit", "--chat=-100123", "--api-hash=abc", "--file=/a/b.json"],
      ["audit", "--chat=-100123", `--api-hash=${R}`, "--file=b.json"],
    ],
    [
      "unknown flag keeps its name; next positional redacted",
      ["audit", "--mystery", "value", "--json"],
      ["audit", "--mystery", R, "--json"],
    ],
    [
      "everything after -- is redacted",
      ["health", "--", "--limit", "5"],
      ["health", "--", R, R],
    ],
    [
      "command words after a value are redacted",
      ["profile", "show", "--profile", "x", "list"],
      ["profile", "show", "--profile", R, R],
    ],
    [
      "trailing value flag without a value",
      ["profile", "add", "--password"],
      ["profile", "add", "--password"],
    ],
  ]
  for (const [name, argv, expected] of cases) {
    test(name, () => {
      expect(sanitizeArgv(argv)).toEqual(expected)
    })
  }
})
