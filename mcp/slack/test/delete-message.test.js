import { afterEach, expect, test } from "bun:test"
import { handleDeleteMessage, TOOL_HANDLERS } from "../lib/handlers.js"
import { TOOLS } from "../lib/tools.js"
import { stubSlackApi } from "./helpers/slack-api-stub.js"

let stub
afterEach(() => stub?.restore())

const CHANNEL = "D0123456789"
const TS = "1767368030.607599"

function parsePayload(result) {
  return JSON.parse(result.content[0].text)
}

test("slack_delete_message is advertised as a destructive write", () => {
  const tool = TOOLS.find((t) => t.name === "slack_delete_message")
  expect(tool).toBeTruthy()
  expect(tool.inputSchema.required).toStrictEqual(["channel_id", "ts"])
  expect(tool.annotations.readOnlyHint).toBe(false)
  expect(tool.annotations.destructiveHint).toBe(true)
  expect(TOOL_HANDLERS.slack_delete_message).toBe(handleDeleteMessage)
})

test("deletes through chat.delete and reports the deleted message", async () => {
  stub = stubSlackApi({
    "chat.delete": (params) => ({
      ok: true,
      channel: params.channel,
      ts: params.ts,
    }),
  })

  const result = await handleDeleteMessage({ channel_id: CHANNEL, ts: TS })

  expect(stub.calls).toStrictEqual([
    { method: "chat.delete", params: { channel: CHANNEL, ts: TS } },
  ])
  expect(result.isError).toBeUndefined()
  expect(parsePayload(result)).toStrictEqual({
    status: "deleted",
    channel: CHANNEL,
    ts: TS,
  })
})

test("someone else's message fails with cant_delete_message", async () => {
  stub = stubSlackApi({
    "chat.delete": { ok: false, error: "cant_delete_message" },
  })

  await expect(
    handleDeleteMessage({ channel_id: CHANNEL, ts: TS }),
  ).rejects.toThrow("cant_delete_message")
})

test("an unknown timestamp fails with message_not_found", async () => {
  stub = stubSlackApi({
    "chat.delete": { ok: false, error: "message_not_found" },
  })

  await expect(
    handleDeleteMessage({ channel_id: CHANNEL, ts: TS }),
  ).rejects.toThrow("message_not_found")
})
