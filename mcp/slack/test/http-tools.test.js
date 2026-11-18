import { afterEach, expect, test } from "bun:test"
import { clearSelfUserIdCache } from "../lib/message-ownership.js"
import {
  createHttpToolHandlers,
  isUnsafeTool,
  UNSAFE_TOOLS_ENV,
} from "../lib/http-tools.js"
import { TOOLS } from "../lib/tools.js"
import { stubSlackApi } from "./helpers/slack-api-stub.js"

let stub
afterEach(() => {
  stub?.restore()
  stub = undefined
  clearSelfUserIdCache()
})

const UNSAFE_NAMES = TOOLS.map((t) => t.name).filter(isUnsafeTool)
const SELF = "U0SELF0001"
const CHANNEL = "C0123456789"
const TS = `${Math.floor(Date.now() / 1000) - 8 * 24 * 3600}.000100`
const TEXT = "Old message"

function parsePayload(result) {
  return JSON.parse(result.content[0].text)
}

function slackWithOldMessage() {
  return {
    "auth.test": { ok: true, user_id: SELF },
    "conversations.history": {
      ok: true,
      messages: [{ ts: TS, user: SELF, text: TEXT }],
    },
    "chat.delete": (params) => ({ ok: true, channel: params.channel, ts: params.ts }),
    "chat.update": (params) => ({ ok: true, channel: params.channel, ts: params.ts, text: params.text }),
  }
}

function argsFor(name) {
  return {
    channel_id: CHANNEL,
    timestamp: TS,
    expected_text: TEXT,
    ...(name.startsWith("slack_update_message") ? { text: "New text" } : {}),
  }
}

test("the stack defines at least one _unsafe tool", () => {
  expect(UNSAFE_NAMES.length).toBeGreaterThan(0)
})

test("HTTP hides and refuses the _unsafe tools by default", async () => {
  const http = createHttpToolHandlers({})
  const listed = (await http.listTools()).tools.map((t) => t.name)

  stub = stubSlackApi(slackWithOldMessage())
  for (const name of UNSAFE_NAMES) {
    expect(listed).not.toContain(name)
    const result = await http.callTool({
      params: { name, arguments: argsFor(name) },
    })
    expect(result.isError).toBe(true)
    expect(parsePayload(result).code).toBe("unknown_tool")
  }
  expect(stub.calls).toStrictEqual([])
  // Every other tool stays listed.
  expect(listed.length).toBe(TOOLS.length - UNSAFE_NAMES.length)
})

for (const value of ["1", "yes", "TRUE", ""]) {
  test(`HTTP keeps them hidden when ${UNSAFE_TOOLS_ENV}=${JSON.stringify(value)}`, async () => {
    const http = createHttpToolHandlers({ [UNSAFE_TOOLS_ENV]: value })
    const listed = (await http.listTools()).tools.map((t) => t.name)
    for (const name of UNSAFE_NAMES) expect(listed).not.toContain(name)
  })
}

test(`HTTP lists and dispatches the _unsafe tools with ${UNSAFE_TOOLS_ENV}=true`, async () => {
  const http = createHttpToolHandlers({ [UNSAFE_TOOLS_ENV]: "true" })
  const listed = (await http.listTools()).tools.map((t) => t.name)
  expect(listed).toStrictEqual(TOOLS.map((t) => t.name))

  for (const name of UNSAFE_NAMES) {
    stub = stubSlackApi(slackWithOldMessage())
    const result = await http.callTool({
      params: { name, arguments: argsFor(name) },
    })
    expect(result.isError).toBeUndefined()
    expect(["deleted", "updated"]).toContain(parsePayload(result).status)
    stub.restore()
    stub = undefined
    clearSelfUserIdCache()
  }
})

test("HTTP still dispatches the safe tools by default", async () => {
  const http = createHttpToolHandlers({})
  stub = stubSlackApi(slackWithOldMessage())
  // An invalid call reaches the handler, which refuses it before any Slack
  // request; unknown_tool would mean the dispatch gate blocked it.
  const result = await http.callTool({
    params: { name: "slack_delete_message", arguments: {} },
  })
  expect(parsePayload(result).code).toBe("invalid_argument")
  expect(stub.calls).toStrictEqual([])
})
