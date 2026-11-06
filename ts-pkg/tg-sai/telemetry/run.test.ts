import { describe, expect, test } from "bun:test"
import { mkdtempSync, readFileSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

import { NoopSink } from "./db.ts"
import {
  activeTelemetryRun,
  nextJsonlName,
  startTelemetryRun,
  telemetryJsonlFields,
} from "./run.ts"

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
        err: "Error: FLOOD_WAIT_15382",
      })
    })
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
