import { afterEach, expect, test } from "bun:test"
import { handleDeleteMessage, TOOL_HANDLERS } from "../lib/handlers.js"
import {
  clearSelfUserIdCache,
  MAX_REPLY_PAGES,
} from "../lib/message-ownership.js"
import { TOOLS } from "../lib/tools.js"
import { callTool } from "../src/server.js"
import { stubSlackApi } from "./helpers/slack-api-stub.js"

const AGE_ENV = "SLACK_MCP_EDIT_DELETE_MAX_AGE_HOURS"
const originalAgeEnv = process.env[AGE_ENV]

let stub
afterEach(() => {
  stub?.restore()
  stub = undefined
  clearSelfUserIdCache()
  if (originalAgeEnv === undefined) delete process.env[AGE_ENV]
  else process.env[AGE_ENV] = originalAgeEnv
})

const SELF = "U0SELF0001"
const OTHER = "U0OTHER002"
const CHANNEL = "C0123456789"
const NOW_SEC = Math.floor(Date.now() / 1000)
/** A Slack ts `hoursAgo` hours before the test run. */
function tsHoursAgo(hoursAgo) {
  return `${NOW_SEC - Math.round(hoursAgo * 3600)}.000100`
}
const TS = tsHoursAgo(0.25)
const OLD_TEXT = "Wrong copy sent to attendees"
const ARGS = { channel_id: CHANNEL, timestamp: TS, expected_text: OLD_TEXT }

function parsePayload(result) {
  return JSON.parse(result.content[0].text)
}

/** Slack responses for one message in CHANNEL; `message` overrides fields. */
function slackWithMessage(message = {}, overrides = {}) {
  return {
    "auth.test": { ok: true, user_id: SELF },
    "conversations.history": {
      ok: true,
      messages: [{ ts: TS, user: SELF, text: OLD_TEXT, ...message }],
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
  expect(tool.inputSchema.required).toStrictEqual([
    "channel_id",
    "timestamp",
    "expected_text",
  ])
  expect(tool.annotations).toMatchObject({
    readOnlyHint: false,
    destructiveHint: true,
    idempotentHint: false,
    openWorldHint: true,
  })
  for (const phrase of [
    "Permanently",
    "This message was deleted.",
    "never authorize",
    "24 hours",
  ]) {
    expect(tool.description).toContain(phrase)
  }
  expect(TOOL_HANDLERS.slack_delete_message).toBe(handleDeleteMessage)
})

test("deletes your own message and returns its old text", async () => {
  stub = stubSlackApi(slackWithMessage())

  // expected_text is compared after trimming.
  const result = await handleDeleteMessage({
    ...ARGS,
    expected_text: `  ${OLD_TEXT}\n`,
  })

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
  stub = stubSlackApi(slackWithMessage({ user: OTHER }))

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

test("refuses a bot message that has no user", async () => {
  stub = stubSlackApi(slackWithMessage({
    user: undefined,
    bot_id: "B0BOT00001",
    subtype: "bot_message",
  }))

  const result = await handleDeleteMessage(ARGS)

  expect(result.isError).toBe(true)
  expect(parsePayload(result)).toMatchObject({
    code: "not_own_message",
    author: "B0BOT00001",
  })
  expect(methods()).not.toContain("chat.delete")
})

test("finds a thread reply through conversations.replies", async () => {
  stub = stubSlackApi(slackWithMessage({}, {
    "conversations.history": {
      ok: true,
      messages: [{ ts: tsHoursAgo(1), user: SELF }],
    },
    "conversations.replies": {
      ok: true,
      messages: [
        { ts: tsHoursAgo(1), user: OTHER, text: "parent" },
        { ts: TS, user: SELF, text: OLD_TEXT },
      ],
    },
  }))

  const result = await handleDeleteMessage(ARGS)

  expect(parsePayload(result)).toMatchObject({
    status: "deleted",
    old_text: OLD_TEXT,
  })
  // The replies call is narrowed to the target ts.
  const replies = stub.calls.find((c) => c.method === "conversations.replies")
  expect(replies.params).toMatchObject({
    ts: TS,
    oldest: TS,
    latest: TS,
    inclusive: "true",
  })
})

test("finds a thread reply beyond the first conversations.replies page", async () => {
  stub = stubSlackApi(slackWithMessage({}, {
    "conversations.history": { ok: true, messages: [] },
    "conversations.replies": (params) =>
      params.cursor === "page-2"
        ? { ok: true, messages: [{ ts: TS, user: SELF, text: OLD_TEXT }] }
        : {
            ok: true,
            messages: [{ ts: tsHoursAgo(1), user: OTHER, text: "parent" }],
            response_metadata: { next_cursor: "page-2" },
          },
  }))

  const result = await handleDeleteMessage(ARGS)

  expect(parsePayload(result).status).toBe("deleted")
  const cursors = stub.calls
    .filter((c) => c.method === "conversations.replies")
    .map((c) => c.params.cursor)
  expect(cursors).toStrictEqual([undefined, "page-2"])
})

test("a reply past the page cap fails with lookup_incomplete, not message_not_found", async () => {
  let page = 0
  stub = stubSlackApi(slackWithMessage({}, {
    "conversations.history": { ok: true, messages: [] },
    "conversations.replies": () => {
      page += 1
      return {
        ok: true,
        messages: [{ ts: tsHoursAgo(1), user: OTHER, text: "parent" }],
        response_metadata: { next_cursor: `page-${page + 1}` },
      }
    },
  }))

  const result = await handleDeleteMessage(ARGS)

  expect(result.isError).toBe(true)
  expect(parsePayload(result).code).toBe("lookup_incomplete")
  expect(methods().filter((m) => m === "conversations.replies")).toHaveLength(
    MAX_REPLY_PAGES,
  )
  expect(methods()).not.toContain("chat.delete")
})

test("an unknown timestamp fails with message_not_found before chat.delete", async () => {
  stub = stubSlackApi(slackWithMessage({}, {
    "conversations.history": { ok: true, messages: [] },
    "conversations.replies": { ok: false, error: "thread_not_found" },
  }))

  const result = await handleDeleteMessage(ARGS)

  expect(result.isError).toBe(true)
  expect(parsePayload(result).code).toBe("message_not_found")
  expect(methods()).not.toContain("chat.delete")
})

test("refuses with text_mismatch when expected_text differs", async () => {
  stub = stubSlackApi(slackWithMessage())

  const result = await handleDeleteMessage({
    ...ARGS,
    expected_text: "Some other message",
  })

  expect(result.isError).toBe(true)
  expect(parsePayload(result).code).toBe("text_mismatch")
  expect(methods()).not.toContain("chat.delete")
})

test("refuses messages older than 24 hours before any Slack call", async () => {
  const oldTs = tsHoursAgo(48)
  stub = stubSlackApi(slackWithMessage({ ts: oldTs }))

  const result = await handleDeleteMessage({ ...ARGS, timestamp: oldTs })

  expect(result.isError).toBe(true)
  expect(parsePayload(result)).toMatchObject({
    code: "message_too_old",
    max_age_hours: 24,
  })
  expect(stub.calls).toStrictEqual([])
})

test("SLACK_MCP_EDIT_DELETE_MAX_AGE_HOURS widens the age window", async () => {
  const oldTs = tsHoursAgo(48)
  process.env[AGE_ENV] = "72"
  stub = stubSlackApi(slackWithMessage({ ts: oldTs }))

  const result = await handleDeleteMessage({ ...ARGS, timestamp: oldTs })

  expect(parsePayload(result).status).toBe("deleted")
})

for (const code of ["cant_delete_message", "no_permission"]) {
  test(`Slack error ${code} maps to a clear error result`, async () => {
    stub = stubSlackApi(slackWithMessage({}, {
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

/**
 * Responses where the first chat.delete hits a network error and the retry
 * gets message_not_found; the re-read after that returns `reread`.
 */
function slackWithRetriedDelete(reread) {
  let historyReads = 0
  let deleteAttempts = 0
  return slackWithMessage({}, {
    "conversations.history": () => {
      historyReads += 1
      return historyReads === 1
        ? { ok: true, messages: [{ ts: TS, user: SELF, text: OLD_TEXT }] }
        : { ok: true, messages: reread }
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
  })
}

test("message_not_found after a network retry reports probably_deleted", async () => {
  stub = stubSlackApi(slackWithRetriedDelete([]))

  const result = await handleDeleteMessage(ARGS)

  expect(methods().filter((m) => m === "chat.delete")).toHaveLength(2)
  expect(result.isError).toBeUndefined()
  const payload = parsePayload(result)
  expect(payload.status).toBe("probably_deleted")
  expect(payload.note).toContain("first attempt likely landed")
  expect(payload.old_text).toBe(OLD_TEXT)
}, 10_000)

test("a tombstoned thread parent on re-read counts as deleted", async () => {
  stub = stubSlackApi(slackWithRetriedDelete([
    {
      ts: TS,
      subtype: "tombstone",
      user: "USLACKBOT",
      text: "This message was deleted.",
    },
  ]))

  const result = await handleDeleteMessage(ARGS)

  expect(result.isError).toBeUndefined()
  expect(parsePayload(result).status).toBe("probably_deleted")
}, 10_000)

test("refuses with session_changed when the token rotates during the guard", async () => {
  stub = stubSlackApi(slackWithMessage({}, {
    "conversations.history": () => {
      // Simulates slackAPI swapping in another Chrome session.
      process.env.SLACK_TOKEN = "xoxc-test-other-session"
      return { ok: true, messages: [{ ts: TS, user: SELF, text: OLD_TEXT }] }
    },
  }))

  const result = await handleDeleteMessage(ARGS)

  // Nothing was sent, so this is a plain refusal, not an unknown outcome.
  expect(result.isError).toBe(true)
  const payload = parsePayload(result)
  expect(payload).toMatchObject({ status: "error", code: "session_changed" })
  expect(payload.message).toContain("didn't send")
  expect(methods()).not.toContain("chat.delete")
})

test("a landed delete with a lost response, then a session swap, reports outcome_unknown", async () => {
  let landed = false
  stub = stubSlackApi(slackWithMessage({}, {
    "chat.delete": () => {
      // Slack deletes the message, but the response is lost.
      landed = true
      // Another call swaps the session while slackAPI sleeps before retrying.
      setTimeout(() => {
        process.env.SLACK_TOKEN = "xoxc-test-other-session"
      }, 50)
      const err = new Error("socket hang up")
      err.code = "ECONNRESET"
      throw err
    },
  }))

  const result = await handleDeleteMessage(ARGS)

  expect(landed).toBe(true)
  expect(methods().filter((m) => m === "chat.delete")).toHaveLength(1)
  expect(result.isError).toBe(true)
  const payload = parsePayload(result)
  expect(payload).toMatchObject({
    status: "outcome_unknown",
    code: "session_changed",
    channel: CHANNEL,
    timestamp: TS,
  })
  expect(payload.message).toContain("may have landed")
  expect(payload.message).not.toContain("didn't send")
  expect(payload.next_action).toContain("Re-read")
}, 10_000)

test("auth errors still throw so the transport can show the Lifeboat message", async () => {
  stub = stubSlackApi(slackWithMessage({}, {
    "auth.test": { ok: false, error: "account_inactive" },
  }))

  await expect(handleDeleteMessage(ARGS)).rejects.toThrow("account_inactive")
})

const INVALID_INPUTS = [
  ["missing channel_id", { ...ARGS, channel_id: undefined }, "channel_id"],
  ["empty channel_id", { ...ARGS, channel_id: "" }, "channel_id"],
  ["non-string channel_id", { ...ARGS, channel_id: 42 }, "channel_id"],
  ["missing timestamp", { ...ARGS, timestamp: undefined }, "timestamp"],
  ["null timestamp", { ...ARGS, timestamp: null }, "timestamp"],
  ["non-ts timestamp", { ...ARGS, timestamp: "yesterday" }, "timestamp"],
  ["missing expected_text", { ...ARGS, expected_text: undefined }, "expected_text"],
  ["no arguments", undefined, "channel_id"],
]

for (const [label, args, field] of INVALID_INPUTS) {
  test(`dispatch rejects ${label} before any Slack call`, async () => {
    stub = stubSlackApi(slackWithMessage())

    const result = await callTool({
      params: { name: "slack_delete_message", arguments: args },
    })

    expect(result.isError).toBe(true)
    expect(parsePayload(result)).toMatchObject({
      code: "invalid_argument",
      field,
    })
    expect(stub.calls).toStrictEqual([])
  })
}
