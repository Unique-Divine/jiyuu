import { afterEach, expect, test } from "bun:test"
import { TOOL_HANDLERS } from "../lib/handlers.js"
import { clearSelfUserIdCache } from "../lib/message-ownership.js"
import { stubSlackApi } from "./helpers/slack-api-stub.js"

let stub
afterEach(() => {
  stub?.restore()
  stub = undefined
  clearSelfUserIdCache()
})

const SELF = "U0SELF0001"
const CHANNEL = "D0123456789"
const TS = `${Math.floor(Date.now() / 1000) - 600}.000100`

const WRITE_TOOLS = [
  "slack_delete_message",
  "slack_delete_message_unsafe",
  "slack_update_message",
  "slack_update_message_unsafe",
]

/** A deleted thread parent: Slack's tombstone at the parent's ts. */
function slackWithTombstone() {
  return {
    "auth.test": { ok: true, user_id: SELF },
    "conversations.history": {
      ok: true,
      messages: [{
        ts: TS,
        subtype: "tombstone",
        user: "USLACKBOT",
        text: "This message was deleted.",
        reply_count: 1,
      }],
    },
    "chat.delete": { ok: true },
    "chat.update": { ok: true },
  }
}

for (const name of WRITE_TOOLS) {
  test(`${name} reports a deleted thread parent as message_deleted`, async () => {
    stub = stubSlackApi(slackWithTombstone())

    const result = await TOOL_HANDLERS[name]({
      channel_id: CHANNEL,
      timestamp: TS,
      expected_text: "The original parent text",
      text: "New text",
    })

    expect(result.isError).toBe(true)
    const payload = JSON.parse(result.content[0].text)
    expect(payload).toMatchObject({
      code: "message_deleted",
      channel: CHANNEL,
      timestamp: TS,
    })
    expect(payload.message).toContain("already deleted")
    const methods = stub.calls.map((c) => c.method)
    expect(methods).not.toContain("chat.delete")
    expect(methods).not.toContain("chat.update")
  })
}
