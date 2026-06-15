import { describe, expect, test } from "bun:test"
import {
  MemoryBatchLogger,
  parseFloodWaitSeconds,
  withFloodWaitRetry,
} from "./actionExecutor"

function floodWaitError(seconds: number): Error {
  const error = new Error(`FLOOD_WAIT_${seconds}`)
  error.name = "MtFloodWaitError"
  return error
}

describe("parseFloodWaitSeconds", () => {
  test("parses FLOOD_WAIT seconds from error messages", () => {
    expect(parseFloodWaitSeconds(new Error("FLOOD_WAIT_37"))).toBe(37)
  })

  test("parses FLOOD_WAIT seconds from error names", () => {
    const error = new Error("rate limited")
    error.name = "FLOOD_WAIT_9"

    expect(parseFloodWaitSeconds(error)).toBe(9)
  })

  test("returns null for unrelated errors and non-errors", () => {
    expect(parseFloodWaitSeconds(new Error("CHAT_ADMIN_REQUIRED"))).toBeNull()
    expect(parseFloodWaitSeconds("FLOOD_WAIT_37")).toBeNull()
  })
})

describe("withFloodWaitRetry", () => {
  test("retries after FLOOD_WAIT and records event order", async () => {
    const logger = new MemoryBatchLogger()
    const sleeps: number[] = []
    const times = [1000, 42000]
    let calls = 0

    const result = await withFloodWaitRetry(
      { chatId: -123, action: "promote_bot" },
      async () => {
        calls++
        if (calls === 1) {
          throw floodWaitError(37)
        }
        return "ok"
      },
      {
        logger,
        sleep: async (ms) => {
          sleeps.push(ms)
        },
        now: () => times.shift() ?? 42000,
        maxRetries: 3,
        bufferMs: 2000,
      },
    )

    expect(result).toBe("ok")
    expect(calls).toBe(2)
    expect(sleeps).toEqual([39000])
    expect(logger.events.map((event) => event.type)).toEqual([
      "action_start",
      "flood_wait",
      "action_start",
      "action_success",
    ])
    expect(logger.events).toMatchObject([
      {
        type: "action_start",
        chatId: -123,
        action: "promote_bot",
        attempt: 1,
      },
      {
        type: "flood_wait",
        chatId: -123,
        action: "promote_bot",
        attempt: 1,
        waitSeconds: 37,
        sleepMs: 39000,
      },
      {
        type: "action_start",
        chatId: -123,
        action: "promote_bot",
        attempt: 2,
      },
      {
        type: "action_success",
        chatId: -123,
        action: "promote_bot",
        attempts: 2,
        elapsedMs: 41000,
      },
    ])
  })

  test("logs and rethrows non-flood errors", async () => {
    const logger = new MemoryBatchLogger()
    const error = new Error("CHAT_ADMIN_REQUIRED")
    let calls = 0

    await expect(
      withFloodWaitRetry(
        { chatId: -456, action: "add_owner" },
        async () => {
          calls++
          throw error
        },
        {
          logger,
          sleep: async () => undefined,
          now: () => 1000,
          maxRetries: 3,
          bufferMs: 2000,
        },
      ),
    ).rejects.toThrow("CHAT_ADMIN_REQUIRED")

    expect(calls).toBe(1)
    expect(logger.events.map((event) => event.type)).toEqual([
      "action_start",
      "action_error",
    ])
    expect(logger.events[1]).toMatchObject({
      type: "action_error",
      chatId: -456,
      action: "add_owner",
      attempt: 1,
      errorName: "Error",
      errorMessage: "CHAT_ADMIN_REQUIRED",
    })
  })

  test("stops retrying after maxRetries is reached", async () => {
    const logger = new MemoryBatchLogger()
    const sleeps: number[] = []
    let calls = 0

    await expect(
      withFloodWaitRetry(
        { chatId: -789, action: "transfer_owner" },
        async () => {
          calls++
          throw floodWaitError(5)
        },
        {
          logger,
          sleep: async (ms) => {
            sleeps.push(ms)
          },
          now: () => 1000,
          maxRetries: 2,
          bufferMs: 1000,
        },
      ),
    ).rejects.toThrow("FLOOD_WAIT_5")

    expect(calls).toBe(3)
    expect(sleeps).toEqual([6000, 6000])
    expect(logger.events.map((event) => event.type)).toEqual([
      "action_start",
      "flood_wait",
      "action_start",
      "flood_wait",
      "action_start",
      "action_error",
    ])
  })
})
