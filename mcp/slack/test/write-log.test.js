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

  expect(lines.map(parseLine)).toStrictEqual([{
    tool: "slack_delete_message",
    channel: CHANNEL,
    timestamp: TS,
    status: "thrown",
    code: "account_inactive",
    age_hours: lines.length ? parseLine(lines[0]).age_hours : null,
  }])
})
