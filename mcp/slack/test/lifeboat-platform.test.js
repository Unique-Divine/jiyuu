import { afterEach, beforeEach, expect, test } from "bun:test"
import {
  buildLifeboatPayload,
  resetLifeboatThrottle,
} from "../lib/lifeboat.js"
import { clearSelfUserIdCache } from "../lib/message-ownership.js"
import { setWriteLogSink } from "../lib/write-log.js"
import { callTool } from "../src/server.js"
import { stubSlackApi } from "./helpers/slack-api-stub.js"

let stub
let previousSink
beforeEach(() => {
  resetLifeboatThrottle()
  previousSink = setWriteLogSink(() => {})
})
afterEach(() => {
  stub?.restore()
  stub = undefined
  clearSelfUserIdCache()
  setWriteLogSink(previousSink)
  resetLifeboatThrottle()
})

const authError = new Error("invalid_auth")

test("macOS with file credentials keeps the original wording", () => {
  const payload = buildLifeboatPayload(authError, {
    platformName: "darwin",
    credentialSource: "file",
  })
  expect(payload.self_fix).toContain("slack_refresh_tokens")
  expect(payload.next_action).toContain("--setup")
})

test("Linux with file credentials gets manual-refresh advice", () => {
  const payload = buildLifeboatPayload(authError, {
    platformName: "linux",
    credentialSource: "file",
  })
  expect(payload.self_fix).toContain("Auto-refresh only runs on macOS, not linux")
  expect(payload.self_fix).not.toContain("slack_refresh_tokens")
  expect(payload.next_action).toContain("Sign in to Slack in your browser")
})

for (const platformName of ["linux", "darwin"]) {
  test(`${platformName} with env credentials says to update or unset them`, () => {
    const payload = buildLifeboatPayload(authError, {
      platformName,
      credentialSource: "environment",
    })
    expect(payload.self_fix).toContain("SLACK_TOKEN and SLACK_COOKIE")
    expect(payload.self_fix).toContain("take precedence")
    expect(payload.next_action).toContain("unset both")
    expect(payload.next_action).toContain("setup alone won't help")
  })
}

test("the throttled short form keeps the env advice", () => {
  const options = { platformName: "linux", credentialSource: "environment", now: 1_000 }
  buildLifeboatPayload(authError, options)
  const short = buildLifeboatPayload(authError, { ...options, now: 2_000 })
  expect(short.throttled).toBe(true)
  expect(short.next_action).toContain("SLACK_TOKEN and SLACK_COOKIE")
})

for (const [label, name, args] of [
  ["a read tool", "slack_users_info", { user_id: "U0123456789" }],
  ["a write tool", "slack_delete_message", {
    channel_id: "D0123456789",
    timestamp: `${Math.floor(Date.now() / 1000) - 60}.000100`,
    expected_text: "x",
  }],
]) {
  test.skipIf(process.platform === "darwin")(
    `stdio dispatch with env credentials on Linux: ${label} returns env advice`,
    async () => {
      // stubSlackApi sets SLACK_TOKEN and SLACK_COOKIE, so the credential
      // source is "environment".
      stub = stubSlackApi({
        "users.info": { ok: false, error: "invalid_auth" },
        "auth.test": { ok: false, error: "invalid_auth" },
        "conversations.history": { ok: false, error: "invalid_auth" },
      })

      const result = await callTool({ params: { name, arguments: args } })

      expect(result.isError).toBe(true)
      const payload = JSON.parse(result.content[0].text)
      expect(payload.code).toBe("slack_auth_expired")
      expect(payload.self_fix).toContain("SLACK_TOKEN and SLACK_COOKIE")
      expect(payload.next_action).toContain("unset both")
      expect(JSON.stringify(payload)).not.toContain("Apple Events")
      expect(JSON.stringify(payload)).not.toContain("xoxc-test-placeholder")
    },
  )
}
