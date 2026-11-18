import { afterEach, beforeEach, expect, test } from "bun:test"
import { TOOL_HANDLERS } from "../lib/handlers.js"
import { clearSelfUserIdCache } from "../lib/message-ownership.js"
import { formatWriteLogLine, setWriteLogSink } from "../lib/write-log.js"
import { stubSlackApi } from "./helpers/slack-api-stub.js"

let stub
let lines
let previousSink
beforeEach(() => {
  lines = []
  previousSink = setWriteLogSink((line) => lines.push(line))
})
afterEach(() => {
  setWriteLogSink(previousSink)
  stub?.restore()
  stub = undefined
  clearSelfUserIdCache()
})

const SELF = "U0SELF0001"
const CHANNEL = "D0123456789"
const NOW = Date.now()
const TS = `${Math.floor(NOW / 1000) - 1800}.000100`
const SECRET_TEXT = "Payment address 0xSECRET and a link"
const NEW_TEXT = "Corrected copy, also secret"

function parseLine(line) {
  expect(line.startsWith("slack-mcp write {")).toBe(true)
  expect(line.includes("\n")).toBe(false)
  return JSON.parse(line.slice("slack-mcp write ".length))
}

function slackWithOwnMessage() {
  return {
    "auth.test": { ok: true, user_id: SELF },
    "conversations.history": {
      ok: true,
      messages: [{ ts: TS, user: SELF, text: SECRET_TEXT }],
    },
    "chat.delete": (p) => ({ ok: true, channel: p.channel, ts: p.ts }),
    "chat.update": (p) => ({ ok: true, channel: p.channel, ts: p.ts, text: p.text }),
  }
}

function argsFor(name, overrides = {}) {
  return {
    channel_id: CHANNEL,
    timestamp: TS,
    expected_text: SECRET_TEXT,
    ...(name.startsWith("slack_update") ? { text: NEW_TEXT } : {}),
    ...overrides,
  }
}

test("formatWriteLogLine has exactly the documented fields", () => {
  const entry = parseLine(formatWriteLogLine({
    tool: "slack_delete_message",
    args: { channel_id: CHANNEL, timestamp: TS },
    status: "deleted",
    code: null,
    verified: { channel: true, timestamp: true },
    now: NOW,
  }))
  expect(Object.keys(entry)).toStrictEqual([
    "tool", "channel", "timestamp", "status", "code", "age_hours",
  ])
  expect(entry).toMatchObject({
    tool: "slack_delete_message",
    channel: CHANNEL,
    timestamp: TS,
    status: "deleted",
    code: null,
  })
  expect(entry.age_hours).toBeCloseTo(0.5, 1)
})

test("free text in channel_id or timestamp is never logged", () => {
  const entry = parseLine(formatWriteLogLine({
    tool: "slack_update_message",
    args: { channel_id: "please delete: Payment address", timestamp: SECRET_TEXT },
    status: "error",
    code: "invalid_argument",
  }))
  expect(entry.channel).toBe(null)
  expect(entry.timestamp).toBe(null)
  expect(entry.age_hours).toBe(null)
})

for (const name of [
  "slack_delete_message",
  "slack_delete_message_unsafe",
  "slack_update_message",
  "slack_update_message_unsafe",
]) {
  test(`${name} logs one line per attempt with no message text or token`, async () => {
    stub = stubSlackApi(slackWithOwnMessage())

    await TOOL_HANDLERS[name](argsFor(name))
    await TOOL_HANDLERS[name](argsFor(name, { expected_text: "wrong" }))

    expect(lines).toHaveLength(2)
    const [ok, refused] = lines.map(parseLine)
    expect(ok).toMatchObject({
      tool: name,
      channel: CHANNEL,
      timestamp: TS,
      status: name.startsWith("slack_delete") ? "deleted" : "updated",
      code: null,
    })
    expect(refused).toMatchObject({ tool: name, status: "error", code: "text_mismatch" })
    for (const line of lines) {
      expect(line).not.toContain("Payment")
      expect(line).not.toContain("Corrected")
      expect(line).not.toContain(process.env.SLACK_TOKEN)
      expect(line).not.toContain(process.env.SLACK_COOKIE)
    }
  })
}

test("a thrown error is logged with status thrown and still propagates", async () => {
  stub = stubSlackApi({
    ...slackWithOwnMessage(),
    "auth.test": { ok: false, error: "account_inactive" },
  })

  await expect(
    TOOL_HANDLERS.slack_delete_message(argsFor("slack_delete_message")),
  ).rejects.toThrow("account_inactive")

  // Slack never confirmed the channel or message, so neither is logged.
  expect(lines.map(parseLine)).toStrictEqual([{
    tool: "slack_delete_message",
    channel: null,
    timestamp: null,
    status: "thrown",
    code: "account_inactive",
    age_hours: lines.length ? parseLine(lines[0]).age_hours : null,
  }])
})

for (const channel of ["C0123456789", "D03DN5VMBPW", "G01ABCDEF2", "C0NF1DENT1AL"]) {
  test(`an unverified channel is never logged, even ID-shaped: ${channel}`, () => {
    const entry = parseLine(formatWriteLogLine({
      tool: "slack_delete_message",
      args: { channel_id: channel, timestamp: TS },
      status: "thrown",
      code: "channel_not_found",
    }))
    expect(entry.channel).toBe(null)
    expect(entry.timestamp).toBe(null)
  })
}

for (const [label, channel, expected] of [
  ["a channel ID", "C0123456789", "C0123456789"],
  ["a DM ID", "D03DN5VMBPW", "D03DN5VMBPW"],
  ["a private channel ID", "G01ABCDEF2", "G01ABCDEF2"],
  ["a malformed value", "please delete this", null],
]) {
  test(`a verified channel is logged when ID-shaped: ${label}`, () => {
    const entry = parseLine(formatWriteLogLine({
      tool: "slack_delete_message",
      args: { channel_id: channel, timestamp: TS },
      status: "deleted",
      code: null,
      verified: { channel: true, timestamp: true },
    }))
    expect(entry.channel).toBe(expected)
  })
}

test("C0NF1DENT1AL rejected by Slack with channel_not_found logs null", async () => {
  stub = stubSlackApi({
    "auth.test": { ok: true, user_id: SELF },
    "conversations.history": { ok: false, error: "channel_not_found" },
  })

  await expect(
    TOOL_HANDLERS.slack_delete_message(
      argsFor("slack_delete_message", { channel_id: "C0NF1DENT1AL" }),
    ),
  ).rejects.toThrow("channel_not_found")

  expect(lines.map(parseLine)).toMatchObject([{
    channel: null,
    timestamp: null,
    status: "thrown",
    code: "channel_not_found",
  }])
  expect(lines[0]).not.toContain("C0NF1DENT1AL")
})

test("a refusal before any Slack call logs channel and timestamp as null", async () => {
  stub = stubSlackApi(slackWithOwnMessage())
  const oldTs = `${Math.floor(NOW / 1000) - 8 * 24 * 3600}.000100`

  await TOOL_HANDLERS.slack_delete_message(
    argsFor("slack_delete_message", { timestamp: oldTs }),
  )

  expect(stub.calls).toStrictEqual([])
  const entry = parseLine(lines[0])
  expect(entry).toMatchObject({ channel: null, timestamp: null, code: "message_too_old" })
  expect(entry.age_hours).toBeGreaterThan(190)
})

test("message_not_found logs the verified channel but not the timestamp", async () => {
  stub = stubSlackApi({
    ...slackWithOwnMessage(),
    "conversations.history": { ok: true, messages: [] },
    "conversations.replies": { ok: false, error: "thread_not_found" },
  })

  await TOOL_HANDLERS.slack_delete_message(argsFor("slack_delete_message"))

  expect(parseLine(lines[0])).toMatchObject({
    channel: CHANNEL,
    timestamp: null,
    code: "message_not_found",
  })
})

test("validation rejects C123 and lowercase IDs before any Slack call", async () => {
  stub = stubSlackApi(slackWithOwnMessage())
  for (const channel of ["C123", "c0123456789"]) {
    const result = await TOOL_HANDLERS.slack_delete_message(
      argsFor("slack_delete_message", { channel_id: channel }),
    )
    expect(JSON.parse(result.content[0].text)).toMatchObject({
      code: "invalid_argument",
      field: "channel_id",
    })
  }
  expect(stub.calls).toStrictEqual([])
  expect(lines.map(parseLine).map((e) => e.channel)).toStrictEqual([null, null])
})
