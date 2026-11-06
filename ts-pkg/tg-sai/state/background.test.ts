import { describe, expect, test } from "bun:test"

import {
  BackgroundQueue,
  backgroundPacing,
  DEFAULT_BACKGROUND_DAILY_MAX,
  DEFAULT_BACKGROUND_GAP_MS,
  jitteredGapMs,
  nextUtcDayMs,
  utcDayStartMs,
} from "./background.ts"
import { openStateDb } from "./db.ts"

const DAY = Date.UTC(2026, 10, 6)

function queue(): BackgroundQueue {
  return new BackgroundQueue(openStateDb(":memory:"))
}

describe("background pacing", () => {
  test("defaults and env overrides; invalid values fall back", () => {
    expect(backgroundPacing({})).toEqual({
      gapMs: DEFAULT_BACKGROUND_GAP_MS,
      dailyMax: DEFAULT_BACKGROUND_DAILY_MAX,
      jitter: 0.3,
    })
    expect(DEFAULT_BACKGROUND_GAP_MS).toBe(20 * 60 * 1000)
    expect(DEFAULT_BACKGROUND_DAILY_MAX).toBe(6)
    expect(
      backgroundPacing({
        TG_SAI_BACKGROUND_GAP_MS: "60000",
        TG_SAI_BACKGROUND_DAILY_MAX: "0",
      }),
    ).toMatchObject({ gapMs: 60_000, dailyMax: 0 })
    for (const value of ["-1", "abc", "1.5", "10"])
      expect(
        backgroundPacing({ TG_SAI_BACKGROUND_GAP_MS: value }).gapMs,
      ).toBe(DEFAULT_BACKGROUND_GAP_MS)
  })
  test("jitter stays within ±30 %", () => {
    const pacing = { gapMs: 1_000_000, jitter: 0.3 }
    expect(jitteredGapMs(pacing, () => 0)).toBe(700_000)
    expect(jitteredGapMs(pacing, () => 0.5)).toBe(1_000_000)
    expect(jitteredGapMs(pacing, () => 0.999999)).toBeLessThanOrEqual(
      1_300_000,
    )
    for (let i = 0; i < 200; i++) {
      const gap = jitteredGapMs(pacing)
      expect(gap).toBeGreaterThanOrEqual(700_000)
      expect(gap).toBeLessThanOrEqual(1_300_000)
    }
  })
  test("UTC day bounds", () => {
    expect(utcDayStartMs(DAY + 5_000)).toBe(DAY)
    expect(nextUtcDayMs(DAY + 5_000)).toBe(DAY + 86_400_000)
  })
})

describe("background queue", () => {
  test("migration creates the table with a state check", () => {
    const db = openStateDb(":memory:")
    expect(() =>
      db.run(
        "INSERT INTO background_add VALUES ('r', 1, 2, 'member', 0, 0, " +
          "'bogus', NULL, 0, 0)",
      ),
    ).toThrow()
  })
  test("enqueue is idempotent and keeps finished rows", () => {
    const q = queue()
    const input = {
      requestId: "r1",
      chatId: -100,
      userIds: [5, 6],
      notBeforeMs: DAY,
      nowMs: DAY,
    }
    expect(q.enqueue(input)).toBe(2)
    q.finish(-100, 5, "added", null, DAY + 1)
    expect(q.enqueue(input)).toBe(0)
    expect(q.forChat(-100).map((r) => [r.userId, r.state])).toEqual([
      [5, "added"],
      [6, "pending"],
    ])
  })
  test("next picks the earliest due row; pace and defer move rows", () => {
    const q = queue()
    q.enqueue({ requestId: "a", chatId: -1, userIds: [1], notBeforeMs: 50 })
    q.enqueue({ requestId: "b", chatId: -2, userIds: [2], notBeforeMs: 10 })
    expect(q.next(5)).toBeNull()
    expect(q.next(60)?.userId).toBe(2)
    q.defer(-2, 2, 100, "FLOOD_WAIT_40", 60)
    expect(q.next(60)?.userId).toBe(1)
    q.pace(200)
    expect(q.next(150)).toBeNull()
    expect(q.summary()).toEqual({ pending: 2, nextAtMs: 200 })
    const row = q.forChat(-2)[0]!
    expect(row.attempts).toBe(1)
    expect(row.lastError).toBe("FLOOD_WAIT_40")
  })
  test("addsToday counts finished rows in the current UTC day", () => {
    const q = queue()
    q.enqueue({ requestId: "a", chatId: -1, userIds: [1, 2, 3, 4], notBeforeMs: 0 })
    q.finish(-1, 1, "added", null, DAY - 1)
    q.finish(-1, 2, "invited", "USER_PRIVACY_RESTRICTED", DAY + 1)
    q.finish(-1, 3, "failed", "USER_KICKED", DAY + 2)
    q.defer(-1, 4, DAY + 9, "FLOOD_WAIT_1", DAY + 3)
    expect(q.addsToday(DAY + 10)).toBe(2)
    expect(q.summary()).toEqual({ pending: 1, nextAtMs: DAY + 9 })
  })
})
