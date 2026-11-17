import { afterEach, expect, test } from "bun:test"
import { slackAPI } from "../lib/slack-client.js"
import { stubSlackApi } from "./helpers/slack-api-stub.js"

let stub
afterEach(() => {
  stub?.restore()
  stub = undefined
})

test("expectedToken refuses to send under a different session", async () => {
  stub = stubSlackApi({ "chat.delete": { ok: true } })

  await expect(
    slackAPI("chat.delete", { channel: "C1", ts: "1.2" }, {
      expectedToken: "xoxc-some-other-session",
    }),
  ).rejects.toThrow("session_changed")
  expect(stub.calls).toStrictEqual([])
})

test("expectedToken matching the loaded session sends normally", async () => {
  stub = stubSlackApi({ "chat.delete": { ok: true } })

  const result = await slackAPI("chat.delete", { channel: "C1", ts: "1.2" }, {
    expectedToken: process.env.SLACK_TOKEN,
  })

  expect(result.ok).toBe(true)
  expect(stub.calls.map((c) => c.method)).toStrictEqual(["chat.delete"])
})
