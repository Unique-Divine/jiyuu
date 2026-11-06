import type { Database } from "bun:sqlite"
import { afterAll, beforeAll, describe, expect, test } from "bun:test"
import { mkdtempSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

import {
  countEvents,
  eventStats,
  exportEvents,
  floodReport,
  openTelemetryReadOnly,
  parseCountBy,
  recentRuns,
  showTimeline,
  tailEvents,
  uncertainCalls,
} from "./queries"
import {
  CUSTODY_ACCOUNT,
  IDS,
  NOW_MS,
  PERSONAL_ACCOUNT,
  seedEvents,
  writeSeedDb,
} from "./testEvents"

const DAY = 86_400_000
let dir = ""
let path = ""
let db: Database

beforeAll(() => {
  dir = mkdtempSync(join(tmpdir(), "tg-sai-queries-"))
  path = join(dir, "telemetry.db")
  writeSeedDb(path)
  db = openTelemetryReadOnly(path)
})

afterAll(() => {
  db.close()
  rmSync(dir, { recursive: true, force: true })
})

describe("openTelemetryReadOnly", () => {
  test("rejects writes", () => {
    expect(() => db.run("DELETE FROM event")).toThrow()
  })

  test("does not create a missing database", () => {
    expect(() => openTelemetryReadOnly(join(dir, "missing.db"))).toThrow()
  })
})

describe("tailEvents", () => {
  test("is newest first and shows each call once", () => {
    const rows = tailEvents(db, { limit: 100 })
    const ts = rows.map((r) => r.ts_ms)
    expect(ts).toEqual([...ts].sort((a, b) => b - a))
    const flooded = rows.filter(
      (r) => r.request_id === IDS.reqFlooded && r.event === "tg.rpc.call",
    )
    expect(flooded.map((r) => r.outcome)).toEqual(["ok"])
    // The uncertain call has no completion, so its started row stays.
    const uncertain = rows.filter(
      (r) => r.request_id === IDS.reqUncertain && r.event === "tg.rpc.call",
    )
    expect(uncertain.map((r) => r.outcome)).toEqual(["started"])
    expect(rows.some((r) => r.event.startsWith("run."))).toBe(false)
    expect(rows.some((r) => r.event === "worker.tick")).toBe(false)
  })

  test("applies limit, account, method, run, errors, and since", () => {
    expect(tailEvents(db, { limit: 2 })).toHaveLength(2)
    const custody = tailEvents(db, { limit: 100, accountId: CUSTODY_ACCOUNT })
    expect(custody.length).toBeGreaterThan(0)
    expect(custody.every((r) => r.account_id === CUSTODY_ACCOUNT)).toBe(true)
    const method = tailEvents(db, {
      limit: 100,
      method: "channels.editAdmin",
    })
    expect(method.map((r) => r.event)).toEqual([
      "tg.rpc.call",
      "tg.rpc.attempt",
    ])
    const run = tailEvents(db, { limit: 100, runId: IDS.runWorker })
    expect(run.every((r) => r.run_id === IDS.runWorker)).toBe(true)
    const errors = tailEvents(db, { limit: 100, errorsOnly: true })
    expect(
      errors.every(
        (r) =>
          ["error", "timeout", "aborted"].includes(r.outcome) ||
          r.event === "tg.flood.wait",
      ),
    ).toBe(true)
    expect(errors.map((r) => r.err)).toContain("CHAT_ADMIN_REQUIRED")
    const all = tailEvents(db, { limit: 100 })
    const week = tailEvents(db, { limit: 100, sinceMs: NOW_MS - 7 * DAY })
    expect(all.some((r) => r.request_id === IDS.reqOld)).toBe(true)
    expect(week.some((r) => r.request_id === IDS.reqOld)).toBe(false)
  })
})

describe("showTimeline", () => {
  test("links call, attempts, and flood wait by request_id in order", () => {
    const rows = showTimeline(db, IDS.reqFlooded)
    expect(rows.map((r) => [r.event, r.outcome, r.attempt])).toEqual([
      ["tg.rpc.call", "started", null],
      ["tg.rpc.attempt", "error", 1],
      ["tg.flood.wait", "error", null],
      ["tg.rpc.attempt", "ok", 2],
      ["tg.rpc.call", "ok", null],
    ])
  })

  test("matches run_id and job_id", () => {
    const run = showTimeline(db, IDS.runCli)
    expect(run[0]?.event).toBe("run.start")
    expect(run.at(-1)?.event).toBe("run.end")
    expect(run).toHaveLength(10)
    const job = showTimeline(db, IDS.jobWorker)
    expect(job.every((r) => r.job_id === IDS.jobWorker)).toBe(true)
    expect(job.map((r) => r.event)).toContain("tg.flood.wait")
  })

  test("returns nothing for an unknown id", () => {
    expect(showTimeline(db, "nope")).toEqual([])
  })
})

describe("countEvents", () => {
  test("attempt layer counts requests sent, grouped by method,outcome", () => {
    const rows = countEvents(db, {
      sinceMs: NOW_MS - DAY,
      by: parseCountBy("method,outcome"),
      layer: "attempt",
    })
    expect(rows).toContainEqual({
      method: "messages.getDialogs",
      outcome: "error",
      count: 1,
    })
    expect(rows).toContainEqual({
      method: "messages.getDialogs",
      outcome: "ok",
      count: 1,
    })
    // The synthetic flood never sent a request.
    expect(rows.some((r) => r.method === "channels.inviteToChannel")).toBe(
      false,
    )
    const total = rows.reduce((sum, r) => sum + r.count, 0)
    expect(total).toBe(4)
  })

  test("call layer counts finished calls only", () => {
    const rows = countEvents(db, {
      sinceMs: NOW_MS - DAY,
      by: ["outcome"],
      layer: "call",
    })
    expect(rows).toEqual([
      { outcome: "aborted", count: 1 },
      { outcome: "error", count: 1 },
      { outcome: "ok", count: 1 },
    ])
  })

  test("filters by account", () => {
    const rows = countEvents(db, {
      accountId: PERSONAL_ACCOUNT,
      sinceMs: NOW_MS - DAY,
      by: ["account_id"],
      layer: "attempt",
    })
    expect(rows).toEqual([{ account_id: PERSONAL_ACCOUNT, count: 3 }])
  })

  test("parseCountBy maps aliases and rejects unknown fields", () => {
    expect(parseCountBy("account, method,account_id")).toEqual([
      "account_id",
      "method",
    ])
    expect(() => parseCountBy("method;drop")).toThrow(/Unknown --by/)
    expect(() => parseCountBy(" , ")).toThrow()
    expect(() =>
      countEvents(db, { by: ["1; DROP"], layer: "attempt" }),
    ).toThrow()
  })
})

describe("floodReport", () => {
  test("groups waits and reports the synthetic flood as active", () => {
    const report = floodReport(db, { sinceMs: NOW_MS - 7 * DAY, nowMs: NOW_MS })
    expect(report.floods).toContainEqual({
      event: "tg.flood.wait",
      account_id: CUSTODY_ACCOUNT,
      method: "channels.inviteToChannel",
      wait_s: 15_382,
      count: 1,
      latest_ms: NOW_MS - 20 * 60_000 + 5,
    })
    expect(report.floods).toContainEqual(
      expect.objectContaining({
        event: "tg.rpc.attempt",
        method: "messages.getDialogs",
        wait_s: 3,
      }),
    )
    expect(report.active).toEqual([
      {
        account_id: CUSTODY_ACCOUNT,
        method: "channels.inviteToChannel",
        wait_s: 15_382,
        started_ms: NOW_MS - 20 * 60_000 + 5,
        ends_ms: NOW_MS - 20 * 60_000 + 5 + 15_382_000,
        request_id: IDS.reqSynthetic,
      },
    ])
  })

  test("a wait is no longer active after it ends", () => {
    const later = NOW_MS + 5 * 3_600_000
    expect(floodReport(db, { nowMs: later }).active).toEqual([])
  })

  test("filters by account", () => {
    const report = floodReport(db, {
      accountId: PERSONAL_ACCOUNT,
      nowMs: NOW_MS,
    })
    expect(report.floods.every((r) => r.account_id === PERSONAL_ACCOUNT)).toBe(
      true,
    )
    expect(report.active).toEqual([])
  })
})

describe("uncertainCalls", () => {
  test("returns started calls with no completion", () => {
    const rows = uncertainCalls(db, { sinceMs: NOW_MS - 7 * DAY })
    expect(rows.map((r) => r.request_id)).toEqual([IDS.reqUncertain])
    expect(rows[0]?.outcome).toBe("started")
  })
})

describe("recentRuns", () => {
  test("pairs run.start with run.end and counts errors", () => {
    const runs = recentRuns(db, 10)
    expect(runs.map((r) => r.run_id)).toEqual([IDS.runWorker, IDS.runCli])
    const [worker, cli] = runs
    expect(cli).toMatchObject({
      command: "ops sai-find",
      subsystem: "cli",
      outcome: "ok",
      duration_ms: 10_000,
      calls: 2,
      errors: 1,
      attempts: 3,
      ticks: 0,
    })
    expect(worker).toMatchObject({
      command: "custody-worker",
      subsystem: "worker",
      outcome: null,
      ended_ms: null,
      duration_ms: null,
      calls: 1,
      errors: 1,
      attempts: 1,
      ticks: 1,
    })
    expect(recentRuns(db, 1)).toHaveLength(1)
  })
})

describe("eventStats and exportEvents", () => {
  test("stats count every row by kind", () => {
    const seeded = seedEvents()
    const stats = eventStats(db)
    expect(stats.rows).toBe(seeded.length)
    expect(stats.oldest_ms).toBe(NOW_MS - 10 * DAY)
    expect(stats.newest_ms).toBe(NOW_MS - 20 * 60_000 + 1005)
    const byKind = Object.fromEntries(
      stats.kinds.map((k) => [k.event, k.rows]),
    )
    expect(byKind).toEqual({
      "tg.rpc.call": 7,
      "tg.rpc.attempt": 5,
      "tg.flood.wait": 2,
      "run.start": 2,
      "run.end": 1,
      "worker.tick": 1,
    })
  })

  test("export streams oldest first within the window", () => {
    const all = [...exportEvents(db)]
    expect(all).toHaveLength(seedEvents().length)
    expect(all[0]?.request_id).toBe(IDS.reqOld)
    const week = [...exportEvents(db, { sinceMs: NOW_MS - 7 * DAY })]
    expect(week).toHaveLength(all.length - 1)
    const ts = week.map((r) => r.ts_ms)
    expect(ts).toEqual([...ts].sort((a, b) => a - b))
  })
})
