import { describe, expect, test } from "bun:test"

import {
  buildCustodyStatus,
  custodyStatusKey,
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
    // One background add and two setup adds: the account's adds today.
    queue.recordAdds({
      source: "background",
      outcome: "added",
      chatId: -100,
      userIds: [4],
      nowMs: NOW,
    })
    queue.recordAdds({
      source: "setup",
      outcome: "added",
      chatId: -200,
      userIds: [7, 8],
      nowMs: NOW,
    })
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
      addsToday: 3,
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
    expect("groupCount" in report).toBe(false)
  })
  test("a counted group total rides on the report and its change key", () => {
    const { holds, queue } = stores()
    const input = {
      accountId: ACCOUNT,
      workerVersion: "0.2.0",
      holds,
      queue,
      nowMs: NOW,
    }
    const counted = buildCustodyStatus({ ...input, groupCount: 212 })
    expect(counted.groupCount).toBe(212)
    expect(custodyStatusKey(counted)).not.toBe(
      custodyStatusKey(buildCustodyStatus({ ...input, groupCount: 213 })),
    )
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
    for (const stderr of [
      "error: invalid value 'custody-status' for '<ACTION>'\n",
      "Unknown field chatSetupCustodyStatus",
      'Unknown field "chatSetupCustodyStatus" on type "MutationRoot"',
      'Cannot query field "chatSetupCustodyStatus" on type "Mutation"',
      // Deal Desk's JSON error output escapes the quotes.
      '{"error":"Unknown field \\"chatSetupCustodyStatus\\" on type \\"MutationRoot\\""}',
    ])
      expect(isUnknownOperationOutput("custody-status", stderr)).toBe(true)
    expect(
      isUnknownOperationOutput("custody-status", "error: capability denied"),
    ).toBe(false)
  })
  test("an unknown field in the report is a send failure, not an old desk", () => {
    for (const stderr of [
      // serde `deny_unknown_fields` on the report or a child.
      "Error: unknown field `extra`, expected one of `accountId`, `holds`",
      // GraphQL input validation, prefixed by the mutation path.
      '[chatSetupCustodyStatus] Invalid value for argument "input", ' +
        'unknown field "extra" of type "CustodyStatusReport"',
      // An unknown field on another operation's mutation.
      'Unknown field "chatSetupProgress" on type "MutationRoot"',
    ])
      expect(isUnknownOperationOutput("custody-status", stderr)).toBe(false)
  })
})
