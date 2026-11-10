// No Telegram connection anywhere: a real client builds its real mtcute
// NetworkManager with our middlewares, and its private `_call` (the hop to
// the DC) is replaced with a queue of canned results, as in the #42 spike.
import { afterEach, beforeEach, describe, expect, test } from "bun:test"
import { mkdtempSync, readdirSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

import { Long, MtPeerNotFoundError, MtTimeoutError, tl } from "@mtcute/bun"

import {
  clientMiddlewares,
  closeProcessHoldStores,
  addTargetToChat,
  createMtcuteRuntimeClient,
  NO_FLOOD_SLEEP,
  promoteExistingMember,
  promoteTargetAdmin,
  sessionAccountId,
} from "../mtcuteAdapter"
import { HoldAccountUnknownError, HoldError } from "../state/guard"
import { HoldStore } from "../state/holds"
import { stateDbPath } from "../state/db"
import {
  openTelemetryDb,
  SqliteTelemetrySink,
  type EventRow,
  type TelemetryEvent,
  type TelemetrySink,
} from "./db"
import {
  createTelemetryMiddlewares,
  parseWaitSeconds,
  rpcErrorText,
  safeErrorText,
  type ClientTelemetry,
  withTelemetryJob,
  type TelemetryContext,
} from "./middleware"
import {
  floodMaxWaitMs,
  sanitizeArgv,
  startTelemetryRun,
  REDACTED,
} from "./run"

// Every client now carries the flood guard, whose store is `state.db` under
// TG_SAI_DIR. Each test gets its own directory so holds never leak between
// tests or into the real config dir.
const savedTgSaiDir = process.env.TG_SAI_DIR
let stateDir: string

beforeEach(() => {
  closeProcessHoldStores()
  stateDir = mkdtempSync(join(tmpdir(), "tg-sai-mw-state-"))
  process.env.TG_SAI_DIR = stateDir
})

afterEach(() => {
  closeProcessHoldStores()
  if (savedTgSaiDir === undefined) delete process.env.TG_SAI_DIR
  else process.env.TG_SAI_DIR = savedTgSaiDir
  rmSync(stateDir, { recursive: true, force: true })
})

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

/**
 * Real client network whose transport answers from `results`. The flood
 * guard keys by `accountId` (default 42, as in `CONTEXT`); `null` leaves
 * it to the session's getMe.
 */
function makeNetwork(
  telemetry: ClientTelemetry,
  results: unknown[],
  accountId: number | null = 42,
) {
  const client: any = createMtcuteRuntimeClient(
    {
      apiId: 1,
      apiHash: "0".repeat(32),
      storage: ":memory:",
      ...(accountId === null ? {} : { accountId }),
    },
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
    // No account and a send (not invite/create): the guard passes through,
    // so mtcute's stored wait answers.
    const sink = new MemorySink()
    const { nm, transport } = makeNetwork(
      { sink, context: () => ({ ...CONTEXT, accountId: null }) },
      [flood(15382)],
      null,
    )
    await nm.call({ _: "messages.sendMessage" })
    sink.events.length = 0

    const res: any = await nm.call({ _: "messages.sendMessage" })
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
      err: "Error",
    })
    expect(sink.of("tg.rpc.call").at(-1)).toMatchObject({ outcome: "error" })
  })

  test("timeout errors are classified as timeout", async () => {
    const sink = new MemorySink()
    const { nm } = build([], sink)
    nm._call = async () => {
      throw new MtTimeoutError()
    }
    await expect(nm.call({ _: "help.getConfig" })).rejects.toThrow()
    expect(sink.of("tg.rpc.call").at(-1)).toMatchObject({
      outcome: "timeout",
      err: "MtTimeoutError",
    })
  })

  test("a message that mentions a timeout is still an error", async () => {
    const sink = new MemorySink()
    const { nm } = build([], sink)
    nm._call = async () => {
      throw new Error("chat title: timed out waiting")
    }
    await expect(nm.call({ _: "help.getConfig" })).rejects.toThrow()
    expect(sink.of("tg.rpc.call").at(-1)).toMatchObject({
      outcome: "error",
      err: "Error",
    })
  })

  test("calls cut short by client shutdown are aborted", async () => {
    const sink = new MemorySink()
    const { nm } = build([], sink)
    const controller = new AbortController()
    nm.params.stopSignal = controller.signal
    nm._call = async () => {
      controller.abort()
      throw new Error("connection closed")
    }
    await expect(nm.call({ _: "help.getConfig" })).rejects.toThrow()
    expect(sink.of("tg.rpc.call").at(-1)?.outcome).toBe("aborted")
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
      null,
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

describe("safeErrorText", () => {
  const rpc = (errorCode: number, errorMessage: string) =>
    tl.RpcError.fromTl({ _: "mt_rpc_error", errorCode, errorMessage })

  test("keeps the class and Telegram code, drops free text", () => {
    const cases: [unknown, string][] = [
      [rpc(420, "FLOOD_WAIT_400"), "RpcError [420 FLOOD_WAIT_400]"],
      [rpc(400, "PEER_FLOOD"), "RpcError [400 PEER_FLOOD]"],
      [rpc(420, "SLOWMODE_WAIT_30"), "RpcError [420 SLOWMODE_WAIT_30]"],
      [
        new Error("Missing chatId in JSONL input line: t.me/+abc"),
        "Error",
      ],
      [new MtPeerNotFoundError("Peer @alice not found"), "MtPeerNotFoundError"],
      [new MtTimeoutError(), "MtTimeoutError"],
      [new Error("Unknown profile: x"), "Error"],
      [new TypeError("bad @alice"), "TypeError"],
      [
        Object.assign(new Error("Participant privacy restriction"), {
          text: "USER_PRIVACY_RESTRICTED",
        }),
        "Error [USER_PRIVACY_RESTRICTED]",
      ],
      [new DOMException("stopped", "AbortError"), "AbortError"],
      ["free text", "string"],
      [null, "object"],
    ]
    for (const [error, text] of cases) {
      expect(safeErrorText(error)).toBe(text)
    }
  })

  test("rpcErrorText reads codes from text, seconds, or the message", () => {
    expect(rpcErrorText(rpc(420, "FLOOD_WAIT_7"))).toBe("FLOOD_WAIT_7")
    expect(rpcErrorText(new Error("Telegram API error 400: CHAT_INVALID")))
      .toBe("CHAT_INVALID")
    expect(rpcErrorText("FLOOD_WAIT_9")).toBe("FLOOD_WAIT_9")
    expect(rpcErrorText(new Error("flood wait 9"))).toBeNull()
  })

  test("a thrown RpcError row carries only the code", async () => {
    const sink = new MemorySink()
    const { nm } = build([], sink)
    nm._call = async () => {
      throw rpc(400, "CHAT_ADMIN_REQUIRED")
    }
    await expect(nm.call({ _: "help.getConfig" })).rejects.toThrow()
    expect(sink.of("tg.rpc.call").at(-1)).toMatchObject({
      outcome: "error",
      errorCode: 400,
      err: "RpcError [400 CHAT_ADMIN_REQUIRED]",
    })
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
  test("sanitizeArgv redacts credential values", () => {
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
      REDACTED,
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
  test("rows land in a temp DB with one run id", async () => {
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

      // JSONL run logs were removed; the DB is the only record.
      expect(readdirSync(dir)).not.toContain("logs")
    } finally {
      rmSync(dir, { recursive: true, force: true })
    }
  })

  test("without telemetry the client gets the guard around basic()", () => {
    const client: any = createMtcuteRuntimeClient({
      apiId: 1,
      apiHash: "0".repeat(32),
      storage: ":memory:",
    })
    // check + floodWaiter + internalErrorsHandler + record
    expect(client._client.mt.network.params.middlewares).toHaveLength(4)
    expect(clientMiddlewares(undefined, undefined)).toHaveLength(4)
    // With telemetry: outer + check + 2 basic + inner + record
    expect(
      clientMiddlewares(undefined, {
        sink: new MemorySink(),
        context: () => CONTEXT,
      }),
    ).toHaveLength(6)
  })
})

/** Real client whose transport answers from `results`; no telemetry. */
function plainNetwork(results: unknown[], accountId?: number) {
  const client: any = createMtcuteRuntimeClient({
    apiId: 1,
    apiHash: "0".repeat(32),
    storage: ":memory:",
    ...(accountId === undefined ? {} : { accountId }),
  })
  const nm = client._client.mt.network
  const transport: unknown[] = []
  nm._call = async (request: unknown) => {
    transport.push(request)
    return results.length > 1 ? results.shift() : results[0]
  }
  return { nm, transport }
}

const INVITE = { _: "channels.inviteToChannel" }

function savedHolds(): ReturnType<HoldStore["activeHolds"]> {
  const store = HoldStore.open(stateDbPath())
  try {
    return store.activeHolds()
  } finally {
    store.close()
  }
}

describe("flood guard through a real NetworkManager", () => {
  test("a saved FLOOD_WAIT_400 refuses the call in a new client", async () => {
    const first = build([flood(400)])
    expect(await first.nm.call(INVITE)).toEqual(flood(400))
    expect(first.transport).toHaveLength(1)
    const holds = savedHolds()
    expect(holds).toHaveLength(1)
    expect(holds[0]).toMatchObject({
      accountId: "42",
      grp: "invite",
      method: "channels.inviteToChannel",
      waitS: 400,
      source: "flood_wait",
    })
    const startedId = (first.sink as MemorySink).of("tg.rpc.call")[0]
      ?.requestId
    expect(holds[0]?.requestId).toBe(startedId!)

    // Like a restart: drop the open store; the next client reopens state.db.
    closeProcessHoldStores()
    const sink = new MemorySink()
    const second = build([{ _: "updates" }], sink)
    const error = await second.nm.call(INVITE).catch((e: unknown) => e)
    expect(error).toBeInstanceOf(HoldError)
    expect((error as HoldError).message).toMatch(/^TG_SAI_HOLD_\d+$/)
    expect((error as HoldError).seconds).toBeGreaterThan(400)
    expect(second.transport).toEqual([])

    expect(sink.of("tg.rpc.attempt")).toEqual([])
    const done = sink.of("tg.rpc.call").at(-1)
    expect(done).toMatchObject({
      outcome: "error",
      attempt: 0,
      errorCode: 420,
      method: "channels.inviteToChannel",
    })
    expect(done?.err).toMatch(/^HoldError \[420 TG_SAI_HOLD_\d+\]$/)
  })

  test("a short FLOOD_WAIT_3 on sendMessage is slept, not saved", async () => {
    const { nm, transport, waits } = build([flood(3), { _: "updates" }])
    expect(await nm.call({ _: "messages.sendMessage" })).toEqual({
      _: "updates",
    })
    expect(waits).toEqual([3])
    expect(transport).toHaveLength(2)
    expect(savedHolds()).toEqual([])
  }, 8000)

  test("works without telemetry, keyed by config.accountId", async () => {
    const first = plainNetwork([flood(400)], 7)
    expect(await first.nm.call(INVITE)).toEqual(flood(400))
    expect(savedHolds()).toMatchObject([
      { accountId: "7", grp: "invite", requestId: null },
    ])

    closeProcessHoldStores()
    const second = plainNetwork([{ _: "updates" }], 7)
    await expect(second.nm.call(INVITE)).rejects.toBeInstanceOf(HoldError)
    expect(second.transport).toEqual([])

    // Another account goes through.
    const other = plainNetwork([{ _: "updates" }], 8)
    expect(await other.nm.call(INVITE)).toEqual({ _: "updates" })
  })

  test("the telemetry context's account never keys holds", async () => {
    // CONTEXT says 42 (e.g. a profile's saved userId); the session is 77.
    const sink = new MemorySink()
    const { client, nm, transport } = sessionNetwork(77, [flood(400)], {
      sink,
      context: () => CONTEXT,
    })
    expect(await nm.call(INVITE)).toEqual(flood(400))
    expect(transport.map((r: any) => r._)).toEqual([
      "users.getUsers",
      "channels.inviteToChannel",
    ])
    expect(sessionAccountId(client)).toBe(77)
    expect(savedHolds().map((h) => h.accountId)).toEqual(["77"])
  })

  test("config.accountId wins over the telemetry context", async () => {
    const sink = new MemorySink()
    const client: any = createMtcuteRuntimeClient(
      { apiId: 1, apiHash: "0".repeat(32), storage: ":memory:", accountId: 5 },
      { sink, context: () => CONTEXT },
    )
    const nm = client._client.mt.network
    nm._call = async () => flood(400)
    await nm.call(INVITE)
    expect(savedHolds().map((h) => h.accountId)).toEqual(["5"])
  })

  test("an unverified session refuses invite/create and sends nothing", async () => {
    // getMe answers with an error: the account stays unknown, and each
    // invite/create tries the lookup again.
    const { nm, transport } = plainNetwork([
      { _: "mt_rpc_error", errorCode: 400, errorMessage: "USER_ID_INVALID" },
    ])
    for (const request of [INVITE, { _: "channels.createChannel" }]) {
      const error = await nm.call(request).catch((e: unknown) => e)
      expect(error).toBeInstanceOf(HoldAccountUnknownError)
      expect((error as HoldAccountUnknownError).text).toBe(
        "TG_SAI_HOLD_ACCOUNT_UNKNOWN",
      )
    }
    expect(transport.map((r: any) => r._)).toEqual([
      "users.getUsers",
      "users.getUsers",
    ])
    expect(readdirSync(stateDir)).toEqual([])
  })

  test("a session's own getMe keys its holds", async () => {
    const first = sessionNetwork(77, [flood(400)])
    expect(sessionAccountId(first.client)).toBeNull()
    expect(await first.nm.call(INVITE)).toEqual(flood(400))
    expect(savedHolds()).toMatchObject([{ accountId: "77", grp: "invite" }])

    // getMe already ran: the next invite is refused without another lookup.
    first.transport.length = 0
    await expect(first.nm.call(INVITE)).rejects.toBeInstanceOf(HoldError)
    expect(first.transport).toEqual([])

    // A new client for the same session is refused after its own getMe.
    closeProcessHoldStores()
    const second = sessionNetwork(77, [{ _: "updates" }])
    await expect(second.nm.call(INVITE)).rejects.toBeInstanceOf(HoldError)
    expect(second.transport.map((r: any) => r._)).toEqual(["users.getUsers"])

    // Another session's account is not held.
    const other = sessionNetwork(78, [{ _: "updates" }])
    expect(await other.nm.call(INVITE)).toEqual({ _: "updates" })
  })

  test("other calls pass unguarded until the account is known", async () => {
    const { nm, transport } = plainNetwork([{ _: "updates" }])
    expect(await nm.call({ _: "messages.sendMessage" })).toEqual({
      _: "updates",
    })
    expect(transport.map((r: any) => r._)).toEqual(["messages.sendMessage"])
  })

  test("a client with no account never creates state.db", async () => {
    const { nm } = plainNetwork([flood(400)])
    await nm.call({ _: "messages.sendMessage" })
    await nm.call(INVITE).catch(() => {})
    expect(readdirSync(stateDir)).toEqual([])
  })
})

describe("CLI invite and admin writes never sleep through a flood wait", () => {
  const CHANNEL = { _: "inputPeerChannel", channelId: 1, accessHash: Long.ZERO }
  const USER = { _: "inputPeerUser", userId: 2, accessHash: Long.ZERO }

  /** Real client keyed to account 7 whose transport answers `result`. */
  function writeClient(result: unknown) {
    const client: any = createMtcuteRuntimeClient({
      apiId: 1,
      apiHash: "0".repeat(32),
      storage: ":memory:",
      accountId: 7,
    })
    const transport: unknown[] = []
    client._client.mt.network._call = async (request: unknown) => {
      transport.push(request)
      return result
    }
    return { client, transport }
  }

  test("addTargetToChat surfaces FLOOD_WAIT_3 at once and saves it", async () => {
    const { client, transport } = writeClient(flood(3))
    const started = Date.now()
    const error = await addTargetToChat(client, {
      chatId: CHANNEL as any,
      targetPeer: USER as any,
    }).catch((e: unknown) => e)
    expect(Date.now() - started).toBeLessThan(2_000)
    expect((error as { text?: string }).text).toBe("FLOOD_WAIT_%d")
    expect(transport).toHaveLength(1)
    expect(savedHolds()).toMatchObject([
      { accountId: "7", grp: "invite", waitS: 3 },
    ])
    // The saved hold refuses the next add before the wire.
    await expect(
      addTargetToChat(client, {
        chatId: CHANNEL as any,
        targetPeer: USER as any,
      }),
    ).rejects.toBeInstanceOf(HoldError)
    expect(transport).toHaveLength(1)
  })

  test("promoteTargetAdmin surfaces FLOOD_WAIT_3 at once by default", async () => {
    const { client, transport } = writeClient(flood(3))
    const started = Date.now()
    await expect(
      promoteTargetAdmin(client, {
        chatId: CHANNEL as any,
        targetPeer: USER as any,
        rights: { addAdmins: true },
      }),
    ).rejects.toMatchObject({ text: "FLOOD_WAIT_%d" })
    expect(Date.now() - started).toBeLessThan(2_000)
    expect(transport).toHaveLength(1)
    expect(savedHolds()).toMatchObject([
      { accountId: "7", grp: "method:channels.editAdmin", waitS: 3 },
    ])
  })

  /**
   * Real client keyed to account 7 whose transport answers each request by
   * its method name and records every method sent.
   */
  function methodClient(answers: Record<string, unknown>) {
    const { client } = writeClient(null)
    const methods: string[] = []
    client._client.mt.network._call = async (request: { _: string }) => {
      methods.push(request._)
      if (!(request._ in answers)) throw new Error(`unexpected ${request._}`)
      return answers[request._]
    }
    return { client, methods }
  }

  const TARGET_USER = {
    _: "user",
    id: 2,
    firstName: "Target",
    accessHash: Long.ZERO,
  }
  const participantAnswer = (participant: unknown) => ({
    _: "channels.channelParticipant",
    participant,
    chats: [],
    users: [TARGET_USER],
  })
  const promoteParams = {
    chatId: CHANNEL as any,
    targetPeer: USER as any,
    rights: { addAdmins: true },
  }

  for (const [name, answer] of [
    [
      "USER_NOT_PARTICIPANT",
      {
        _: "mt_rpc_error",
        errorCode: 400,
        errorMessage: "USER_NOT_PARTICIPANT",
      },
    ],
    [
      "channelParticipantLeft",
      participantAnswer({
        _: "channelParticipantLeft",
        peer: { _: "peerUser", userId: 2 },
      }),
    ],
  ] as const) {
    test(`promoteExistingMember refuses on ${name} and never promotes`, async () => {
      const { client, methods } = methodClient({
        "channels.getParticipant": answer,
      })
      await expect(
        promoteExistingMember(client, promoteParams),
      ).rejects.toThrow("Refusing to promote")
      expect(methods).toEqual(["channels.getParticipant"])
      expect(methods).not.toContain("channels.editAdmin")
    })
  }

  test("promoteExistingMember sends one editAdmin for a member", async () => {
    const { client, methods } = methodClient({
      "channels.getParticipant": participantAnswer({
        _: "channelParticipant",
        userId: 2,
        date: 0,
      }),
      "channels.editAdmin": {
        _: "updates",
        updates: [],
        users: [],
        chats: [],
        date: 0,
        seq: 0,
      },
    })
    await promoteExistingMember(client, { ...promoteParams, expectedUserId: 2 })
    expect(methods).toEqual(["channels.getParticipant", "channels.editAdmin"])
  })

  test("promoteExistingMember refuses a member that is not the planned user", async () => {
    const { client, methods } = methodClient({
      "channels.getParticipant": participantAnswer({
        _: "channelParticipant",
        userId: 2,
        date: 0,
      }),
    })
    await expect(
      promoteExistingMember(client, { ...promoteParams, expectedUserId: 3 }),
    ).rejects.toThrow("resolved to user 2, expected 3")
    expect(methods).not.toContain("channels.editAdmin")
  })

  test("NO_FLOOD_SLEEP sets a zero sleep threshold", () => {
    expect(NO_FLOOD_SLEEP).toEqual({ floodSleepThreshold: 0 })
  })
})

/** The `user` getMe returns for a session acting as `id`. */
function selfUser(id: number) {
  return { _: "user", id, self: true, firstName: "Fixture", accessHash: 1n }
}

/**
 * Real client with no `accountId` whose transport answers getMe with user
 * `selfId` and every other call from `results`.
 */
function sessionNetwork(
  selfId: number,
  results: unknown[],
  telemetry?: ClientTelemetry,
) {
  const client: any = createMtcuteRuntimeClient(
    { apiId: 1, apiHash: "0".repeat(32), storage: ":memory:" },
    telemetry,
  )
  const nm = client._client.mt.network
  const transport: unknown[] = []
  nm._call = async (request: any) => {
    transport.push(request)
    if (request._ === "users.getUsers") return [selfUser(selfId)]
    return results.length > 1 ? results.shift() : results[0]
  }
  return { client, nm, transport }
}
