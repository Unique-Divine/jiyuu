import { describe, expect, test } from "bun:test"

import {
  buildCustodyStatus,
  CUSTODY_STATUS_INTERVAL_MS,
  CustodyStatusReporter,
  UNKNOWN_OP_LOG_MS,
} from "./custodyStatus"
import type { CustodyStatusReport } from "./custodyStatus"
import {
  CliProvisionCloud,
  isUnknownOperationOutput,
  UnknownCloudOperationError,
} from "./provisioningCloud"
import { BackgroundQueue } from "./state/background"
import { openStateDb } from "./state/db"
import { HoldStore } from "./state/holds"
import packageJson from "./package.json" with { type: "json" }

const ACCOUNT = 8274304175
const NOW = Date.UTC(2026, 10, 6, 5)

function stores() {
  const db = openStateDb(":memory:")
  return { holds: HoldStore.fromDb(db), queue: new BackgroundQueue(db) }
}

describe("custody status report", () => {
  test("payload matches the Deal Desk contract", () => {
    const { holds, queue } = stores()
    holds.recordHold({
      accountId: ACCOUNT,
      grp: "invite",
      method: "channels.inviteToChannel",
      source: "flood_wait",
      waitS: 15382,
      nowMs: NOW,
    })
    queue.enqueue({
      requestId: "r",
      chatId: -100,
      userIds: [1, 2, 3, 4],
      notBeforeMs: NOW + 1_200_000,
    })
    queue.finish(-100, 4, "added", null, NOW)
    const report = buildCustodyStatus({
      accountId: ACCOUNT,
      workerVersion: packageJson.version,
      holds,
      queue,
      nowMs: NOW,
    })
    expect(report).toEqual({
      accountId: "8274304175",
      workerVersion: packageJson.version,
      observedAt: "2026-11-06T05:00:00.000Z",
      holds: [
        {
          group: "invite",
          untilMs: expect.any(Number),
          source: "flood_wait",
          method: "channels.inviteToChannel",
          waitS: 15382,
        },
      ],
      addsToday: 1,
      background: { pending: 3, nextAt: "2026-11-06T05:20:00.000Z" },
    })
    expect(report.holds[0]!.untilMs).toBeGreaterThan(NOW + 15382 * 1000)
  })
  test("an empty queue reports no next time", () => {
    const { holds, queue } = stores()
    const report = buildCustodyStatus({
      accountId: ACCOUNT,
      workerVersion: "0.2.0",
      holds,
      queue,
      nowMs: NOW,
    })
    expect(report.holds).toEqual([])
    expect(report.background).toEqual({ pending: 0, nextAt: null })
  })
  test("the package version is 0.2.0", () => {
    expect(packageJson.version).toBe("0.2.0")
  })
})

describe("custody status cadence", () => {
  function reporter(
    options: {
      send?: (report: CustodyStatusReport) => Promise<unknown>
    } = {},
  ) {
    let pending = 1
    const sent: CustodyStatusReport[] = []
    const logs: string[] = []
    const r = new CustodyStatusReporter({
      build: (nowMs) => ({
        accountId: String(ACCOUNT),
        workerVersion: "0.2.0",
        observedAt: new Date(nowMs).toISOString(),
        holds: [],
        addsToday: 0,
        background: { pending, nextAt: null },
      }),
      send:
        options.send ??
        (async (report) => {
          sent.push(report)
        }),
      log: (level, msg) => logs.push(`${level}:${msg}`),
    })
    return {
      r,
      sent,
      logs,
      setPending: (n: number) => {
        pending = n
      },
    }
  }
  test("sends on change and at least every 5 minutes", async () => {
    const f = reporter()
    expect(await f.r.tick(NOW)).toBe(true)
    expect(await f.r.tick(NOW + 5_000)).toBe(false)
    f.setPending(2)
    expect(await f.r.tick(NOW + 10_000)).toBe(true)
    expect(await f.r.tick(NOW + 10_000 + CUSTODY_STATUS_INTERVAL_MS - 1)).toBe(
      false,
    )
    expect(await f.r.tick(NOW + 10_000 + CUSTODY_STATUS_INTERVAL_MS)).toBe(true)
    expect(f.sent.map((s) => s.background.pending)).toEqual([1, 2, 2])
  })
  test("an unknown op is logged once an hour and never thrown", async () => {
    const f = reporter({
      send: async () => {
        throw new UnknownCloudOperationError("custody-status")
      },
    })
    let now = NOW
    for (let i = 0; i < 13; i++) {
      expect(await f.r.tick(now)).toBe(false)
      now += CUSTODY_STATUS_INTERVAL_MS
    }
    // 13 tries over an hour: logs at 0 and 60 min.
    expect(now - NOW).toBe(UNKNOWN_OP_LOG_MS + CUSTODY_STATUS_INTERVAL_MS)
    expect(f.logs).toEqual([
      "warn:deal desk lacks custody-status",
      "warn:deal desk lacks custody-status",
    ])
  })
  test("send and build failures log a code and wait for the next change", async () => {
    const f = reporter({
      send: async () => {
        throw new Error("secret stderr")
      },
    })
    expect(await f.r.tick(NOW)).toBe(false)
    expect(await f.r.tick(NOW + 5_000)).toBe(false)
    expect(f.logs).toEqual(["warn:custody status send failed"])
    const broken = new CustodyStatusReporter({
      build: () => {
        throw new Error("db")
      },
      send: async () => {},
    })
    expect(await broken.tick(NOW)).toBe(false)
  })
  test("cloud adapter sends the report as the custody-status op", async () => {
    const calls: [string, Record<string, unknown>][] = []
    const cloud = new CliProvisionCloud(async (operation, input) => {
      calls.push([operation, input])
      return { ok: true }
    })
    const { holds, queue } = stores()
    const report = buildCustodyStatus({
      accountId: ACCOUNT,
      workerVersion: "0.2.0",
      holds,
      queue,
      nowMs: NOW,
    })
    await cloud.custodyStatus(report)
    expect(calls).toEqual([["custody-status", { ...report }]])
  })
  test("unknown-op detection reads clap and GraphQL refusals", () => {
    expect(
      isUnknownOperationOutput(
        "custody-status",
        "error: invalid value 'custody-status' for '<ACTION>'\n",
      ),
    ).toBe(true)
    expect(
      isUnknownOperationOutput("custody-status", "Unknown field chatSetupCustodyStatus"),
    ).toBe(true)
    expect(
      isUnknownOperationOutput("custody-status", "error: capability denied"),
    ).toBe(false)
  })
})
