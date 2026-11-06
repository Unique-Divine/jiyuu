import { describe, expect, test } from "bun:test"
import {
  MemoryBatchLogger,
  parseFloodWaitSeconds,
  runTelegramAction,
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

describe("runTelegramAction", () => {
  test("runs once and records start then success", async () => {
    const logger = new MemoryBatchLogger()
    const times = [1000, 1250]
    let calls = 0

    const result = await runTelegramAction(
      { chatId: -123, action: "promote_bot" },
      async () => {
        calls++
        return "ok"
      },
      { logger, now: () => times.shift() ?? 1250 },
    )

    expect(result).toBe("ok")
    expect(calls).toBe(1)
    expect(logger.events).toEqual([
      { type: "action_start", chatId: -123, action: "promote_bot", attempt: 1 },
      {
        type: "action_success",
        chatId: -123,
        action: "promote_bot",
        attempts: 1,
        elapsedMs: 250,
      },
    ])
  })

  test("logs and rethrows errors", async () => {
    const logger = new MemoryBatchLogger()
    const error = new Error("CHAT_ADMIN_REQUIRED")
    let calls = 0

    await expect(
      runTelegramAction(
        { chatId: -456, action: "add_owner" },
        async () => {
          calls++
          throw error
        },
        { logger, now: () => 1000 },
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

  test("does not retry flood waits; mtcute owns that layer", async () => {
    const logger = new MemoryBatchLogger()
    let calls = 0

    await expect(
      runTelegramAction(
        { chatId: -789, action: "transfer_owner" },
        async () => {
          calls++
          throw floodWaitError(5)
        },
        { logger, now: () => 1000 },
      ),
    ).rejects.toThrow("FLOOD_WAIT_5")

    expect(calls).toBe(1)
    expect(logger.events.map((event) => event.type)).toEqual([
      "action_start",
      "action_error",
    ])
  })
})
