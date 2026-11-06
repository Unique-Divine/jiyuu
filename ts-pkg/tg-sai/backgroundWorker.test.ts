import { describe, expect, test } from "bun:test"

import { backgroundEnqueue, drainBackgroundOnce } from "./backgroundWorker"
import type { TelegramProvisioner } from "./provisioning"
import { HoldError } from "./state/guard"
import { BackgroundQueue, nextUtcDayMs } from "./state/background"
import type { BackgroundPacing } from "./state/background"
import { openStateDb } from "./state/db"
import { HoldStore } from "./state/holds"

const ACCOUNT = 8274304175
const NOW = Date.UTC(2026, 10, 6, 5)
const PACING: BackgroundPacing = { gapMs: 1_200_000, dailyMax: 6, jitter: 0.3 }

function fixture(
  options: {
    add?: (userId: number) => Promise<void>
    /** Runs before the operation, like connect and identity checks. */
    connect?: () => Promise<void>
    signal?: AbortSignal
  } = {},
) {
  const db = openStateDb(":memory:")
  const queue = new BackgroundQueue(db)
  const holds = HoldStore.fromDb(db)
  const calls: string[] = []
  const telegram = {
    add: async (_chat: number, userId: number) => {
      calls.push(`add:${userId}`)
      await options.add?.(userId)
    },
    inviteLink: async (_chat: number, userId: number) => {
      calls.push(`link:${userId}`)
      return {
        inviteLink: "https://t.me/+fixture",
        expiresAt: new Date(NOW + 86_400_000).toISOString(),
      }
    },
  } as unknown as TelegramProvisioner
  const drain = (nowMs = NOW, random = () => 0.5) =>
    drainBackgroundOnce({
      queue,
      holds,
      accountId: ACCOUNT,
      pacing: PACING,
      nowMs,
      random,
      telegram: async (operation) => {
        calls.push("connect")
        await options.connect?.()
        return operation(telegram)
      },
      ...(options.signal ? { signal: options.signal } : {}),
    })
  const enqueue = (userIds: number[], notBeforeMs = NOW) =>
    queue.enqueue({ requestId: "r1", chatId: -100, userIds, notBeforeMs })
  return { queue, holds, calls, drain, enqueue }
}

describe("background drain", () => {
  test("one add per step, then the rest wait one jittered gap", async () => {
    const f = fixture()
    f.enqueue([1, 2])
    expect(await f.drain()).toEqual({
      state: "added",
      chatId: -100,
      userId: 1,
      nextAtMs: NOW + 1_200_000,
    })
    expect(f.calls).toEqual(["connect", "add:1"])
    expect(await f.drain(NOW + 60_000)).toEqual({ state: "idle" })
    expect(f.queue.summary()).toEqual({ pending: 1, nextAtMs: NOW + 1_200_000 })
    expect((await f.drain(NOW + 1_200_000)).userId).toBe(2)
    expect(f.queue.addsToday(NOW + 1_200_000)).toBe(2)
  })
  test("gap jitter stays within ±30 %", async () => {
    for (const [r, gap] of [
      [0, 840_000],
      [0.999999, 1_560_000],
    ] as const) {
      const f = fixture()
      f.enqueue([1])
      const result = await f.drain(NOW, () => r)
      expect(Math.abs(result.nextAtMs! - NOW - gap)).toBeLessThanOrEqual(1)
    }
  })
  test("the daily cap stops adds until the next UTC day", async () => {
    const f = fixture()
    f.enqueue([1, 2, 3, 4, 5, 6, 7])
    for (let i = 0; i < 6; i++) f.queue.finish(-100, i + 1, "added", null, NOW)
    const result = await f.drain(NOW + 1)
    expect(result).toMatchObject({ state: "capped", userId: 7 })
    expect(result.nextAtMs).toBe(nextUtcDayMs(NOW))
    expect(f.calls).toEqual([])
    expect((await f.drain(nextUtcDayMs(NOW))).state).toBe("added")
  })
  test("an active invite hold blocks the add and moves rows to its end", async () => {
    const f = fixture()
    f.enqueue([1])
    f.holds.recordHold({
      accountId: ACCOUNT,
      grp: "invite",
      method: "channels.inviteToChannel",
      source: "flood_wait",
      untilMs: NOW + 3_600_000,
      nowMs: NOW,
    })
    expect(await f.drain()).toMatchObject({
      state: "held",
      nextAtMs: NOW + 3_600_000,
    })
    expect(f.calls).toEqual([])
    expect(f.queue.summary().nextAtMs).toBe(NOW + 3_600_000)
  })
  test("a privacy refusal creates an invite link and marks the row invited", async () => {
    const f = fixture({
      add: async () => {
        throw { text: "USER_PRIVACY_RESTRICTED", code: 403 }
      },
    })
    f.enqueue([1])
    expect(await f.drain()).toMatchObject({
      state: "invited",
      error: "USER_PRIVACY_RESTRICTED",
    })
    expect(f.calls).toEqual(["connect", "add:1", "link:1"])
    expect(f.queue.forChat(-100)[0]).toMatchObject({
      state: "invited",
      attempts: 1,
    })
  })
  test("a flood or hold leaves the row pending until it ends", async () => {
    for (const error of [
      { text: "FLOOD_WAIT_%d", seconds: 7200, code: 420 },
      new HoldError(NOW + 7_200_000, "invite", NOW),
    ]) {
      const f = fixture({
        add: async () => {
          throw error
        },
      })
      f.enqueue([1])
      const result = await f.drain()
      expect(result.state).toBe("deferred")
      expect(result.nextAtMs).toBeGreaterThanOrEqual(NOW + 7_200_000)
      const row = f.queue.forChat(-100)[0]!
      expect(row.state).toBe("pending")
      expect(row.notBeforeMs).toBe(result.nextAtMs!)
      expect(f.queue.addsToday(NOW)).toBe(0)
    }
  })
  test("a per-user refusal fails the row; unknown errors retry, then fail", async () => {
    const refused = fixture({
      add: async () => {
        throw { text: "USER_CHANNELS_TOO_MUCH", code: 400 }
      },
    })
    refused.enqueue([1])
    expect(await refused.drain()).toMatchObject({
      state: "failed",
      error: "USER_CHANNELS_TOO_MUCH",
    })
    const flaky = fixture({
      add: async () => {
        throw new Error("network")
      },
    })
    flaky.enqueue([1])
    let now = NOW
    const states: string[] = []
    for (let i = 0; i < 5; i++) {
      const result = await flaky.drain(now)
      states.push(result.state)
      now = result.nextAtMs!
    }
    expect(states).toEqual([
      "deferred",
      "deferred",
      "deferred",
      "deferred",
      "failed",
    ])
  })
  test("errors before the add call defer one gap without an attempt", async () => {
    for (const error of [
      new Error("Custody account identity mismatch"),
      new Error("connect ETIMEDOUT"),
    ]) {
      const f = fixture({
        connect: async () => {
          throw error
        },
      })
      f.enqueue([1, 2])
      let now = NOW
      for (let i = 0; i < 8; i++) {
        const result = await f.drain(now)
        expect(result).toMatchObject({ state: "deferred", userId: 1 })
        expect(result.nextAtMs).toBe(now + 1_200_000)
        now = result.nextAtMs!
      }
      expect(f.calls).not.toContain("add:1")
      expect(f.queue.forChat(-100)[0]).toMatchObject({
        state: "pending",
        attempts: 0,
        notBeforeMs: now,
      })
      expect(f.queue.addsToday(now)).toBe(0)
    }
  })
  test("shutdown skips the drain and never counts a cut-off call", async () => {
    const aborted = new AbortController()
    aborted.abort()
    const idle = fixture({ signal: aborted.signal })
    idle.enqueue([1])
    expect(await idle.drain()).toEqual({ state: "idle" })
    expect(idle.calls).toEqual([])

    const controller = new AbortController()
    const cut = fixture({
      signal: controller.signal,
      add: async () => {
        controller.abort()
        throw new Error("client destroyed")
      },
    })
    cut.enqueue([1])
    expect((await cut.drain()).state).toBe("deferred")
    expect(cut.queue.forChat(-100)[0]).toMatchObject({
      state: "pending",
      attempts: 0,
    })
  })
  test("a flood from the add call does not count an attempt", async () => {
    const f = fixture({
      add: async () => {
        throw { text: "FLOOD_WAIT_%d", seconds: 60, code: 420 }
      },
    })
    f.enqueue([1])
    await f.drain()
    expect(f.queue.forChat(-100)[0]!.attempts).toBe(0)
  })
  test("enqueue adapter starts rows one jittered gap out", () => {
    const f = fixture()
    backgroundEnqueue(f.queue, PACING, {
      now: () => NOW,
      random: () => 0,
    }).enqueue({ requestId: "r2", chatId: -200, userIds: [9] })
    expect(f.queue.forChat(-200)[0]).toMatchObject({
      requestId: "r2",
      role: "member",
      notBeforeMs: NOW + 840_000,
      state: "pending",
    })
  })
})
