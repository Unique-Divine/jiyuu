import { describe, expect, test } from "bun:test"
import { MtcuteError, MtTimeoutError, tl } from "@mtcute/bun"

import {
  backgroundEnqueue,
  drainBackgroundOnce,
  recordingMemberAdds,
} from "./backgroundWorker"
import type { TelegramProvisioner } from "./provisioning"
import { HoldError } from "./state/guard"
import {
  BackgroundQueue,
  MAX_BACKGROUND_ATTEMPTS,
  nextUtcDayMs,
} from "./state/background"
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
    /** Users already in the chat, e.g. joined by link. */
    joined?: number[]
    /** Users with a pending join request; omit for an adapter without approval. */
    requests?: number[]
    approveError?: unknown
    /** Error for the member read, or a function choosing one per chat. */
    readError?: unknown
  } = {},
) {
  const db = openStateDb(":memory:")
  const queue = new BackgroundQueue(db)
  const holds = HoldStore.fromDb(db)
  const calls: string[] = []
  const telegram = {
    member: async (chatId: number, userId: number) => {
      const error =
        typeof options.readError === "function"
          ? options.readError(chatId)
          : options.readError
      if (error) throw error
      return {
        role: options.joined?.includes(userId) ? "member" : "absent",
        rights: {},
      }
    },
    ...(options.requests || options.approveError
      ? {
          approveJoin: async (_chat: number, userId: number) => {
            calls.push(`approve:${userId}`)
            if (options.approveError) throw options.approveError
            return options.requests!.includes(userId)
          },
        }
      : {}),
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
  test("a member who already joined by link costs no add", async () => {
    const f = fixture({ joined: [1] })
    f.enqueue([1, 2])
    expect(await f.drain()).toEqual({
      state: "joined",
      chatId: -100,
      userId: 1,
      nextAtMs: NOW + 1_200_000,
    })
    expect(f.calls).toEqual(["connect"])
    expect(f.queue.addsToday(NOW)).toBe(0)
    expect(f.queue.summary().pending).toBe(1)
    expect((await f.drain(NOW + 1_200_000)).state).toBe("added")
    expect(f.calls).toEqual(["connect", "connect", "add:2"])
    expect(f.queue.addsToday(NOW + 1_200_000)).toBe(1)
  })
  test("a waiting join request is approved instead of adding", async () => {
    const f = fixture({ requests: [1] })
    f.enqueue([1, 2])
    expect((await f.drain()).state).toBe("joined")
    expect((await f.drain(NOW + 1_200_000)).state).toBe("added")
    expect(f.calls).toEqual([
      "connect",
      "approve:1",
      "connect",
      "approve:2",
      "add:2",
    ])
    expect(f.queue.addsToday(NOW + 1_200_000)).toBe(1)
  })
  test("a flood on the approval defers the row until it ends, uncounted", async () => {
    const f = fixture({
      approveError: Object.assign(new Error("x"), {
        code: 420,
        text: "FLOOD_WAIT_7200",
      }),
    })
    f.enqueue([1, 2])
    const result = await f.drain()
    expect(result).toMatchObject({ state: "deferred", error: "FLOOD_WAIT_7200" })
    expect(result.nextAtMs!).toBeGreaterThanOrEqual(NOW + 7_200_000)
    expect(f.calls).toEqual(["connect", "approve:1"])
    expect(f.queue.addsToday(NOW)).toBe(0)
    // Every row waits for the hold, and the flooded row counts no attempt.
    expect(f.queue.next(NOW + 7_000_000)).toBeNull()
    expect(f.queue.next(result.nextAtMs!)).toMatchObject({
      userId: 1,
      attempts: 0,
    })
  })
  test("a chat or member fault on the read or approval fails the row without an add", async () => {
    const faults: [string, Record<string, unknown>][] = [
      ["read", { code: 400, text: "CHANNEL_PRIVATE" }],
      ["read", { code: 400, text: "PEER_ID_INVALID" }],
      ["approve", { code: 400, text: "USER_CHANNELS_TOO_MUCH" }],
      ["approve", { code: 400, text: "INPUT_USER_DEACTIVATED" }],
    ]
    for (const [where, fault] of faults) {
      const error = Object.assign(new Error("x"), fault)
      const f = fixture(
        where === "read" ? { readError: error } : { approveError: error },
      )
      f.enqueue([1])
      expect(await f.drain()).toMatchObject({
        state: "failed",
        error: fault.text,
      })
      expect(f.calls.filter((c) => c.startsWith("add:"))).toEqual([])
      expect(f.queue.addsToday(NOW)).toBe(0)
      expect(f.queue.summary().pending).toBe(0)
    }
  })
  test("a row whose read keeps failing does not starve the others", async () => {
    // Chat -200 errors on every read; chat -100 rows are healthy.
    const f = fixture({
      readError: (chatId: number) =>
        chatId === -200 ? new Error("unexpected read failure") : null,
    })
    f.queue.enqueue({
      requestId: "r0",
      chatId: -200,
      userIds: [9],
      notBeforeMs: NOW - 1,
    })
    f.enqueue([1, 2, 3])
    let now = NOW
    const results: string[] = []
    for (let i = 0; i < 12 && f.queue.summary().pending; i++) {
      const result = await f.drain(now)
      if (result.state !== "idle")
        results.push(`${result.state}:${result.chatId}/${result.userId}`)
      now = f.queue.summary().nextAtMs ?? now
    }
    expect(results.filter((r) => r.startsWith("added:"))).toEqual([
      "added:-100/1",
      "added:-100/2",
      "added:-100/3",
    ])
    // The healthy rows go between the stuck row's attempts.
    expect(results.slice(0, 4)).toEqual([
      "deferred:-200/9",
      "added:-100/1",
      "deferred:-200/9",
      "added:-100/2",
    ])
    // The stuck row counts attempts and finally fails.
    expect(results.at(-1)).toBe("failed:-200/9")
    expect(results.filter((r) => r.endsWith("-200/9"))).toHaveLength(5)
  })
  test("a flood or transient approval error defers the row without an add", async () => {
    for (const approveError of [
      { text: "FLOOD_WAIT_%d", seconds: 60, code: 420 },
      new MtTimeoutError(10_000),
    ]) {
      const f = fixture({ approveError })
      f.enqueue([1])
      expect(await f.drain()).toMatchObject({
        state: "deferred",
        nextAtMs: NOW + 1_200_000,
      })
      expect(f.calls).toEqual(["connect", "approve:1"])
      expect(f.queue.forChat(-100)[0]).toMatchObject({
        state: "pending",
        attempts: 0,
      })
    }
  })
  test("transient add errors never fail the row or count an attempt", async () => {
    for (const error of [
      new MtTimeoutError(10_000),
      tl.RpcError.fromTl({
        _: "rpc_error",
        errorCode: -503,
        errorMessage: "Timeout",
      }),
      new MtcuteError("Connection closed"),
    ]) {
      const f = fixture({
        add: async () => {
          throw error
        },
      })
      f.enqueue([1])
      let now = NOW
      for (let i = 0; i < MAX_BACKGROUND_ATTEMPTS + 2; i++) {
        const result = await f.drain(now)
        expect(result).toMatchObject({ state: "deferred", error: "TG_SAI_TRANSIENT" })
        now = f.queue.summary().nextAtMs!
      }
      expect(f.queue.forChat(-100)[0]).toMatchObject({
        state: "pending",
        attempts: 0,
      })
      expect(f.queue.addsToday(now)).toBe(0)
    }
  })
  test("a transient read error waits behind the other rows, uncounted", async () => {
    const f = fixture({
      readError: (chatId: number) =>
        chatId === -200 ? new MtTimeoutError(10_000) : null,
    })
    f.queue.enqueue({
      requestId: "r0",
      chatId: -200,
      userIds: [9],
      notBeforeMs: NOW - 1,
    })
    f.enqueue([1])
    expect(await f.drain()).toMatchObject({
      state: "deferred",
      chatId: -200,
      error: "TG_SAI_TRANSIENT",
    })
    expect((await f.drain(f.queue.summary().nextAtMs!)).chatId).toBe(-100)
    expect(f.queue.forChat(-200)[0]).toMatchObject({
      state: "pending",
      attempts: 0,
    })
  })
  test("after a lost add response the next step finds the member", async () => {
    const joined: number[] = []
    const f = fixture({
      joined,
      add: async (userId) => {
        joined.push(userId)
        throw new MtTimeoutError(10_000)
      },
    })
    f.enqueue([1])
    expect((await f.drain()).state).toBe("deferred")
    expect((await f.drain(f.queue.summary().nextAtMs!)).state).toBe("joined")
    expect(f.calls.filter((c) => c === "add:1")).toHaveLength(1)
  })
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
    f.queue.recordAdds({
      source: "setup",
      outcome: "added",
      chatId: -300,
      userIds: [8],
      nowMs: NOW,
    })
    expect(f.queue.addsToday(NOW + 1_200_000)).toBe(3)
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
    f.enqueue([7])
    // Setup adds alone can reach the cap: it counts the account's total.
    f.queue.recordAdds({
      source: "setup",
      outcome: "added",
      chatId: -300,
      userIds: [1, 2, 3, 4, 5, 6],
      nowMs: NOW,
    })
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
  test("a privacy refusal makes no link, ends privacy, and skips the cap", async () => {
    const f = fixture({
      add: async () => {
        throw { text: "USER_PRIVACY_RESTRICTED", code: 403 }
      },
    })
    f.enqueue([1])
    expect(await f.drain()).toMatchObject({
      state: "privacy",
      error: "USER_PRIVACY_RESTRICTED",
    })
    expect(f.calls).toEqual(["connect", "add:1"])
    expect(f.queue.forChat(-100)[0]).toMatchObject({
      state: "privacy",
      attempts: 1,
      lastError: "USER_PRIVACY_RESTRICTED",
    })
    expect(f.queue.addsToday(NOW)).toBe(0)
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
      now = flaky.queue.summary().nextAtMs ?? result.nextAtMs!
    }
    expect(states).toEqual([
      "deferred",
      "deferred",
      "deferred",
      "deferred",
      "failed",
    ])
    // A generic error is not a per-user refusal: no add counted.
    expect(flaky.queue.addsToday(now)).toBe(0)
    expect(refused.queue.addsToday(NOW)).toBe(1)
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
  test("setup adds log each user added or refused alone, never privacy", async () => {
    const f = fixture()
    let now = NOW
    const base = {
      add: async (_chat: number, userId: number) => {
        if (userId === 2) throw { text: "USER_PRIVACY_RESTRICTED", code: 403 }
        if (userId === 3) throw { text: "USER_CHANNELS_TOO_MUCH", code: 400 }
        if (userId === 4) throw { text: "FLOOD_WAIT_%d", seconds: 60, code: 420 }
      },
      addMany: async (_chat: number, userIds: number[]) =>
        userIds.filter((id) => id === 12),
    } as unknown as TelegramProvisioner
    const adapter = recordingMemberAdds(base, f.queue, () => now)
    await adapter.add(-1, 1)
    for (const userId of [2, 3, 4])
      await adapter.add(-1, userId).catch(() => {})
    expect(await adapter.addMany!(-1, [11, 12, 13])).toEqual([12])
    expect(f.queue.addsToday(NOW)).toBe(4)
    now = nextUtcDayMs(NOW)
    await adapter.add(-1, 5)
    expect(f.queue.addsToday(now)).toBe(1)
  })
  test("a background add and a per-user refusal both count", async () => {
    const f = fixture({
      add: async (userId) => {
        if (userId === 2) throw { text: "USER_CHANNELS_TOO_MUCH", code: 400 }
      },
    })
    f.enqueue([1, 2])
    await f.drain()
    await f.drain(NOW + 1_200_000)
    expect(f.queue.addsToday(NOW + 1_200_000)).toBe(2)
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
