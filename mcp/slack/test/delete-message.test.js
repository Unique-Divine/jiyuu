import { afterEach, expect, test } from "bun:test"
import { handleDeleteMessage, TOOL_HANDLERS } from "../lib/handlers.js"
import { clearSelfUserIdCache } from "../lib/message-ownership.js"
import { TOOLS } from "../lib/tools.js"
import { stubSlackApi } from "./helpers/slack-api-stub.js"

let stub
afterEach(() => {
  stub?.restore()
  stub = undefined
  clearSelfUserIdCache()
})

const SELF = "U0SELF0001"
const OTHER = "U0OTHER002"
const CHANNEL = "C0123456789"
const TS = "1767368030.607599"
const OLD_TEXT = "Wrong copy sent to attendees"
const ARGS = { channel_id: CHANNEL, timestamp: TS }

function parsePayload(result) {
  return JSON.parse(result.content[0].text)
}

/** Slack responses for one message in CHANNEL authored by `author`. */
function slackWithMessage(author, overrides = {}) {
  return {
    "auth.test": { ok: true, user_id: SELF },
    "conversations.history": {
      ok: true,
      messages: [{ ts: TS, user: author, text: OLD_TEXT }],
    },
    "chat.delete": (params) => ({ ok: true, channel: params.channel, ts: params.ts }),
    ...overrides,
  }
}

function methods() {
  return stub.calls.map((c) => c.method)
}

test("slack_delete_message is advertised as a destructive, non-idempotent write", () => {
  const tool = TOOLS.find((t) => t.name === "slack_delete_message")
  expect(tool).toBeTruthy()
  expect(tool.inputSchema.required).toStrictEqual(["channel_id", "timestamp"])
  expect(tool.annotations).toMatchObject({
    readOnlyHint: false,
    destructiveHint: true,
    idempotentHint: false,
    openWorldHint: true,
  })
  expect(tool.description).toContain("Permanently")
  expect(tool.description).toContain("This message was deleted.")
  expect(TOOL_HANDLERS.slack_delete_message).toBe(handleDeleteMessage)
})

test("deletes your own message and returns its old text", async () => {
  stub = stubSlackApi(slackWithMessage(SELF))

  const result = await handleDeleteMessage(ARGS)

  expect(result.isError).toBeUndefined()
  expect(parsePayload(result)).toStrictEqual({
    status: "deleted",
    channel: CHANNEL,
    timestamp: TS,
    old_text: OLD_TEXT,
  })
  const del = stub.calls.find((c) => c.method === "chat.delete")
  expect(del.params).toStrictEqual({ channel: CHANNEL, ts: TS })
})

test("refuses someone else's message locally with not_own_message", async () => {
  stub = stubSlackApi(slackWithMessage(OTHER))

  const result = await handleDeleteMessage(ARGS)

  expect(result.isError).toBe(true)
  expect(parsePayload(result)).toMatchObject({
    code: "not_own_message",
    author: OTHER,
    channel: CHANNEL,
    timestamp: TS,
  })
  expect(methods()).not.toContain("chat.delete")
})

test("finds a thread reply through conversations.replies", async () => {
  stub = stubSlackApi(slackWithMessage(SELF, {
    "conversations.history": {
      ok: true,
      messages: [{ ts: "1767368000.000100", user: SELF }],
    },
    "conversations.replies": {
      ok: true,
      messages: [
        { ts: "1767368000.000100", user: OTHER, text: "parent" },
        { ts: TS, user: SELF, text: OLD_TEXT },
      ],
    },
  }))

  const result = await handleDeleteMessage(ARGS)

  expect(parsePayload(result)).toMatchObject({
    status: "deleted",
    old_text: OLD_TEXT,
  })
})

test("an unknown timestamp fails with message_not_found before chat.delete", async () => {
  stub = stubSlackApi(slackWithMessage(SELF, {
    "conversations.history": { ok: true, messages: [] },
    "conversations.replies": { ok: false, error: "thread_not_found" },
  }))

  const result = await handleDeleteMessage(ARGS)

  expect(result.isError).toBe(true)
  expect(parsePayload(result).code).toBe("message_not_found")
  expect(methods()).not.toContain("chat.delete")
})

for (const code of ["cant_delete_message", "no_permission"]) {
  test(`Slack error ${code} maps to a clear error result`, async () => {
    stub = stubSlackApi(slackWithMessage(SELF, {
      "chat.delete": { ok: false, error: code },
    }))

    const result = await handleDeleteMessage(ARGS)

    expect(result.isError).toBe(true)
    const payload = parsePayload(result)
    expect(payload.code).toBe(code)
    expect(payload.message).not.toBe(code)
    expect(payload.next_action).toBeTruthy()
  })
}

test("message_not_found after a network retry reports probably_deleted", async () => {
  let deleteAttempts = 0
  let historyReads = 0
  stub = stubSlackApi(slackWithMessage(SELF, {
    // The first read is the ownership check; the re-check after the failed
    // delete finds nothing.
    "conversations.history": () => {
      historyReads += 1
      return historyReads === 1
        ? { ok: true, messages: [{ ts: TS, user: SELF, text: OLD_TEXT }] }
        : { ok: true, messages: [] }
    },
    "conversations.replies": { ok: false, error: "thread_not_found" },
    "chat.delete": () => {
      deleteAttempts += 1
      if (deleteAttempts === 1) {
        const err = new Error("socket hang up")
        err.code = "ECONNRESET"
        throw err
      }
      return { ok: false, error: "message_not_found" }
    },
  }))

  const result = await handleDeleteMessage(ARGS)

  expect(deleteAttempts).toBe(2)
  expect(result.isError).toBeUndefined()
  const payload = parsePayload(result)
  expect(payload.status).toBe("probably_deleted")
  expect(payload.note).toContain("first attempt likely landed")
  expect(payload.old_text).toBe(OLD_TEXT)
}, 10_000)

test("auth errors still throw so the transport can show the Lifeboat message", async () => {
  stub = stubSlackApi(slackWithMessage(SELF, {
    "auth.test": { ok: false, error: "account_inactive" },
  }))

  await expect(handleDeleteMessage(ARGS)).rejects.toThrow("account_inactive")
})
