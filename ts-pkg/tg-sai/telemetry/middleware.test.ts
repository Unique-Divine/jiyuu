// No Telegram connection anywhere: a real client builds its real mtcute
// NetworkManager with our middlewares, and its private `_call` (the hop to
// the DC) is replaced with a queue of canned results, as in the #42 spike.
import { describe, expect, test } from "bun:test"
import { mkdtempSync, readFileSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

import { createMtcuteRuntimeClient } from "../mtcuteAdapter.ts"
import {
  openTelemetryDb,
  SqliteTelemetrySink,
  type EventRow,
  type TelemetryEvent,
  type TelemetrySink,
} from "./db.ts"
import {
  createTelemetryMiddlewares,
  parseWaitSeconds,
  type ClientTelemetry,
  withTelemetryJob,
  type TelemetryContext,
} from "./middleware.ts"
import {
  floodMaxWaitMs,
  sanitizeArgv,
  startTelemetryRun,
  REDACTED,
} from "./run.ts"

/** In-memory sink: a real array of the events it was given. */
class MemorySink implements TelemetrySink {
  readonly events: TelemetryEvent[] = []
  record(event: TelemetryEvent): void {
    this.events.push(event)
  }
  dropped(): number {
    return 0
  }
  close(): void {}
  of(name: string): TelemetryEvent[] {
    return this.events.filter((e) => e.event === name)
  }
}

/** Sink whose every write throws, to prove calls are unaffected. */
class ThrowingSink implements TelemetrySink {
  writes = 0
  record(): void {
    this.writes += 1
    throw new Error("disk full")
  }
  dropped(): number {
    return this.writes
  }
  close(): void {}
}

const CONTEXT: TelemetryContext = {
  runId: "run-1",
  subsystem: "cli",
  command: "ops sai-find",
  accountId: 42,
  profile: "custody",
  version: "0.0.0-test",
}

const flood = (s: number) => ({
  _: "mt_rpc_error",
  errorCode: 420,
  errorMessage: `FLOOD_WAIT_${s}`,
})

/** Real client network whose transport answers from `results`. */
function makeNetwork(telemetry: ClientTelemetry, results: unknown[]) {
  const client: any = createMtcuteRuntimeClient(
    { apiId: 1, apiHash: "0".repeat(32), storage: ":memory:" },
    telemetry,
  )
  const nm = client._client.mt.network
  const transport: unknown[] = []
  nm._call = async (request: unknown) => {
    transport.push(request)
    return results.length > 1 ? results.shift() : results[0]
  }
  return { nm, transport }
}

function build(
  results: unknown[],
  sink: TelemetrySink = new MemorySink(),
  waits: number[] = [],
) {
  const telemetry: ClientTelemetry = {
    sink,
    context: () => CONTEXT,
    floodWaiter: { onBeforeWait: (_ctx, seconds) => waits.push(seconds) },
  }
  return { ...makeNetwork(telemetry, results), sink, waits }
}

describe("createTelemetryMiddlewares", () => {
  test("wraps mtcute's basic middlewares with outer and inner loggers", () => {
    const mws = createTelemetryMiddlewares(new MemorySink(), () => CONTEXT)
    // outer + floodWaiter + internalErrorsHandler + inner
    expect(mws).toHaveLength(4)
  })

  test("success: started + ok call rows and one attempt row", async () => {
    const sink = new MemorySink()
    const { nm } = build([{ _: "config", thisDc: 2 }], sink)
    const res = await nm.call({ _: "help.getConfig" })
    expect(res).toEqual({ _: "config", thisDc: 2 })

    expect(sink.events.map((e) => [e.event, e.outcome])).toEqual([
      ["tg.rpc.call", "started"],
      ["tg.rpc.attempt", "ok"],
      ["tg.rpc.call", "ok"],
    ])
    const [started, attempt, done] = sink.events
    expect(started).toMatchObject({
      runId: "run-1",
      subsystem: "cli",
      command: "ops sai-find",
      accountId: 42,
      profile: "custody",
      method: "help.getConfig",
      req: { _: "help.getConfig" },
    })
    expect(started?.requestId).toBeString()
    expect(attempt?.requestId).toBe(started?.requestId)
    expect(done?.requestId).toBe(started?.requestId)
    expect(attempt?.attempt).toBe(1)
    expect(done).toMatchObject({ attempt: 1, res: { _: "config", thisDc: 2 } })
    expect(done?.durationMs).toBeNumber()
  })

  test("short flood: two attempts and one wait row, linked", async () => {
    const sink = new MemorySink()
    const { nm, waits } = build([flood(1), { _: "config" }], sink)
    expect(await nm.call({ _: "help.getConfig" })).toEqual({ _: "config" })

    const attempts = sink.of("tg.rpc.attempt")
    expect(attempts.map((e) => [e.attempt, e.outcome, e.waitS])).toEqual([
      [1, "error", 1],
      [2, "ok", null],
    ])
    const floodRows = sink.of("tg.flood.wait")
    expect(floodRows).toHaveLength(1)
    expect(floodRows[0]).toMatchObject({ waitS: 1, method: "help.getConfig" })
    const ids = new Set(sink.events.map((e) => e.requestId))
    expect(ids.size).toBe(1)
    expect(sink.of("tg.rpc.call").at(-1)).toMatchObject({
      outcome: "ok",
      attempt: 2,
    })
    // The caller's hook still runs after the wait row.
    expect(waits).toEqual([1])
  }, 6000)

  test("long flood: error completion with wait_s, no wait row", async () => {
    const sink = new MemorySink()
    const { nm, waits } = build([flood(15382)], sink)
    expect(await nm.call({ _: "channels.inviteToChannel" })).toEqual(
      flood(15382),
    )
    expect(sink.of("tg.flood.wait")).toEqual([])
    expect(waits).toEqual([])
    const done = sink.of("tg.rpc.call").at(-1)
    expect(done).toMatchObject({
      outcome: "error",
      errorCode: 420,
      err: "FLOOD_WAIT_15382",
      waitS: 15382,
      attempt: 1,
      req: null,
    })
  })

  test("stored flood: synthetic error with zero attempts", async () => {
    const sink = new MemorySink()
    const { nm, transport } = build([flood(15382)], sink)
    await nm.call({ _: "channels.inviteToChannel" })
    sink.events.length = 0

    const res: any = await nm.call({ _: "channels.inviteToChannel" })
    expect(res.errorMessage).toMatch(/^FLOOD_WAIT_1538[12]$/)
    expect(transport).toHaveLength(1)
    expect(sink.of("tg.rpc.attempt")).toEqual([])
    const done = sink.of("tg.rpc.call").at(-1)
    expect(done).toMatchObject({
      outcome: "error",
      attempt: 0,
      req: { synthetic: true },
    })
    expect(done?.waitS).toBeGreaterThan(15000)
  })

  test("transport throw: error rows, rethrown unchanged", async () => {
    const sink = new MemorySink()
    const { nm } = build([], sink)
    const boom = new Error("connection reset")
    nm._call = async () => {
      throw boom
    }
    await expect(nm.call({ _: "help.getConfig" })).rejects.toBe(boom)
    expect(sink.of("tg.rpc.attempt")[0]).toMatchObject({
      outcome: "error",
      err: "Error: connection reset",
    })
    expect(sink.of("tg.rpc.call").at(-1)).toMatchObject({ outcome: "error" })
  })

  test("timeout errors are classified as timeout", async () => {
    const sink = new MemorySink()
    const { nm } = build([], sink)
    nm._call = async () => {
      throw Object.assign(new Error("Request timed out"), {
        name: "MtTimeoutError",
      })
    }
    await expect(nm.call({ _: "help.getConfig" })).rejects.toThrow()
    expect(sink.of("tg.rpc.call").at(-1)?.outcome).toBe("timeout")
  })

  test("a throwing sink changes neither result nor error", async () => {
    const sink = new ThrowingSink()
    const ok = build([{ _: "config" }], sink)
    expect(await ok.nm.call({ _: "help.getConfig" })).toEqual({ _: "config" })
    expect(sink.writes).toBeGreaterThan(0)

    const failing = build([flood(15382)], sink)
    expect(await failing.nm.call({ _: "help.getConfig" })).toEqual(
      flood(15382),
    )

    const { nm } = makeNetwork(
      {
        sink: new MemorySink(),
        context: () => {
          throw new Error("no context")
        },
      },
      [{ _: "config" }],
    )
    expect(await nm.call({ _: "help.getConfig" })).toEqual({ _: "config" })
  })

  test("withTelemetryJob tags rows with the job ID", async () => {
    const sink = new MemorySink()
    const { nm } = build([{ _: "config" }], sink)
    await withTelemetryJob("setup-7", () => nm.call({ _: "help.getConfig" }))
    await nm.call({ _: "help.getConfig" })
    const jobs = sink.events.map((e) => e.jobId)
    expect(jobs.slice(0, 3)).toEqual(["setup-7", "setup-7", "setup-7"])
    expect(jobs.slice(3)).toEqual([null, null, null])
  })

  test("auth methods record the method name only", async () => {
    const sink = new MemorySink()
    const { nm } = build([{ _: "auth.authorization" }], sink)
    await nm.call({
      _: "auth.checkPassword",
      password: { _: "inputCheckPasswordSRP", A: new Uint8Array(8) },
    })
    expect(sink.events[0]?.req).toEqual({ _: "auth.checkPassword" })
  })
})

describe("parseWaitSeconds", () => {
  test("reads flood and slowmode waits only", () => {
    expect(parseWaitSeconds("FLOOD_WAIT_37")).toBe(37)
    expect(parseWaitSeconds("FLOOD_PREMIUM_WAIT_5")).toBe(5)
    expect(parseWaitSeconds("SLOWMODE_WAIT_10")).toBe(10)
    expect(parseWaitSeconds("CHAT_ADMIN_REQUIRED")).toBeNull()
    expect(parseWaitSeconds(null)).toBeNull()
  })
})

describe("run helpers", () => {
  test("sanitizeArgv drops secret values", () => {
    expect(
      sanitizeArgv([
        "profile",
        "add",
        "--password",
        "hunter2",
        "--api-hash=abc",
        "--session-string",
        "1BVtsOK8Bu0aaaa",
        "--name",
        "custody",
        "x".repeat(40),
      ]),
    ).toEqual([
      "profile",
      "add",
      "--password",
      REDACTED,
      `--api-hash=${REDACTED}`,
      "--session-string",
      REDACTED,
      "--name",
      "custody",
      REDACTED,
    ])
  })

  test("flood max wait: subsystem defaults and env override", () => {
    expect(floodMaxWaitMs("cli", {})).toBe(10000)
    expect(floodMaxWaitMs("worker", {})).toBe(30000)
    expect(floodMaxWaitMs("cli", { TG_SAI_FLOOD_MAX_WAIT_MS: "2500" })).toBe(
      2500,
    )
    expect(floodMaxWaitMs("worker", { TG_SAI_FLOOD_MAX_WAIT_MS: "x" })).toBe(
      30000,
    )
  })
})

describe("createMtcuteRuntimeClient with telemetry", () => {
  test("rows land in a temp DB and the JSONL run log share ids", async () => {
    const dir = mkdtempSync(join(tmpdir(), "tg-sai-telemetry-client-"))
    try {
      const db = openTelemetryDb(join(dir, "telemetry.db"))
      const run = startTelemetryRun({
        subsystem: "cli",
        command: "audit",
        argv: ["audit", "--password", "secret-value"],
        version: "0.0.0-test",
        env: { TG_SAI_DIR: dir },
        sink: new SqliteTelemetrySink(db, { version: "0.0.0-test" }),
      })
      expect(run.floodMaxWaitMs).toBe(10000)
      const client: any = createMtcuteRuntimeClient(
        {
          apiId: 1,
          apiHash: "0".repeat(32),
          storage: join(dir, "session"),
        },
        run.client({ accountId: 99, profile: "p" }),
      )
      const network = client._client.mt.network
      network._call = async () => ({ _: "config" })
      expect(await network.call({ _: "help.getConfig" })).toEqual({
        _: "config",
      })
      run.end({ exitCode: 0 })

      const check = openTelemetryDb(join(dir, "telemetry.db"))
      const rows = check
        .query<EventRow, []>("SELECT * FROM event ORDER BY ts_ms, event_id")
        .all()
      check.close()
      expect(rows.map((r) => r.event)).toEqual([
        "run.start",
        "tg.rpc.call",
        "tg.rpc.attempt",
        "tg.rpc.call",
        "run.end",
      ])
      expect(new Set(rows.map((r) => r.run_id)).size).toBe(1)
      expect(rows[0]?.req).not.toContain("secret-value")
      expect(rows[1]).toMatchObject({ account_id: 99, profile: "p" })
      expect(rows[4]).toMatchObject({ outcome: "ok", error_code: 0 })

      const lines = readFileSync(run.jsonlPath!, "utf8")
        .trim()
        .split("\n")
        .map((line) => JSON.parse(line))
      expect(run.jsonlPath).toBe(join(dir, "logs", "audit-00.jsonl"))
      expect(lines.map((l) => l.event_id)).toEqual(
        rows.map((r) => r.event_id),
      )
      expect(lines[1].request_id).toBe(rows[1]?.request_id)
    } finally {
      rmSync(dir, { recursive: true, force: true })
    }
  })

  test("without telemetry the client keeps mtcute's default middlewares", () => {
    const client: any = createMtcuteRuntimeClient({
      apiId: 1,
      apiHash: "0".repeat(32),
      storage: ":memory:",
    })
    expect(client._client.mt.network.params.middlewares).toBeUndefined()
  })
})
