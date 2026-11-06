import { describe, expect, test } from "bun:test"
import { tl } from "@mtcute/bun"

import {
  floodStopFor,
  MemoryBatchLogger,
  runChatBatch,
  runTelegramAction,
} from "./actionExecutor"
import { HoldAccountUnknownError, HoldError } from "./state/guard"
import { HoldStoreUnavailableError } from "./state/holds"

/** The error mtcute throws for a Telegram RPC error reply. */
function rpcError(errorCode: number, errorMessage: string): Error {
  return tl.RpcError.fromTl({ _: "mt_rpc_error", errorCode, errorMessage })
}

function floodWaitError(seconds: number): Error {
  const error = new Error(`FLOOD_WAIT_${seconds}`)
  error.name = "MtFloodWaitError"
  return error
}

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
      { type: "action_start", chatId: -123, action: "promote_bot" },
      {
        type: "action_success",
        chatId: -123,
        action: "promote_bot",
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

describe("floodStopFor", () => {
  const now = Date.parse("2026-11-06T00:00:00.000Z")

  test("a flood wait longer than the threshold stops with a resume time",
    () => {
      expect(floodStopFor(rpcError(420, "FLOOD_WAIT_300"), 300_000, now))
        .toBeNull()
      expect(
        floodStopFor(rpcError(420, "FLOOD_WAIT_15382"), 300_000, now),
      ).toEqual({
        code: "FLOOD_WAIT_15382",
        waitS: 15382,
        resumeAt: "2026-11-06T04:16:22.000Z",
        hold: false,
      })
    })

  test("PEER_FLOOD always stops; SLOWMODE and other errors do not", () => {
    expect(floodStopFor(rpcError(400, "PEER_FLOOD"), 300_000, now)).toEqual({
      code: "PEER_FLOOD",
      waitS: null,
      resumeAt: null,
      hold: false,
    })
    expect(
      floodStopFor(rpcError(420, "SLOWMODE_WAIT_9999"), 300_000, now),
    ).toBeNull()
    expect(
      floodStopFor(rpcError(400, "CHAT_ADMIN_REQUIRED"), 300_000, now),
    ).toBeNull()
    expect(floodStopFor(new Error("Unknown profile: x"), 0, now)).toBeNull()
  })

  test("a saved hold stops with the hold's end time", () => {
    const untilMs = now + 3_600_000
    const error = new HoldError(untilMs, "invite", now)
    expect(floodStopFor(error, 300_000, now)).toEqual({
      code: "TG_SAI_HOLD_3600",
      waitS: 3600,
      resumeAt: "2026-11-06T01:00:00.000Z",
      hold: true,
    })
    // Even a hold shorter than the threshold: nothing would be sent.
    expect(
      floodStopFor(new HoldError(now + 5_000, "invite", now), 300_000, now),
    ).toMatchObject({ code: "TG_SAI_HOLD_5", hold: true })
  })

  test("unreadable holds and an unknown account stop without a time", () => {
    for (const error of [
      new HoldStoreUnavailableError("disk I/O error"),
      new HoldAccountUnknownError("channels.inviteToChannel"),
    ]) {
      expect(floodStopFor(error, 300_000, now)).toEqual({
        code: error.text,
        waitS: null,
        resumeAt: null,
        hold: true,
      })
    }
  })
})

describe("runChatBatch", () => {
  /**
   * Fake action runner: each chat runs one `runTelegramAction` whose
   * operation answers from `outcomes` (undefined = success).
   */
  async function runBatch(outcomes: (Error | undefined)[]) {
    const logger = new MemoryBatchLogger()
    const chats = outcomes.map((_, i) => -100 - i)
    const errors: number[] = []
    const delays: number[] = []
    const result = await runChatBatch(chats, {
      processItem: (chatId, index) =>
        runTelegramAction(
          { chatId, action: "bot_add" },
          async () => {
            const outcome = outcomes[index]
            if (outcome) throw outcome
          },
          { logger, now: () => 0 },
        ),
      onItemError: (chatId) => {
        errors.push(chatId)
      },
      betweenItems: (chatId) => {
        delays.push(chatId)
      },
      maxWaitMs: 300_000,
      now: () => 0,
    })
    return { result, logger, errors, delays }
  }

  test("isolates ordinary per-chat errors and finishes the batch", async () => {
    const { result, errors, delays } = await runBatch([
      undefined,
      rpcError(400, "CHAT_ADMIN_REQUIRED"),
      undefined,
    ])
    expect(result).toEqual({ processed: 3, floodStop: null })
    expect(errors).toEqual([-101])
    expect(delays).toEqual([-100, -101])
  })

  test("SLOWMODE_WAIT is per chat; the batch continues", async () => {
    const { result, errors } = await runBatch([
      rpcError(420, "SLOWMODE_WAIT_900"),
      undefined,
    ])
    expect(result.floodStop).toBeNull()
    expect(result.processed).toBe(2)
    expect(errors).toEqual([-100])
  })

  test("a long FLOOD_WAIT stops the remaining chats", async () => {
    const { result, logger, errors, delays } = await runBatch([
      undefined,
      rpcError(420, "FLOOD_WAIT_15382"),
      undefined,
      undefined,
    ])
    expect(result.processed).toBe(2)
    expect(result.floodStop).toEqual({
      code: "FLOOD_WAIT_15382",
      waitS: 15382,
      resumeAt: "1970-01-01T04:16:22.000Z",
      hold: false,
      item: -101,
      index: 1,
      remaining: 2,
    })
    expect(errors).toEqual([-101])
    expect(delays).toEqual([-100])
    expect(logger.events.map((e) => [e.type, e.chatId])).toEqual([
      ["action_start", -100],
      ["action_success", -100],
      ["action_start", -101],
      ["action_error", -101],
    ])
  })

  test("a saved hold stops the remaining chats", async () => {
    const hold = new HoldError(3_600_000, "invite", 0)
    const { result, errors } = await runBatch([hold, undefined, undefined])
    expect(result.processed).toBe(1)
    expect(result.floodStop).toMatchObject({
      code: "TG_SAI_HOLD_3600",
      hold: true,
      index: 0,
      remaining: 2,
      resumeAt: "1970-01-01T01:00:00.000Z",
    })
    expect(errors).toEqual([-100])
  })

  test("PEER_FLOOD stops the batch at once", async () => {
    const { result } = await runBatch([
      rpcError(400, "PEER_FLOOD"),
      undefined,
    ])
    expect(result.floodStop).toMatchObject({
      code: "PEER_FLOOD",
      index: 0,
      remaining: 1,
      resumeAt: null,
    })
  })
})
